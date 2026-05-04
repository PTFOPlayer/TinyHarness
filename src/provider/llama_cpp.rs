use reqwest::Client;
use tokio::sync::mpsc::Sender;

use crate::provider::{ChatMessageResponse, Message, Provider, ToolInfo};

use super::openai_compat::{
    ChatRequest, health_check, stream_chat_completions, to_openai_message, to_openai_tool,
};

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
        health_check(&self.client, &self.base_url).await
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
        let url = format!(
            "{}/v1/chat/completions",
            self.base_url.trim_end_matches('/')
        );

        let openai_messages = messages.into_iter().map(to_openai_message).collect();
        let openai_tools = tools.into_iter().map(to_openai_tool).collect();

        let body = ChatRequest {
            model,
            messages: openai_messages,
            stream: true,
            tools: openai_tools,
        };

        stream_chat_completions(&self.client, &url, &body, &send).await;
    }
}
