use std::collections::HashMap;

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;
use tokio_stream::StreamExt;

use crate::provider::{ChatMessageResponse, Message, Provider, ToolInfo};

use super::{ChatMessage, Role, ToolCall, ToolCallFunction};

#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<OpenAITool>,
}

#[derive(Serialize)]
struct OpenAIMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Serialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    tool_type: String,
    function: OpenAIToolFunction,
}

#[derive(Serialize)]
struct OpenAIToolFunction {
    name: String,
    description: String,
    parameters: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone)]
struct OpenAIToolCall {
    #[serde(default)]
    index: usize,
    #[serde(default)]
    id: String,
    #[serde(rename = "type", default)]
    call_type: String,
    #[serde(default)]
    function: OpenAIToolCallFunction,
}

#[derive(Serialize, Deserialize, Clone, Default)]
struct OpenAIToolCallFunction {
    #[serde(default)]
    name: String,
    #[serde(default)]
    arguments: String,
}

#[derive(Deserialize)]
struct ChunkChoice {
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
}

pub struct LlamaCppProvider {
    client: Client,
    base_url: String,
    model: Option<String>,
}

impl LlamaCppProvider {
    pub fn new(base_url: String) -> Self {
        let client = Client::new();
        LlamaCppProvider {
            client,
            base_url,
            model: None,
        }
    }

    pub async fn health_check(&self) -> Result<(), String> {
        let url = format!("{}/health", self.base_url.trim_end_matches('/'));
        match self.client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => Ok(()),
            Ok(resp) => Err(format!(
                "Server returned {}: {}",
                resp.status().as_u16(),
                resp.text().await.unwrap_or_default()
            )),
            Err(e) => Err(format!("Cannot reach {}: {}", url, e)),
        }
    }
}


#[async_trait::async_trait]
impl Provider for LlamaCppProvider {
    async fn list_models(&self) -> Vec<String> {
        self.model.clone().into_iter().collect()
    }

    fn select_model(&mut self, name: String) {
        self.model = Some(name);
    }

    fn current_model(&self) -> Option<String> {
        self.model.clone()
    }

    async fn chat(
        &mut self,
        messages: Vec<Message>,
        _prompt: String,
        send: Sender<ChatMessageResponse>,
        tools: Vec<ToolInfo>,
    ) {
        let model = self.model.clone().unwrap_or_default();
        let url = format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'));

        let openai_messages: Vec<OpenAIMessage> = messages.into_iter().map(to_openai_message).collect();
        let openai_tools: Vec<OpenAITool> = tools.into_iter().map(to_openai_tool).collect();

        let body = ChatRequest {
            model,
            messages: openai_messages,
            stream: true,
            tools: openai_tools,
        };

        let response = match self.client.post(&url).json(&body).send().await {
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

        // Accumulate tool calls across chunks (OpenAI sends arguments incrementally)
        let mut acc_tool_calls: HashMap<usize, OpenAIToolCall> = HashMap::new();
        let mut response_content = String::new();
        let mut finish_reason: Option<String> = None;

        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(_) => break,
            };

            buf.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete SSE lines
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

            // Send incremental content if we have any
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

        // Build final tool calls from accumulated state
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
}

fn to_openai_message(msg: Message) -> OpenAIMessage {
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

fn to_openai_tool(ti: ToolInfo) -> OpenAITool {
    OpenAITool {
        tool_type: "function".to_string(),
        function: OpenAIToolFunction {
            name: ti.function.name,
            description: ti.function.description,
            parameters: serde_json::to_value(ti.function.parameters).unwrap_or_default(),
        },
    }
}
