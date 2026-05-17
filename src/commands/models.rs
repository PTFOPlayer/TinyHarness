use tinyharness_lib::config::{load_settings, save_settings};
use tinyharness_lib::provider::{Message, Provider};

use crate::commands::registry::{Command, CommandContext, CommandResult};
use crate::style::*;

use std::future::Future;
use std::pin::Pin;

pub struct ModelsCommand;

impl Command for ModelsCommand {
    fn name(&self) -> &'static str {
        "/models"
    }

    fn description(&self) -> &'static str {
        "List available models"
    }

    fn execute<'a>(
        &'a self,
        raw_arg: Option<&str>,
        ctx: &'a mut CommandContext,
        _messages: &'a mut Vec<Message>,
    ) -> Pin<Box<dyn Future<Output = Result<CommandResult, String>> + Send + 'a>> {
        let name = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();

        Box::pin(async move {
            if name.is_empty() {
                // No argument — list available models and show current
                let p = provider.lock().await;
                execute_list(&*p).await?;

                // Show current selection
                if let Some(model) = p.current_model() {
                    println!(
                        "{}Current model: {}{}{}{}",
                        BOLD, GREEN, model, RESET, RESET
                    );
                } else {
                    println!("{}No model currently selected.{}", ORANGE, RESET);
                }
                return Ok(CommandResult::Ok);
            }

            let mut p = provider.lock().await;
            execute_select(&mut *p, &name).await?;

            // Auto-save model
            let mut settings = load_settings();
            settings.last_model = p.current_model();
            save_settings(&settings);

            Ok(CommandResult::Ok)
        })
    }
}

/// Also handle "/model <name>" as a separate command that delegates
pub struct ModelCommand;

impl Command for ModelCommand {
    fn name(&self) -> &'static str {
        "/model"
    }

    fn description(&self) -> &'static str {
        "Switch to a different model"
    }

    fn usage(&self) -> &'static str {
        "/model <name>"
    }

    fn execute<'a>(
        &'a self,
        raw_arg: Option<&str>,
        ctx: &'a mut CommandContext,
        messages: &'a mut Vec<Message>,
    ) -> Pin<Box<dyn Future<Output = Result<CommandResult, String>> + Send + 'a>> {
        // Delegate to ModelsCommand with the arg
        ModelsCommand.execute(raw_arg, ctx, messages)
    }
}

pub async fn execute_list(provider: &dyn Provider) -> Result<(), String> {
    let models = provider.list_models().await;
    if models.is_empty() {
        println!("{}No models available.{}", ORANGE, RESET);
    } else {
        println!("\n{}Available models:{}", BOLD, RESET);
        for model in &models {
            println!("  {}{}{}", BLUE, model, RESET);
        }
        println!();
    }
    Ok(())
}

pub async fn execute_select(provider: &mut dyn Provider, name: &str) -> Result<(), String> {
    let models = provider.list_models().await;
    if models.iter().any(|m| m == name) {
        provider.select_model(name.to_string());
        println!("{}Switched to model: {}{}{}", BOLD, BLUE, name, RESET);
        Ok(())
    } else {
        // Still switch even if not in list (model might be pullable)
        provider.select_model(name.to_string());
        println!("{}Set model to: {}{}{}", BOLD, BLUE, name, RESET);
        Ok(())
    }
}
