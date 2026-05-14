use std::{
    error::Error,
    io::{self, Write},
};

use super::diff::{show_edit_diff, show_write_preview};
use super::wrap::MAX_LINE_WIDTH;
use tinyharness_lib::provider::ToolCall;

use crate::style::*;

/// Maximum width available for the command text on the first line,
/// after the `  │ ` prefix (4 chars) and `$ ` (2 chars).
const CMD_FIRST_AVAIL: usize = MAX_LINE_WIDTH - 6;
/// Maximum width available for continuation lines,
/// after the `  │ > ` prefix (6 chars).
const CMD_CONT_AVAIL: usize = MAX_LINE_WIDTH - 6;

/// Display a shell command, splitting it across multiple lines at word
/// boundaries when it exceeds the available terminal width.
///
/// First line: `  │ $ cmd...`
/// Continuation: `  │ > rest...`
fn write_command_lines<W: Write>(stdout: &mut W, cmd: &str) -> Result<(), Box<dyn Error>> {
    let prefix_first = format!("  │ {}{}$ {}", BOLD, TITLE_COLOR, RESET);
    let prefix_cont = format!("  │ {}> {}{}", DIM, TITLE_COLOR, RESET);

    // Split at spaces for word-wrapping
    let mut remaining = cmd;
    let mut first = true;
    while !remaining.is_empty() {
        let (prefix, avail) = if first {
            first = false;
            (prefix_first.as_str(), CMD_FIRST_AVAIL)
        } else {
            (prefix_cont.as_str(), CMD_CONT_AVAIL)
        };

        if remaining.len() <= avail {
            writeln!(stdout, "{}{}{}{}{}", prefix, CYAN, remaining, BOLD, RESET)?;
            break;
        }

        // Find the last space within `avail` characters
        let chunk_end = remaining.floor_char_boundary(avail);
        let chunk = &remaining[..chunk_end];
        let split_at = match chunk.rfind(' ') {
            Some(pos) if pos > 0 => pos,
            _ => {
                // No space found — hard-break at width limit
                writeln!(
                    stdout,
                    "{}{}{}{}{}",
                    prefix,
                    CYAN,
                    &remaining[..chunk_end],
                    BOLD,
                    RESET
                )?;
                remaining = remaining[chunk_end..].trim_start();
                continue;
            }
        };
        writeln!(
            stdout,
            "{}{}{}{}{}",
            prefix,
            CYAN,
            &chunk[..split_at],
            BOLD,
            RESET
        )?;
        remaining = remaining[chunk[..split_at].len()..].trim_start();
    }
    Ok(())
}

/// Result of a tool confirmation prompt.
pub enum Confirmation {
    /// User approved this tool call.
    Yes,
    /// User denied this tool call.
    No,
    /// User approved this and all remaining tool calls in the current loop.
    AutoAccept,
}

