use std::time::Duration;

use ollama_rs::{
    IntoUrlSealed, Ollama,
    generation::{
        chat::{
            ChatMessage as OllamaChatMessage, ChatMessageResponse as OllamaChatMessageResponse,
            request::ChatMessageRequest,
        },
        parameters::ThinkType,
    },
};
use tokio::sync::mpsc::Sender;
use tokio_stream::StreamExt;

use crate::provider::{ChatMessageResponse, Message, Provider, ToolInfo};

use super::{ChatMessage, Role, ToolCall, ToolCallFunction};

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

fn from_ollama_response(resp: OllamaChatMessageResponse) -> ChatMessageResponse {
    let usage = resp.final_data.as_ref().map(|data| super::TokenUsage {
        prompt_tokens: data.prompt_eval_count as u32,
        completion_tokens: data.eval_count as u32,
        total_tokens: (data.prompt_eval_count + data.eval_count) as u32,
    });

    ChatMessageResponse {
        message: ChatMessage {
            content: resp.message.content,
            tool_calls: resp
                .message
                .tool_calls
                .into_iter()
                .map(|tc| ToolCall {
                    function: ToolCallFunction {
                        name: tc.function.name,
                        arguments: tc.function.arguments,
                    },
                })
                .collect(),
        },
        done: resp.done,
        is_error: false,
        usage,
    }
}

fn to_ollama_tool_info(ti: ToolInfo) -> ollama_rs::generation::tools::ToolInfo {
    ollama_rs::generation::tools::ToolInfo {
        tool_type: ollama_rs::generation::tools::ToolType::Function,
        function: ollama_rs::generation::tools::ToolFunctionInfo {
            name: ti.function.name,
            description: ti.function.description,
            parameters: ti.function.parameters,
        },
    }
}

pub struct OllamaProvider {
    client: Ollama,
    model: Option<String>,
    timeout_secs: u64,
    max_retries: u32,
    last_usage: Option<super::TokenUsage>,
}

impl OllamaProvider {
    pub fn new(base: String, timeout_secs: u64, max_retries: u32) -> Self {
        let client = Ollama::from_url(base.into_url().unwrap());
        OllamaProvider {
            client,
            model: None,
            timeout_secs,
            max_retries,
            last_usage: None,
        }
    }
}

#[async_trait::async_trait]
impl Provider for OllamaProvider {
    async fn health_check(&self) -> Result<(), String> {
        self.client
            .list_local_models()
            .await
            .map(|_| ())
            .map_err(|e| format!("Cannot reach Ollama: {}", e))
    }

    async fn list_models(&self) -> Vec<String> {
        if let Ok(models) = self.client.list_local_models().await {
            models.into_iter().map(|m| m.name).collect()
        } else {
            vec![]
        }
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

    fn last_token_usage(&self) -> Option<super::TokenUsage> {
        self.last_usage.clone()
    }

    async fn chat(
        &mut self,
        messages: Vec<Message>,
        _prompt: String,
        send: Sender<ChatMessageResponse>,
        tools: Vec<ToolInfo>,
    ) {
        let model = self.model.clone().expect("Model not set");
        let timeout_secs = self.timeout_secs;
        let max_retries = self.max_retries;

        let chat_messages: Vec<OllamaChatMessage> =
            messages.into_iter().map(|m| m.into()).collect();

        let ollama_tools: Vec<ollama_rs::generation::tools::ToolInfo> =
            tools.into_iter().map(to_ollama_tool_info).collect();

        let mut request = ChatMessageRequest::new(model, chat_messages).think(ThinkType::Medium);

        if !ollama_tools.is_empty() {
            request = request.tools(ollama_tools);
            request.think = None;
        }

        // Retry loop with exponential backoff
        let mut attempt = 0;
        let mut stream = loop {
            attempt += 1;

            let stream_result = tokio::time::timeout(
                Duration::from_secs(timeout_secs),
                self.client.send_chat_messages_stream(request.clone()),
            )
            .await;

            match stream_result {
                Ok(Ok(s)) => break s,
                Ok(Err(e)) => {
                    if attempt >= max_retries {
                        let _ = send
                            .send(ChatMessageResponse {
                                message: ChatMessage {
                                    content: format!("Error after {} retries: {}", attempt, e),
                                    tool_calls: vec![],
                                },
                                done: true,
                                is_error: true,
                                usage: None,
                            })
                            .await;
                        return;
                    }
                }
                Err(_) => {
                    if attempt >= max_retries {
                        let _ = send
                            .send(ChatMessageResponse {
                                message: ChatMessage {
                                    content: format!(
                                        "Error: Request timed out after {} seconds ({} retries)",
                                        timeout_secs, attempt
                                    ),
                                    tool_calls: vec![],
                                },
                                done: true,
                                is_error: true,
                                usage: None,
                            })
                            .await;
                        return;
                    }
                }
            }

            // Exponential backoff: 1s, 2s, 4s, ...
            let backoff = Duration::from_secs(1 << (attempt - 1));
            tokio::time::sleep(backoff).await;
        };

        while let Some(result) = stream.next().await {
            match result {
                Ok(res) => {
                    let ours = from_ollama_response(res);
                    let is_done = ours.done;
                    // Store usage info from the final response
                    if is_done && let Some(usage) = &ours.usage {
                        self.last_usage = Some(usage.clone());
                    }
                    if send.send(ours).await.is_err() {
                        break;
                    }
                    if is_done {
                        break;
                    }
                }
                Err(_) => {
                    // Stream error from Ollama — send it to the agent loop before terminating
                    let _ = send
                        .send(ChatMessageResponse {
                            message: ChatMessage {
                                content: "Stream error: connection to Ollama was lost.".into(),
                                tool_calls: vec![],
                            },
                            done: true,
                            is_error: true,
                            usage: None,
                        })
                        .await;
                    break;
                }
            }
        }
    }
}
