pub mod ollama;

use std::fmt::Display;

use ollama_rs::generation::{chat::ChatMessageResponse, tools::{ToolCall, ToolInfo}};
use tokio::sync::mpsc::Sender;

#[derive(Debug, Clone, Copy)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => f.write_str("System"),
            Role::User => f.write_str("User"),
            Role::Assistant => f.write_str("Assistant"),
        }
    }
}

#[derive(Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
}

#[async_trait::async_trait]
pub trait Provider {
    async fn list_models(&self) -> Vec<String>;

    fn select_model(&mut self, name: String);

    async fn chat(
        &mut self, 
        messages: Vec<Message>, 
        prompt: String, 
        send: Sender<ChatMessageResponse>,
        tools: Vec<ToolInfo>,
    );
}

