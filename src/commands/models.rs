use std::io::Write;

use tinyharness_lib::config::{load_settings, save_settings};
use tinyharness_lib::provider::{AnyProvider, Provider};
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
                execute_list(&mut ctx.output, &p).await?;

                if let Some(model) = p.current_model() {
                    let _ = writeln!(ctx.output, "{BOLD}Current model: {GREEN}{model}{RESET}",);
                } else {
                    let _ = writeln!(ctx.output, "{ORANGE}No model currently selected.{RESET}");
                }
                return Ok(CommandResult::Ok);
            }

            let mut p = provider.lock().await;
            execute_select(&mut ctx.output, &mut p, &name).await?;

            let mut settings = load_settings();
            if let Some(model) = p.current_model() {
                settings.set_model_for(settings.last_provider, model);
                save_settings(&settings);
            }

            Ok(CommandResult::Ok)
        }
    }
);

pub async fn execute_list(out: &mut Output, provider: &AnyProvider) -> Result<(), String> {
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
    provider: &mut AnyProvider,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::registry::Command;
    use crate::test_helpers::{captured_output, captured_string, make_context, strip_ansi};

    #[tokio::test]
    async fn list_models_shows_available() {
        let (mut ctx, _mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;

        execute_list(&mut ctx.output, &*ctx.provider.lock().await)
            .await
            .unwrap();

        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Available models:"));
        assert!(plain.contains("mock-model"));
        assert!(plain.contains("other-model"));
    }

    #[tokio::test]
    async fn list_models_shows_current() {
        let (mut ctx, _mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;

        // Call the full command with no arg
        let cmd = ModelCommand;
        let mut messages = vec![];
        cmd.execute(None, &mut ctx, &mut messages).await.unwrap();

        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Current model:"));
        assert!(plain.contains("mock-model"));
    }

    #[tokio::test]
    async fn select_known_model_switches() {
        let (mut ctx, _mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;

        let cmd = ModelCommand;
        let mut messages = vec![];
        cmd.execute(Some("other-model"), &mut ctx, &mut messages)
            .await
            .unwrap();

        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Switched to model:"));
        assert!(plain.contains("other-model"));
        assert_eq!(
            ctx.provider.lock().await.current_model(),
            Some("other-model".to_string())
        );
    }

    #[tokio::test]
    async fn select_unknown_model_still_sets() {
        let (mut ctx, _mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;

        let cmd = ModelCommand;
        let mut messages = vec![];
        cmd.execute(Some("nonexistent-model"), &mut ctx, &mut messages)
            .await
            .unwrap();

        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Set model to:"));
        assert_eq!(
            ctx.provider.lock().await.current_model(),
            Some("nonexistent-model".to_string())
        );
    }
}
