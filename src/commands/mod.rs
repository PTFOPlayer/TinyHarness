pub mod apikey;
pub mod clear;
pub mod compact;
pub mod context;
pub mod exit;
pub mod files;
pub mod help;
pub mod init;
pub mod models;
pub mod sessions;
pub mod settings;

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    config::Settings,
    context::WorkspaceContext,
    mode::AgentMode,
    provider::{Message, Provider, Role},
    style::*,
};

pub use files::FileContext;
pub use init::InitResult;

pub enum Command {
    Help,
    Clear,
    Models,
    Model(String),
    Mode(String),
    Context,
    Exit,
    Sessions,
    SessionLoad(String),
    Rename(String),
    Settings,
    ApiKey(String),
    Compact(String),
    Add(String),
    Drop(String),
    Files,
    DropAll,
    Refresh,
    Init,
}

/// Result of dispatching a command.
pub enum CommandResult {
    /// Command completed normally.
    Ok,
    /// The user wants to switch to a different session.
    SwitchSession(String),
    /// The user wants to rename the current session.
    RenameSession(String),
    /// The /init command was run — workspace context should be refreshed.
    Init(InitResult),
}

pub struct CommandDispatcher {
    pub provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    pub exit_requested: bool,
    pub current_mode: AgentMode,
    pub workspace_ctx: WorkspaceContext,
    pub file_context: FileContext,
    pub session_id: Option<String>,
}

impl CommandDispatcher {
    pub fn new(
        provider: Arc<Mutex<dyn Provider + Send + Sync>>,
        workspace_ctx: WorkspaceContext,
    ) -> Self {
        CommandDispatcher {
            provider,
            exit_requested: false,
            current_mode: AgentMode::Casual,
            workspace_ctx,
            file_context: FileContext::new(),
            session_id: None,
        }
    }

    /// Update the system prompt message in the conversation to reflect the current
    /// mode, workspace context, and pinned files. Call this after any change that
    /// affects the system prompt content (mode switch, add/drop/refresh files, etc.).
    pub fn refresh_system_prompt(&self, messages: &mut Vec<Message>) {
        if let Some(sys_msg) = messages.iter_mut().find(|m| m.role == Role::System) {
            sys_msg.content = self.build_system_prompt();
        }
    }

    /// Switch the current mode to `new_mode`. Updates the system prompt in the
    /// conversation and auto-saves the new mode to settings.
    /// Returns `Ok(())` on success or an error string if the mode is unchanged/invalid.
    pub fn switch_mode(
        &mut self,
        new_mode: AgentMode,
        messages: &mut Vec<Message>,
    ) -> Result<(), String> {
        if new_mode == self.current_mode {
            return Err(format!("Already in '{}' mode", new_mode));
        }

        self.current_mode = new_mode;
        self.refresh_system_prompt(messages);

        // Auto-save mode
        let mut settings = Settings::load();
        settings.preferred_mode = self.current_mode;
        settings.save();

        Ok(())
    }

    /// Build the system prompt for the current mode, appending workspace context
    /// and pinned file content.
    pub fn build_system_prompt(&self) -> String {
        let mut prompt = format!(
            "{}\n\n---\n{}",
            self.current_mode.system_prompt(),
            self.workspace_ctx.format()
        );

        // Inject pinned file content
        if !self.file_context.is_empty() {
            prompt.push_str(&self.file_context.format_for_prompt());
        }

        prompt
    }

