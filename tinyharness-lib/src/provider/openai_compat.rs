use std::collections::HashMap;
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

use crate::{
    SecretString,
    provider::{
        ChatMessage, ChatMessageResponse, Message, Role, ToolCall, ToolCallFunction, ToolDefinition,
    },
};

/// Default per-request timeout for OpenAI-compatible providers when no
/// explicit value is configured (via `/timeout` or settings).
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 30;

/// Shared inner state for OpenAI-compatible providers (llama.cpp, vLLM, etc.).
///
/// Encapsulates the common `{client, base_url, model, api_key}` fields and all
/// shared logic so that provider implementations only need to differ in
/// `list_models()`.
pub struct OpenAiCompatInner {
    client: Client,
    base_url: String,
    model: Option<String>,
    /// Optional bearer token sent as `Authorization: Bearer <key>` on every
    /// request. Used by hosted OpenAI-compatible APIs (e.g. OpenRouter,
    /// Together, self-hosted gateways) that require authentication.
    api_key: Option<SecretString>,
    /// Per-request timeout in seconds, applied to connecting and reading the
    /// HTTP response. The default (30s) covers a slow backend; increase via
    /// `/timeout <secs>` for large-context requests.
    timeout_secs: u64,
    /// Maximum number of attempts for transient failures (connection errors,
    /// timeouts, 5xx responses). 1 = no retries. 4xx responses are never
    /// retried. Backoff between attempts: 1s, 2s, 4s, …
    max_retries: u32,
}

impl OpenAiCompatInner {
    pub fn new(base_url: String) -> Self {
        Self::with_api_key(base_url, None)
    }

    /// Create a new inner state with an optional bearer token.
    pub fn with_api_key(base_url: String, api_key: Option<SecretString>) -> Self {
        Self::with_options(base_url, api_key, DEFAULT_REQUEST_TIMEOUT_SECS, 0)
    }

    /// Create a new inner state with explicit timeout/retry configuration.
    pub fn with_options(
        base_url: String,
        api_key: Option<SecretString>,
        timeout_secs: u64,
        max_retries: u32,
    ) -> Self {
        // Read timeout bounds how long we wait between bytes on an active
        // stream; add slack over the request timeout so long generations
        // that keep producing tokens are never cut off mid-stream.
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(timeout_secs + 60))
            .build()
            .unwrap_or_else(|_| Client::new());
        OpenAiCompatInner {
            client,
            base_url,
            model: None,
            api_key,
            timeout_secs,
            max_retries,
        }
    }

    /// Current request timeout in seconds.
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs
    }

    /// Current maximum retry count.
    pub fn max_retries(&self) -> u32 {
        self.max_retries
    }

    /// Update the request timeout. Applies to subsequent requests (the HTTP
    /// client's read timeout is rebuilt lazily on the next `chat()`).
    pub fn set_timeout(&mut self, timeout_secs: u64) {
        self.timeout_secs = timeout_secs;
        self.client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(timeout_secs + 60))
            .build()
            .unwrap_or_else(|_| Client::new());
    }

    /// Update the maximum retry count. Applies to subsequent requests.
    pub fn set_retries(&mut self, max_retries: u32) {
        self.max_retries = max_retries;
    }

    /// Perform a health check against the server's `/health` endpoint.
    pub async fn health_check(&self) -> Result<(), String> {
        let url = format!("{}/health", self.base_url.trim_end_matches('/'));
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key.expose_secret());
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => Ok(()),
            // A 404 usually means the server simply has no /health
            // endpoint. Don't dump its response body (often a large HTML
            // or JSON error page) into the warning.
            Ok(resp) if resp.status() == reqwest::StatusCode::NOT_FOUND => {
                Err("Server returned 404 (no /health endpoint)".to_string())
            }
            Ok(resp) => Err(format!(
                "Server returned {}: {}",
                resp.status().as_u16(),
                resp.text().await.unwrap_or_default()
            )),
            Err(e) => Err(format!("Cannot reach {}: {}", url, e)),
        }
    }

    pub fn select_model(&mut self, name: String) {
        self.model = Some(name);
    }

    pub fn current_model(&self) -> Option<String> {
        self.model.clone()
    }

    /// Return the `/v1/chat/completions` URL for this server.
    pub fn chat_url(&self) -> String {
        format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/').trim_end_matches("/v1")
        )
    }

    /// Fetch the model list from the server's `/v1/models` endpoint.
    /// Returns the list of model IDs, or an empty vec on failure.
    pub async fn fetch_model_list(&self) -> Vec<String> {
        let url = format!(
            "{}/v1/models",
            self.base_url.trim_end_matches('/').trim_end_matches("/v1")
        );
        let mut req = self.client.get(&url);
        if let Some(key) = &self.api_key {
            req = req.bearer_auth(key.expose_secret());
        }
        match req.send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<ModelListResponse>().await {
                    Ok(list) => list.data.into_iter().map(|m| m.id).collect(),
                    Err(_) => self.model.clone().into_iter().collect(),
                }
            }
            _ => self.model.clone().into_iter().collect(),
        }
    }

    /// Stream chat completions using the OpenAI-compatible API.
    /// Returns a receiver for streaming response chunks, or an error string
    /// if the request cannot be started.
    pub async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String> {
        let (send, recv) =
            tokio::sync::mpsc::channel::<ChatMessageResponse>(super::STREAM_CHANNEL_CAPACITY);

        let model = self.model.clone().unwrap_or_default();
        let openai_messages = messages.into_iter().map(to_openai_message).collect();
        let openai_tools = tools.into_iter().map(to_openai_tool).collect();
        let client = self.client.clone();
        let chat_url = self.chat_url();
        let api_key = self.api_key.clone();
        let timeout_secs = self.timeout_secs;
        let max_retries = self.max_retries;

        let body = ChatRequest {
            model,
            messages: openai_messages,
            stream: true,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
            tools: openai_tools,
        };

        // Spawn the streaming work on a background task
        tokio::spawn(async move {
            let _usage = chat_with_retries(
                &client,
                &chat_url,
                &body,
                api_key.as_ref(),
                timeout_secs,
                max_retries,
                &send,
            )
            .await;
        });

        Ok(recv)
    }
}

