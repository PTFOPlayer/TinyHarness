use std::collections::HashSet;
use std::io::Write;

use tinyharness_lib::config::{get_default_safe_commands, load_settings, save_settings};
use tinyharness_ui::output::Output;

use crate::commands::registry::CommandResult;
use tinyharness_ui::style::*;

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

pub fn execute(out: &mut Output, args: &str) -> Result<CommandResult, String> {
    let args = args.trim();

    if args.is_empty() {
        execute_list(out);
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
        "add" => execute_add(out, &cmd_arg),
        "rm" | "remove" => execute_remove(out, &cmd_arg),
        "deny" => execute_deny(out, &cmd_arg),
        "undeny" | "allow" => execute_undeny(out, &cmd_arg),
        "list" | "ls" => execute_list(out),
        "reset" => execute_reset(out),
        "resetdeny" => execute_reset_deny(out),
        "help" => execute_help(out),
        _ => execute_list(out),
    }

    Ok(CommandResult::Ok)
}

pub fn execute_add(out: &mut Output, cmd: &str) {
    if cmd.is_empty() {
        let _ = writeln!(
            out,
            "{BOLD}Usage:{RESET} /command add <command> — e.g. /command add docker",
        );
        return;
    }

    let mut settings = load_settings();
    let mut commands = settings.get_safe_commands();

    if commands.contains(&cmd.to_string()) {
        let _ = writeln!(
            out,
            "{ORANGE}Command '{cmd}' is already in the safe list.{RESET}",
        );
        return;
    }

    // Warn if the command is also on the deny list
    let denied = settings.get_denied_commands();
    if denied.contains(&cmd.to_string()) {
        let _ = writeln!(
            out,
            "{YELLOW}Note:{RESET} '{cmd}' is on the deny list. It will still be blocked until removed with {BOLD}/command undeny {cmd}{RESET}",
        );
    }

    commands.push(cmd.to_string());
    settings.safe_command_prefixes = Some(commands);
    save_settings(&settings);
    let _ = writeln!(
        out,
        "{GREEN}Added '{cmd}' to auto-accepted commands.{RESET}",
    );
}

