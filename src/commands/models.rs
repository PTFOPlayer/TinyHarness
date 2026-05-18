use tinyharness_lib::config::{load_settings, save_settings};
use tinyharness_lib::provider::Provider;

use crate::async_command;
use crate::commands::registry::CommandResult;
use crate::style::*;

async_command!(
    ModelsCommand,
    "/models",
    "List available models or switch to a different model",
    "/models [name]",
    |raw_arg, ctx, _messages| {
        let name = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();
        async move {
            if name.is_empty() {
                let p = provider.lock().await;
                execute_list(&*p).await?;

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

            let mut settings = load_settings();
            settings.last_model = p.current_model();
            save_settings(&settings);

            Ok(CommandResult::Ok)
        }
    }
);

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
        provider.select_model(name.to_string());
        println!("{}Set model to: {}{}{}", BOLD, BLUE, name, RESET);
        Ok(())
    }
}