/// Send the chat request with retry + exponential backoff for transient
/// failures, then stream the response chunks through `send`.
///
/// Retryable failures: connection errors, request timeouts (when the whole
/// attempt is cut off before any output), and 5xx / 429 responses. Once a
/// terminal chunk has reached the receiver (success, mid-stream error, or a
/// 4xx error), the outcome is final — retrying would duplicate output.
///
/// Backoff between attempts: 1s, 2s, 4s, … Returns the token usage when a
/// final attempt succeeds.
async fn chat_with_retries(
    client: &reqwest::Client,
    url: &str,
    body: &ChatRequest,
    api_key: Option<&SecretString>,
    timeout_secs: u64,
    max_retries: u32,
    send: &tokio::sync::mpsc::Sender<ChatMessageResponse>,
) -> Option<crate::provider::TokenUsage> {
    let max_attempts = max_retries.max(1);

    for attempt in 1..=max_attempts {
        let result = tokio::time::timeout(
            Duration::from_secs(timeout_secs),
            stream_chat_completions(client, url, body, api_key, send),
        )
        .await;

        match result {
            // The attempt produced a terminal chunk (success or surfaced
            // error) — the receiver already has the outcome, don't retry.
            Ok(AttemptOutcome::Final(usage)) => return usage,
            // The attempt failed before delivering anything.
            Ok(AttemptOutcome::Retryable(e)) => {
                if attempt >= max_attempts {
                    send_error_chunk(send, &format!("Error after {attempt} attempt(s): {e}")).await;
                    return None;
                }
            }
            // The whole attempt timed out before completing.
            Err(_) => {
                if attempt >= max_attempts {
                    send_error_chunk(
                        send,
                        &format!(
                            "Error: Request timed out after {timeout_secs}s ({attempt} attempt(s))"
                        ),
                    )
                    .await;
                    return None;
                }
            }
        }

        // Exponential backoff: 1s, 2s, 4s, ...
        let backoff = Duration::from_secs(1 << (attempt - 1));
        tokio::time::sleep(backoff).await;
    }

    None
}

// ── OpenAI-compatible request/response types ──

#[derive(Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAITool>,
}

