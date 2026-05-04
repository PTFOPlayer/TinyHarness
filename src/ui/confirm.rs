use std::{
    error::Error,
    io::{self, Write},
};

use super::diff::{show_edit_diff, show_write_preview};
use crate::provider::ToolCall;
use crate::style::*;

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
        "\n{}  ┌─── {}⚠ {}{} ───{}",
        BOLD, RED, name, BOLD, RESET
    )?;

    // ── Arguments (skip large fields already shown in diff/preview) ──
    let skip_keys: &[&str] = match name.as_str() {
        "edit" => &["old_str", "new_str", "content"],
        "write" => &["content"],
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
        if !path.trim().is_empty() {
            if name == "edit" {
                let old_str = map.get("old_str").and_then(|v| v.as_str()).unwrap_or("");
                let new_str = map.get("new_str").and_then(|v| v.as_str()).unwrap_or("");
                if !old_str.is_empty() {
                    show_edit_diff(stdout, path, old_str, new_str)?;
                }
            } else if name == "run" {
                let cmd = map.get("command").and_then(|v| v.as_str()).unwrap_or("");
                if !cmd.is_empty() {
                    writeln!(stdout, "  │ {}{}$ {}{}", BOLD, CYAN, cmd, RESET)?;
                }
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
        BOLD, RESET
    )?;
    write!(stdout, "  {}Allow? {}/n/a{}: {}", BOLD, GREEN, RESET, RESET)?;
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
