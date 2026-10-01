//! The approval prompt: the one place the CLI interrupts the user.
//!
//! Rendered in the same visual language as the tool cards (see
//! [`super::frame`]): a filled band that states what is being asked and why,
//! the arguments/preview underneath, then a single-line choice list. The band
//! is deliberately *lighter* than a card band ([`BG_PROMPT`] vs [`BG_BAND`])
//! so a pending question never reads as a completed action.

use std::{
    error::Error,
    io::{self, Write},
};

use super::diff::{show_edit_diff, show_write_preview};
use super::frame::{
    GUTTER, GUTTER_WIDTH, gutter_line, title_case, truncate_ellipsis, write_band_on,
};
use super::wrap::{MAX_LINE_WIDTH, wrap_line};
use tinyharness_lib::provider::ToolCall;

use crate::style::*;

/// What the prompt is asking the user to bless, phrased as a sentence.
///
/// Naming the *effect* (not just the tool identifier) is what makes an
/// approval prompt useful: `run` means nothing on its own, "execute a shell
/// command" tells you what could happen.
fn describe(name: &str) -> &'static str {
    match name {
        "run" => "execute a shell command",
        "write" => "create or overwrite a file",
        "edit" => "modify a file in place",
        "read" => "read a file",
        "ls" => "list a directory",
        "grep" => "search file contents",
        "glob" => "match files by pattern",
        "web_search" => "search the web",
        "web_fetch" => "fetch a web page",
        "screenshot" => "capture a screenshot",
        _ => "",
    }
}

/// Columns left for command text after the gutter and `$ ` prefix.
const CMD_PREFIX_WIDTH: usize = GUTTER_WIDTH + 2;

/// Display a shell command, splitting it across multiple lines at word
/// boundaries when it exceeds the available terminal width.
///
/// Drawn inside the frame's gutter with a `$` prompt prefix, so a wrapped
/// multi-line command still reads as one quoted statement. Rows stay open
/// ([`GUTTER`], never an elbow): the prompt's closing `▲` line ends the
/// block, and nested diff cards close themselves.
fn write_command_lines<W: Write>(stdout: &mut W, cmd: &str) -> Result<(), Box<dyn Error>> {
    let avail = MAX_LINE_WIDTH - CMD_PREFIX_WIDTH;
    for (i, row) in wrap_line(cmd, avail, avail).into_iter().enumerate() {
        let prefix = if i == 0 { "$ " } else { "> " };
        gutter_line(
            stdout,
            GUTTER,
            &format!("{FG_FAINT}{prefix}{RESET}{FG_CMD}{row}{RESET}", row = row),
        )?;
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

/// Display a tool confirmation prompt and ask the user to confirm.
///
/// Shows a band naming the tool and its target, the remaining arguments and
/// any diff/command preview, then a one-line choice list.
pub fn prompt_tool_confirmation<W: Write>(
    stdout: &mut W,
    call: &ToolCall,
) -> Result<Confirmation, Box<dyn Error>> {
    let name = &call.function.name;
    let args = &call.function.arguments;

    // ── Title band ──
    // Glyph + question + tool + effect, so the reason for the interruption is
    // on the same line as the thing being interrupted.
    let effect = describe(name);
    let mut segments: Vec<(&str, &str)> = vec![
        ("▲", FG_WARN),
        (" ", FG_FAINT),
        ("Allow?", FG_WARN),
        (" · ", FG_FAINT),
    ];
    // Known tools get their effect spelled out; custom tools fall back to a
    // humanized form of their identifier rather than the raw wire name.
    let label = title_case(name);
    if effect.is_empty() {
        segments.push((label.as_str(), FG_BAND));
    } else {
        segments.push((name.as_str(), FG_BAND));
        segments.push((" — ", FG_FAINT));
        segments.push((effect, FG_MUTED));
    }
    write_band_on(stdout, BG_PROMPT, &segments)?;

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
                serde_json::Value::String(s) => truncate_ellipsis(
                    s.lines().next().unwrap_or_default(),
                    MAX_LINE_WIDTH - GUTTER_WIDTH - key.len() - 2,
                ),
                other => other.to_string(),
            };
            gutter_line(
                stdout,
                GUTTER,
                &format!(
                    "{FG_MUTED}{key}:{RESET} {FG_ACCENT}{val}{RESET}",
                    key = key,
                    val = val_str
                ),
            )?;
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

    // ── Diff / preview for write (shown after the band) ──
    if let serde_json::Value::Object(map) = args {
        let path = map.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if !path.trim().is_empty() && name == "write" {
            let content = map.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if !content.is_empty() {
                show_write_preview(stdout, path, content)?;
            }
        }
    }

    // ── Choice list ──
    // Part of the same frame as the band above: the question was asked there,
    // the details are between, so this line only offers the answers. Brackets
    // mark the accepted keystroke, and every option is spelled out so the
    // user never has to remember what `a` meant three prompts ago.
    write!(
        stdout,
        "  {FG_WARN}▲{RESET} {BOLD}[y]{RESET}{FG_FAINT}es{RESET}  {BOLD}[n]{RESET}{FG_FAINT}o{RESET}  \
         {BOLD}[a]{RESET}{FG_FAINT}uto-accept the rest of this turn{RESET} {FG_FAINT}›{RESET} "
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
        // Match SGR sequences (\x1b[...m) and other CSI sequences like \x1b[K (clear to EOL)
        let re = regex::Regex::new(r"\x1b\[[0-9;]*[mK]").unwrap();
        re.replace_all(s, "").to_string()
    }

    // ── command rendering ──

    #[test]
    fn test_short_command_single_line() {
        let mut buf = Vec::new();
        write_command_lines(&mut buf, "ls -la").unwrap();
        let output = strip_ansi(&String::from_utf8(buf).unwrap());
        assert_eq!(output, "  │ $ ls -la\n", "rows stay open until the ▲ line");
    }

    #[test]
    fn test_long_command_wraps() {
        // Build a command that exceeds the line budget
        let long_cmd: Vec<String> = (0..60).map(|i| format!("argument_number_{i}")).collect();
        let cmd = long_cmd.join(" ");

        let mut buf = Vec::new();
        write_command_lines(&mut buf, &cmd).unwrap();
        let output = strip_ansi(&String::from_utf8(buf).unwrap());
        let lines: Vec<&str> = output.lines().collect();
        assert!(lines.len() > 1, "long command should wrap: {output}");
        assert!(lines[0].starts_with("  │ $ "), "first line has $ prompt");
        assert!(
            lines[1..].iter().all(|l| l.starts_with("  │ > ")),
            "continuation lines use > and stay in the gutter"
        );
        for line in &lines {
            assert!(line.chars().count() <= MAX_LINE_WIDTH, "overflow: {line}");
        }
    }

    #[test]
    fn test_command_no_spaces_hard_breaks() {
        // A very long string with no spaces should hard-break
        let cmd = "a".repeat(MAX_LINE_WIDTH);
        let mut buf = Vec::new();
        write_command_lines(&mut buf, &cmd).unwrap();
        let output = strip_ansi(&String::from_utf8(buf).unwrap());
        assert!(
            output.lines().count() > 1,
            "no-space command should hard-break"
        );
    }

    // ── description ──

    #[test]
    fn known_tools_describe_their_effect() {
        assert_eq!(describe("run"), "execute a shell command");
        assert_eq!(describe("write"), "create or overwrite a file");
        // Unknown/custom tools get no invented description.
        assert!(describe("my_tool").is_empty());
    }
}
