use std::io::Write;

use tinyharness_lib::Paths;
use tinyharness_lib::config::{
    AutoAcceptMode, load_settings_from, save_settings_to, timeout_default_for,
};
use tinyharness_lib::provider::Provider;
use tinyharness_ui::output::Output;

use crate::async_command;
use crate::commands::registry::{CommandContext, CommandResult};
use tinyharness_ui::style::*;

// ── Timeout (async — needs provider.lock().await) ─────────────────────────────

async_command!(
    TimeoutCommand,
    "/timeout",
    "Show or set the request timeout in seconds (0 resets to the per-provider default)",
    "/timeout [secs]",
    |raw_arg, ctx, _messages| {
        let arg = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();
        async move {
            if arg.is_empty() {
                let settings = load_settings_from(&ctx.paths);
                let _ = writeln!(
                    ctx.output,
                    "{BOLD}Current timeout: {BLUE}{}s{RESET} {GRAY}({}){RESET}",
                    settings.effective_timeout_secs(),
                    settings.last_provider,
                );
                return Ok(CommandResult::Ok);
            }

            match arg.parse::<u64>() {
                Ok(0) => {
                    let mut settings = load_settings_from(&ctx.paths);
                    settings.request_timeout_secs = None;
                    if let Err(e) = save_settings_to(&ctx.paths, &settings) {
                        tracing::warn!("Failed to save settings: {e}");
                    }
                    let default_secs = timeout_default_for(settings.last_provider);
                    let mut p = provider.lock().await;
                    p.set_timeout(default_secs);
                    let _ = writeln!(
                        ctx.output,
                        "{BOLD}Timeout reset to the default {BLUE}{default_secs}s{RESET} {GRAY}({}){RESET}.",
                        settings.last_provider,
                    );
                    Ok(CommandResult::Ok)
                }
                Ok(secs) => {
                    let mut settings = load_settings_from(&ctx.paths);
                    settings.request_timeout_secs = Some(secs);
                    if let Err(e) = save_settings_to(&ctx.paths, &settings) {
                        tracing::warn!("Failed to save settings: {e}");
                    }
                    let mut p = provider.lock().await;
                    p.set_timeout(secs);
                    let _ = writeln!(ctx.output, "{BOLD}Timeout set to {BLUE}{secs}s.{RESET}",);
                    Ok(CommandResult::Ok)
                }
                Err(_) => Err(format!(
                    "Invalid timeout value: '{}'. Use a number of seconds, e.g. /timeout 30 (0 resets to the default)",
                    arg
                )),
            }
        }
    }
);

// ── Retries (async — needs provider.lock().await) ─────────────────────────────

async_command!(
    RetriesCommand,
    "/retries",
    "Show or set the maximum number of request retries (applies to the active provider)",
    "/retries [count]",
    |raw_arg, ctx, _messages| {
        let arg = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();
        async move {
            if arg.is_empty() {
                let settings = load_settings_from(&ctx.paths);
                let _ = writeln!(
                    ctx.output,
                    "{BOLD}Current max retries: {BLUE}{}{RESET} {GRAY}({}){RESET}",
                    settings.effective_max_retries(),
                    settings.last_provider,
                );
                return Ok(CommandResult::Ok);
            }

            match arg.parse::<u32>() {
                Ok(count) => {
                    let mut settings = load_settings_from(&ctx.paths);
                    settings.request_max_retries = Some(count);
                    if let Err(e) = save_settings_to(&ctx.paths, &settings) {
                        tracing::warn!("Failed to save settings: {e}");
                    }
                    let mut p = provider.lock().await;
                    p.set_retries(count);
                    let _ = writeln!(ctx.output, "{BOLD}Max retries set to {BLUE}{count}.{RESET}",);
                    Ok(CommandResult::Ok)
                }
                Err(_) => Err(format!(
                    "Invalid retries value: '{}'. Use a number, e.g. /retries 5",
                    arg
                )),
            }
        }
    }
);

// ── ContextLimit (sync — no provider access needed) ──────────────────────────

/// Execute the /contextlimit command (sync — no provider access needed).
pub fn execute_context_limit(
    out: &mut Output,
    paths: &Paths,
    arg: Option<&str>,
) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
        let settings = load_settings_from(paths);
        match settings.context_limit {
            Some(limit) => {
                let _ = writeln!(
                    out,
                    "{BOLD}Context limit for warnings: {BLUE}{limit} tokens{RESET}",
                );
            }
            None => {
                let _ = writeln!(
                    out,
                    "{BOLD}Context limit: {GRAY}auto (using model default){RESET}",
                );
            }
        }
        return Ok(CommandResult::Ok);
    }

    if a == "auto" || a == "default" {
        let mut settings = load_settings_from(paths);
        settings.context_limit = None;
        if let Err(e) = save_settings_to(paths, &settings) {
            tracing::warn!("Failed to save settings: {e}");
        }
        let _ = writeln!(
            out,
            "{BOLD}Context limit cleared. Using model default for warnings.{RESET}",
        );
        return Ok(CommandResult::Ok);
    }

    match a.parse::<u32>() {
        Ok(limit) if limit > 0 => {
            let mut settings = load_settings_from(paths);
            settings.context_limit = Some(limit);
            if let Err(e) = save_settings_to(paths, &settings) {
                tracing::warn!("Failed to save settings: {e}");
            }
            let _ = writeln!(
                out,
                "{BOLD}Context limit set to {BLUE}{limit} tokens{RESET} for warning calculations.",
            );
            Ok(CommandResult::Ok)
        }
        Ok(_) => Err("Context limit must be a positive number of tokens.".to_string()),
        Err(_) => Err(format!(
            "Invalid context limit value: '{}'. Use a number of tokens, e.g. /contextlimit 32768, or 'auto' to use model default",
            a
        )),
    }
}

