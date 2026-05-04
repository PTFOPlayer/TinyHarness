use tokio::sync::mpsc::Sender;

use crate::provider::{ChatMessageResponse, Message, Provider, ToolInfo};

use super::openai_compat::OpenAiCompatInner;

pub struct LlamaCppProvider {
    inner: OpenAiCompatInner,
}

impl LlamaCppProvider {
    pub fn new(base_url: String) -> Self {
        LlamaCppProvider {
            inner: OpenAiCompatInner::new(base_url),
        }
    }

    pub async fn health_check(&self) -> Result<(), String> {
        self.inner.health_check().await
    }
}

#[async_trait::async_trait]
impl Provider for LlamaCppProvider {
    async fn list_models(&self) -> Vec<String> {
        self.inner.model.clone().into_iter().collect()
    }

    fn select_model(&mut self, name: String) {
        self.inner.select_model(name);
    }

    fn current_model(&self) -> Option<String> {
        self.inner.current_model()
    }

    async fn chat(
        &mut self,
        messages: Vec<Message>,
        prompt: String,
        send: Sender<ChatMessageResponse>,
        tools: Vec<ToolInfo>,
    ) {
        self.inner.chat(messages, prompt, send, tools).await;
    }
}
