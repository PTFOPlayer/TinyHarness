use std::collections::HashSet;

use tinyharness_lib::config::{get_default_safe_commands, load_settings, save_settings};

use crate::commands::registry::CommandResult;
use crate::style::*;

// ── Core implementation ─────────────────────────────────────────────────────

/// Check if a command prefix would match any command in the safe list.
fn matches_safe_prefix(cmd: &str, safe_commands: &[String]) -> bool {
    for prefix in safe_commands {
        if cmd.starts_with(prefix) {
            let rest = &cmd[prefix.len()..];
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('=') {
                return true;
            }
        }
    }
    false
}

pub fn execute(args: &str) -> Result<CommandResult, String> {
    let args = args.trim();

    if args.is_empty() {
        execute_list();
        return Ok(CommandResult::Ok);
    }

    // Split into subcommand + rest
    let sub_parts: Vec<&str> = args.splitn(2, ' ').collect();
    let sub = sub_parts[0].to_lowercase();
    let cmd_arg = sub_parts
        .get(1)
        .map(|s| s.trim())
        .unwrap_or("")
        .trim_matches('"')
        .trim_matches('\'')
        .to_string();

    match sub.as_str() {
        "add" => execute_add(&cmd_arg),
        "rm" | "remove" => execute_remove(&cmd_arg),
        "deny" => execute_deny(&cmd_arg),
        "undeny" | "allow" => execute_undeny(&cmd_arg),
        "list" | "ls" => execute_list(),
        "reset" => execute_reset(),
        "resetdeny" => execute_reset_deny(),
        "help" => execute_help(),
        _ => execute_list(),
    }

    Ok(CommandResult::Ok)
}

pub fn execute_add(cmd: &str) {
    if cmd.is_empty() {
        println!(
            "{}Usage:{} /command add <command>{} — e.g. /command add docker",
            BOLD, RESET, RESET
        );
        return;
    }

    let mut settings = load_settings();
    let mut commands = settings.get_safe_commands();

    if commands.contains(&cmd.to_string()) {
        println!(
            "{}Command '{}' is already in the safe list.{}",
            ORANGE, cmd, RESET
        );
        return;
    }

    // Warn if the command is also on the deny list
    let denied = settings.get_denied_commands();
    if denied.contains(&cmd.to_string()) {
        println!(
            "{}Note:{} '{}' is on the deny list. It will still be blocked until removed with {}/command undeny {}{}",
            YELLOW, RESET, cmd, BOLD, cmd, RESET
        );
    }

    commands.push(cmd.to_string());
    settings.safe_command_prefixes = Some(commands);
    save_settings(&settings);
    println!(
        "{}Added '{}' to auto-accepted commands.{}",
        GREEN, cmd, RESET
    );
}

pub fn execute_remove(cmd: &str) {
    if cmd.is_empty() {
        println!(
            "{}Usage:{} /command rm <command>{} — e.g. /command rm docker",
            BOLD, RESET, RESET
        );
        return;
    }

    let mut settings = load_settings();
    let mut commands = settings.get_safe_commands();
    let initial_len = commands.len();

    commands.retain(|c| c != cmd);

    if commands.len() < initial_len {
        settings.safe_command_prefixes = if commands.is_empty() {
            None // Revert to defaults if empty
        } else {
            Some(commands)
        };
        save_settings(&settings);
        println!(
            "{}Removed '{}' from auto-accepted commands.{}",
            GREEN, cmd, RESET
        );
    } else {
        println!(
            "{}Command '{}' not found in auto-accepted list.{}",
            ORANGE, cmd, RESET
        );
    }
}