#[derive(Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Serialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Serialize)]
pub struct OpenAITool {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: OpenAIToolFunction,
}

#[derive(Serialize)]
pub struct OpenAIToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct OpenAIToolCall {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default)]
    pub call_type: String,
    #[serde(default)]
    pub function: OpenAIToolCallFunction,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub struct OpenAIToolCallFunction {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

#[derive(Deserialize)]
pub struct ChunkChoice {
    pub delta: Delta,
    #[serde(default, rename = "finish_reason")]
    pub _finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Deserialize)]
pub struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
    #[serde(default)]
    pub usage: Option<OpenAIUsage>,
}

#[derive(Deserialize, Clone)]
pub struct OpenAIUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

// ── Model list response types ──

#[derive(Deserialize)]
pub struct ModelListResponse {
    pub data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
pub struct ModelEntry {
    pub id: String,
}

// ── Conversion helpers ──

pub fn to_openai_message(msg: Message) -> OpenAIMessage {
    /// Build the content value: if images are present, use multipart array format;
    /// otherwise use plain string.
    fn build_content(msg: &Message) -> serde_json::Value {
        if msg.images.is_empty() {
            serde_json::Value::String(msg.content.clone())
        } else {
            let mut parts: Vec<serde_json::Value> = Vec::new();
            // Add text part
            if !msg.content.is_empty() {
                parts.push(serde_json::json!({
                    "type": "text",
                    "text": msg.content
                }));
            }
            // Add image parts
            for img in &msg.images {
                parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": img.data_uri()
                    }
                }));
            }
            serde_json::Value::Array(parts)
        }
    }

    match msg.role {
        Role::System => OpenAIMessage {
            role: "system".to_string(),
            content: serde_json::Value::String(msg.content),
            tool_calls: None,
            tool_call_id: None,
        },
        Role::User => OpenAIMessage {
            role: "user".to_string(),
            content: build_content(&msg),
            tool_calls: None,
            tool_call_id: None,
        },
        Role::Assistant => {
            if msg.tool_calls.is_empty() {
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: serde_json::Value::String(msg.content),
                    tool_calls: None,
                    tool_call_id: None,
                }
            } else {
                let tool_calls: Vec<OpenAIToolCall> = msg
                    .tool_calls
                    .into_iter()
                    .enumerate()
                    .map(|(i, tc)| {
                        // Synthesize a stable id when the upstream provider
                        // didn't return one (some local / non-OpenAI servers
                        // omit it). OpenAI requires a non-empty id.
                        let id = tc
                            .id
                            .filter(|s| !s.is_empty())
                            .unwrap_or_else(|| format!("call_{}", i));
                        let args_str = if tc.function.arguments.is_null() {
                            "{}".to_string()
                        } else {
                            tc.function.arguments.to_string()
                        };
                        OpenAIToolCall {
                            index: i,
                            id,
                            call_type: "function".to_string(),
                            function: OpenAIToolCallFunction {
                                name: tc.function.name,
                                arguments: args_str,
                            },
                        }
                    })
                    .collect();
                OpenAIMessage {
                    role: "assistant".to_string(),
                    // MiniMax and some other strict providers reject empty
                    // strings in assistant messages that contain only tool
                    // calls.  Send `null` when there is no text content.
                    content: if msg.content.is_empty() {
                        serde_json::Value::Null
                    } else {
                        serde_json::Value::String(msg.content)
                    },
                    tool_calls: Some(tool_calls),
                    tool_call_id: None,
                }
            }
        }
        Role::Tool => {
            // tool_call_id should have been set by the agent loop. If somehow
            // missing, skip the field rather than emitting a bogus id that the
            // server will reject.
            let tool_call_id = msg.tool_call_id.filter(|s| !s.is_empty());
            OpenAIMessage {
                role: "tool".to_string(),
                content: serde_json::Value::String(msg.content),
                tool_calls: None,
                tool_call_id,
            }
        }
    }
}

pub fn to_openai_tool(ti: ToolDefinition) -> OpenAITool {
    OpenAITool {
        tool_type: "function".to_string(),
        function: OpenAIToolFunction {
            name: ti.name,
            description: ti.description,
            parameters: serde_json::to_value(ti.parameters).unwrap_or_default(),
        },
    }
}

