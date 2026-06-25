use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio_stream::StreamExt;

use crate::provider::{
    ChatMessage, ChatMessageResponse, Message, Role, ToolCall, ToolCallFunction, ToolDefinition,
};

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
    api_key: Option<String>,
}

impl OpenAiCompatInner {
    pub fn new(base_url: String) -> Self {
        Self::with_api_key(base_url, None)
    }

    /// Create a new inner state with an optional bearer token.
    pub fn with_api_key(base_url: String, api_key: Option<String>) -> Self {
        let client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| Client::new());
        OpenAiCompatInner {
            client,
            base_url,
            model: None,
            api_key,
        }
    }

    /// Perform a health check against the server's `/health` endpoint.
    pub fn health_check(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let url = format!("{}/health", self.base_url.trim_end_matches('/'));
        let client = self.client.clone();
        let api_key = self.api_key.clone();
        Box::pin(async move {
            let mut req = client.get(&url);
            if let Some(key) = &api_key {
                req = req.bearer_auth(key);
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => Ok(()),
                Ok(resp) => Err(format!(
                    "Server returned {}: {}",
                    resp.status().as_u16(),
                    resp.text().await.unwrap_or_default()
                )),
                Err(e) => Err(format!("Cannot reach {}: {}", url, e)),
            }
        })
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
            self.base_url.trim_end_matches('/')
        )
    }

    /// Fetch the model list from the server's `/v1/models` endpoint.
    /// Returns the list of model IDs, or an empty vec on failure.
    pub fn fetch_model_list(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send>> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let client = self.client.clone();
        let current_model = self.model.clone();
        let api_key = self.api_key.clone();
        Box::pin(async move {
            let mut req = client.get(&url);
            if let Some(key) = &api_key {
                req = req.bearer_auth(key);
            }
            match req.send().await {
                Ok(resp) if resp.status().is_success() => {
                    match resp.json::<ModelListResponse>().await {
                        Ok(list) => list.data.into_iter().map(|m| m.id).collect(),
                        Err(_) => current_model.into_iter().collect(),
                    }
                }
                _ => current_model.into_iter().collect(),
            }
        })
    }

    /// Stream chat completions using the OpenAI-compatible API.
    /// Returns a receiver for streaming response chunks, or an error string
    /// if the request cannot be started.
    pub fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String>>
                + Send,
        >,
    > {
        let (send, recv) = tokio::sync::mpsc::channel::<ChatMessageResponse>(1024);

        let model = self.model.clone().unwrap_or_default();
        let openai_messages = messages.into_iter().map(to_openai_message).collect();
        let openai_tools = tools.into_iter().map(to_openai_tool).collect();
        let client = self.client.clone();
        let chat_url = self.chat_url();
        let api_key = self.api_key.clone();

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
            let _usage =
                stream_chat_completions(&client, &chat_url, &body, api_key.as_deref(), &send).await;
        });

        Box::pin(async move { Ok(recv) })
    }
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

/// Stream chat completions from an OpenAI-compatible endpoint.
/// Returns accumulated tool calls and final content via the sender.
/// Also returns the token usage if available.
///
/// If `api_key` is `Some`, the request includes an
/// `Authorization: Bearer <key>` header.
pub async fn stream_chat_completions(
    client: &reqwest::Client,
    url: &str,
    body: &ChatRequest,
    api_key: Option<&str>,
    send: &tokio::sync::mpsc::Sender<ChatMessageResponse>,
) -> Option<crate::provider::TokenUsage> {
    let mut request = client.post(url).json(body);
    if let Some(key) = api_key {
        request = request.bearer_auth(key);
    }
    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            let _ = send
                .send(ChatMessageResponse {
                    message: ChatMessage {
                        content: format!("Error: {}", e),
                        tool_calls: vec![],
                        thinking: None,
                    },
                    done: true,
                    is_error: true,
                    usage: None,
                })
                .await;
            return None;
        }
    };

    // If the server returned a non-success status, surface the body as an
    // error instead of feeding it into the SSE parser (which would silently
    // drop it).
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        let _ = send
            .send(ChatMessageResponse {
                message: ChatMessage {
                    content: format!("Error: HTTP {} — {}", status.as_u16(), body),
                    tool_calls: vec![],
                    thinking: None,
                },
                done: true,
                is_error: true,
                usage: None,
            })
            .await;
        return None;
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
                // Stream read error — surface it rather than silently breaking
                let _ = send
                    .send(ChatMessageResponse {
                        message: ChatMessage {
                            content: format!("\n\nStream error: {}", e),
                            tool_calls: vec![],
                            thinking: None,
                        },
                        done: true,
                        is_error: true,
                        usage: None,
                    })
                    .await;
                break;
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
            let _ = send
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
                .await;
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

    token_usage
}
