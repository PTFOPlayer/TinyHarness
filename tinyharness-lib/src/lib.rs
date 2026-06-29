pub mod config;
pub mod context;
pub mod image;
pub mod mode;
pub mod provider;
pub mod secret;
pub mod session;
pub mod skill;
pub mod token;
pub mod tools;

// Re-export key types at crate root for convenience
pub use config::{
    MergedSettings, ProjectSettings, ProviderKind, SettingSource, Settings, SettingsError,
    SettingsStore, discover_project_settings, ensure_prompts_initialized,
    generate_project_config_template, load_merged_settings, load_settings, prompts_dir,
    resolve_project_md_files, save_settings,
};
pub use context::WorkspaceContext;
pub use image::ImageAttachment;
pub use mode::AgentMode;
pub use provider::{
    ChatMessage, ChatMessageResponse, Message, Provider, Role, TokenUsage, ToolCall,
    ToolCallFunction, ToolDefinition,
};
pub use secret::SecretString;
pub use session::{Session, SessionEntry, SessionMeta, SessionStore};
pub use skill::{Skill, SkillRegistry, SkillSource, discover_skills};
pub use token::ContextWindowSize;
pub use tools::tool::ToolCategory;
pub use tools::{SignalEvent, ToolManager};

// #[macro_export] macro at crate root:
// - extract_args!