// ── AutoAccept (sync — no provider access needed) ─────────────────────────────

/// Execute the /autoaccept command (sync — no provider access needed).
pub fn execute_autoaccept(
    out: &mut Output,
    paths: &Paths,
    arg: Option<&str>,
) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
        let settings = load_settings_from(paths);
        let (mode, color) = match settings.auto_accept_mode {
            AutoAcceptMode::All => ("all (all tools)", GREEN),
            AutoAcceptMode::Safe => ("safe commands", GREEN),
            AutoAcceptMode::Off => ("off", ORANGE),
        };
        let _ = writeln!(out, "{BOLD}Auto-accept: {color}{mode}{RESET}",);
        return Ok(CommandResult::Ok);
    }

    let mode = match a.to_lowercase().as_str() {
        "all" | "always" | "true" | "yes" | "1" => AutoAcceptMode::All,
        "safe" | "on" => AutoAcceptMode::Safe,
        "off" | "false" | "no" | "0" => AutoAcceptMode::Off,
        _ => {
            return Err(
                "Invalid value. Use 'all', 'safe', or 'off', e.g. /autoaccept all".to_string(),
            );
        }
    };

    let mut settings = load_settings_from(paths);
    settings.auto_accept_mode = mode;
    if let Err(e) = save_settings_to(paths, &settings) {
        tracing::warn!("Failed to save settings: {e}");
    }

    let (mode_str, color) = match mode {
        AutoAcceptMode::All => ("all tools", GREEN),
        AutoAcceptMode::Safe => ("safe commands", GREEN),
        AutoAcceptMode::Off => ("off", ORANGE),
    };
    let _ = writeln!(out, "{BOLD}Auto-accept set to {color}{mode_str}{RESET}",);

    Ok(CommandResult::Ok)
}

// ── AutoCompact (sync — no provider access needed) ───────────────────────────

/// Execute the /autocompact command to toggle the auto_compact tool.
pub fn execute_autocompact(
    out: &mut Output,
    paths: &Paths,
    arg: Option<&str>,
) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
        let settings = load_settings_from(paths);
        let (status, color) = if settings.auto_compact_enabled {
            ("on", GREEN)
        } else {
            ("off", ORANGE)
        };
        let _ = writeln!(out, "{BOLD}Auto-compact: {color}{status}{RESET}",);
        return Ok(CommandResult::Ok);
    }

    let new_value = match a.to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => true,
        "off" | "false" | "no" | "0" => false,
        _ => {
            return Err("Invalid value. Use 'on' or 'off', e.g. /autocompact off".to_string());
        }
    };

    let mut settings = load_settings_from(paths);
    settings.auto_compact_enabled = new_value;
    if let Err(e) = save_settings_to(paths, &settings) {
        tracing::warn!("Failed to save settings: {e}");
    }

    let (status, color) = if new_value {
        ("on", GREEN)
    } else {
        ("off", ORANGE)
    };
    let _ = writeln!(out, "{BOLD}Auto-compact set to {color}{status}{RESET}",);

    Ok(CommandResult::Ok)
}

// ── Questions (sync — no provider access needed) ─────────────────────────────

/// Execute the /questions command to toggle the question tool.
pub fn execute_questions(
    out: &mut Output,
    paths: &Paths,
    arg: Option<&str>,
) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
        let settings = load_settings_from(paths);
        let (status, color) = if settings.questions_enabled {
            ("on", GREEN)
        } else {
            ("off", ORANGE)
        };
        let _ = writeln!(out, "{BOLD}Questions: {color}{status}{RESET}",);
        return Ok(CommandResult::Ok);
    }

    let new_value = match a.to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => true,
        "off" | "false" | "no" | "0" => false,
        _ => {
            return Err("Invalid value. Use 'on' or 'off', e.g. /questions off".to_string());
        }
    };

    let mut settings = load_settings_from(paths);
    settings.questions_enabled = new_value;
    if let Err(e) = save_settings_to(paths, &settings) {
        tracing::warn!("Failed to save settings: {e}");
    }

    let (status, color) = if new_value {
        ("on", GREEN)
    } else {
        ("off", ORANGE)
    };
    let _ = writeln!(out, "{BOLD}Questions set to {color}{status}{RESET}",);

    Ok(CommandResult::Ok)
}

