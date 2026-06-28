use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use ollama_rs::{
    IntoUrlSealed, Ollama,
    generation::{
        chat::{ChatMessage as OllamaChatMessage, request::ChatMessageRequest},
        parameters::ThinkType,
    },
};
use serde::Deserialize;
use tokio_stream::StreamExt;

use crate::config::OllamaThinkType;
use crate::provider::{ChatMessage, ChatMessageResponse, Message, Provider, ToolDefinition};

use super::{Role, TokenUsage, ToolCall, ToolCallFunction};

impl From<Message> for OllamaChatMessage {
    fn from(msg: Message) -> Self {
        match msg.role {
            Role::System => OllamaChatMessage::system(msg.content),
            Role::User => {
                let mut m = OllamaChatMessage::user(msg.content);
                if !msg.images.is_empty() {
                    let images: Vec<ollama_rs::generation::images::Image> = msg
                        .images
                        .iter()
                        .map(|img| {
                            ollama_rs::generation::images::Image::from_base64(
                                img.base64_data.clone(),
                            )
                        })
                        .collect();
                    m.images = Some(images);
                }
                m
            }
            Role::Assistant => {
                let mut m = OllamaChatMessage::assistant(msg.content);
                if !msg.tool_calls.is_empty() {
                    m.tool_calls = msg
                        .tool_calls
                        .into_iter()
                        .map(|tc| ollama_rs::generation::tools::ToolCall {
                            function: ollama_rs::generation::tools::ToolCallFunction {
                                name: tc.function.name,
                                arguments: tc.function.arguments,
                            },
                        })
                        .collect();
                }
                m
            }
            Role::Tool => OllamaChatMessage::tool(msg.content),
        }
    }
}

fn to_ollama_tool_info(ti: ToolDefinition) -> ollama_rs::generation::tools::ToolInfo {
    ollama_rs::generation::tools::ToolInfo {
        tool_type: ollama_rs::generation::tools::ToolType::Function,
        function: ollama_rs::generation::tools::ToolFunctionInfo {
            name: ti.name,
            description: ti.description,
            parameters: ti.parameters,
        },
    }
}

fn to_ollama_think_type(tt: OllamaThinkType) -> ThinkType {
    match tt {
        OllamaThinkType::Off => ThinkType::False,
        OllamaThinkType::Low => ThinkType::Low,
        OllamaThinkType::Medium => ThinkType::Medium,
        OllamaThinkType::High => ThinkType::High,
    }
}

pub struct OllamaProvider {
    client: Ollama,
    http_client: reqwest::Client,
    base_url: String,
    model: Option<String>,
    timeout_secs: u64,
    max_retries: u32,
    think_type: OllamaThinkType,
}

impl OllamaProvider {
    pub fn new(
        base: String,
        timeout_secs: u64,
        max_retries: u32,
        think_type: OllamaThinkType,
    ) -> Result<Self, String> {
        // Normalize URL: ensure it ends with '/'
        let base_url = if base.ends_with('/') {
            base
        } else {
            format!("{base}/")
        };
        // Parse the (normalized) URL once, propagating errors instead of panicking
        // on user-supplied configuration.
        let parsed_url = base_url
            .clone()
            .into_url()
            .map_err(|e| format!("Invalid Ollama base URL `{base_url}`: {e}"))?;
        let client = Ollama::from_url(parsed_url);
        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(timeout_secs + 60))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Ok(OllamaProvider {
            client,
            http_client,
            base_url,
            model: None,
            timeout_secs,
            max_retries,
            think_type,
        })
    }
}

impl Provider for OllamaProvider {
    fn health_check(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let client = self.client.clone();
        Box::pin(async move {
            match client.list_local_models().await {
                Ok(_) => Ok(()),
                Err(e) => Err(format!("Cannot reach Ollama: {}", e)),
            }
        })
    }

