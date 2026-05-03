use reqwest::Client;
use tokio::sync::mpsc::Sender;

use crate::provider::{ChatMessageResponse, Message, Provider, ToolInfo};

use super::openai_compat::{
    health_check, stream_chat_completions, to_openai_message, to_openai_tool, ChatRequest,
    ModelListResponse,
};

pub struct VllmProvider {
    client: Client,
    base_url: String,
    model: Option<String>,
}

impl VllmProvider {
    pub fn new(base_url: String) -> Self {
        let client = Client::new();
        VllmProvider {
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
impl Provider for VllmProvider {
    async fn list_models(&self) -> Vec<String> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        match self.client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() => {
                match resp.json::<ModelListResponse>().await {
                    Ok(list) => list.data.into_iter().map(|m| m.id).collect(),
                    Err(_) => self.model.clone().into_iter().collect(),
                }
            }
            _ => self.model.clone().into_iter().collect(),
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
        let model = self.model.clone().unwrap_or_default();
        let url = format!("{}/v1/chat/completions", self.base_url.trim_end_matches('/'));

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