// ── Think Type (async — needs provider.lock().await) ───────────────────────

async_command!(
    ThinkCommand,
    "/think",
    "Show or set the Ollama think/reasoning level (off, low, medium, high)",
    "/think [off|low|medium|high]",
    |raw_arg, ctx, _messages| {
        let arg = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();
        async move {
            if arg.is_empty() {
                let settings = load_settings_from(&ctx.paths);
                let _ = writeln!(
                    ctx.output,
                    "{BOLD}Current think level: {BLUE}{}{RESET}",
                    settings.ollama_think_type,
                );
                return Ok(CommandResult::Ok);
            }

            let think_type = match arg.parse::<tinyharness_lib::config::OllamaThinkType>() {
                Ok(tt) => tt,
                Err(e) => return Err(e),
            };

            let mut settings = load_settings_from(&ctx.paths);
            settings.ollama_think_type = think_type;
            if let Err(e) = save_settings_to(&ctx.paths, &settings) {
                tracing::warn!("Failed to save settings: {e}");
            }

            let mut p = provider.lock().await;
            p.set_think_type(think_type);

            let _ = writeln!(
                ctx.output,
                "{BOLD}Think level set to {BLUE}{think_type}.{RESET}",
            );

            Ok(CommandResult::Ok)
        }
    }
);

// ── Show Think (sync — no provider access needed) ─────────────────────────────

/// Execute the /showthink command to toggle display of the model's thinking/reasoning chain.
pub fn execute_showthink(
    arg: Option<&str>,
    ctx: &mut CommandContext,
) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
        let (status, color) = if ctx.show_thinking {
            ("on", GREEN)
        } else {
            ("off", GRAY)
        };
        let _ = writeln!(
            ctx.output,
            "{BOLD}Show thinking chain: {color}{status}{RESET}",
        );
        return Ok(CommandResult::Ok);
    }

    let new_value = match a.to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => true,
        "off" | "false" | "no" | "0" => false,
        _ => {
            return Err("Invalid value. Use 'on' or 'off', e.g. /showthink on".to_string());
        }
    };

    ctx.show_thinking = new_value;
    let mut settings = load_settings_from(&ctx.paths);
    settings.show_thinking = new_value;
    if let Err(e) = save_settings_to(&ctx.paths, &settings) {
        tracing::warn!("Failed to save settings: {e}");
    }

    let (status, color) = if new_value {
        ("on", GREEN)
    } else {
        ("off", GRAY)
    };
    let _ = writeln!(
        ctx.output,
        "{BOLD}Show thinking chain set to {color}{status}{RESET}",
    );

    Ok(CommandResult::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{captured_output, captured_string, isolated_paths, strip_ansi};

    #[test]
    fn context_limit_invalid_value_returns_error() {
        let (mut out, _buf) = captured_output();
        let paths = isolated_paths("config_settings");
        let result = execute_context_limit(&mut out, &paths, Some("abc"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid context limit"));
    }

    #[test]
    fn context_limit_zero_returns_error() {
        let (mut out, _buf) = captured_output();
        let paths = isolated_paths("config_settings");
        let result = execute_context_limit(&mut out, &paths, Some("0"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("positive number"));
    }

    #[test]
    fn autoaccept_invalid_value_returns_error() {
        let (mut out, _buf) = captured_output();
        let paths = isolated_paths("config_settings");
        let result = execute_autoaccept(&mut out, &paths, Some("invalid"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid value"));
    }

    #[test]
    fn autoaccept_empty_shows_current() {
        let (mut out, buf) = captured_output();
        let paths = isolated_paths("config_settings");
        let _ = execute_autoaccept(&mut out, &paths, None);
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Auto-accept:"));
    }

    #[test]
    fn autocompact_invalid_value_returns_error() {
        let (mut out, _buf) = captured_output();
        let paths = isolated_paths("config_settings");
        let result = execute_autocompact(&mut out, &paths, Some("maybe"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid value"));
    }

    #[test]
    fn autocompact_empty_shows_current() {
        let (mut out, buf) = captured_output();
        let paths = isolated_paths("config_settings");
        let _ = execute_autocompact(&mut out, &paths, None);
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Auto-compact:"));
    }

    #[test]
    fn questions_invalid_value_returns_error() {
        let (mut out, _buf) = captured_output();
        let paths = isolated_paths("config_settings");
        let result = execute_questions(&mut out, &paths, Some("maybe"));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid value"));
    }

    #[test]
    fn questions_empty_shows_current() {
        let (mut out, buf) = captured_output();
        let paths = isolated_paths("config_settings");
        let _ = execute_questions(&mut out, &paths, None);
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Questions:"));
    }
}
