pub mod config;
pub mod context;
pub mod custom_tools;
pub mod image;
pub mod mode;
pub mod paths;
pub mod provider;
pub mod sandbox;
pub mod secret;
pub mod session;
pub mod skill;
pub mod token;
pub mod tools;

// Re-export key types at crate root for convenience
pub use config::{
    MergedSettings, ProjectSettings, ProviderKind, SettingSource, Settings, SettingsError,
    SettingsStore, discover_project_settings, ensure_prompts_initialized,
    ensure_prompts_initialized_for, ensure_prompts_initialized_in,
    generate_project_config_template, load_merged_settings, load_merged_settings_from,
    load_settings, load_settings_from, prompts_dir, prompts_dir_for, resolve_project_md_files,
    save_settings, save_settings_to,
};
pub use context::WorkspaceContext;
pub use custom_tools::{
    CustomToolCategory, CustomToolConfig, CustomToolDefinition, CustomToolError, CustomToolManager,
    ShellCommand,
};
pub use image::ImageAttachment;
pub use mode::AgentMode;
pub use paths::Paths;
pub use provider::{
    ChatMessage, ChatMessageResponse, Message, Provider, Role, TokenUsage, ToolCall,
    ToolCallFunction, ToolDefinition,
};
pub use sandbox::Sandbox;
pub use secret::SecretString;
pub use session::{Session, SessionEntry, SessionMeta, SessionStore};
pub use skill::{
    Skill, SkillRegistry, SkillSource, discover_skills, discover_skills_in, personal_skills_dir,
    personal_skills_dirs, project_skills_dir,
};
pub use token::ContextWindowSize;
pub use tools::tool::ToolCategory;
pub use tools::{SignalEvent, ToolManager};

// #[macro_export] macro at crate root:
// - extract_args!
