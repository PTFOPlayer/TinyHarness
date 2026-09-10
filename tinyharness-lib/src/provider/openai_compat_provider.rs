use std::future::Future;
use std::pin::Pin;

use crate::{
    SecretString,
    provider::{ChatMessageResponse, Message, Provider, ToolDefinition},
};

use super::openai_compat::OpenAiCompatInner;

/// Unified provider for any OpenAI-compatible chat-completions endpoint.
///
/// Covers three concrete back-ends that previously had their own wrapper
/// modules:
///
/// * Local llama.cpp servers (no auth, hard-coded single-model list).
/// * Local vLLM servers (no auth, fetches `/v1/models`).
/// * Hosted gateways like OpenRouter / Together / Groq (Bearer auth,
///   fetches `/v1/models`).
///
/// The variation between them is just (a) whether an `Authorization: Bearer`
/// header is sent and (b) how `list_models` resolves, so they collapse into
/// a single struct with builder-style options.
pub struct OpenAiCompatProvider {
    inner: OpenAiCompatInner,
    /// When `Some`, `list_models` returns this fixed list instead of querying
    /// the server. Used for back-ends like llama.cpp that don't expose a
    /// useful `/v1/models` endpoint.
    static_models: Option<Vec<String>>,
}

impl OpenAiCompatProvider {
    /// Create a new provider with no authentication. Suitable for local
    /// unauthenticated servers (llama.cpp, vLLM, etc.).
    pub fn new(base_url: String) -> Self {
        OpenAiCompatProvider {
            inner: OpenAiCompatInner::new(base_url),
            static_models: None,
        }
    }

    /// Create a new provider that sends `Authorization: Bearer <api_key>`
    /// on every request. Required by hosted OpenAI-compatible gateways
    /// (OpenRouter, Together, custom proxies, etc.).
    pub fn with_api_key(base_url: String, api_key: SecretString) -> Self {
        OpenAiCompatProvider {
            inner: OpenAiCompatInner::with_api_key(base_url, Some(api_key)),
            static_models: None,
        }
    }

    /// Create a new provider with explicit timeout/retry configuration.
    ///
    /// `timeout_secs` bounds each request attempt; `max_retries` is the
    /// number of attempts for transient failures (1 = no retries).
    pub fn with_options(
        base_url: String,
        api_key: Option<SecretString>,
        timeout_secs: u64,
        max_retries: u32,
    ) -> Self {
        OpenAiCompatProvider {
            inner: OpenAiCompatInner::with_options(base_url, api_key, timeout_secs, max_retries),
            static_models: None,
        }
    }

    /// Override `list_models` to return a fixed list instead of querying
    /// the server. Useful for back-ends like llama.cpp that serve a single
    /// model and don't expose a meaningful model-listing endpoint.
    pub fn with_static_models(mut self, models: Vec<String>) -> Self {
        self.static_models = Some(models);
        self
    }
}

impl Provider for OpenAiCompatProvider {
    fn health_check(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        self.inner.health_check()
    }

    fn list_models(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send>> {
        if let Some(models) = &self.static_models {
            let models = models.clone();
            return Box::pin(async move { models });
        }
        self.inner.fetch_model_list()
    }

    fn select_model(&mut self, name: String) {
        self.inner.select_model(name);
    }

    fn current_model(&self) -> Option<String> {
        self.inner.current_model()
    }

    fn set_timeout(&mut self, timeout_secs: u64) {
        self.inner.set_timeout(timeout_secs);
    }

    fn set_retries(&mut self, max_retries: u32) {
        self.inner.set_retries(max_retries);
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
