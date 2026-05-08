pub mod llama_cpp;
pub mod ollama;
pub mod openai_compat;
pub mod vllm;

use std::fmt::Display;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc::Sender;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub content: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolInfo {
    #[serde(rename = "type")]
    pub tool_type: ToolType,
    pub function: ToolFunctionInfo,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ToolType {
    Function,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolFunctionInfo {
    pub name: String,
    pub description: String,
    pub parameters: schemars::Schema,
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
}

#[async_trait::async_trait]
pub trait Provider {
    /// Check whether the backend is reachable and healthy.
    async fn health_check(&self) -> Result<(), String>;

    async fn list_models(&self) -> Vec<String>;

    fn select_model(&mut self, name: String);

    fn current_model(&self) -> Option<String>;

    async fn chat(
        &mut self,
        messages: Vec<Message>,
        prompt: String,
        send: Sender<ChatMessageResponse>,
        tools: Vec<ToolInfo>,
    );

    /// Set the request timeout in seconds. Only meaningful for providers that use timeouts.
    fn set_timeout(&mut self, _timeout_secs: u64) {}

    /// Set the maximum number of retries. Only meaningful for providers that use retries.
    fn set_retries(&mut self, _max_retries: u32) {}

    /// Get the token usage from the last request, if available.
    fn last_token_usage(&self) -> Option<TokenUsage> {
        None
    }
}
