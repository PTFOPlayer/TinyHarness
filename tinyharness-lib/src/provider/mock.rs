//! Dev-only test utility: a mock [`Provider`] with pre-programmed responses.
//!
//! Available under the `test-util` feature. Queued responses are popped in
//! order on each `chat()` call; an empty queue yields a default text
//! response. Used by `tinyharness-lib` unit tests and by the binary crate's
//! command/agent-loop tests (via `AnyProvider::Mock`).

use std::sync::Mutex;

use crate::config::OllamaThinkType;
use crate::provider::{
    AnyProvider, ChatMessage, ChatMessageResponse, Message, Provider, TokenUsage, ToolDefinition,
};

/// A mock provider that returns pre-programmed responses.
///
/// Each call to `chat()` pops the next response from the queue. If the queue
/// is empty, returns a default text response.
pub struct MockProvider {
    model: Option<String>,
    models: Vec<String>,
    /// Queue of pre-programmed responses. Each entry is a list of streaming
    /// chunks; the last chunk in each entry should have `done: true`.
    responses: Mutex<Vec<Vec<ChatMessageResponse>>>,
    pub timeout: u64,
    pub retries: u32,
    pub think_type: OllamaThinkType,
    health_ok: bool,
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl MockProvider {
    pub fn new() -> Self {
        MockProvider {
            model: Some("mock-model".to_string()),
            models: vec!["mock-model".to_string(), "other-model".to_string()],
            responses: Mutex::new(Vec::new()),
            timeout: 5,
            retries: 3,
            think_type: OllamaThinkType::Off,
            health_ok: true,
        }
    }

    /// Queue a simple text response.
    pub fn enqueue_text(&self, text: &str) {
        let chunk = ChatMessageResponse {
            message: ChatMessage {
                content: text.to_string(),
                tool_calls: vec![],
                thinking: None,
            },
            done: true,
            is_error: false,
            usage: Some(TokenUsage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        };
        self.responses.lock().unwrap().push(vec![chunk]);
    }

    /// Queue a tool call response.
    pub fn enqueue_tool_call(&self, tool_name: &str, arguments: serde_json::Value) {
        use crate::provider::{ToolCall, ToolCallFunction};
        let chunk = ChatMessageResponse {
            message: ChatMessage {
                content: String::new(),
                tool_calls: vec![ToolCall {
                    id: Some("call_mock".to_string()),
                    function: ToolCallFunction {
                        name: tool_name.to_string(),
                        arguments,
                        thought_signature: None,
                    },
                }],
                thinking: None,
            },
            done: true,
            is_error: false,
            usage: None,
        };
        self.responses.lock().unwrap().push(vec![chunk]);
    }

    /// Queue a tool call followed by a text response (two turns).
    pub fn enqueue_tool_call_then_text(
        &self,
        tool_name: &str,
        arguments: serde_json::Value,
        text: &str,
    ) {
        self.enqueue_tool_call(tool_name, arguments);
        self.enqueue_text(text);
    }

    /// Queue an error response.
    pub fn enqueue_error(&self, error_msg: &str) {
        let chunk = ChatMessageResponse {
            message: ChatMessage {
                content: error_msg.to_string(),
                tool_calls: vec![],
                thinking: None,
            },
            done: true,
            is_error: true,
            usage: None,
        };
        self.responses.lock().unwrap().push(vec![chunk]);
    }

    pub fn set_health_ok(&mut self, ok: bool) {
        self.health_ok = ok;
    }

    pub fn set_models(&mut self, models: Vec<String>) {
        self.models = models;
    }
}

impl Provider for MockProvider {
    async fn health_check(&self) -> Result<(), String> {
        if self.health_ok {
            Ok(())
        } else {
            Err("mock provider health check failed".to_string())
        }
    }

    async fn list_models(&self) -> Vec<String> {
        self.models.clone()
    }

    fn select_model(&mut self, name: String) {
        self.model = Some(name);
    }

    fn current_model(&self) -> Option<String> {
        self.model.clone()
    }

    async fn chat(
        &mut self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String> {
        let chunks = self.responses.lock().unwrap().pop().unwrap_or_else(|| {
            vec![ChatMessageResponse {
                message: ChatMessage {
                    content: "mock response".to_string(),
                    tool_calls: vec![],
                    thinking: None,
                },
                done: true,
                is_error: false,
                usage: None,
            }]
        });

        let (tx, rx) = tokio::sync::mpsc::channel(super::STREAM_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            for chunk in chunks {
                let _ = tx.send(chunk).await;
            }
        });
        Ok(rx)
    }

    fn set_timeout(&mut self, timeout_secs: u64) {
        self.timeout = timeout_secs;
    }

    fn set_retries(&mut self, max_retries: u32) {
        self.retries = max_retries;
    }

    fn set_think_type(&mut self, think_type: OllamaThinkType) {
        self.think_type = think_type;
    }
}

/// Convenience wrapper so tests can hold an `Arc<Mutex<MockProvider>>`
/// (for enqueuing responses) while handing out an [`AnyProvider`].
pub struct MockProviderHandle(std::sync::Arc<Mutex<MockProvider>>);

impl MockProviderHandle {
    pub fn new(inner: std::sync::Arc<Mutex<MockProvider>>) -> Self {
        MockProviderHandle(inner)
    }

    /// Access the inner mock for enqueuing responses.
    pub fn inner(&self) -> &std::sync::Arc<Mutex<MockProvider>> {
        &self.0
    }
}

impl Provider for MockProviderHandle {
    async fn health_check(&self) -> Result<(), String> {
        // Read state under the lock, then drop the guard before returning —
        // a std::sync::MutexGuard held across an await would make the
        // future non-Send (and risks deadlocks under contention).
        let ok = self.0.lock().unwrap().health_ok;
        if ok {
            Ok(())
        } else {
            Err("mock provider health check failed".to_string())
        }
    }

    async fn list_models(&self) -> Vec<String> {
        self.0.lock().unwrap().models.clone()
    }

    fn select_model(&mut self, name: String) {
        self.0.lock().unwrap().select_model(name);
    }

    fn current_model(&self) -> Option<String> {
        self.0.lock().unwrap().current_model()
    }

    async fn chat(
        &mut self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String> {
        // Pop the queued chunks under the lock, then build the stream
        // outside the guard so the future stays Send.
        let chunks = self
            .0
            .lock()
            .unwrap()
            .responses
            .lock()
            .unwrap()
            .pop()
            .unwrap_or_else(|| {
                vec![ChatMessageResponse {
                    message: ChatMessage {
                        content: "mock response".to_string(),
                        tool_calls: vec![],
                        thinking: None,
                    },
                    done: true,
                    is_error: false,
                    usage: None,
                }]
            });

        let (tx, rx) = tokio::sync::mpsc::channel(super::STREAM_CHANNEL_CAPACITY);
        tokio::spawn(async move {
            for chunk in chunks {
                let _ = tx.send(chunk).await;
            }
        });
        Ok(rx)
    }

    fn set_timeout(&mut self, timeout_secs: u64) {
        self.0.lock().unwrap().set_timeout(timeout_secs);
    }

    fn set_retries(&mut self, max_retries: u32) {
        self.0.lock().unwrap().set_retries(max_retries);
    }

    fn set_think_type(&mut self, think_type: OllamaThinkType) {
        self.0.lock().unwrap().set_think_type(think_type);
    }
}

impl From<MockProviderHandle> for AnyProvider {
    fn from(handle: MockProviderHandle) -> Self {
        AnyProvider::Mock(handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn enqueue_then_chat_returns_queued_chunks() {
        let mut mock = MockProvider::new();
        mock.enqueue_text("hello world");

        let messages = vec![
            Message::simple(crate::provider::Role::System, "sys"),
            Message::simple(crate::provider::Role::User, "hi"),
        ];
        let mut rx = mock
            .chat(messages, vec![])
            .await
            .expect("chat should succeed");

        let mut got_done = false;
        while let Some(chunk) = rx.recv().await {
            if chunk.done {
                assert_eq!(chunk.message.content, "hello world");
                got_done = true;
                break;
            }
        }
        assert!(got_done, "should receive the queued done chunk");
    }

    #[tokio::test]
    async fn empty_queue_yields_default_response() {
        let mut mock = MockProvider::new();
        let messages = vec![Message::simple(crate::provider::Role::User, "hi")];
        let mut rx = mock.chat(messages, vec![]).await.unwrap();
        let chunk = rx.recv().await.expect("default chunk");
        assert!(chunk.done);
        assert_eq!(chunk.message.content, "mock response");
    }
}
