
use ollama_rs::{
    IntoUrlSealed, Ollama, generation::{chat::{ChatMessage, ChatMessageResponse, request::ChatMessageRequest}, parameters::ThinkType}
};
use tokio::sync::mpsc::Sender;
use tokio_stream::StreamExt;

use crate::provider::{Message, Provider};

use super::Role;

pub struct OllamaProvider {
    client: Ollama,
    model: Option<String>,
}


impl Into<ChatMessage> for Message {
    fn into(self) -> ChatMessage {
        match self.role {
            Role::System => ChatMessage::system(self.content),
            Role::User => ChatMessage::user(self.content),
            Role::Assistant => {
                let mut msg = ChatMessage::assistant(self.content);
                if !self.tool_calls.is_empty() {
                    msg.tool_calls = self.tool_calls;
                }
                msg
            },
        }
    }
}

impl OllamaProvider {
    pub fn new(base: String) -> Self {
        let client = Ollama::from_url(base.clone().into_url().unwrap());

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
            models
                .into_iter()
                .map(|model| model.name)
                .collect::<Vec<_>>()
        } else {
            return vec![];
        }
    }

    fn select_model(&mut self, name: String) {
        self.model = Some(name);
    }

    async fn chat(&mut self, messages: Vec<Message>, _prompt: String, send: Sender<ChatMessageResponse>, tools: Vec<ollama_rs::generation::tools::ToolInfo>) {
        let model = self.model.clone().expect("Model not set");

        let chat_messages: Vec<ChatMessage> = messages
            .into_iter()
            .map(|message| message.into())
            .collect();
        
        let request = ChatMessageRequest::new(model.clone(), chat_messages)
            .think(ThinkType::False)
            .tools(tools);
        
        let mut stream = self
            .client
            .send_chat_messages_stream(request)
            .await
            .unwrap();

        while let Some(Ok(res)) = stream.next().await {
            let is_done = res.done;
            if let Err(e) = send.send(res).await {
                eprintln!("Failed to send response: {}", e);
                break;
            }
            if is_done {
                break;
            }
        }
    }
}
