
use ollama_rs::{
    IntoUrlSealed, Ollama,
    generation::{
        chat::{ChatMessage as OllamaChatMessage, ChatMessageResponse as OllamaChatMessageResponse, request::ChatMessageRequest},
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
}

impl OllamaProvider {
    pub fn new(base: String) -> Self {
        let client = Ollama::from_url(base.into_url().unwrap());
        OllamaProvider {
            client,
            model: None,
        }
    }
}

#[async_trait::async_trait]
impl Provider for OllamaProvider {
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

    async fn chat(
        &mut self,
        messages: Vec<Message>,
        _prompt: String,
        send: Sender<ChatMessageResponse>,
        tools: Vec<ToolInfo>,
    ) {
        let model = self.model.clone().expect("Model not set");

        let chat_messages: Vec<OllamaChatMessage> =
            messages.into_iter().map(|m| m.into()).collect();

        let ollama_tools: Vec<ollama_rs::generation::tools::ToolInfo> =
            tools.into_iter().map(to_ollama_tool_info).collect();

        let mut request = ChatMessageRequest::new(model, chat_messages)
            .think(ThinkType::Medium);

        if !ollama_tools.is_empty() {
            request = request.tools(ollama_tools);
            request.think = None;
        }

        let stream = self
            .client
            .send_chat_messages_stream(request)
            .await;

        let mut stream = match stream {
            Ok(s) => s,
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

        while let Some(Ok(res)) = stream.next().await {
            let ours = from_ollama_response(res);
            let is_done = ours.done;
            if send.send(ours).await.is_err() {
                break;
            }
            if is_done {
                break;
            }
        }
    }
}
