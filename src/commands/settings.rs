use std::collections::HashSet;
use std::io::Write;

use tinyharness_lib::config::{AutoAcceptMode, load_settings};
use tinyharness_ui::output::Output;

use crate::commands::registry::CommandResult;
use tinyharness_ui::style::*;

pub fn execute(out: &mut Output, arg: Option<&str>) -> Result<CommandResult, String> {
    match arg {
        Some(sub) if sub.to_lowercase() == "all" => execute_all(out, &load_settings()),
        Some(other) => {
            let _ = writeln!(
                out,
                "{ORANGE}Unknown argument '{other}'.{RESET} Use {BOLD}/settings{RESET} to show settings or {BOLD}/settings all{RESET} to list all safe commands.",
            );
        }
        None => execute_summary(out, &load_settings()),
    }
    Ok(CommandResult::Ok)
}

fn execute_summary(out: &mut Output, settings: &tinyharness_lib::config::Settings) {
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{BOLD}╭─ Settings ─────────────────────────────────╮{RESET}",
    );

    let provider_str = format!("{}", settings.last_provider);
    let _ = writeln!(out, "{BOLD}│{RESET} Provider:  {BLUE}{provider_str}{RESET}",);

    match settings.get_current_model() {
        Some(model) => {
            let _ = writeln!(out, "{BOLD}│{RESET} Model:     {BLUE}{model}{RESET}");
        }
        None => {
            let _ = writeln!(out, "{BOLD}│{RESET} Model:     {ORANGE}none{RESET}");
        }
    }

    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Mode:      {BLUE}{}{RESET}",
        settings.preferred_mode,
    );

    match &settings.ollama_api_key {
        Some(key) => {
            let masked = if key.len() > 8 {
                format!("{}...{}", &key[..4], &key[key.len() - 4..])
            } else {
                "****".to_string()
            };
            let _ = writeln!(out, "{BOLD}│{RESET} API Key:   {BLUE}{masked}{RESET}");
        }
        None => {
            let _ = writeln!(out, "{BOLD}│{RESET} API Key:   {ORANGE}not set{RESET}");
        }
    }

    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Timeout:   {BLUE}{}s{RESET}",
        settings.ollama_timeout_secs,
    );
    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Retries:   {BLUE}{}{RESET}",
        settings.ollama_max_retries,
    );

    match settings.context_limit {
        Some(limit) => {
            let _ = writeln!(out, "{BOLD}│{RESET} Ctx Limit: {BLUE}{limit} tokens{RESET}",);
        }
        None => {
            let _ = writeln!(
                out,
                "{BOLD}│{RESET} Ctx Limit: {GRAY}auto (model default){RESET}",
            );
        }
    }

    let (auto_accept_str, auto_accept_color) = match settings.auto_accept_mode {
        AutoAcceptMode::All => ("ALL", GREEN),
        AutoAcceptMode::Safe => ("safe commands", GREEN),
        AutoAcceptMode::Off => ("off", ORANGE),
    };
    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Auto-Accept: {auto_accept_color}{auto_accept_str}{RESET}",
    );

    let safe_commands = settings.get_safe_commands();
    let denied_commands = settings.get_denied_commands();
    let _ = writeln!(
        out,
        "{BOLD}│{RESET} Safe Cmds:  {BLUE}{}{RESET} configured (use {BOLD}/settings all{RESET} to list all)",
        safe_commands.len(),
    );
    if !denied_commands.is_empty() {
        let _ = writeln!(
            out,
            "{BOLD}│{RESET} Denied Cmds: {RED}{}{RESET} always denied",
            denied_commands.len(),
        );
    }

    let _ = writeln!(
        out,
        "{BOLD}╰────────────────────────────────────────────╯{RESET}",
    );
    let _ = writeln!(out);
}

fn execute_all(out: &mut Output, settings: &tinyharness_lib::config::Settings) {
    let safe_commands = settings.get_safe_commands();
    let denied_commands = settings.get_denied_commands();
    let using_defaults = settings.safe_command_prefixes.is_none();

    // Compute default set for markers
    let defaults = tinyharness_lib::config::get_default_safe_commands();
    let default_set: HashSet<&str> = defaults.iter().map(|s| s.as_str()).collect();
    let custom_count = if using_defaults {
        0
    } else {
        safe_commands
            .iter()
            .filter(|c| !default_set.contains(c.as_str()))
            .count()
    };

    let _ = writeln!(out);
    if using_defaults {
        let _ = writeln!(
            out,
            "{BOLD}╭─ All Safe Commands ({0} defaults) ────────────╮{RESET}",
            safe_commands.len(),
        );
    } else {
        let _ = writeln!(
            out,
            "{BOLD}╭─ All Safe Commands ({0} configured, {custom_count} custom) ─╮{RESET}",
            safe_commands.len(),
        );
    }

    // Build markers: · for defaults, + for custom
    let cmds: Vec<&str> = safe_commands.iter().map(|s| s.as_str()).collect();
    let markers: Vec<char> = safe_commands
        .iter()
        .map(|c| {
            if default_set.contains(c.as_str()) {
                '·'
            } else {
                '+'
            }
        })
        .collect();

    let rows = crate::commands::command::format_command_rows(&cmds, &markers);
    for row in &rows {
        let _ = writeln!(out, "{BOLD}│{RESET}   {row}");
    }

    if !using_defaults {
        let _ = writeln!(
            out,
            "{BOLD}│{RESET}   {GRAY}· default{RESET}  {GREEN}+ custom{RESET}",
        );
    }

    let _ = writeln!(
        out,
        "{BOLD}╰───────────────────────────────────────────────╯{RESET}",
    );

    if !denied_commands.is_empty() {
        let denied_refs: Vec<&str> = denied_commands.iter().map(|s| s.as_str()).collect();
        let denied_rows = crate::commands::command::format_denied_command_rows(&denied_refs);

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{BOLD}╭─ Always-Deny Commands ({0} configured) ────────╮{RESET}",
            denied_commands.len(),
        );
        for row in &denied_rows {
            let _ = writeln!(out, "{BOLD}│{RESET}   {row}");
        }
        let _ = writeln!(
            out,
            "{BOLD}╰───────────────────────────────────────────────╯{RESET}",
        );
    }

    let _ = writeln!(out);
}
