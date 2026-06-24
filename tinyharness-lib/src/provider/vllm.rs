use std::future::Future;
use std::pin::Pin;

use crate::provider::{ChatMessageResponse, Message, Provider, ToolDefinition};

use super::openai_compat::OpenAiCompatInner;

pub struct VllmProvider {
    inner: OpenAiCompatInner,
}

impl VllmProvider {
    pub fn new(base_url: String) -> Self {
        Self::with_api_key(base_url, None)
    }

    /// Create a new vLLM provider, optionally sending an
    /// `Authorization: Bearer <api_key>` header on every request.
    pub fn with_api_key(base_url: String, api_key: Option<String>) -> Self {
        VllmProvider {
            inner: OpenAiCompatInner::with_api_key(base_url, api_key),
        }
    }
}

impl Provider for VllmProvider {
    fn health_check(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        self.inner.health_check()
    }

    fn list_models(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send>> {
        self.inner.fetch_model_list()
    }

    fn select_model(&mut self, name: String) {
        self.inner.select_model(name);
    }

    fn current_model(&self) -> Option<String> {
        self.inner.current_model()
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
        self.inner.chat(messages, tools)
    }
}