    pub fn parse(input: &str) -> Option<Command> {
        let input = input.trim();
        if !input.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = input.splitn(2, ' ').collect();
        let cmd = parts[0].to_lowercase();
        let arg = parts.get(1).map(|s| s.trim().to_string());

        match cmd.as_str() {
            "/help" => Some(Command::Help),
            "/clear" => Some(Command::Clear),
            "/models" => Some(Command::Models),
            "/model" => Some(Command::Model(arg.unwrap_or_default())),
            "/mode" => Some(Command::Mode(arg.unwrap_or_default())),
            "/plan" => Some(Command::Mode("planning".to_string())),
            "/agent" => Some(Command::Mode("agent".to_string())),
            "/research" => Some(Command::Mode("research".to_string())),
            "/casual" => Some(Command::Mode("casual".to_string())),
            "/context" => Some(Command::Context),
            "/exit" | "/quit" => Some(Command::Exit),
            "/sessions" => Some(Command::Sessions),
            "/session" => Some(Command::SessionLoad(arg.unwrap_or_default())),
            "/rename" => Some(Command::Rename(arg.unwrap_or_default())),
            "/settings" => Some(Command::Settings),
            "/apikey" => {
                let arg = arg.unwrap_or_default();
                Some(Command::ApiKey(arg))
            }
            "/compact" => Some(Command::Compact(arg.unwrap_or_default())),
            "/add" => Some(Command::Add(arg.unwrap_or_default())),
            "/drop" => Some(Command::Drop(arg.unwrap_or_default())),
            "/dropall" => Some(Command::DropAll),
            "/files" => Some(Command::Files),
            "/refresh" => Some(Command::Refresh),
            "/init" => Some(Command::Init),
            _ => None,
        }
    }

