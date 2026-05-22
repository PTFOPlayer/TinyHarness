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
            Role::User => OllamaChatMessage::user(msg.content),
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
    ) -> Self {
        // Normalize URL: ensure it ends with '/'
        let base_url = if base.ends_with('/') {
            base.clone()
        } else {
            format!("{base}/")
        };
        let client = Ollama::from_url(base.into_url().unwrap());
        let http_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(timeout_secs + 60))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        OllamaProvider {
            client,
            http_client,
            base_url,
            model: None,
            timeout_secs,
            max_retries,
            think_type,
        }
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
                messages.into_iter().map(|m| m.into()).collect();

            let ollama_tools: Vec<ollama_rs::generation::tools::ToolInfo> =
                tools.into_iter().map(to_ollama_tool_info).collect();

            let mut req = ChatMessageRequest::new(model, chat_messages)
                .think(to_ollama_think_type(self.think_type));

            if !ollama_tools.is_empty() {
                req = req.tools(ollama_tools);
                req.think = None;
            }
            req
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
                    stream_ollama_chat(&http_client, &base_url, &request, &send),
                )
                .await;

                match result {
                    Ok(Ok(())) => return, // success
                    Ok(Err(e)) => {
                        if attempt >= max_attempts {
                            send_err(format!("Error after {attempt} retries: {e}"));
                            return;
                        }
                    }
                    Err(_) => {
                        if attempt >= max_attempts {
                            send_err(format!(
                                "Error: Request timed out after {timeout_secs}s ({attempt} retries)"
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
    fn to_chat_message_response(&self) -> ChatMessageResponse {
        let usage = if let Some(u) = &self.usage {
            // OpenAI-compatible format (cloud proxy)
            Some(TokenUsage {
                prompt_tokens: u.prompt_tokens,
                completion_tokens: u.completion_tokens,
                total_tokens: u.total_tokens,
            })
        } else if self.prompt_eval_count.is_some() || self.eval_count.is_some() {
            // Native Ollama format
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

        ChatMessageResponse {
            message: ChatMessage {
                content: self.message.content.clone(),
                thinking: self.message.thinking.clone(),
                tool_calls: self
                    .message
                    .tool_calls
                    .clone()
                    .into_iter()
                    .map(|tc| ToolCall {
                        function: ToolCallFunction {
                            name: tc.function.name,
                            arguments: tc.function.arguments,
                        },
                    })
                    .collect(),
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
) -> Result<(), String> {
    let url = format!("{base_url}api/chat");
    let request = serde_json::to_value(request).map_err(|e| format!("serialize: {e}"))?;

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
            buffer = buffer[pos + 1..].to_string();

            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<OllamaChunk>(&line) {
                Ok(chunk) => {
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
        && let Ok(chunk) = serde_json::from_str::<OllamaChunk>(buffer.trim())
    {
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
