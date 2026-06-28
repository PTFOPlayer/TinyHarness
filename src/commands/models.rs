use std::io::Write;

use tinyharness_lib::config::{load_settings, save_settings};
use tinyharness_lib::provider::Provider;
use tinyharness_ui::output::Output;

use crate::async_command;
use crate::commands::registry::CommandResult;
use tinyharness_ui::style::*;

async_command!(
    ModelCommand,
    "/model",
    "List available models or switch to a different model",
    "/model [name]",
    |raw_arg, ctx, _messages| {
        let name = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();
        async move {
            if name.is_empty() {
                let p = provider.lock().await;
                execute_list(&mut ctx.output, &*p).await?;

                if let Some(model) = p.current_model() {
                    let _ = writeln!(ctx.output, "{BOLD}Current model: {GREEN}{model}{RESET}",);
                } else {
                    let _ = writeln!(ctx.output, "{ORANGE}No model currently selected.{RESET}");
                }
                return Ok(CommandResult::Ok);
            }

            let mut p = provider.lock().await;
            execute_select(&mut ctx.output, &mut *p, &name).await?;

            let mut settings = load_settings();
            if let Some(model) = p.current_model() {
                settings.set_model_for(settings.last_provider, model);
                save_settings(&settings);
            }

            Ok(CommandResult::Ok)
        }
    }
);

pub async fn execute_list(out: &mut Output, provider: &dyn Provider) -> Result<(), String> {
    let models = provider.list_models().await;
    if models.is_empty() {
        let _ = writeln!(out, "{ORANGE}No models available.{RESET}");
    } else {
        let _ = writeln!(out, "\n{BOLD}Available models:{RESET}");
        for model in &models {
            let _ = writeln!(out, "  {BLUE}{model}{RESET}");
        }
        let _ = writeln!(out);
    }
    Ok(())
}

pub async fn execute_select(
    out: &mut Output,
    provider: &mut dyn Provider,
    name: &str,
) -> Result<(), String> {
    let models = provider.list_models().await;
    if models.iter().any(|m| m == name) {
        provider.select_model(name.to_string());
        let _ = writeln!(out, "{BOLD}Switched to model: {BLUE}{name}{RESET}");
        Ok(())
    } else {
        provider.select_model(name.to_string());
        let _ = writeln!(out, "{BOLD}Set model to: {BLUE}{name}{RESET}");
        Ok(())
    }
}