    pub fn command_names() -> &'static [&'static str] {
        &[
            "/help",
            "/clear",
            "/models",
            "/model",
            "/mode",
            "/plan",
            "/agent",
            "/research",
            "/casual",
            "/context",
            "/exit",
            "/quit",
            "/sessions",
            "/session",
            "/rename",
            "/settings",
            "/apikey",
            "/compact",
            "/add",
            "/drop",
            "/dropall",
            "/files",
            "/refresh",
            "/init",
        ]
    }

    pub fn command_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("/help", "Show this help message"),
            ("/clear", "Clear the terminal screen"),
            ("/models", "List available models"),
            ("/model <name>", "Switch to a different model"),
            ("/mode [mode]", "Show or switch mode (casual/planning/agent/research)"),
            ("/plan", "Switch to planning mode (alias for /mode planning)"),
            ("/agent", "Switch to agent mode (alias for /mode agent)"),
            ("/research", "Switch to research mode (alias for /mode research)"),
            ("/casual", "Switch to casual mode (alias for /mode casual)"),
            ("/context", "Show the workspace context available to the agent"),
            ("/exit", "Exit the application"),
            ("/quit", "Exit the application"),
            ("/sessions", "List all saved sessions"),
            ("/session <id>", "Switch to an existing session (accepts ID prefix)"),
            ("/rename <name>", "Rename the current session"),
            ("/settings", "Show current settings (provider, model, mode)"),
            ("/apikey [key]", "Set or show the Ollama API key for web search. Use /apikey clear to remove it."),
            ("/compact [focus]", "Summarize conversation history to free context space. Optionally specify a focus area."),
            ("/add <path>", "Pin a file into the AI's context so it's always available"),
            ("/drop <path>", "Remove a pinned file from context"),
            ("/dropall", "Remove all pinned files from context"),
            ("/files", "List all pinned files in context"),
            ("/refresh", "Re-read all pinned files from disk (updates content)"),
            ("/init", "Generate or update TINYHARNESS.md project instructions"),
        ]
    }

    pub async fn dispatch(
        &mut self,
        cmd: Command,
        messages: &mut Vec<Message>,
    ) -> Result<CommandResult, String> {
        match cmd {
            Command::Help => {
                help::execute();
                Ok(CommandResult::Ok)
            }
            Command::Clear => {
                clear::execute();
                Ok(CommandResult::Ok)
            }
            Command::Models => {
                let provider = self.provider.lock().await;
                models::execute_list(&*provider).await?;
                Ok(CommandResult::Ok)
            }
            Command::Model(name) => {
                if name.is_empty() {
                    let provider = self.provider.lock().await;
                    match provider.current_model() {
                        Some(model) => println!("{}Current model: {}{}{}", BOLD, BLUE, model, RESET),
                        None => println!("{}No model selected.{}", ORANGE, RESET),
                    }
                    return Ok(CommandResult::Ok);
                }
                let mut provider = self.provider.lock().await;
                models::execute_select(&mut *provider, &name).await?;
                // Auto-save model
                let mut settings = Settings::load();
                settings.last_model = provider.current_model();
                settings.save();
                Ok(CommandResult::Ok)
            }
            Command::Mode(mode_str) => {
                if mode_str.is_empty() {
                    println!(
                        "{}Current mode: {}{}{}",
                        BOLD,
                        BLUE,
                        self.current_mode,
                        RESET
                    );
                    return Ok(CommandResult::Ok);
                }
                let new_mode: AgentMode = mode_str.parse()?;
                match self.switch_mode(new_mode, messages) {
                    Ok(()) => {
                        println!(
                            "{}Switched to {} mode.{}",
                            BOLD,
                            BLUE,
                            RESET
                        );
                    }
                    Err(msg) => {
                        println!(
                            "{}{}{}",
                            ORANGE,
                            msg,
                            RESET
                        );
                    }
                }
                Ok(CommandResult::Ok)
            }
            Command::Context => {
                context::execute(&self.workspace_ctx);
                Ok(CommandResult::Ok)
            }
            Command::Exit => {
                exit::execute();
                self.exit_requested = true;
                Ok(CommandResult::Ok)
            }
            Command::Sessions => {
                sessions::execute_list(self.session_id.as_deref());
                Ok(CommandResult::Ok)
            }
            Command::SessionLoad(id_prefix) => {
                if id_prefix.is_empty() {
                    return Err("Usage: /session <id> — use /sessions to list available sessions".to_string());
                }
                Ok(CommandResult::SwitchSession(id_prefix))
            }
            Command::Rename(name) => {
                if name.is_empty() {
                    return Err("Usage: /rename <name> — give the current session a descriptive name".to_string());
                }
                Ok(CommandResult::RenameSession(name))
            }
            Command::Settings => {
                settings::execute();
                Ok(CommandResult::Ok)
            }
            Command::ApiKey(arg) => {
                if arg.is_empty() {
                    apikey::execute_show();
                } else if arg == "clear" {
                    apikey::execute_clear();
                } else {
                    apikey::execute_set(&arg);
                }
                Ok(CommandResult::Ok)
            }
            Command::Compact(focus) => {
                let mut provider = self.provider.lock().await;
                compact::execute_compact(&mut *provider, messages, &focus).await?;
                Ok(CommandResult::Ok)
            }
            Command::Add(path) => {
                if path.is_empty() {
                    return Err("Usage: /add <file_path> — e.g. /add src/main.rs".to_string());
                }
                files::execute_add(&mut self.file_context, &path);
                self.refresh_system_prompt(messages);
                Ok(CommandResult::Ok)
            }
            Command::Drop(path) => {
                if path.is_empty() {
                    return Err("Usage: /drop <file_path> — e.g. /drop src/main.rs".to_string());
                }
                files::execute_drop(&mut self.file_context, &path);
                self.refresh_system_prompt(messages);
                Ok(CommandResult::Ok)
            }
            Command::Files => {
                files::execute_list(&self.file_context);
                Ok(CommandResult::Ok)
            }
            Command::DropAll => {
                files::execute_clear(&mut self.file_context);
                self.refresh_system_prompt(messages);
                Ok(CommandResult::Ok)
            }
            Command::Refresh => {
                files::execute_refresh(&mut self.file_context);
                self.refresh_system_prompt(messages);
                Ok(CommandResult::Ok)
            }
            Command::Init => {
                let mut provider = self.provider.lock().await;
                let result = init::execute_init(&mut *provider, &self.workspace_ctx, messages).await?;
                // Refresh workspace context since the project instruction file may have changed
                self.workspace_ctx = WorkspaceContext::collect();
                self.refresh_system_prompt(messages);
                Ok(CommandResult::Init(result))
            }
        }
    }
}