/// Outcome of a single streaming attempt.
///
/// `Retryable` failures happen before anything was delivered to the
/// receiver, so the attempt can be repeated cleanly. `Final` outcomes
/// already produced a terminal chunk on the channel (success or an error
/// the receiver has seen), so retrying would duplicate output.
enum AttemptOutcome {
    /// Nothing was sent to the receiver — safe to retry.
    Retryable(String),
    /// A final chunk (success or error) was already sent.
    Final(Option<crate::provider::TokenUsage>),
}

/// Determine whether an HTTP status code should be retried.
///
/// 5xx responses (server-side trouble) and 429 (rate limit) are transient;
/// other 4xx errors indicate a request problem that retrying won't fix.
fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// Stream chat completions from an OpenAI-compatible endpoint.
///
/// Returns `AttemptOutcome::Retryable` when the request failed before any
/// chunk reached the receiver (connection error, non-retryable status
/// surfaced via `Err`); returns `AttemptOutcome::Final` once a terminal
/// chunk (success or surfaced error) has been delivered.
///
/// If `api_key` is `Some`, the request includes an
/// `Authorization: Bearer <key>` header.
async fn stream_chat_completions(
    client: &reqwest::Client,
    url: &str,
    body: &ChatRequest,
    api_key: Option<&SecretString>,
    send: &tokio::sync::mpsc::Sender<ChatMessageResponse>,
) -> AttemptOutcome {
    let mut request = client.post(url).json(body);
    if let Some(key) = api_key {
        request = request.bearer_auth(key.expose_secret());
    }
    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => return AttemptOutcome::Retryable(format!("Error: {}", e)),
    };

    // If the server returned a non-success status, surface the body as an
    // error instead of feeding it into the SSE parser (which would silently
    // drop it). Server-side (5xx) and rate-limit (429) failures are retried
    // by the caller; client errors (4xx) are surfaced immediately.
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let msg = format!("Error: HTTP {} — {}", status.as_u16(), body);
        if is_retryable_status(status) {
            return AttemptOutcome::Retryable(msg);
        }
        send_error_chunk(send, &msg).await;
        return AttemptOutcome::Final(None);
    }

    let mut stream = response.bytes_stream();
    let mut buf = String::new();

    let mut acc_tool_calls: HashMap<usize, OpenAIToolCall> = HashMap::new();
    let mut response_content = String::new();
    let mut token_usage: Option<crate::provider::TokenUsage> = None;

    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(c) => c,
            Err(e) => {
                // Stream read error — surface it rather than silently breaking.
                // This is final: partial output may already have been delivered.
                send_error_chunk(send, &format!("\n\nStream error: {}", e)).await;
                return AttemptOutcome::Final(token_usage);
            }
        };

        buf.push_str(&String::from_utf8_lossy(&chunk));

        loop {
            match buf.find('\n') {
                None => break,
                Some(pos) => {
                    let line = buf[..pos].trim().to_string();
                    buf = buf[pos + 1..].to_string();

                    if line.is_empty() || line == "data: [DONE]" {
                        continue;
                    }

                    if let Some(data) = line.strip_prefix("data: ")
                        && let Ok(chunk) = serde_json::from_str::<StreamChunk>(data)
                    {
                        // Capture token usage if present (usually in the final chunk)
                        if let Some(usage) = &chunk.usage {
                            token_usage = Some(crate::provider::TokenUsage {
                                prompt_tokens: usage.prompt_tokens,
                                completion_tokens: usage.completion_tokens,
                                total_tokens: usage.total_tokens,
                            });
                        }

                        for choice in chunk.choices {
                            if let Some(content) = &choice.delta.content {
                                response_content.push_str(content);
                            }

                            if let Some(tool_calls) = &choice.delta.tool_calls {
                                for tc in tool_calls {
                                    let entry =
                                        acc_tool_calls.entry(tc.index).or_insert(OpenAIToolCall {
                                            index: tc.index,
                                            id: String::new(),
                                            call_type: "function".to_string(),
                                            function: OpenAIToolCallFunction::default(),
                                        });

                                    if !tc.id.is_empty() {
                                        entry.id = tc.id.clone();
                                    }
                                    if !tc.function.name.is_empty() {
                                        entry.function.name = tc.function.name.clone();
                                    }
                                    entry.function.arguments.push_str(&tc.function.arguments);
                                }
                            }
                        }
                    }
                }
            }
        }

        if !response_content.is_empty() {
            // If the receiver is gone (user interrupt, shutdown), stop
            // reading the HTTP stream instead of draining the rest of it.
            if send
                .send(ChatMessageResponse {
                    message: ChatMessage {
                        content: response_content.clone(),
                        tool_calls: vec![],
                        thinking: None,
                    },
                    done: false,
                    is_error: false,
                    usage: None,
                })
                .await
                .is_err()
            {
                return AttemptOutcome::Final(token_usage);
            }
            response_content.clear();
        }
    }

    let tool_calls: Vec<ToolCall> = if !acc_tool_calls.is_empty() {
        acc_tool_calls
            .into_values()
            .map(|tc| {
                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::Value::Null);
                ToolCall {
                    id: if tc.id.is_empty() { None } else { Some(tc.id) },
                    function: ToolCallFunction {
                        name: tc.function.name,
                        arguments: args,
                        thought_signature: None,
                    },
                }
            })
            .collect()
    } else {
        vec![]
    };

    // Send the final response with tool calls and token usage
    let _ = send
        .send(ChatMessageResponse {
            message: ChatMessage {
                content: String::new(),
                tool_calls,
                thinking: None,
            },
            done: true,
            is_error: false,
            usage: token_usage.clone(),
        })
        .await;

    AttemptOutcome::Final(token_usage)
}

