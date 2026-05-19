use std::future::Future;
use std::pin::Pin;
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
use tokio_stream::StreamExt;

use crate::config::OllamaThinkType;
use crate::provider::{ChatMessage, ChatMessageResponse, Message, Provider, ToolDefinition};

use super::{Role, ToolCall, ToolCallFunction};

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
        let client = Ollama::from_url(base.into_url().unwrap());
        OllamaProvider {
            client,
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
        let client = self.client.clone();

        let chat_messages: Vec<OllamaChatMessage> =
            messages.into_iter().map(|m| m.into()).collect();

        let ollama_tools: Vec<ollama_rs::generation::tools::ToolInfo> =
            tools.into_iter().map(to_ollama_tool_info).collect();

        let mut request = ChatMessageRequest::new(model, chat_messages)
            .think(to_ollama_think_type(self.think_type));

        if !ollama_tools.is_empty() {
            request = request.tools(ollama_tools);
            request.think = None;
        }

        // Spawn the streaming work on a background task
        tokio::spawn(async move {
            // Retry loop with exponential backoff
            let max_attempts = max_retries.max(1);
            let mut stream: Option<_> = None;

            for attempt in 1..=max_attempts {
                let stream_result = tokio::time::timeout(
                    Duration::from_secs(timeout_secs),
                    client.send_chat_messages_stream(request.clone()),
                )
                .await;

                match stream_result {
                    Ok(Ok(s)) => {
                        stream = Some(s);
                        break;
                    }
                    Ok(Err(e)) => {
                        if attempt >= max_attempts {
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
                        if attempt >= max_attempts {
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
            }

            let Some(mut stream) = stream else {
                return;
            };

            while let Some(result) = stream.next().await {
                match result {
                    Ok(res) => {
                        let ours = from_ollama_response(res);
                        let is_done = ours.done;
                        if send.send(ours).await.is_err() {
                            break;
                        }
                        if is_done {
                            break;
                        }
                    }
                    Err(_) => {
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
        });

        Box::pin(async move { Ok(recv) })
    }
}
