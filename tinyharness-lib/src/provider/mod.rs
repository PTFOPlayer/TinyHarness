#[cfg(feature = "test-util")]
pub mod mock;
pub mod ollama;
pub mod openai_compat;
pub mod openai_compat_provider;
pub mod sockudo;

use std::fmt::Display;
use std::future::Future;

use serde::{Deserialize, Serialize};

use crate::SecretString;
use crate::config::{OllamaThinkType, Settings};
use crate::image::ImageAttachment;
use crate::provider::ollama::OllamaProvider;
use crate::provider::openai_compat_provider::OpenAiCompatProvider;
use crate::provider::sockudo::SockudoProvider;

/// Channel capacity for streaming chat responses from providers to the
/// agent loop.
///
/// A small buffer applies backpressure to the provider's HTTP read loop
/// when the UI consumes chunks slowly: once the buffer is full,
/// `send().await` suspends the provider task instead of letting chunks
/// accumulate without bound. 64 chunks is far more than a terminal renders
/// per frame, so streaming throughput is unaffected in practice.
pub const STREAM_CHANNEL_CAPACITY: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: schemars::Schema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    /// OpenAI tool call ID (e.g. `call_abc123`). Required by OpenAI-compatible
    /// servers when echoing the call back in subsequent assistant messages
    /// and when building matching `tool` result messages. Some servers generate
    /// it for us; others (or local models) may not, in which case we fall
    /// back to a synthetic id at serialization time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: serde_json::Value,
    /// Gemini `thought_signature` required for multi-turn tool calling.
    /// Gemini returns this on tool calls and requires it back in subsequent
    /// turns. Ollama Cloud doesn't preserve it, so we capture and re-inject it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thought_signature: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// Thinking/reasoning content from the model (Ollama's `thinking` field).
    /// Only populated when the model supports reasoning (e.g. qwen2.5 variants).
    #[serde(default)]
    pub thinking: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatMessageResponse {
    pub message: ChatMessage,
    pub done: bool,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

/// Token usage information from the provider.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::System => f.write_str("System"),
            Role::User => f.write_str("User"),
            Role::Assistant => f.write_str("Assistant"),
            Role::Tool => f.write_str("Tool"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    /// OpenAI `tool_call_id` for `Role::Tool` messages — links the result back
    /// to the originating assistant tool call. Required by OpenAI-compatible
    /// servers. `None` is allowed for round-trips with providers that don't
    /// use it, but the OpenAI-compatible serialiser will substitute a
    /// synthetic id when missing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Optional images attached to the message (multimodal models).
    /// Only meaningful for `User` role messages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<ImageAttachment>,
    /// Thinking/reasoning chain from the model, captured during streaming
    /// and persisted for debugging. Not sent to the provider in API requests
    /// (skipped in serialization for request types; only stored in sessions).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
}

impl Default for Message {
    fn default() -> Self {
        Message::simple(Role::User, "")
    }
}

impl Message {
    /// Create a new message with the given role and content, no tool calls, no images.
    pub fn simple(role: Role, content: impl Into<String>) -> Self {
        Message {
            role,
            content: content.into(),
            tool_calls: vec![],
            tool_call_id: None,
            images: vec![],
            thinking: None,
        }
    }
}

/// Trait for LLM provider backends (Ollama, OpenAI-compatible, Sockudo).
///
/// Async methods return `impl Future<...> + Send` (RPITIT) rather than
/// `Pin<Box<dyn Future>>`: no allocation, no extra crates. The trade-off is
/// that the trait is not dyn-compatible — for dynamic dispatch over a
/// runtime-selected backend use [`AnyProvider`], the concrete enum covering
/// all built-in providers.
///
/// Implementations are expected to be cheap to clone where the provider
/// spawns background streaming tasks (see [`Provider::chat`]).
pub trait Provider: Send + Sync {
    /// Check whether the backend is reachable and healthy.
    ///
    /// Called once at startup; a failure is a non-fatal warning — errors
    /// surface on the first real request.
    fn health_check(&self) -> impl Future<Output = Result<(), String>> + Send;

    /// List model names available on the backend.
    ///
    /// Returns an empty vec when the backend doesn't expose a model list or
    /// the request fails; callers fall back to saved or first-known models.
    fn list_models(&self) -> impl Future<Output = Vec<String>> + Send;

    /// Select the model used for subsequent [`Provider::chat`] requests.
    fn select_model(&mut self, name: String);

    /// The currently selected model, if any.
    fn current_model(&self) -> Option<String>;

    /// Send a chat request and return a receiver for streaming response chunks.
    ///
    /// Returns `Err(String)` if the request cannot be started (e.g. no model selected).
    /// On success, the provider spawns a background task that streams `ChatMessageResponse`
    /// chunks through the returned receiver. The caller drains the receiver until it
    /// receives a chunk with `done: true`; dropping the receiver stops the stream.
    ///
    /// Token usage, when available, is included in the final `ChatMessageResponse`
    /// chunk (in the `usage` field). No separate method is needed to retrieve it.
    fn chat(
        &mut self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> impl Future<Output = Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String>> + Send;

    /// Set the request timeout in seconds. Only meaningful for providers that use timeouts.
    fn set_timeout(&mut self, _timeout_secs: u64) {}

    /// Set the maximum number of retries. Only meaningful for providers that use retries.
    fn set_retries(&mut self, _max_retries: u32) {}

    /// Set the think/reasoning level. Only meaningful for Ollama.
    fn set_think_type(&mut self, _think_type: OllamaThinkType) {}
}

/// Concrete wrapper over all built-in provider backends.
///
/// The [`Provider`] trait is not dyn-compatible (its async methods use
/// RPITIT return types), so dynamic dispatch over a runtime-selected
/// backend goes through this enum instead of `Arc<Mutex<dyn Provider>>`.
/// Commands and the agent loop share an `Arc<Mutex<AnyProvider>>`.
pub enum AnyProvider {
    Ollama(crate::provider::ollama::OllamaProvider),
    OpenAiCompat(crate::provider::openai_compat_provider::OpenAiCompatProvider),
    Sockudo(crate::provider::sockudo::SockudoProvider),
    /// Test-only variant. Constructed via `MockProviderHandle` (feature
    /// `test-util`); never built in production code paths.
    #[cfg(feature = "test-util")]
    Mock(crate::provider::mock::MockProviderHandle),
}

impl AnyProvider {
    /// Build the provider for `kind` from resolved settings values.
    ///
    /// `url` and credentials come from the caller (CLI flags / settings);
    /// `timeout_secs`/`max_retries` are the effective per-provider values
    /// from `Settings::effective_timeout_secs()`/`effective_max_retries()`.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        kind: crate::config::ProviderKind,
        base_url: String,
        api_key: Option<SecretString>,
        timeout_secs: u64,
        max_retries: u32,
        think_type: OllamaThinkType,
        sockudo: SockudoCredentials,
    ) -> Result<Self, String> {
        Ok(match kind {
            crate::config::ProviderKind::Ollama => AnyProvider::Ollama(OllamaProvider::new(
                base_url,
                timeout_secs,
                max_retries,
                think_type,
            )?),
            crate::config::ProviderKind::LlamaCpp => AnyProvider::OpenAiCompat(
                OpenAiCompatProvider::with_options(base_url, None, timeout_secs, max_retries)
                    .with_static_models(vec!["llama-cpp".to_string()]),
            ),
            crate::config::ProviderKind::Vllm => AnyProvider::OpenAiCompat(
                OpenAiCompatProvider::with_options(base_url, None, timeout_secs, max_retries),
            ),
            crate::config::ProviderKind::OpenAiCompat => {
                // Empty key = explicit no-auth (e.g. `--api-key ""`).
                let key = api_key.filter(|k| !k.is_empty());
                AnyProvider::OpenAiCompat(OpenAiCompatProvider::with_options(
                    base_url,
                    key,
                    timeout_secs,
                    max_retries,
                ))
            }
            crate::config::ProviderKind::Sockudo => AnyProvider::Sockudo(SockudoProvider::new(
                base_url,
                sockudo.app_id,
                sockudo.app_key,
                sockudo.app_secret.unwrap_or_default(),
            )),
        })
    }

    /// The provider kind this instance wraps.
    ///
    /// Note: `LlamaCpp`/`Vllm` are both backed by `OpenAiCompatProvider`,
    /// so `kind()` cannot distinguish them — it reports `OpenAiCompat` for
    /// both. Use `build()`'s input `ProviderKind` where the exact kind
    /// matters.
    pub fn kind(&self) -> crate::config::ProviderKind {
        match self {
            AnyProvider::Ollama(_) => crate::config::ProviderKind::Ollama,
            AnyProvider::OpenAiCompat(_) => crate::config::ProviderKind::OpenAiCompat,
            AnyProvider::Sockudo(_) => crate::config::ProviderKind::Sockudo,
            #[cfg(feature = "test-util")]
            AnyProvider::Mock(_) => crate::config::ProviderKind::Ollama,
        }
    }
}

/// Credentials needed to construct the Sockudo provider.
#[derive(Debug, Clone, Default)]
pub struct SockudoCredentials {
    pub app_id: String,
    pub app_key: String,
    pub app_secret: Option<SecretString>,
}

impl From<&Settings> for SockudoCredentials {
    fn from(settings: &Settings) -> Self {
        SockudoCredentials {
            app_id: settings.sockudo_app_id.clone().unwrap_or_default(),
            app_key: settings.sockudo_app_key.clone().unwrap_or_default(),
            app_secret: settings.sockudo_app_secret.clone(),
        }
    }
}

impl Provider for AnyProvider {
    async fn health_check(&self) -> Result<(), String> {
        match self {
            AnyProvider::Ollama(p) => p.health_check().await,
            AnyProvider::OpenAiCompat(p) => p.health_check().await,
            AnyProvider::Sockudo(p) => p.health_check().await,
            #[cfg(feature = "test-util")]
            AnyProvider::Mock(p) => p.health_check().await,
        }
    }

    async fn list_models(&self) -> Vec<String> {
        match self {
            AnyProvider::Ollama(p) => p.list_models().await,
            AnyProvider::OpenAiCompat(p) => p.list_models().await,
            AnyProvider::Sockudo(p) => p.list_models().await,
            #[cfg(feature = "test-util")]
            AnyProvider::Mock(p) => p.list_models().await,
        }
    }

    fn select_model(&mut self, name: String) {
        match self {
            AnyProvider::Ollama(p) => p.select_model(name),
            AnyProvider::OpenAiCompat(p) => p.select_model(name),
            AnyProvider::Sockudo(p) => p.select_model(name),
            #[cfg(feature = "test-util")]
            AnyProvider::Mock(p) => p.select_model(name),
        }
    }

    fn current_model(&self) -> Option<String> {
        match self {
            AnyProvider::Ollama(p) => p.current_model(),
            AnyProvider::OpenAiCompat(p) => p.current_model(),
            AnyProvider::Sockudo(p) => p.current_model(),
            #[cfg(feature = "test-util")]
            AnyProvider::Mock(p) => p.current_model(),
        }
    }

    async fn chat(
        &mut self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String> {
        match self {
            AnyProvider::Ollama(p) => p.chat(messages, tools).await,
            AnyProvider::OpenAiCompat(p) => p.chat(messages, tools).await,
            AnyProvider::Sockudo(p) => p.chat(messages, tools).await,
            #[cfg(feature = "test-util")]
            AnyProvider::Mock(p) => p.chat(messages, tools).await,
        }
    }

    fn set_timeout(&mut self, timeout_secs: u64) {
        match self {
            AnyProvider::Ollama(p) => p.set_timeout(timeout_secs),
            AnyProvider::OpenAiCompat(p) => p.set_timeout(timeout_secs),
            AnyProvider::Sockudo(p) => p.set_timeout(timeout_secs),
            #[cfg(feature = "test-util")]
            AnyProvider::Mock(p) => p.set_timeout(timeout_secs),
        }
    }

    fn set_retries(&mut self, max_retries: u32) {
        match self {
            AnyProvider::Ollama(p) => p.set_retries(max_retries),
            AnyProvider::OpenAiCompat(p) => p.set_retries(max_retries),
            AnyProvider::Sockudo(p) => p.set_retries(max_retries),
            #[cfg(feature = "test-util")]
            AnyProvider::Mock(p) => p.set_retries(max_retries),
        }
    }

    fn set_think_type(&mut self, think_type: OllamaThinkType) {
        match self {
            AnyProvider::Ollama(p) => p.set_think_type(think_type),
            AnyProvider::OpenAiCompat(p) => p.set_think_type(think_type),
            AnyProvider::Sockudo(p) => p.set_think_type(think_type),
            #[cfg(feature = "test-util")]
            AnyProvider::Mock(p) => p.set_think_type(think_type),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::mock::{MockProvider, MockProviderHandle};
    use std::sync::{Arc, Mutex as StdMutex};

    #[test]
    fn build_returns_provider_for_each_kind() {
        let creds = SockudoCredentials::default();
        for kind in [
            crate::config::ProviderKind::Ollama,
            crate::config::ProviderKind::LlamaCpp,
            crate::config::ProviderKind::Vllm,
            crate::config::ProviderKind::OpenAiCompat,
            crate::config::ProviderKind::Sockudo,
        ] {
            let provider = AnyProvider::build(
                kind,
                "http://localhost:1234".to_string(),
                Some(SecretString::new("k")),
                30,
                0,
                OllamaThinkType::Medium,
                creds.clone(),
            )
            .unwrap_or_else(|e| panic!("build({kind}) failed: {e}"));
            let _ = provider.current_model();
        }
    }

    #[test]
    fn build_ollama_invalid_url_is_an_error() {
        let creds = SockudoCredentials::default();
        let result = AnyProvider::build(
            crate::config::ProviderKind::Ollama,
            "::not a url::".to_string(),
            None,
            5,
            0,
            OllamaThinkType::Medium,
            creds,
        );
        assert!(result.is_err(), "invalid Ollama URL should be rejected");
    }

    #[tokio::test]
    async fn any_provider_delegates_to_mock() {
        let mock = Arc::new(StdMutex::new(MockProvider::new()));
        mock.lock().unwrap().enqueue_text("via any");
        let mut provider = AnyProvider::Mock(MockProviderHandle::new(mock));

        assert_eq!(provider.current_model().as_deref(), Some("mock-model"));
        provider.select_model("m2".to_string());
        assert_eq!(provider.current_model().as_deref(), Some("m2"));

        let mut rx = provider
            .chat(vec![Message::simple(Role::User, "hi")], vec![])
            .await
            .expect("chat via AnyProvider should work");
        let chunk = rx.recv().await.expect("chunk");
        assert!(chunk.done);
        assert_eq!(chunk.message.content, "via any");
    }

    #[tokio::test]
    async fn any_provider_health_check_delegates() {
        let mut mock = MockProvider::new();
        assert!(mock.health_check().await.is_ok());
        mock.set_health_ok(false);
        assert!(mock.health_check().await.is_err());
    }
}
