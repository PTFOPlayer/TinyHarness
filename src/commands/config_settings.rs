use tinyharness_lib::config::{load_settings, save_settings};

use crate::async_command;
use crate::commands::registry::CommandResult;
use crate::style::*;

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
                println!(
                    "{}Current timeout: {}{}s{}",
                    BOLD, BLUE, settings.ollama_timeout_secs, RESET
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
                println!(
                    "{}Current max retries: {}{}{}",
                    BOLD, BLUE, settings.ollama_max_retries, RESET
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
    }
);

// ── ContextLimit (sync — no provider access needed) ──────────────────────────

/// Execute the /contextlimit command (sync — no provider access needed).
pub fn execute_context_limit(arg: Option<&str>) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
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

    if a == "auto" || a == "default" {
        let mut settings = load_settings();
        settings.context_limit = None;
        save_settings(&settings);
        println!(
            "{}Context limit cleared. Using model default for warnings.{}",
            BOLD, RESET
        );
        return Ok(CommandResult::Ok);
    }

    match a.parse::<u32>() {
        Ok(limit) if limit > 0 => {
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
            a
        )),
    }
}

// ── AutoAccept (sync — no provider access needed) ─────────────────────────────

/// Execute the /autoaccept command (sync — no provider access needed).
pub fn execute_autoaccept(arg: Option<&str>) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
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
    let status = if new_value { "enabled" } else { "disabled" };
    let color = if new_value { GREEN } else { ORANGE };
    println!(
        "{}Auto-accept safe commands set to {}{}{}{}",
        BOLD, color, status, RESET, RESET
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
                println!(
                    "{}Current think level: {}{}{}{}",
                    BOLD, BLUE, settings.ollama_think_type, RESET, RESET
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

            println!(
                "{}Think level set to {}{}{}.{}",
                BOLD, BLUE, think_type, RESET, RESET
            );

            Ok(CommandResult::Ok)
        }
    }
);
