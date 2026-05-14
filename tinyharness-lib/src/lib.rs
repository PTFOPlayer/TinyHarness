pub mod config;
pub mod context;
pub mod mode;
pub mod provider;
pub mod session;
pub mod token;
pub mod tools;

// Re-export key types at crate root for convenience
pub use config::{
    ProviderKind, Settings, SettingsError, SettingsStore, load_settings, save_settings,
};
pub use context::WorkspaceContext;
pub use mode::AgentMode;
pub use provider::{
    ChatMessage, ChatMessageResponse, Message, Provider, Role, TokenUsage, ToolCall,
    ToolCallFunction, ToolDefinition,
};
pub use session::{Session, SessionEntry, SessionMeta, SessionStore};
pub use token::ContextWindowSize;
pub use tools::tool::ToolCategory;
pub use tools::{SignalEvent, ToolManager};

// #[macro_export] macro at crate root:
// - extract_args!
