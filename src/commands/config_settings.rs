use std::io::Write;

use tinyharness_lib::Paths;
use tinyharness_lib::config::{
    AutoAcceptMode, load_settings_from, timeout_default_for, update_settings_from,
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
                    let settings = update_settings_from(&ctx.paths, |s| {
                        s.request_timeout_secs = None;
                    });
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
                    update_settings_from(&ctx.paths, |s| {
                        s.request_timeout_secs = Some(secs);
                    });
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
                    update_settings_from(&ctx.paths, |s| {
                        s.request_max_retries = Some(count);
                    });
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
        update_settings_from(paths, |s| {
            s.context_limit = None;
        });
        let _ = writeln!(
            out,
            "{BOLD}Context limit cleared. Using model default for warnings.{RESET}",
        );
        return Ok(CommandResult::Ok);
    }

    match a.parse::<u32>() {
        Ok(limit) if limit > 0 => {
            update_settings_from(paths, |s| {
                s.context_limit = Some(limit);
            });
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

    update_settings_from(paths, |s| {
        s.auto_accept_mode = mode;
    });

    let (mode_str, color) = match mode {
        AutoAcceptMode::All => ("all tools", GREEN),
        AutoAcceptMode::Safe => ("safe commands", GREEN),
        AutoAcceptMode::Off => ("off", ORANGE),
    };
    let _ = writeln!(out, "{BOLD}Auto-accept set to {color}{mode_str}{RESET}",);

    Ok(CommandResult::Ok)
}

// ── Shared boolean toggle helper ─────────────────────────────────────────────

/// Parse an on/off argument string. Returns `Some(bool)` for valid values,
/// `None` otherwise.
fn parse_toggle(s: &str) -> Option<bool> {
    match s.to_lowercase().as_str() {
        "on" | "true" | "yes" | "1" => Some(true),
        "off" | "false" | "no" | "0" => Some(false),
        _ => None,
    }
}

/// Generic show/set toggle for a boolean setting field.
///
/// `label` is the display name (e.g. "Auto-compact"), `cmd` is the command
/// name for error messages (e.g. "autocompact"), `off_color` is the ANSI
/// color for the off state.
fn toggle_bool_setting(
    out: &mut Output,
    paths: &Paths,
    arg: Option<&str>,
    label: &str,
    cmd: &str,
    off_color: &str,
    get: impl Fn(&tinyharness_lib::config::Settings) -> bool,
    set: impl Fn(&mut tinyharness_lib::config::Settings, bool),
) -> Result<CommandResult, String> {
    let a = arg.unwrap_or("");

    if a.is_empty() {
        let settings = load_settings_from(paths);
        let (status, color) = if get(&settings) {
            ("on", GREEN)
        } else {
            ("off", off_color)
        };
        let _ = writeln!(out, "{BOLD}{label}: {color}{status}{RESET}",);
        return Ok(CommandResult::Ok);
    }

    let new_value = parse_toggle(a).ok_or_else(|| {
        format!("Invalid value. Use 'on' or 'off', e.g. /{cmd} off")
    })?;

    update_settings_from(paths, |s| set(s, new_value));

    let (status, color) = if new_value {
        ("on", GREEN)
    } else {
        ("off", off_color)
    };
    let _ = writeln!(out, "{BOLD}{label} set to {color}{status}{RESET}",);

    Ok(CommandResult::Ok)
}

// ── AutoCompact (sync — no provider access needed) ───────────────────────────

/// Execute the /autocompact command to toggle the auto_compact tool.
pub fn execute_autocompact(
    out: &mut Output,
    paths: &Paths,
    arg: Option<&str>,
) -> Result<CommandResult, String> {
    toggle_bool_setting(
        out,
        paths,
        arg,
        "Auto-compact",
        "autocompact",
        ORANGE,
        |s| s.auto_compact_enabled,
        |s, v| s.auto_compact_enabled = v,
    )
}

// ── Questions (sync — no provider access needed) ─────────────────────────────

/// Execute the /questions command to toggle the question tool.
pub fn execute_questions(
    out: &mut Output,
    paths: &Paths,
    arg: Option<&str>,
) -> Result<CommandResult, String> {
    toggle_bool_setting(
        out,
        paths,
        arg,
        "Questions",
        "questions",
        ORANGE,
        |s| s.questions_enabled,
        |s, v| s.questions_enabled = v,
    )
}

// ── Show Think (sync — no provider access needed) ─────────────────────────────

/// Execute the /showthink command to toggle display of the model's thinking/reasoning chain.
pub fn execute_showthink(
    arg: Option<&str>,
    ctx: &mut CommandContext,
) -> Result<CommandResult, String> {
    // show_thinking lives on the context, not just in settings — update both.
    let result = toggle_bool_setting(
        &mut ctx.output,
        &ctx.paths,
        arg,
        "Show thinking chain",
        "showthink",
        GRAY,
        |s| s.show_thinking,
        |s, v| s.show_thinking = v,
    );
    // Sync the in-memory context flag with the parsed value.
    if let Some(v) = arg.and_then(parse_toggle) {
        ctx.show_thinking = v;
    }
    result
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
