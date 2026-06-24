use std::future::Future;
use std::pin::Pin;

use crate::provider::{ChatMessageResponse, Message, Provider, ToolDefinition};

use super::openai_compat::OpenAiCompatInner;

/// Provider for generic OpenAI-compatible API gateways (OpenRouter, Together,
/// custom proxies, etc.) that require a Bearer API key.
///
/// Unlike `LlamaCppProvider` and `VllmProvider` which target local
/// unauthenticated servers, this provider always sends an
/// `Authorization: Bearer <key>` header. The API key is required at
/// construction time.
pub struct OpenAiCompatProvider {
    inner: OpenAiCompatInner,
}

impl OpenAiCompatProvider {
    /// Create a new OpenAI-compatible provider with the given base URL and
    /// API key. The key is sent as `Authorization: Bearer <key>` on every
    /// request (health check, model list, and chat streaming).
    pub fn new(base_url: String, api_key: String) -> Self {
        OpenAiCompatProvider {
            inner: OpenAiCompatInner::with_api_key(base_url, Some(api_key)),
        }
    }
}

impl Provider for OpenAiCompatProvider {
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
