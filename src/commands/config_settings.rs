use std::io::Write;

use tinyharness_lib::config::{load_settings, save_settings};
use tinyharness_ui::output::Output;

use crate::async_command;
use crate::commands::registry::{CommandContext, CommandResult};
use tinyharness_ui::style::*;

// ── Timeout (async — needs provider.lock().await) ─────────────────────────────

async_command!(
    TimeoutCommand,
    "/timeout",
    "Show or set the Ollama request timeout in seconds (default: 5)",
    "/timeout [secs]",
    |raw_arg, ctx, _messages| {
        let arg = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();
        async move {
            if arg.is_empty() {
                let settings = load_settings();
                let _ = writeln!(
                    ctx.output,
                    "{BOLD}Current timeout: {BLUE}{}s{RESET}",
                    settings.ollama_timeout_secs,
                );
                return Ok(CommandResult::Ok);
            }

            match arg.parse::<u64>() {
                Ok(secs) if secs > 0 => {
                    let mut settings = load_settings();
                    settings.ollama_timeout_secs = secs;
                    save_settings(&settings);
                    let mut p = provider.lock().await;
                    p.set_timeout(secs);
                    let _ = writeln!(ctx.output, "{BOLD}Timeout set to {BLUE}{secs}s.{RESET}",);
                    Ok(CommandResult::Ok)
                }
                Ok(_) => Err("Timeout must be a positive number of seconds.".to_string()),
                Err(_) => Err(format!(
                    "Invalid timeout value: '{}'. Use a number of seconds, e.g. /timeout 30",
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
    "Show or set the maximum number of Ollama request retries (default: 3)",
    "/retries [count]",
    |raw_arg, ctx, _messages| {
        let arg = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();
        async move {
            if arg.is_empty() {
                let settings = load_settings();
                let _ = writeln!(
                    ctx.output,
                    "{BOLD}Current max retries: {BLUE}{}{RESET}",
                    settings.ollama_max_retries,
                );
                return Ok(CommandResult::Ok);
            }

            match arg.parse::<u32>() {
                Ok(count) => {
                    let mut settings = load_settings();
                    settings.ollama_max_retries = count;
                    save_settings(&settings);
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
pub fn execute_context_limit(out: &mut Output, arg: Option<&str>) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
        let settings = load_settings();
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
        let mut settings = load_settings();
        settings.context_limit = None;
        save_settings(&settings);
        let _ = writeln!(
            out,
            "{BOLD}Context limit cleared. Using model default for warnings.{RESET}",
        );
        return Ok(CommandResult::Ok);
    }

    match a.parse::<u32>() {
        Ok(limit) if limit > 0 => {
            let mut settings = load_settings();
            settings.context_limit = Some(limit);
            save_settings(&settings);
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
pub fn execute_autoaccept(out: &mut Output, arg: Option<&str>) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
        let settings = load_settings();
        let (status, color) = if settings.auto_accept_safe_commands {
            ("enabled", GREEN)
        } else {
            ("disabled", ORANGE)
        };
        let _ = writeln!(
            out,
            "{BOLD}Auto-accept safe commands: {color}{status}{RESET}",
        );
        return Ok(CommandResult::Ok);
    }

    let new_value = match a.to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => true,
        "off" | "false" | "no" | "0" => false,
        _ => {
            return Err("Invalid value. Use 'on' or 'off', e.g. /autoaccept on".to_string());
        }
    };

    let mut settings = load_settings();
    settings.auto_accept_safe_commands = new_value;
    save_settings(&settings);
    let (status, color) = if new_value {
        ("enabled", GREEN)
    } else {
        ("disabled", ORANGE)
    };
    let _ = writeln!(
        out,
        "{BOLD}Auto-accept safe commands set to {color}{status}{RESET}",
    );

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
                let settings = load_settings();
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

            let mut settings = load_settings();
            settings.ollama_think_type = think_type;
            save_settings(&settings);

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
    let mut settings = load_settings();
    settings.show_thinking = new_value;
    save_settings(&settings);

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
