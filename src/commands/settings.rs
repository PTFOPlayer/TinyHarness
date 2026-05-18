use std::collections::HashSet;

use tinyharness_lib::config::load_settings;

use crate::commands::registry::CommandResult;
use crate::style::*;

pub fn execute(arg: Option<&str>) -> Result<CommandResult, String> {
    match arg {
        Some(sub) if sub.to_lowercase() == "all" => execute_all(&load_settings()),
        Some(other) => {
            println!(
                "{}Unknown argument '{}'.{} Use {}/settings{} to show settings or {}/settings all{} to list all safe commands.",
                ORANGE, other, RESET, BOLD, RESET, BOLD, RESET
            );
        }
        None => execute_summary(&load_settings()),
    }
    Ok(CommandResult::Ok)
}

fn execute_summary(settings: &tinyharness_lib::config::Settings) {
    println!();
    println!(
        "{}╭─ Settings ─────────────────────────────────╮{}",
        BOLD, RESET
    );

    let provider_str = format!("{}", settings.last_provider);
    println!(
        "{}│{} Provider:  {}{}{}",
        BOLD, RESET, BLUE, provider_str, RESET
    );

    match &settings.last_model {
        Some(model) => println!("{}│{} Model:     {}{}{}", BOLD, RESET, BLUE, model, RESET),
        None => println!("{}│{} Model:     {}none{}", BOLD, RESET, ORANGE, RESET),
    }

    println!(
        "{}│{} Mode:      {}{}{}",
        BOLD, RESET, BLUE, settings.preferred_mode, RESET
    );

    match &settings.ollama_api_key {
        Some(key) => {
            let masked = if key.len() > 8 {
                format!("{}...{}", &key[..4], &key[key.len() - 4..])
            } else {
                "****".to_string()
            };
            println!("{}│{} API Key:   {}{}{}", BOLD, RESET, BLUE, masked, RESET);
        }
        None => println!("{}│{} API Key:   {}not set{}", BOLD, RESET, ORANGE, RESET),
    }

    println!(
        "{}│{} Timeout:   {}{}s{}",
        BOLD, RESET, BLUE, settings.ollama_timeout_secs, RESET
    );
    println!(
        "{}│{} Retries:   {}{}{}",
        BOLD, RESET, BLUE, settings.ollama_max_retries, RESET
    );

    match settings.context_limit {
        Some(limit) => println!(
            "{}│{} Ctx Limit: {}{} tokens{}",
            BOLD, RESET, BLUE, limit, RESET
        ),
        None => println!(
            "{}│{} Ctx Limit: {}auto (model default){}",
            BOLD, RESET, GRAY, RESET
        ),
    }

    let auto_accept_str = if settings.auto_accept_safe_commands {
        "enabled"
    } else {
        "disabled"
    };
    let auto_accept_color = if settings.auto_accept_safe_commands {
        GREEN
    } else {
        ORANGE
    };
    println!(
        "{}│{} Auto-Accept: {}{}{} (safe commands)",
        BOLD, RESET, auto_accept_color, auto_accept_str, RESET
    );

    let safe_commands = settings.get_safe_commands();
    let denied_commands = settings.get_denied_commands();
    println!(
        "{}│{} Safe Cmds:  {}{}{} configured (use {}/settings all{} to list all)",
        BOLD,
        RESET,
        BLUE,
        safe_commands.len(),
        RESET,
        BOLD,
        RESET
    );
    if !denied_commands.is_empty() {
        println!(
            "{}│{} Denied Cmds: {}{}{} always denied",
            BOLD,
            RED,
            denied_commands.len(),
            RESET,
            RESET
        );
    }

    println!(
        "{}╰────────────────────────────────────────────╯{}",
        BOLD, RESET
    );
    println!();
}

fn execute_all(settings: &tinyharness_lib::config::Settings) {
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

    println!();
    if using_defaults {
        println!(
            "{}╭─ All Safe Commands ({} defaults) ────────────╮{}",
            BOLD,
            safe_commands.len(),
            RESET
        );
    } else {
        println!(
            "{}╭─ All Safe Commands ({} configured, {} custom) ─╮{}",
            BOLD,
            safe_commands.len(),
            custom_count,
            RESET
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
        println!("{}│{}   {}", BOLD, RESET, row);
    }

    if !using_defaults {
        println!(
            "{}│{}   {}· {}default{}  {}+ {}custom{}",
            BOLD, RESET, GRAY, RESET, GRAY, GREEN, RESET, RESET
        );
    }

    println!(
        "{}╰───────────────────────────────────────────────╯{}",
        BOLD, RESET
    );

    if !denied_commands.is_empty() {
        let denied_refs: Vec<&str> = denied_commands.iter().map(|s| s.as_str()).collect();
        let denied_rows = crate::commands::command::format_denied_command_rows(&denied_refs);

        println!();
        println!(
            "{}╭─ Always-Deny Commands ({} configured) ────────╮{}",
            BOLD,
            denied_commands.len(),
            RESET
        );
        for row in &denied_rows {
            println!("{}│{}   {}", BOLD, RESET, row);
        }
        println!(
            "{}╰───────────────────────────────────────────────╯{}",
            BOLD, RESET
        );
    }

    println!();
}