    fn list_models(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send>> {
        let client = self.client.clone();
        Box::pin(async move {
            match client.list_local_models().await {
                Ok(models) => models.into_iter().map(|m| m.name).collect(),
                Err(_) => vec![],
            }
        })
    }

    fn select_model(&mut self, name: String) {
        self.model = Some(name);
    }

    fn current_model(&self) -> Option<String> {
        self.model.clone()
    }

    fn set_timeout(&mut self, timeout_secs: u64) {
        self.timeout_secs = timeout_secs;
    }

    fn set_retries(&mut self, max_retries: u32) {
        self.max_retries = max_retries;
    }

    fn set_think_type(&mut self, think_type: OllamaThinkType) {
        self.think_type = think_type;
    }

    fn chat(
        &mut self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Pin<
        Box<
            dyn Future<Output = Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String>>
                + Send,
        >,
    > {
        let model = match self.model.clone() {
            Some(m) => m,
            None => {
                return Box::pin(async move {
                    Err("No model selected. Use /model <name> to select one.".to_string())
                });
            }
        };
        let (send, recv) = tokio::sync::mpsc::channel::<ChatMessageResponse>(1024);
        let timeout_secs = self.timeout_secs;
        let max_retries = self.max_retries;
        let http_client = self.http_client.clone();
        let base_url = self.base_url.clone();
        let request = {
            let chat_messages: Vec<OllamaChatMessage> =
                messages.iter().map(|m| m.clone().into()).collect();

            // Collect Gemini thought_signatures from incoming messages before
            // ollama-rs conversion drops them. These must be re-injected into
            // the serialized request so Gemini accepts multi-turn tool calling.
            let thought_signatures: Vec<Vec<Option<String>>> = messages
                .iter()
                .map(|m| {
                    m.tool_calls
                        .iter()
                        .map(|tc| tc.function.thought_signature.clone())
                        .collect()
                })
                .collect();

            let ollama_tools: Vec<ollama_rs::generation::tools::ToolInfo> =
                tools.into_iter().map(to_ollama_tool_info).collect();

            let mut req = ChatMessageRequest::new(model, chat_messages)
                .think(to_ollama_think_type(self.think_type));

            if !ollama_tools.is_empty() {
                req = req.tools(ollama_tools);
                req.think = None;
            }
            (req, thought_signatures)
        };

        // Spawn the streaming work on a background task.
        //
        // We use our own raw SSE parser instead of ollama-rs's streaming to handle
        // both Ollama-native usage format (flat prompt_eval_count/eval_count) and
        // cloud proxy / OpenAI-compatible format (nested usage object). The ollama-rs
        // library only recognises the flat format, so cloud proxies that return
        // {"usage": {"prompt_tokens": ..., "completion_tokens": ..., "total_tokens": ...}}
        // would lose their usage data.
        tokio::spawn(async move {
            let (request, thought_signatures) = request;
            // Retry loop with exponential backoff
            let max_attempts = max_retries.max(1);

            let send_err = |msg: String| {
                let _ = send.try_send(ChatMessageResponse {
                    message: ChatMessage {
                        content: msg,
                        tool_calls: vec![],
                        thinking: None,
                    },
                    done: true,
                    is_error: true,
                    usage: None,
                });
            };

            for attempt in 1..=max_attempts {
                let result = tokio::time::timeout(
                    Duration::from_secs(timeout_secs),
                    stream_ollama_chat(
                        &http_client,
                        &base_url,
                        &request,
                        &send,
                        &thought_signatures,
                    ),
                )
                .await;

                match result {
                    Ok(Ok(())) => return, // success
                    Ok(Err(e)) => {
                        if attempt >= max_attempts {
                            send_err(format!("Error after {attempt} attempt(s): {e}"));
                            return;
                        }
                    }
                    Err(_) => {
                        if attempt >= max_attempts {
                            send_err(format!(
                                "Error: Request timed out after {timeout_secs}s ({attempt} attempt(s))"
                            ));
                            return;
                        }
                    }
                }

                // Exponential backoff: 1s, 2s, 4s, ...
                let backoff = Duration::from_secs(1 << (attempt - 1));
                tokio::time::sleep(backoff).await;
            }
        });

        Box::pin(async move { Ok(recv) })
    }
}

// ── Raw Ollama SSE streaming ────────────────────────────────────────────────

/// A raw Ollama SSE chunk that accepts **both** native Ollama and
/// OpenAI-compatible usage formats.
///
/// Ollama-native format has flat fields:
/// ```json
/// { "prompt_eval_count": 123, "eval_count": 45, ... }
/// ```
///
/// Some cloud proxies return an OpenAI-compatible nested `usage` object:
/// ```json
/// { "usage": { "prompt_tokens": 123, "completion_tokens": 45, "total_tokens": 168 } }
/// ```
///
/// We try both sources and prefer the nested `usage` object when present.
#[derive(Debug, Clone, Deserialize)]
struct OllamaChunk {
    #[serde(default)]
    message: OllamaChunkMessage,
    #[serde(default)]
    done: bool,
    // Native Ollama flat fields
    #[serde(default)]
    prompt_eval_count: Option<u64>,
    #[serde(default)]
    eval_count: Option<u64>,
    // OpenAI-compatible nested usage (cloud proxies)
    #[serde(default)]
    usage: Option<OllamaUsage>,
    /// Raw JSON for the tool_calls array from the message, used to extract
    /// Gemini-specific fields like `thought_signature` that ollama-rs drops.
    #[serde(skip, default)]
    raw_tool_calls_json: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct OllamaUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct OllamaChunkMessage {
    #[serde(default)]
    content: String,
    #[serde(default)]
    thinking: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ollama_rs::generation::tools::ToolCall>,
}

impl OllamaChunk {
    /// Set raw tool calls JSON from deserialized chunk line.
    fn capture_raw_tool_calls(&mut self, raw_line: &serde_json::Value) {
        if let Some(msg) = raw_line.get("message")
            && let Some(tcs) = msg.get("tool_calls")
        {
            self.raw_tool_calls_json = Some(tcs.clone());
        }
    }

    fn to_chat_message_response(&self) -> ChatMessageResponse {
        let usage = if let Some(u) = &self.usage {
            Some(TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            })
        } else if self.prompt_eval_count.is_some() || self.eval_count.is_some() {
            let prompt = self.prompt_eval_count.unwrap_or(0) as u32;
            let completion = self.eval_count.unwrap_or(0) as u32;
            Some(TokenUsage {
                prompt_tokens: prompt,
                completion_tokens: completion,
                total_tokens: prompt.saturating_add(completion),
            })
        } else {
            None
        };

        // Build tool_calls list. Use raw JSON when available to extract
        // Gemini `thought_signature` fields that ollama-rs drops.
        let mut raw_tc_iter = self
            .raw_tool_calls_json
            .as_ref()
            .and_then(|v| v.as_array())
            .map(|a| a.iter())
            .into_iter()
            .flatten();
        let tool_calls: Vec<ToolCall> = self
            .message
            .tool_calls
            .clone()
            .into_iter()
            .map(|tc| {
                let thought_signature = raw_tc_iter
                    .next()
                    .and_then(|raw| {
                        raw.get("function")
                            .and_then(|f| f.get("thought_signature"))
                            .and_then(|ts| ts.as_str())
                    })
                    .map(|s| s.to_string());
                ToolCall {
                    id: None,
                    function: ToolCallFunction {
                        name: tc.function.name,
                        arguments: tc.function.arguments,
                        thought_signature,
                    },
                }
            })
            .collect();

        ChatMessageResponse {
            message: ChatMessage {
                content: self.message.content.clone(),
                thinking: self.message.thinking.clone(),
                tool_calls,
            },
            done: self.done,
            is_error: false,
            usage,
        }
    }
}

/// Stream chat completions from the Ollama `/api/chat` endpoint using raw SSE parsing.
///
/// This bypasses ollama-rs's `send_chat_messages_stream` so we can recognise both
/// native Ollama and OpenAI-compatible usage formats in the response chunks.
async fn stream_ollama_chat(
    client: &reqwest::Client,
    base_url: &str,
    request: &ChatMessageRequest,
    send: &tokio::sync::mpsc::Sender<ChatMessageResponse>,
    thought_signatures: &[Vec<Option<String>>],
) -> Result<(), String> {
    let url = format!("{base_url}api/chat");
    let mut request = serde_json::to_value(request).map_err(|e| format!("serialize: {e}"))?;

    // Force streaming — ollama-rs sets `stream: true` only inside its own
    // `send_chat_messages_stream()` method, which we bypass. Without this,
    // Ollama returns the entire response as a single JSON object instead of
    // streaming line-by-line, causing the UI to block until the full message
    // arrives.
    request["stream"] = serde_json::Value::Bool(true);

    // Fix ollama-rs 0.3.4 serialization quirks for Ollama Cloud / Gemini compatibility:
    //
    // 1. ToolType::Function serializes as "Function" (uppercase F), but the
    //    API spec requires lowercase "function". Ollama Cloud / Gemini rejects
    //    the uppercase variant with "Invalid tool type". Fix by lowercasing.
    // Fix 2: "role": "tool" messages need a "name" field (tool_name) so Gemini
    //    can match results to calls. ollama-rs doesn't add this, so we inject
    //    it from the preceding assistant message's tool_calls.
    // 3. Re-inject Gemini `thought_signature` fields captured from tool calls.
    fn fix_request_json(value: &mut serde_json::Value, thought_signatures: &[Vec<Option<String>>]) {
        match value {
            serde_json::Value::Object(map) => {
                // Fix 1: Lowercase "Function" → "function" in tool definitions
                if let Some(t) = map.get_mut("type")
                    && t.as_str() == Some("Function")
                {
                    *t = serde_json::Value::String("function".to_string());
                }
                for v in map.values_mut() {
                    fix_request_json(v, thought_signatures);
                }
            }
            serde_json::Value::Array(arr) => {
                // Fix 2: Add "name" field to tool result messages by tracking
                // tool_calls from the most recent assistant message.
                // Also inject "tool_call_id" for OpenAI-compatible backends.
                // Fix 3: Re-inject thought_signatures into assistant tool_calls.
                //
                // `thought_signatures` is indexed by the position of each
                // message in the original `messages` slice (one entry per
                // message, regardless of role). We advance `ts_idx` on every
                // message so the indices stay aligned; otherwise non-assistant
                // messages (system, user, tool) cause off-by-N misalignment
                // and signatures are never re-injected, silently breaking
                // Gemini multi-turn tool calling.
                let mut prev_tool_names: Vec<String> = Vec::new();
                let mut prev_tool_ids: Vec<String> = Vec::new();
                let mut ts_idx: usize = 0;
                let looks_like_messages_array = arr.iter().any(|v| {
                    v.as_object()
                        .and_then(|o| o.get("role"))
                        .and_then(|r| r.as_str())
                        .is_some()
                });
                if looks_like_messages_array {
                    for msg in arr.iter_mut() {
                        if let serde_json::Value::Object(msg_map) = msg {
                            let role = msg_map.get("role").and_then(|v| v.as_str()).unwrap_or("");
                            match role {
                                "assistant" => {
                                    prev_tool_names.clear();
                                    prev_tool_ids.clear();
                                    if let Some(tcs) = msg_map.get_mut("tool_calls")
                                        && let Some(tc_arr) = tcs.as_array_mut()
                                    {
                                        // Re-inject thought_signatures
                                        if let Some(sigs) = thought_signatures.get(ts_idx) {
                                            for (i, tc) in tc_arr.iter_mut().enumerate() {
                                                if let Some(sig) =
                                                    sigs.get(i).and_then(|s| s.clone())
                                                    && let Some(func) = tc.get_mut("function")
                                                    && let Some(obj) = func.as_object_mut()
                                                {
                                                    obj.insert(
                                                        "thought_signature".to_string(),
                                                        serde_json::Value::String(sig),
                                                    );
                                                }
                                            }
                                        }

                                        // Track tool names and ids for Fix 2
                                        for (i, tc) in tc_arr.iter().enumerate() {
                                            if let Some(name) = tc
                                                .get("function")
                                                .and_then(|f| f.get("name"))
                                                .and_then(|n| n.as_str())
                                            {
                                                prev_tool_names.push(name.to_string());
                                            }
                                            // Track tool call ids (synthesize if missing)
                                            let id = tc
                                                .get("id")
                                                .and_then(|v| v.as_str())
                                                .filter(|s| !s.is_empty())
                                                .map(|s| s.to_string())
                                                .unwrap_or_else(|| format!("call_{}", i));
                                            prev_tool_ids.push(id);
                                        }
                                    }
                                }
                                "tool" => {
                                    // Add "name" field for Ollama/Gemini compatibility
                                    // (only when there's exactly one tool call)
                                    if !msg_map.contains_key("name") && prev_tool_names.len() == 1 {
                                        msg_map.insert(
                                            "name".to_string(),
                                            serde_json::Value::String(prev_tool_names[0].clone()),
                                        );
                                    }
                                    // Add "tool_call_id" for OpenAI-compatible backends
                                    if !msg_map.contains_key("tool_call_id")
                                        && prev_tool_ids.len() == 1
                                    {
                                        msg_map.insert(
                                            "tool_call_id".to_string(),
                                            serde_json::Value::String(prev_tool_ids[0].clone()),
                                        );
                                    }
                                }
                                _ => {}
                            }
                        }
                        // Always advance — `thought_signatures` is 1:1 with
                        // the original `messages` Vec, including non-assistant
                        // entries.
                        ts_idx += 1;
                    }
                }
                // Recurse into children for non-message fixes (e.g. lowercasing
                // tool `type`). We pass an empty slice to avoid re-applying
                // thought_signatures from index 0 in any nested array.
                for v in arr.iter_mut() {
                    fix_request_json(v, &[]);
                }
            }
            _ => {}
        }
    }
    fix_request_json(&mut request, thought_signatures);

    let response = client
        .post(&url)
        .json(&request)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "HTTP {}: {}",
            response.status().as_u16(),
            response.text().await.unwrap_or_default()
        ));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk_result) = stream.next().await {
        let chunk = chunk_result.map_err(|e| format!("stream read: {e}"))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        // Process complete SSE lines
        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer.drain(..=pos);

            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<OllamaChunk>(&line) {
                Ok(mut chunk) => {
                    // Capture raw JSON before extracting to preserve Gemini-
                    // specific fields like `thought_signature`.
                    if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&line) {
                        chunk.capture_raw_tool_calls(&raw);
                    }
                    let done = chunk.done;
                    let ours = chunk.to_chat_message_response();
                    if send.send(ours).await.is_err() {
                        return Ok(()); // receiver dropped
                    }
                    if done {
                        return Ok(());
                    }
                }
                Err(_) => {
                    // Non-JSON line (e.g. "data: [DONE]" or empty) — skip silently
                }
            }
        }
    }

    // Process remaining buffer
    if !buffer.trim().is_empty()
        && let Ok(mut chunk) = serde_json::from_str::<OllamaChunk>(buffer.trim())
    {
        if let Ok(raw) = serde_json::from_str::<serde_json::Value>(buffer.trim()) {
            chunk.capture_raw_tool_calls(&raw);
        }
        let done = chunk.done;
        let ours = chunk.to_chat_message_response();
        if send.send(ours).await.is_err() {
            return Ok(());
        }
        if done {
            return Ok(());
        }
    }

    // Stream ended without a done marker — send a synthetic done response
    let _ = send
        .send(ChatMessageResponse {
            message: ChatMessage {
                content: String::new(),
                tool_calls: vec![],
                thinking: None,
            },
            done: true,
            is_error: false,
            usage: None,
        })
        .await;

    Ok(())
}
