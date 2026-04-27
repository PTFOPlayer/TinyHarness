pub mod clear;
pub mod context;
pub mod exit;
pub mod help;
pub mod models;
pub mod save_load;

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::{
    context::WorkspaceContext,
    mode::AgentMode,
    provider::{Message, Provider, Role},
};

pub enum Command {
    Help,
    Clear,
    Models,
    Model(String),
    Mode(String),
    Context,
    Exit,
    Save(String),
    Load(String),
}

pub struct CommandDispatcher {
    pub provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    pub exit_requested: bool,
    pub current_mode: AgentMode,
    pub workspace_ctx: WorkspaceContext,
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
        }
    }

    /// Build the system prompt for the current mode, appending workspace context.
    pub fn build_system_prompt(&self) -> String {
        format!(
            "{}\n\n---\n{}",
            self.current_mode.system_prompt(),
            self.workspace_ctx.format()
        )
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
            "/casual" => Some(Command::Mode("casual".to_string())),
            "/context" => Some(Command::Context),
            "/exit" | "/quit" => Some(Command::Exit),
            "/save" => Some(Command::Save(arg.unwrap_or_default())),
            "/load" => Some(Command::Load(arg.unwrap_or_default())),
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
            "/casual",
            "/context",
            "/exit",
            "/quit",
            "/save",
            "/load",
        ]
    }

    pub fn command_descriptions() -> &'static [(&'static str, &'static str)] {
        &[
            ("/help", "Show this help message"),
            ("/clear", "Clear the terminal screen"),
            ("/models", "List available models"),
            ("/model <name>", "Switch to a different model"),
            ("/mode [mode]", "Show or switch mode (casual/planning/agent)"),
            ("/plan", "Switch to planning mode (alias for /mode planning)"),
            ("/agent", "Switch to agent mode (alias for /mode agent)"),
            ("/casual", "Switch to casual mode (alias for /mode casual)"),
            ("/context", "Show the workspace context available to the agent"),
            ("/exit", "Exit the application"),
            ("/quit", "Exit the application"),
            ("/save <file>", "Save conversation to a JSON file"),
            ("/load <file>", "Load conversation from a JSON file"),
        ]
    }

    pub async fn dispatch(
        &mut self,
        cmd: Command,
        messages: &mut Vec<Message>,
    ) -> Result<(), String> {
        match cmd {
            Command::Help => {
                help::execute();
                Ok(())
            }
            Command::Clear => {
                clear::execute();
                Ok(())
            }
            Command::Models => {
                let provider = self.provider.lock().await;
                models::execute_list(&*provider).await
            }
            Command::Model(name) => {
                if name.is_empty() {
                    return Err("Usage: /model <name> — e.g. /model gemma4:31b-cloud".to_string());
                }
                let mut provider = self.provider.lock().await;
                models::execute_select(&mut *provider, &name).await
            }
            Command::Mode(mode_str) => {
                if mode_str.is_empty() {
                    println!(
                        "{}Current mode: {}{}{}",
                        crate::BOLD,
                        crate::BLUE,
                        self.current_mode,
                        crate::RESET
                    );
                    return Ok(());
                }
                let new_mode: AgentMode = mode_str.parse()?;
                if new_mode == self.current_mode {
                    println!(
                        "{}Already in {} mode.{}",
                        crate::ORANGE,
                        new_mode,
                        crate::RESET
                    );
                    return Ok(());
                }
                self.current_mode = new_mode;
                // Replace the system prompt (first System message) with context preserved
                if let Some(sys_msg) = messages.iter_mut().find(|m| m.role == Role::System) {
                    sys_msg.content = self.build_system_prompt();
                }
                println!(
                    "{}Switched to {} mode.{}",
                    crate::BOLD,
                    crate::BLUE,
                    crate::RESET
                );
                Ok(())
            }
            Command::Context => {
                context::execute(&self.workspace_ctx);
                Ok(())
            }
            Command::Exit => {
                exit::execute();
                self.exit_requested = true;
                Ok(())
            }
            Command::Save(path) => {
                if path.is_empty() {
                    return Err("Usage: /save <filename> — e.g. /save conversation.json".to_string());
                }
                save_load::execute_save(&path, messages, self.current_mode)
            }
            Command::Load(path) => {
                if path.is_empty() {
                    return Err("Usage: /load <filename> — e.g. /load conversation.json".to_string());
                }
                let (loaded_mode, loaded_msgs) = save_load::execute_load(&path)?;
                *messages = loaded_msgs;
                self.current_mode = loaded_mode;
                // Ensure the system prompt matches the loaded mode with context
                if let Some(sys_msg) = messages.iter_mut().find(|m| m.role == Role::System) {
                    sys_msg.content = self.build_system_prompt();
                }
                Ok(())
            }
        }
    }
}
