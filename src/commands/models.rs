use std::io::Write;

use tinyharness_lib::config::{load_settings_from, save_settings_to};
use tinyharness_lib::provider::{AnyProvider, Provider, model_variants::is_variant_available};
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

            let mut settings = load_settings_from(&ctx.paths);
            if let Some(model) = p.current_model() {
                settings.set_model_for(settings.last_provider, model);
                if let Err(e) = save_settings_to(&ctx.paths, &settings) {
                    tracing::warn!("Failed to save settings: {e}");
                }
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
    if is_variant_available(name, &models) {
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

    #[tokio::test]
    async fn select_variant_suffix_matches_base_model() {
        let (mut ctx, mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;

        // The mock lists "mock-model"; a `:free` variant of it is the same model.
        mock.lock()
            .unwrap()
            .set_models(vec!["mock-model".to_string()]);

        let cmd = ModelCommand;
        let mut messages = vec![];
        cmd.execute(Some("mock-model:free"), &mut ctx, &mut messages)
            .await
            .unwrap();

        let plain = strip_ansi(&captured_string(&buf));
        assert!(
            plain.contains("Switched to model:"),
            "suffixed variant of a listed base model should count as known: {plain}"
        );
        assert_eq!(
            ctx.provider.lock().await.current_model(),
            Some("mock-model:free".to_string())
        );
    }

    #[tokio::test]
    async fn select_variant_suffix_unknown_base_still_sets() {
        let (mut ctx, mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;

        mock.lock()
            .unwrap()
            .set_models(vec!["mock-model".to_string()]);

        let cmd = ModelCommand;
        let mut messages = vec![];
        cmd.execute(Some("unknown-model:free"), &mut ctx, &mut messages)
            .await
            .unwrap();

        let plain = strip_ansi(&captured_string(&buf));
        assert!(
            plain.contains("Set model to:"),
            "variant of an unknown base should still be selectable: {plain}"
        );
        assert_eq!(
            ctx.provider.lock().await.current_model(),
            Some("unknown-model:free".to_string())
        );
    }

    #[tokio::test]
    async fn select_ollama_style_tag_stays_unknown() {
        let (mut ctx, mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;

        // Ollama-style tags ("qwen2.5:7b") are not OpenRouter variants — the
        // listed base "mock-model" must not swallow "mock-model:7b".
        mock.lock()
            .unwrap()
            .set_models(vec!["mock-model".to_string()]);

        let cmd = ModelCommand;
        let mut messages = vec![];
        cmd.execute(Some("mock-model:7b"), &mut ctx, &mut messages)
            .await
            .unwrap();

        let plain = strip_ansi(&captured_string(&buf));
        assert!(
            plain.contains("Set model to:"),
            "non-variant colon tag should not match the base id: {plain}"
        );
    }
}