pub fn execute_remove(out: &mut Output, cmd: &str) {
    if cmd.is_empty() {
        let _ = writeln!(
            out,
            "{BOLD}Usage:{RESET} /command rm <command> — e.g. /command rm docker",
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
        let _ = writeln!(
            out,
            "{GREEN}Removed '{cmd}' from auto-accepted commands.{RESET}",
        );
    } else {
        let _ = writeln!(
            out,
            "{ORANGE}Command '{cmd}' not found in auto-accepted list.{RESET}",
        );
    }
}

pub fn execute_deny(out: &mut Output, cmd: &str) {
    if cmd.is_empty() {
        let _ = writeln!(
            out,
            "{BOLD}Usage:{RESET} /command deny <command> — e.g. /command deny git push",
        );
        return;
    }

    let mut settings = load_settings();
    let denied = settings.get_denied_commands();

    if denied.contains(&cmd.to_string()) {
        let _ = writeln!(
            out,
            "{ORANGE}Command '{cmd}' is already in the deny list.{RESET}",
        );
        return;
    }

    // Warn if the command is currently auto-accepted (on safe list)
    let safe_commands = settings.get_safe_commands();
    if matches_safe_prefix(cmd, &safe_commands) {
        let _ = writeln!(
            out,
            "{YELLOW}Note:{RESET} '{cmd}' is currently auto-accepted. Denying will override it — it will always require confirmation.{RESET}",
        );
    } else {
        // Warn if the command isn't on the safe list anyway
        let _ = writeln!(
            out,
            "{GRAY}Note:{RESET} '{cmd}' is not currently auto-accepted, so denying it has no practical effect.{RESET}",
        );
    }

    let mut denied = denied; // move out of &settings
    denied.push(cmd.to_string());
    settings.denied_command_prefixes = Some(denied);
    save_settings(&settings);
    let _ = writeln!(
        out,
        "{RED}Denied '{cmd}' — it will always require confirmation.{RESET}",
    );
}

pub fn execute_undeny(out: &mut Output, cmd: &str) {
    if cmd.is_empty() {
        let _ = writeln!(
            out,
            "{BOLD}Usage:{RESET} /command undeny <command> — e.g. /command undeny git push",
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
        let _ = writeln!(out, "{GREEN}Removed '{cmd}' from the deny list.{RESET}",);
    } else {
        let _ = writeln!(
            out,
            "{ORANGE}Command '{cmd}' not found in the deny list.{RESET}",
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

pub fn execute_list(out: &mut Output) {
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

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{BOLD}╭─ Auto-Accepted Commands ─────────────────────╮{RESET}",
    );
    if using_defaults {
        let _ = writeln!(
            out,
            "{BOLD}│{RESET} {GRAY}Using defaults{RESET} ({}) commands",
            safe_commands.len(),
        );
    } else {
        let _ = writeln!(
            out,
            "{BOLD}│{RESET} {BLUE}{}{RESET} configured ({custom_count} custom)",
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
    let cmd_refs: Vec<&str> = cmds.to_vec();

    // Format and print rows
    let rows = format_command_rows(&cmd_refs, &markers);
    for row in &rows {
        let _ = writeln!(out, "{BOLD}│{RESET}   {row}");
    }

    // Legend
    if !using_defaults {
        let _ = writeln!(
            out,
            "{BOLD}│{RESET}   {GRAY}· default{RESET}  {GREEN}+ custom{RESET}",
        );
    }

    let _ = writeln!(
        out,
        "{BOLD}╰──────────────────────────────────────────────╯{RESET}",
    );

    if !denied.is_empty() {
        let denied_refs: Vec<&str> = denied.iter().map(|s| s.as_str()).collect();
        let denied_rows = format_denied_command_rows(&denied_refs);

        let _ = writeln!(out);
        let _ = writeln!(
            out,
            "{BOLD}╭─ Always-Deny Commands ──────────────────────╮{RESET}",
        );
        let _ = writeln!(
            out,
            "{BOLD}│{RESET} {RED}{}{RESET} commands denied (always require confirmation)",
            denied.len(),
        );
        for row in &denied_rows {
            let _ = writeln!(out, "{BOLD}│{RESET}   {row}");
        }
        let _ = writeln!(
            out,
            "{BOLD}╰──────────────────────────────────────────────╯{RESET}",
        );
    }

    let _ = writeln!(out);
}

pub fn execute_help(out: &mut Output) {
    let _ = writeln!(out);
    let _ = writeln!(out, "{BOLD}Command management — subcommands:{RESET}",);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "  {CYAN}{0:<16}{RESET} Show auto-accepted and denied commands",
        "list",
    );
    let _ = writeln!(
        out,
        "  {CYAN}{0:<16}{RESET} Add a command to the auto-accept list",
        "add <cmd>",
    );
    let _ = writeln!(
        out,
        "  {CYAN}{0:<16}{RESET} Remove a command from the auto-accept list",
        "rm <cmd>",
    );
    let _ = writeln!(
        out,
        "  {CYAN}{0:<16}{RESET} Always require confirmation for a command",
        "deny <cmd>",
    );
    let _ = writeln!(
        out,
        "  {CYAN}{0:<16}{RESET} Remove a command from the deny list",
        "undeny <cmd>",
    );
    let _ = writeln!(
        out,
        "  {CYAN}{0:<16}{RESET} Reset auto-accepted commands to defaults",
        "reset",
    );
    let _ = writeln!(
        out,
        "  {CYAN}{0:<16}{RESET} Clear the entire deny list",
        "resetdeny",
    );
    let _ = writeln!(out, "  {CYAN}{0:<16}{RESET} Show this help message", "help",);
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{GRAY}Tip:{RESET} Use {BOLD}/settings all{RESET} to see all safe commands in detail.",
    );
    let _ = writeln!(out);
}

pub fn execute_reset(out: &mut Output) {
    let mut settings = load_settings();
    let defaults = get_default_safe_commands();
    let count = defaults.len();
    settings.safe_command_prefixes = None;
    save_settings(&settings);
    let _ = writeln!(
        out,
        "{GREEN}Reset auto-accepted commands to defaults ({count} commands).{RESET}",
    );
}

pub fn execute_reset_deny(out: &mut Output) {
    let mut settings = load_settings();
    if settings.get_denied_commands().is_empty() {
        let _ = writeln!(out, "{ORANGE}Deny list is already empty.{RESET}");
        return;
    }
    let count = settings.get_denied_commands().len();
    settings.denied_command_prefixes = None;
    save_settings(&settings);
    let _ = writeln!(
        out,
        "{GREEN}Cleared deny list (removed {count} command{}).{RESET}",
        if count == 1 { "" } else { "s" },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{captured_output, captured_string, strip_ansi};

    #[test]
    fn format_command_rows_basic() {
        let cmds = vec!["ls", "cat", "pwd"];
        let markers = vec!['·', '·', '·'];
        let rows = format_command_rows(&cmds, &markers);
        assert_eq!(rows.len(), 1);
        // Each row should contain all three commands (after ANSI strip)
        let plain = strip_ansi(&rows[0]);
        assert!(plain.contains("ls"));
        assert!(plain.contains("cat"));
        assert!(plain.contains("pwd"));
    }

    #[test]
    fn format_command_rows_wraps_to_multiple_rows() {
        let cmds: Vec<&str> = (0..7)
            .map(|i| ["cmd1", "cmd2", "cmd3", "cmd4", "cmd5", "cmd6", "cmd7"][i])
            .collect();
        let markers: Vec<char> = (0..7).map(|_| '·').collect();
        let rows = format_command_rows(&cmds, &markers);
        // 7 commands / 3 per row = 3 rows (3, 3, 1)
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn format_denied_command_rows_basic() {
        let cmds = vec!["git push", "rm"];
        let rows = format_denied_command_rows(&cmds);
        assert_eq!(rows.len(), 1);
        let plain = strip_ansi(&rows[0]);
        assert!(plain.contains("git push"));
        assert!(plain.contains("rm"));
    }

    #[test]
    fn matches_safe_prefix_exact_match() {
        let safe = vec!["ls".to_string(), "cat".to_string()];
        assert!(matches_safe_prefix("ls", &safe));
        assert!(matches_safe_prefix("ls -la", &safe));
        assert!(matches_safe_prefix("cat file.txt", &safe));
    }

    #[test]
    fn matches_safe_prefix_word_boundary() {
        let safe = vec!["ls".to_string()];
        assert!(!matches_safe_prefix("lsx", &safe));
        assert!(!matches_safe_prefix("lsls", &safe));
    }

    #[test]
    fn matches_safe_prefix_empty_safe_list() {
        let safe: Vec<String> = vec![];
        assert!(!matches_safe_prefix("ls", &safe));
    }

    #[test]
    fn execute_list_shows_commands() {
        let (mut out, buf) = captured_output();
        execute_list(&mut out);
        let plain = strip_ansi(&captured_string(&buf));
        // Should contain the box header
        assert!(plain.contains("Auto-Accepted Commands"));
        // Should list some default commands
        assert!(plain.contains("ls"));
    }

    #[test]
    fn execute_help_shows_subcommands() {
        let (mut out, buf) = captured_output();
        execute_help(&mut out);
        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Command management"));
        assert!(plain.contains("add"));
        assert!(plain.contains("deny"));
        assert!(plain.contains("undeny"));
        assert!(plain.contains("reset"));
    }
}