/// Send a terminal error chunk (done: true, is_error: true) to the receiver.
async fn send_error_chunk(send: &tokio::sync::mpsc::Sender<ChatMessageResponse>, message: &str) {
    let _ = send
        .send(ChatMessageResponse {
            message: ChatMessage {
                content: message.to_string(),
                tool_calls: vec![],
                thinking: None,
            },
            done: true,
            is_error: true,
            usage: None,
        })
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 5xx and 429 are transient and should be retried.
    #[test]
    fn retryable_status_codes() {
        assert!(is_retryable_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(is_retryable_status(
            reqwest::StatusCode::SERVICE_UNAVAILABLE
        ));
        assert!(is_retryable_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(is_retryable_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
    }

    /// 4xx responses (other than 429) mean the request itself is wrong —
    /// retrying won't help, so they must not be retried.
    #[test]
    fn non_retryable_status_codes() {
        assert!(!is_retryable_status(reqwest::StatusCode::UNAUTHORIZED));
        assert!(!is_retryable_status(reqwest::StatusCode::NOT_FOUND));
        assert!(!is_retryable_status(reqwest::StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(
            reqwest::StatusCode::UNPROCESSABLE_ENTITY
        ));
    }

    /// Defaults for fresh inner state: 30s timeout, no retries (matches the
    /// pre-change hardcoded behaviour for retries and read-timeout headroom).
    #[test]
    fn defaults_timeout_and_retries() {
        let inner = OpenAiCompatInner::new("http://localhost:8080".to_string());
        assert_eq!(inner.timeout_secs(), 30);
        assert_eq!(inner.max_retries(), 0);
    }

    /// Explicit options constructor stores timeout and retries.
    #[test]
    fn with_options_stores_configuration() {
        let inner =
            OpenAiCompatInner::with_options("http://localhost:8080".to_string(), None, 120, 5);
        assert_eq!(inner.timeout_secs(), 120);
        assert_eq!(inner.max_retries(), 5);
    }

    /// set_timeout rebuilds the HTTP client and updates the stored value.
    #[test]
    fn set_timeout_updates_value() {
        let mut inner = OpenAiCompatInner::new("http://localhost:8080".to_string());
        inner.set_timeout(60);
        assert_eq!(inner.timeout_secs(), 60);
    }

    /// set_retries updates the stored value.
    #[test]
    fn set_retries_updates_value() {
        let mut inner = OpenAiCompatInner::new("http://localhost:8080".to_string());
        inner.set_retries(7);
        assert_eq!(inner.max_retries(), 7);
    }
}