/// Display a tool confirmation header and prompt the user.
///
/// Shows a bordered box with the tool name, relevant arguments, and optional
/// diff/preview content, then asks the user to confirm.
pub fn prompt_tool_confirmation<W: Write>(
    stdout: &mut W,
    call: &ToolCall,
) -> Result<Confirmation, Box<dyn Error>> {
    let name = &call.function.name;
    let args = &call.function.arguments;

    // ── Header ──
    writeln!(
        stdout,
        "\n{}  ┌{}─── {}⚠ {}{}{} ───{}",
        BOLD, BOX_COLOR, WARNING_COLOR, name, BOLD, BOX_COLOR, RESET
    )?;

    // ── Arguments (skip large fields already shown in diff/preview) ──
    let skip_keys: &[&str] = match name.as_str() {
        "edit" => &["old_str", "new_str", "content"],
        "write" => &["content"],
        "run" => &["command"],
        _ => &[],
    };

    if let serde_json::Value::Object(map) = args {
        for (key, val) in map {
            if skip_keys.contains(&key.as_str()) {
                continue;
            }
            let val_str = match val {
                serde_json::Value::String(s) => {
                    if s.len() > 100 {
                        format!("{}... ({} chars)", &s[..97], s.len())
                    } else {
                        s.clone()
                    }
                }
                other => other.to_string(),
            };
            writeln!(stdout, "  │ {}{}:{} {}", CYAN, key, RESET, val_str)?;
        }
    }

    // ── Diff / preview for write and edit ──
    if let serde_json::Value::Object(map) = args {
        let path = map.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if !path.trim().is_empty() && name == "edit" {
            let old_str = map.get("old_str").and_then(|v| v.as_str()).unwrap_or("");
            let new_str = map.get("new_str").and_then(|v| v.as_str()).unwrap_or("");
            if !old_str.is_empty() {
                show_edit_diff(stdout, path, old_str, new_str)?;
            }
        }

        // Display the shell command for run (not gated by path — run has no path arg)
        if name == "run" {
            let cmd = map.get("command").and_then(|v| v.as_str()).unwrap_or("");
            if !cmd.is_empty() {
                write_command_lines(stdout, cmd)?;
            }
        }
    }

    // ── Diff / preview for write (shown after the box) ──
    if let serde_json::Value::Object(map) = args {
        let path = map.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if !path.trim().is_empty() && name == "write" {
            let content = map.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if !content.is_empty() {
                show_write_preview(stdout, path, content)?;
            }
        }
    }

    // ── Footer with prompt ──
    writeln!(
        stdout,
        "  └{}───────────────────────────────{}",
        BOX_COLOR, RESET
    )?;
    write!(
        stdout,
        "  {}Allow? {}y{}/n/a{}: {}",
        BOLD, GREEN, BOLD, RESET, RESET
    )?;
    stdout.flush()?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("Failed to read line");
    let input = input.trim().to_lowercase();

    Ok(match input.as_str() {
        "y" | "yes" => Confirmation::Yes,
        "a" | "auto" => Confirmation::AutoAccept,
        _ => Confirmation::No,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Strip ANSI escape sequences from a string for easier assertions.
    fn strip_ansi(s: &str) -> String {
        let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
        re.replace_all(s, "").to_string()
    }

    #[test]
    fn test_short_command_single_line() {
        let mut buf = Vec::new();
        write_command_lines(&mut buf, "ls -la").unwrap();
        let output = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            output.contains("$ ls -la\n"),
            "short command should be on a single line, got:\n{output}"
        );
        assert!(
            !output.contains(">"),
            "short command should not have continuation lines, got:\n{output}"
        );
    }

    #[test]
    fn test_long_command_wraps() {
        // Build a command that exceeds MAX_LINE_WIDTH - 6 chars
        let long_cmd: Vec<String> = (0..50).map(|i| format!("arg{i}")).collect();
        let cmd = long_cmd.join(" ");
        assert!(
            cmd.len() > CMD_FIRST_AVAIL,
            "test command must exceed first-line limit"
        );

        let mut buf = Vec::new();
        write_command_lines(&mut buf, &cmd).unwrap();
        let output = strip_ansi(&String::from_utf8(buf).unwrap());
        let lines: Vec<&str> = output.lines().collect();
        assert!(
            lines.len() > 1,
            "long command should wrap to multiple lines, got:\n{output}"
        );
        assert!(lines[0].contains("$ "), "first line should have $ prompt");
        assert!(
            lines[1].contains("> "),
            "continuation lines should have > prompt"
        );
    }

    #[test]
    fn test_command_no_spaces_hard_breaks() {
        // A very long string with no spaces should hard-break
        let cmd = "a".repeat(CMD_FIRST_AVAIL + 50);
        let mut buf = Vec::new();
        write_command_lines(&mut buf, &cmd).unwrap();
        let output = strip_ansi(&String::from_utf8(buf).unwrap());
        let lines: Vec<&str> = output.lines().collect();
        assert!(
            lines.len() > 1,
            "no-space command should hard-break, got:\n{output}"
        );
    }
}
