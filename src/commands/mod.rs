pub mod apikey;
pub mod audit;
pub mod clear;
pub mod command;
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

use tinyharness_lib::{
    config::{load_settings, save_settings},
    context::WorkspaceContext,
    mode::AgentMode,
    provider::{Message, Provider, Role},
};

use crate::style::*;

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
    SessionDelete(String),
    Rename(String),
    Settings(Option<String>),
    ApiKey(String),
    Compact(String),
    Add(String),
    Drop(String),
    Files,
    DropAll,
    Refresh,
    Init,
    Timeout(String),
    Retries(String),
    ContextLimit(String),
    AutoAccept(String),
    CommandAdd(String),
    CommandRemove(String),
    CommandDeny(String),
    CommandUndeny(String),
    CommandList,
    CommandReset,
    CommandResetDeny,
    CommandHelp,
    Audit(String),
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
    pub fn refresh_system_prompt(&self, messages: &mut [Message]) {
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
        messages: &mut [Message],
    ) -> Result<(), String> {
        if new_mode == self.current_mode {
            return Err(format!("Already in '{}' mode", new_mode));
        }

        self.current_mode = new_mode;
        self.refresh_system_prompt(messages);

        // Auto-save mode
        let mut settings = load_settings();
        settings.preferred_mode = self.current_mode;
        save_settings(&settings);

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
            "/session" => {
                let arg = arg.unwrap_or_default();
                // Check for subcommands: delete, load (default)
                if arg.starts_with("delete ") || arg == "delete" {
                    let id = arg.strip_prefix("delete ").unwrap_or("").trim().to_string();
                    Some(Command::SessionDelete(id))
                } else {
                    Some(Command::SessionLoad(arg))
                }
            }
            "/rename" => Some(Command::Rename(arg.unwrap_or_default())),
            "/settings" => Some(Command::Settings(arg)),
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
            "/timeout" => Some(Command::Timeout(arg.unwrap_or_default())),
            "/retries" => Some(Command::Retries(arg.unwrap_or_default())),
            "/contextlimit" => Some(Command::ContextLimit(arg.unwrap_or_default())),
            "/autoaccept" => Some(Command::AutoAccept(arg.unwrap_or_default())),
            "/command" => {
                let arg = arg.unwrap_or_default();
                if arg.is_empty() {
                    // No subcommand — list all commands
                    Some(Command::CommandList)
                } else {
                    // Split into subcommand + rest (e.g. "add docker" or "rm \"git stash\"")
                    let sub_parts: Vec<&str> = arg.splitn(2, ' ').collect();
                    let sub = sub_parts[0].to_lowercase();
                    let cmd_arg = sub_parts
                        .get(1)
                        .map(|s| s.trim())
                        .unwrap_or("")
                        .trim_matches('"')
                        .trim_matches('\'')
                        .to_string();
                    match sub.as_str() {
                        "add" => Some(Command::CommandAdd(cmd_arg)),
                        "rm" | "remove" => Some(Command::CommandRemove(cmd_arg)),
                        "deny" => Some(Command::CommandDeny(cmd_arg)),
                        "undeny" | "allow" => Some(Command::CommandUndeny(cmd_arg)),
                        "list" | "ls" => Some(Command::CommandList),
                        "reset" => Some(Command::CommandReset),
                        "resetdeny" => Some(Command::CommandResetDeny),
                        "help" => Some(Command::CommandHelp),
                        _ => Some(Command::CommandList),
                    }
                }
            }
            "/audit" => Some(Command::Audit(arg.unwrap_or_default())),
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
            "/timeout",
            "/retries",
            "/contextlimit",
            "/autoaccept",
            "/command",
            "/audit",
        ]
    }

    pub fn command_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("/help", "Show this help message"),
            ("/clear", "Clear the terminal screen"),
            ("/models", "List available models"),
            ("/model <name>", "Switch to a different model"),
            (
                "/mode [mode]",
                "Show or switch mode (casual/planning/agent/research)",
            ),
            (
                "/plan",
                "Switch to planning mode (alias for /mode planning)",
            ),
            ("/agent", "Switch to agent mode (alias for /mode agent)"),
            (
                "/research",
                "Switch to research mode (alias for /mode research)",
            ),
            ("/casual", "Switch to casual mode (alias for /mode casual)"),
            (
                "/context",
                "Show the workspace context available to the agent",
            ),
            ("/exit", "Exit the application"),
            ("/quit", "Exit the application"),
            ("/sessions", "List all saved sessions"),
            (
                "/session <id>",
                "Switch to an existing session (accepts ID prefix)",
            ),
            (
                "/session delete <id|name>",
                "Delete a session (with confirmation)",
            ),
            ("/rename <name>", "Rename the current session"),
            (
                "/settings [all]",
                "Show current settings. Use 'all' to list all safe commands.",
            ),
            (
                "/apikey [key]",
                "Set or show the Ollama API key for web search. Use /apikey clear to remove it.",
            ),
            (
                "/compact [focus]",
                "Summarize conversation history to free context space. Optionally specify a focus area.",
            ),
            (
                "/add <path>",
                "Pin a file into the AI's context so it's always available",
            ),
            ("/drop <path>", "Remove a pinned file from context"),
            ("/dropall", "Remove all pinned files from context"),
            ("/files", "List all pinned files in context"),
            (
                "/refresh",
                "Re-read all pinned files from disk (updates content)",
            ),
            (
                "/init",
                "Generate or update TINYHARNESS.md project instructions",
            ),
            (
                "/timeout [secs]",
                "Show or set the Ollama request timeout in seconds (default: 5)",
            ),
            (
                "/retries [count]",
                "Show or set the maximum number of Ollama request retries (default: 3)",
            ),
            (
                "/contextlimit [tokens]",
                "Show or set the context limit for warning calculations (default: model default)",
            ),
            (
                "/autoaccept [on|off]",
                "Show or toggle auto-accept for safe read-only commands (default: on)",
            ),
            ("/command [list]", "Show auto-accepted and denied commands"),
            (
                "/command add <cmd>",
                "Add a command to the auto-accept list",
            ),
            (
                "/command rm <cmd>",
                "Remove a command from the auto-accept list",
            ),
            (
                "/command deny <cmd>",
                "Always require confirmation for a command (even if safe)",
            ),
            (
                "/command undeny <cmd>",
                "Remove a command from the always-deny list",
            ),
            ("/command reset", "Reset auto-accepted commands to defaults"),
            ("/command resetdeny", "Clear the always-deny list"),
            (
                "/audit [last|session|clear]",
                "View command execution audit log",
            ),
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
                    // No argument — list available models and show current
                    let provider = self.provider.lock().await;
                    let current = provider.current_model();

                    // List models
                    models::execute_list(&*provider).await?;

                    // Show current selection
                    if let Some(model) = current {
                        println!(
                            "{}Current model: {}{}{}{}",
                            BOLD, GREEN, model, RESET, RESET
                        );
                    } else {
                        println!("{}No model currently selected.{}", ORANGE, RESET);
                    }
                    return Ok(CommandResult::Ok);
                }
                let mut provider = self.provider.lock().await;
                models::execute_select(&mut *provider, &name).await?;
                // Auto-save model
                let mut settings = load_settings();
                settings.last_model = provider.current_model();
                save_settings(&settings);
                Ok(CommandResult::Ok)
            }
            Command::Mode(mode_str) => {
                if mode_str.is_empty() {
                    println!(
                        "{}Current mode: {}{}{}",
                        BOLD, BLUE, self.current_mode, RESET
                    );
                    return Ok(CommandResult::Ok);
                }
                let new_mode: AgentMode = mode_str.parse()?;
                match self.switch_mode(new_mode, messages) {
                    Ok(()) => {
                        println!("{}Switched to {} mode.{}", BOLD, BLUE, RESET);
                    }
                    Err(msg) => {
                        println!("{}{}{}", ORANGE, msg, RESET);
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
                    return Err(
                        "Usage: /session <id> — use /sessions to list available sessions"
                            .to_string(),
                    );
                }
                Ok(CommandResult::SwitchSession(id_prefix))
            }
            Command::SessionDelete(id_or_name) => {
                if id_or_name.is_empty() {
                    return Err(
                        "Usage: /session delete <id|name> — use /sessions to list available sessions"
                            .to_string(),
                    );
                }
                sessions::execute_delete(&id_or_name, self.session_id.as_deref());
                Ok(CommandResult::Ok)
            }
            Command::Rename(name) => {
                if name.is_empty() {
                    return Err(
                        "Usage: /rename <name> — give the current session a descriptive name"
                            .to_string(),
                    );
                }
                Ok(CommandResult::RenameSession(name))
            }
            Command::Settings(arg) => {
                settings::execute(arg.as_deref());
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
                let result =
                    init::execute_init(&mut *provider, &self.workspace_ctx, messages).await?;
                // Refresh workspace context since the project instruction file may have changed
                self.workspace_ctx = WorkspaceContext::collect();
                self.refresh_system_prompt(messages);
                Ok(CommandResult::Init(result))
            }
            Command::Timeout(arg) => {
                if arg.is_empty() {
                    let settings = load_settings();
                    println!(
                        "{}Current timeout: {}{}s{}",
                        BOLD, BLUE, settings.ollama_timeout_secs, RESET
                    );
                    return Ok(CommandResult::Ok);
                }
                match arg.parse::<u64>() {
                    Ok(secs) if secs > 0 => {
                        // Update settings
                        let mut settings = load_settings();
                        settings.ollama_timeout_secs = secs;
                        save_settings(&settings);
                        // Update live provider
                        let mut provider = self.provider.lock().await;
                        provider.set_timeout(secs);
                        println!(
                            "{}Timeout set to {}{}s{}.{}",
                            BOLD, BLUE, secs, RESET, RESET
                        );
                        Ok(CommandResult::Ok)
                    }
                    Ok(_) => Err("Timeout must be a positive number of seconds.".to_string()),
                    Err(_) => Err(format!(
                        "Invalid timeout value: '{}'. Use a number of seconds, e.g. /timeout 30",
                        arg
                    )),
                }
            }
            Command::Retries(arg) => {
                if arg.is_empty() {
                    let settings = load_settings();
                    println!(
                        "{}Current max retries: {}{}{}",
                        BOLD, BLUE, settings.ollama_max_retries, RESET
                    );
                    return Ok(CommandResult::Ok);
                }
                match arg.parse::<u32>() {
                    Ok(count) => {
                        // Update settings
                        let mut settings = load_settings();
                        settings.ollama_max_retries = count;
                        save_settings(&settings);
                        // Update live provider
                        let mut provider = self.provider.lock().await;
                        provider.set_retries(count);
                        println!(
                            "{}Max retries set to {}{}{}.{}",
                            BOLD, BLUE, count, RESET, RESET
                        );
                        Ok(CommandResult::Ok)
                    }
                    Err(_) => Err(format!(
                        "Invalid retries value: '{}'. Use a number, e.g. /retries 5",
                        arg
                    )),
                }
            }
            Command::ContextLimit(arg) => {
                if arg.is_empty() {
                    let settings = load_settings();
                    match settings.context_limit {
                        Some(limit) => {
                            println!(
                                "{}Context limit for warnings: {}{} tokens{}",
                                BOLD, BLUE, limit, RESET
                            );
                        }
                        None => {
                            println!(
                                "{}Context limit: {}auto (using model default){}",
                                BOLD, GRAY, RESET
                            );
                        }
                    }
                    return Ok(CommandResult::Ok);
                }
                if arg == "auto" || arg == "default" {
                    // Clear the limit
                    let mut settings = load_settings();
                    settings.context_limit = None;
                    save_settings(&settings);
                    println!(
                        "{}Context limit cleared. Using model default for warnings.{}",
                        BOLD, RESET
                    );
                    return Ok(CommandResult::Ok);
                }
                match arg.parse::<u32>() {
                    Ok(limit) if limit > 0 => {
                        // Update settings
                        let mut settings = load_settings();
                        settings.context_limit = Some(limit);
                        save_settings(&settings);
                        println!(
                            "{}Context limit set to {}{} tokens{} for warning calculations.{}",
                            BOLD, BLUE, limit, RESET, RESET
                        );
                        Ok(CommandResult::Ok)
                    }
                    Ok(_) => Err("Context limit must be a positive number of tokens.".to_string()),
                    Err(_) => Err(format!(
                        "Invalid context limit value: '{}'. Use a number of tokens, e.g. /contextlimit 32768, or 'auto' to use model default",
                        arg
                    )),
                }
            }
            Command::AutoAccept(arg) => {
                if arg.is_empty() {
                    let settings = load_settings();
                    let status = if settings.auto_accept_safe_commands {
                        "enabled"
                    } else {
                        "disabled"
                    };
                    let color = if settings.auto_accept_safe_commands {
                        GREEN
                    } else {
                        ORANGE
                    };
                    println!(
                        "{}Auto-accept safe commands: {}{}{}{}",
                        BOLD, color, status, RESET, RESET
                    );
                    return Ok(CommandResult::Ok);
                }
                let new_value = match arg.to_lowercase().as_str() {
                    "on" | "true" | "yes" | "1" => true,
                    "off" | "false" | "no" | "0" => false,
                    _ => {
                        return Err(
                            "Invalid value. Use 'on' or 'off', e.g. /autoaccept on".to_string()
                        );
                    }
                };
                // Update settings
                let mut settings = load_settings();
                settings.auto_accept_safe_commands = new_value;
                save_settings(&settings);
                let status = if new_value { "enabled" } else { "disabled" };
                let color = if new_value { GREEN } else { ORANGE };
                println!(
                    "{}Auto-accept safe commands set to {}{}{}{}",
                    BOLD, color, status, RESET, RESET
                );
                Ok(CommandResult::Ok)
            }
            Command::CommandList => {
                command::execute_list();
                Ok(CommandResult::Ok)
            }
            Command::CommandReset => {
                command::execute_reset();
                Ok(CommandResult::Ok)
            }
            Command::CommandResetDeny => {
                command::execute_reset_deny();
                Ok(CommandResult::Ok)
            }
            Command::CommandHelp => {
                command::execute_help();
                Ok(CommandResult::Ok)
            }
            Command::CommandAdd(cmd) => {
                command::execute_add(&cmd);
                Ok(CommandResult::Ok)
            }
            Command::CommandRemove(cmd) => {
                command::execute_remove(&cmd);
                Ok(CommandResult::Ok)
            }
            Command::CommandDeny(cmd) => {
                command::execute_deny(&cmd);
                Ok(CommandResult::Ok)
            }
            Command::CommandUndeny(cmd) => {
                command::execute_undeny(&cmd);
                Ok(CommandResult::Ok)
            }
            Command::Audit(args) => {
                audit::execute(&args);
                Ok(CommandResult::Ok)
            }
        }
    }
}
