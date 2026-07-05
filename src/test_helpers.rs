//! Shared test helpers: mock provider, captured output, context factory.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use tinyharness_lib::config::OllamaThinkType;
use tinyharness_lib::context::WorkspaceContext;
use tinyharness_lib::provider::{
    ChatMessage, ChatMessageResponse, Message, Provider, Role, TokenUsage, ToolDefinition,
};

use tokio::sync::Mutex as TokioMutex;

use crate::commands::registry::CommandContext;

// ── MockProvider ────────────────────────────────────────────────────────────

/// A mock provider that returns pre-programmed responses.
///
/// Each call to `chat()` pops the next response from the queue. If the queue
/// is empty, returns a default text response.
pub struct MockProvider {
    model: Option<String>,
    models: Vec<String>,
    /// Queue of pre-programmed responses. Each entry is a list of streaming chunks.
    /// The last chunk in each entry should have `done: true`.
    responses: Mutex<Vec<Vec<ChatMessageResponse>>>,
    timeout: u64,
    retries: u32,
    think_type: OllamaThinkType,
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
        use tinyharness_lib::provider::{ToolCall, ToolCallFunction};
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
    fn health_check(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
        let ok = self.health_ok;
        Box::pin(async move {
            if ok {
                Ok(())
            } else {
                Err("mock provider health check failed".to_string())
            }
        })
    }

    fn list_models(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + Send>> {
        let models = self.models.clone();
        Box::pin(async move { models })
    }

    fn select_model(&mut self, name: String) {
        self.model = Some(name);
    }

    fn current_model(&self) -> Option<String> {
        self.model.clone()
    }

    fn chat(
        &mut self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String>,
                > + Send,
        >,
    > {
        let mut queue = self.responses.lock().unwrap();
        let chunks = queue.pop().unwrap_or_else(|| {
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

        Box::pin(async move {
            let (tx, rx) = tokio::sync::mpsc::channel(32);
            tokio::spawn(async move {
                for chunk in chunks {
                    let _ = tx.send(chunk).await;
                }
            });
            Ok(rx)
        })
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

// ── Captured output ─────────────────────────────────────────────────────────

/// Create an `Output` that captures everything written to it.
/// Returns the output and a shared buffer for inspection.
pub fn captured_output() -> (tinyharness_ui::output::Output, Arc<Mutex<Vec<u8>>>) {
    use std::io::Write;
    use std::sync::Mutex;

    struct CaptureWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for CaptureWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = CaptureWriter { buf: buf.clone() };
    let output = tinyharness_ui::output::Output::new(Box::new(writer));
    (output, buf)
}

/// Get captured output as a String.
pub fn captured_string(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8(buf.lock().unwrap().clone()).unwrap_or_default()
}

/// Strip ANSI SGR sequences from a string for content assertions.
pub fn strip_ansi(s: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(s, "").to_string()
}

// ── CommandContext factory ──────────────────────────────────────────────────

/// Build a `CommandContext` with a mock provider for testing.
pub fn make_context() -> (CommandContext, Arc<Mutex<MockProvider>>) {
    let mock = Arc::new(Mutex::new(MockProvider::new()));
    let provider: Arc<TokioMutex<dyn Provider + Send + Sync>> = {
        // We need to share MockProvider behind a tokio Mutex.
        // Since MockProvider: Send + Sync, this works.
        // We'll use a trick: wrap it in a newtype that implements Provider.
        let mock_clone = mock.clone();
        // Create a thin wrapper that delegates to the Arc<Mutex<MockProvider>>
        Arc::new(TokioMutex::new(MockProviderHandle(mock_clone)))
    };

    let workspace_ctx = WorkspaceContext {
        root: std::env::current_dir().unwrap_or_default(),
        project_type: "Test".to_string(),
        project_name: "test-project".to_string(),
        is_git_repo: false,
        build_command: "cargo build".to_string(),
        test_command: "cargo test".to_string(),
        structure: vec![],
        project_md: None,
        additional_project_mds: vec![],
    };

    let prompts_dir = std::env::temp_dir().join("tinyharness-test-prompts");
    std::fs::create_dir_all(&prompts_dir).ok();

    let ctx = CommandContext::new(provider, workspace_ctx, prompts_dir);
    (ctx, mock)
}

/// Wrapper to allow `Arc<Mutex<MockProvider>>` to be used as a `Provider`
/// behind a `tokio::sync::Mutex<dyn Provider>`.
struct MockProviderHandle(Arc<Mutex<MockProvider>>);

impl Provider for MockProviderHandle {
    fn health_check(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
        let ok = self.0.lock().unwrap().health_ok;
        Box::pin(async move {
            if ok {
                Ok(())
            } else {
                Err("mock provider health check failed".to_string())
            }
        })
    }

    fn list_models(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Vec<String>> + Send>> {
        let models = self.0.lock().unwrap().models.clone();
        Box::pin(async move { models })
    }

    fn select_model(&mut self, name: String) {
        self.0.lock().unwrap().model = Some(name);
    }

    fn current_model(&self) -> Option<String> {
        self.0.lock().unwrap().model.clone()
    }

    fn chat(
        &mut self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String>,
                > + Send,
        >,
    > {
        let binding = self.0.lock().unwrap();
        let mut queue = binding.responses.lock().unwrap();
        let chunks = queue.pop().unwrap_or_else(|| {
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
        drop(queue);

        Box::pin(async move {
            let (tx, rx) = tokio::sync::mpsc::channel(32);
            tokio::spawn(async move {
                for chunk in chunks {
                    let _ = tx.send(chunk).await;
                }
            });
            Ok(rx)
        })
    }

    fn set_timeout(&mut self, timeout_secs: u64) {
        self.0.lock().unwrap().timeout = timeout_secs;
    }

    fn set_retries(&mut self, max_retries: u32) {
        self.0.lock().unwrap().retries = max_retries;
    }

    fn set_think_type(&mut self, think_type: OllamaThinkType) {
        self.0.lock().unwrap().think_type = think_type;
    }
}

/// Create a system + user message pair for testing.
pub fn make_messages(user_text: &str) -> Vec<Message> {
    vec![
        Message::simple(Role::System, "You are a test assistant."),
        Message::simple(Role::User, user_text),
    ]
}