pub fn execute_deny(cmd: &str) {
    if cmd.is_empty() {
        println!(
            "{}Usage:{} /command deny <command>{} — e.g. /command deny git push",
            BOLD, RESET, RESET
        );
        return;
    }

    let mut settings = load_settings();
    let denied = settings.get_denied_commands();

    if denied.contains(&cmd.to_string()) {
        println!(
            "{}Command '{}' is already in the deny list.{}",
            ORANGE, cmd, RESET
        );
        return;
    }

    // Warn if the command is currently auto-accepted (on safe list)
    let safe_commands = settings.get_safe_commands();
    if matches_safe_prefix(cmd, &safe_commands) {
        println!(
            "{}Note:{} '{}' is currently auto-accepted. Denying will override it — it will always require confirmation.{}",
            YELLOW, RESET, cmd, RESET
        );
    } else {
        // Warn if the command isn't on the safe list anyway
        println!(
            "{}Note:{} '{}' is not currently auto-accepted, so denying it has no practical effect.{}",
            GRAY, RESET, cmd, RESET
        );
    }

    let mut denied = denied; // move out of &settings
    denied.push(cmd.to_string());
    settings.denied_command_prefixes = Some(denied);
    save_settings(&settings);
    println!(
        "{}Denied '{}' — it will always require confirmation.{}",
        RED, cmd, RESET
    );
}

pub fn execute_undeny(cmd: &str) {
    if cmd.is_empty() {
        println!(
            "{}Usage:{} /command undeny <command>{} — e.g. /command undeny git push",
            BOLD, RESET, RESET
        );
        return;
    }

    let mut settings = load_settings();
    let mut denied = settings.get_denied_commands();
    let initial_len = denied.len();

    denied.retain(|c| c != cmd);

    if denied.len() < initial_len {
        settings.denied_command_prefixes = if denied.is_empty() {
            None
        } else {
            Some(denied)
        };
        save_settings(&settings);
        println!("{}Removed '{}' from the deny list.{}", GREEN, cmd, RESET);
    } else {
        println!(
            "{}Command '{}' not found in the deny list.{}",
            ORANGE, cmd, RESET
        );
    }
}

/// Format commands in a 3-per-row grid, with an optional marker prefix
/// for each command (e.g. `+` for custom, `·` for default).
pub fn format_command_rows(cmds: &[&str], markers: &[char]) -> Vec<String> {
    let mut rows = Vec::new();
    for row in cmds.chunks(3) {
        let mut line = String::new();
        for (i, cmd) in row.iter().enumerate() {
            if i > 0 {
                let prev = row[i - 1];
                // Account for the marker character + space before prev
                let prev_width = prev.len() + 2; // "marker cmd" = cmd.len() + 2
                let padding = 22_usize.saturating_sub(prev_width);
                line.push_str(&" ".repeat(padding));
            }
            let marker = markers.get(i).copied().unwrap_or('·');
            line.push_str(&format!("{}{} {}{}", marker, CYAN, cmd, RESET));
        }
        rows.push(line);
    }
    rows
}

/// Format commands in a 3-per-row grid with ✕ prefix for denied commands.
pub fn format_denied_command_rows(cmds: &[&str]) -> Vec<String> {
    let mut rows = Vec::new();
    for row in cmds.chunks(3) {
        let mut line = String::new();
        for (i, cmd) in row.iter().enumerate() {
            if i > 0 {
                let prev = row[i - 1];
                let prev_width = prev.len() + 2; // "✕ cmd" = cmd.len() + 2
                let padding = 22_usize.saturating_sub(prev_width);
                line.push_str(&" ".repeat(padding));
            }
            line.push_str(&format!("{}✕ {}{}{}", RED, cmd, RESET, RESET));
        }
        rows.push(line)
    }
    rows
}

