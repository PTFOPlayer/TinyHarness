pub mod clear;
pub mod exit;
pub mod help;
pub mod models;
pub mod save_load;

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::provider::{Message, Provider};

pub enum Command {
    Help,
    Clear,
    Models,
    Model(String),
    Exit,
    Save(String),
    Load(String),
}

pub struct CommandDispatcher {
    pub provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    pub exit_requested: bool,
}

impl CommandDispatcher {
    pub fn new(provider: Arc<Mutex<dyn Provider + Send + Sync>>) -> Self {
        CommandDispatcher {
            provider,
            exit_requested: false,
        }
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
            Command::Exit => {
                exit::execute();
                self.exit_requested = true;
                Ok(())
            }
            Command::Save(path) => {
                if path.is_empty() {
                    return Err("Usage: /save <filename> — e.g. /save conversation.json".to_string());
                }
                save_load::execute_save(&path, messages)
            }
            Command::Load(path) => {
                if path.is_empty() {
                    return Err("Usage: /load <filename> — e.g. /load conversation.json".to_string());
                }
                let loaded = save_load::execute_load(&path)?;
                *messages = loaded;
                Ok(())
            }
        }
    }
}
