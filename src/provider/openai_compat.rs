use std::collections::HashMap;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;
use tokio_stream::StreamExt;

/// Perform a health check against an OpenAI-compatible server's /health endpoint.
pub async fn health_check(client: &Client, base_url: &str) -> Result<(), String> {
    let url = format!("{}/health", base_url.trim_end_matches('/'));
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) => Err(format!(
            "Server returned {}: {}",
            resp.status().as_u16(),
            resp.text().await.unwrap_or_default()
        )),
        Err(e) => Err(format!("Cannot reach {}: {}", url, e)),
    }
}

use crate::provider::{ChatMessage, ChatMessageResponse, Message, Role, ToolCall, ToolCallFunction, ToolInfo};

// ── OpenAI-compatible request/response types ──

#[derive(Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<OpenAIMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<OpenAITool>,
}

#[derive(Serialize)]
pub struct OpenAIMessage {
    pub role: String,
    pub content: String,
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
    #[serde(default)]
    pub finish_reason: Option<String>,
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
    match msg.role {
        Role::System => OpenAIMessage {
            role: "system".to_string(),
            content: msg.content,
            tool_calls: None,
            tool_call_id: None,
        },
        Role::User => OpenAIMessage {
            role: "user".to_string(),
            content: msg.content,
            tool_calls: None,
            tool_call_id: None,
        },
        Role::Assistant => {
            if msg.tool_calls.is_empty() {
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: msg.content,
                    tool_calls: None,
                    tool_call_id: None,
                }
            } else {
                let tool_calls: Vec<OpenAIToolCall> = msg
                    .tool_calls
                    .into_iter()
                    .enumerate()
                    .map(|(i, tc)| OpenAIToolCall {
                        index: i,
                        id: String::new(),
                        call_type: "function".to_string(),
                        function: OpenAIToolCallFunction {
                            name: tc.function.name,
                            arguments: tc.function.arguments.to_string(),
                        },
                    })
                    .collect();
                OpenAIMessage {
                    role: "assistant".to_string(),
                    content: msg.content,
                    tool_calls: Some(tool_calls),
                    tool_call_id: None,
                }
            }
        }
        Role::Tool => OpenAIMessage {
            role: "tool".to_string(),
            content: msg.content,
            tool_calls: None,
            tool_call_id: None,
        },
    }
}

pub fn to_openai_tool(ti: ToolInfo) -> OpenAITool {
    OpenAITool {
        tool_type: "function".to_string(),
        function: OpenAIToolFunction {
            name: ti.function.name,
            description: ti.function.description,
            parameters: serde_json::to_value(ti.function.parameters).unwrap_or_default(),
        },
    }
}

/// Stream chat completions from an OpenAI-compatible endpoint.
/// Returns accumulated tool calls and final content via the sender.
pub async fn stream_chat_completions(
    client: &reqwest::Client,
    url: &str,
    body: &ChatRequest,
    send: &Sender<ChatMessageResponse>,
) {
    let response = match client.post(url).json(body).send().await {
        Ok(r) => r,
        Err(e) => {
            let _ = send
                .send(ChatMessageResponse {
                    message: ChatMessage {
                        content: format!("Error: {}", e),
                        tool_calls: vec![],
                    },
                    done: true,
                })
                .await;
            return;
        }
    };

    let mut stream = response.bytes_stream();
    let mut buf = String::new();

    let mut acc_tool_calls: HashMap<usize, OpenAIToolCall> = HashMap::new();
    let mut response_content = String::new();
    let mut finish_reason: Option<String> = None;

    while let Some(chunk_result) = stream.next().await {
        let chunk = match chunk_result {
            Ok(c) => c,
            Err(_) => break,
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

                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(chunk) = serde_json::from_str::<StreamChunk>(data) {
                            for choice in chunk.choices {
                                if let Some(ref reason) = choice.finish_reason {
                                    finish_reason = Some(reason.clone());
                                }

                                if let Some(content) = &choice.delta.content {
                                    response_content.push_str(content);
                                }

                                if let Some(tool_calls) = &choice.delta.tool_calls {
                                    for tc in tool_calls {
                                        let entry = acc_tool_calls
                                            .entry(tc.index)
                                            .or_insert(OpenAIToolCall {
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
        }

        if !response_content.is_empty() {
            let _ = send
                .send(ChatMessageResponse {
                    message: ChatMessage {
                        content: response_content.clone(),
                        tool_calls: vec![],
                    },
                    done: false,
                })
                .await;
            response_content.clear();
        }
    }

    let tool_calls: Vec<ToolCall> = if finish_reason.as_deref() == Some("tool_calls") {
        acc_tool_calls
            .into_values()
            .map(|tc| {
                let args: serde_json::Value =
                    serde_json::from_str(&tc.function.arguments).unwrap_or(serde_json::Value::Null);
                ToolCall {
                    function: ToolCallFunction {
                        name: tc.function.name,
                        arguments: args,
                    },
                }
            })
            .collect()
    } else {
        vec![]
    };

    let _ = send
        .send(ChatMessageResponse {
            message: ChatMessage {
                content: String::new(),
                tool_calls,
            },
            done: true,
        })
        .await;
}