pub fn execute_list() {
    let settings = load_settings();
    let safe_commands = settings.get_safe_commands();
    let denied = settings.get_denied_commands();
    let using_defaults = settings.safe_command_prefixes.is_none();

    // Compute which safe commands are defaults vs custom
    let defaults = get_default_safe_commands();
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
    println!(
        "{}╭─ Auto-Accepted Commands ─────────────────────╮{}",
        BOLD, RESET
    );
    if using_defaults {
        println!(
            "{}│{} {}Using defaults{} ({} commands){}",
            BOLD,
            RESET,
            GRAY,
            RESET,
            safe_commands.len(),
            RESET
        );
    } else {
        println!(
            "{}│{} {}{}{} configured ({} custom){}",
            BOLD,
            RESET,
            BLUE,
            safe_commands.len(),
            RESET,
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
    let cmd_refs: Vec<&str> = cmds.to_vec();

    // Format and print rows
    let rows = format_command_rows(&cmd_refs, &markers);
    for row in &rows {
        println!("{}│{}   {}", BOLD, RESET, row);
    }

    // Legend
    if !using_defaults {
        println!(
            "{}│{}   {}· {}default{}  {}+ {}custom{}",
            BOLD, RESET, GRAY, RESET, GRAY, GREEN, RESET, RESET
        );
    }

    println!(
        "{}╰──────────────────────────────────────────────╯{}",
        BOLD, RESET
    );

    if !denied.is_empty() {
        let denied_refs: Vec<&str> = denied.iter().map(|s| s.as_str()).collect();
        let denied_rows = format_denied_command_rows(&denied_refs);

        println!();
        println!(
            "{}╭─ Always-Deny Commands ──────────────────────╮{}",
            BOLD, RESET
        );
        println!(
            "{}│{} {}{}{} commands denied (always require confirmation){}",
            BOLD,
            RESET,
            RED,
            denied.len(),
            RESET,
            RESET
        );
        for row in &denied_rows {
            println!("{}│{}   {}", BOLD, RESET, row);
        }
        println!(
            "{}╰──────────────────────────────────────────────╯{}",
            BOLD, RESET
        );
    }

    println!();
}

pub fn execute_help() {
    println!();
    println!("{}Command management — subcommands:{}", BOLD, RESET);
    println!();
    println!(
        "  {}{:<16}{} Show auto-accepted and denied commands",
        CYAN, "list", RESET
    );
    println!(
        "  {}{:<16}{} Add a command to the auto-accept list",
        CYAN, "add <cmd>", RESET
    );
    println!(
        "  {}{:<16}{} Remove a command from the auto-accept list",
        CYAN, "rm <cmd>", RESET
    );
    println!(
        "  {}{:<16}{} Always require confirmation for a command",
        CYAN, "deny <cmd>", RESET
    );
    println!(
        "  {}{:<16}{} Remove a command from the deny list",
        CYAN, "undeny <cmd>", RESET
    );
    println!(
        "  {}{:<16}{} Reset auto-accepted commands to defaults",
        CYAN, "reset", RESET
    );
    println!(
        "  {}{:<16}{} Clear the entire deny list",
        CYAN, "resetdeny", RESET
    );
    println!("  {}{:<16}{} Show this help message", CYAN, "help", RESET);
    println!();
    println!(
        "{}Tip:{} Use {}/settings all{} to see all safe commands in detail.",
        GRAY, RESET, BOLD, RESET
    );
    println!();
}

pub fn execute_reset() {
    let mut settings = load_settings();
    let defaults = get_default_safe_commands();
    let count = defaults.len();
    settings.safe_command_prefixes = None;
    save_settings(&settings);
    println!(
        "{}Reset auto-accepted commands to defaults ({} commands).{}",
        GREEN, count, RESET
    );
}

pub fn execute_reset_deny() {
    let mut settings = load_settings();
    if settings.get_denied_commands().is_empty() {
        println!("{}Deny list is already empty.{}", ORANGE, RESET);
        return;
    }
    let count = settings.get_denied_commands().len();
    settings.denied_command_prefixes = None;
    save_settings(&settings);
    println!(
        "{}Cleared deny list (removed {} command{}).{}",
        GREEN,
        count,
        if count == 1 { "" } else { "s" },
        RESET
    );
}
