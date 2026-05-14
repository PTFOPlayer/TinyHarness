use std::io::Write;

use tinyharness_lib::{
    config::load_settings,
    provider::{Message, Role},
    token::{
        ContextWindowSize, check_context_warning, estimate_conversation_tokens, estimate_tokens,
        format_token_count,
    },
};

use crate::style::*;

/// Print a warning if the loaded session's conversation is near or exceeding
/// the context window limit.
pub fn print_context_load_warning<W: Write>(
    messages: &[Message],
    stdout: &mut W,
) -> Result<(), Box<dyn std::error::Error>> {
    if messages.len() <= 1 {
        return Ok(());
    }

    let estimated_tokens = estimate_conversation_tokens(messages);
    let settings = load_settings();
    let context_size = settings
        .context_limit
        .map(ContextWindowSize::Custom)
        .unwrap_or_else(ContextWindowSize::default_size);
    let usage_pct = context_size.usage_percentage(estimated_tokens);

    if usage_pct >= 90.0 {
        writeln!(
            stdout,
            "\n{}⚠ This session has {} messages ({}{}{}) — exceeds the context window!{}",
            RED,
            messages.len(),
            BOLD,
            format_token_count(estimated_tokens),
            RED,
            RESET
        )?;
        writeln!(
            stdout,
            "{}  The conversation may not work properly until you compact it.{}",
            RED, RESET
        )?;
        writeln!(
            stdout,
            "{}  Use {}/compact{} [focus] to summarize older messages.{}",
            RED, BOLD, RED, RESET
        )?;
    } else if usage_pct >= 70.0 {
        writeln!(
            stdout,
            "\n{}⚠ This session has {} messages ({}{}{}, {:.1}% of context).{}",
            YELLOW,
            messages.len(),
            BOLD,
            format_token_count(estimated_tokens),
            YELLOW,
            usage_pct,
            RESET
        )?;
        writeln!(
            stdout,
            "{}  Consider using {}/compact{} to free context space.{}",
            YELLOW, BOLD, YELLOW, RESET
        )?;
    }

    stdout.flush()?;
    Ok(())
}

/// Print the conversation history from loaded messages so the user can see
/// what was discussed in the resumed session.
pub fn print_conversation_history<W: Write>(
    messages: &[Message],
    stdout: &mut W,
) -> Result<(), Box<dyn std::error::Error>> {
    if messages.is_empty() {
        return Ok(());
    }

    for msg in messages {
        match msg.role {
            Role::System => {}
            Role::User => {
                writeln!(stdout, "{}> {}{}", BLUE, msg.content, RESET)?;
            }
            Role::Assistant => {
                if !msg.content.is_empty() {
                    write!(stdout, "{}", ORANGE)?;
                    stdout.write_all(msg.content.as_bytes())?;
                    writeln!(stdout, "{}", RESET)?;
                }
                if !msg.tool_calls.is_empty() {
                    for tc in &msg.tool_calls {
                        writeln!(stdout, "{}  {}▶{} {}", DIM, CYAN, RESET, tc.function.name)?;
                    }
                }
                writeln!(stdout)?;
            }
            Role::Tool => {
                let tool_name = msg.content.split('\'').nth(1).unwrap_or("tool");
                let result_body = extract_tool_result_body(&msg.content, tool_name);

                if tool_name == "read" {
                    let summary = result_body.lines().next().unwrap_or("(empty result)");
                    writeln!(stdout, "    {}", summary)?;
                } else if matches!(tool_name, "ls" | "grep" | "glob") {
                    let summary = summarize_listing_result(result_body, tool_name);
                    writeln!(stdout, "    {}", summary)?;
                } else {
                    crate::ui::wrap::write_wrapped_lines(
                        stdout,
                        result_body,
                        "    ",
                        "      ",
                        crate::ui::wrap::MAX_LINE_WIDTH,
                    )?;
                }
            }
        }
    }

    stdout.flush()?;
    Ok(())
}

/// Display token usage after a turn is complete.
pub fn display_token_usage<W: Write>(
    messages: &[Message],
    token_usage: Option<&tinyharness_lib::provider::TokenUsage>,
    stdout: &mut W,
) -> Result<(), Box<dyn std::error::Error>> {
    let estimated_total = estimate_conversation_tokens(messages);

    let settings = load_settings();
    let context_size = settings
        .context_limit
        .map(ContextWindowSize::Custom)
        .unwrap_or_else(ContextWindowSize::default_size);
    let usage_pct = context_size.usage_percentage(estimated_total);

    let is_estimated = token_usage.is_none();
    let (prompt_tokens, completion_tokens, total_tokens) = if let Some(usage) = token_usage {
        (
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.total_tokens,
        )
    } else {
        let prompt_est =
            estimate_conversation_tokens(&messages[..messages.len().saturating_sub(1)]);
        let last_msg = messages.last().map(|m| m.content.as_str()).unwrap_or("");
        let completion_est = estimate_tokens(last_msg);
        (prompt_est, completion_est, prompt_est + completion_est)
    };

    let estimation_marker = if is_estimated { " (estimated)" } else { "" };

    writeln!(
        stdout,
        "\n{}{}Tokens{}{}:{} prompt={}, completion={}, total={} | {}{}{} of context ({:.1}%){}",
        DIM,
        BOLD,
        if is_estimated { YELLOW } else { RESET },
        estimation_marker,
        RESET,
        prompt_tokens,
        completion_tokens,
        total_tokens,
        if usage_pct >= 90.0 {
            RED
        } else if usage_pct >= 70.0 {
            YELLOW
        } else {
            GRAY
        },
        format_token_count(estimated_total),
        RESET,
        usage_pct,
        RESET
    )?;

    // Show context warning if needed
    if let Some(warning) = check_context_warning(estimated_total, context_size) {
        let (icon, color) = if warning.is_critical() {
            ("⚠", RED)
        } else {
            ("⚠", YELLOW)
        };
        writeln!(
            stdout,
            "{}{}{} Context window {:.1}% full. Consider using {}/compact{} to free space.{}",
            icon,
            color,
            BOLD,
            warning.percentage(),
            BLUE,
            color,
            RESET
        )?;
    }

    stdout.write_all("\n".as_bytes())?;
    stdout.flush()?;
    Ok(())
}

/// Extract the result body from a tool result message.
fn extract_tool_result_body<'a>(content: &'a str, tool_name: &str) -> &'a str {
    let prefix = format!("Tool '{tool_name}' result:\n");
    content
        .strip_prefix(&prefix)
        .or_else(|| content.strip_prefix(&prefix))
        .unwrap_or(content)
}

/// Produce a one-line summary for listing tools (ls, grep, glob).
pub fn summarize_listing_result(result: &str, tool_name: &str) -> String {
    if result.starts_with("Error:") || result.starts_with("No ") || result == "Directory is empty" {
        return result.to_string();
    }

    let lines: Vec<&str> = result.lines().collect();
    let count = lines.len();
    let label = match tool_name {
        "ls" => "entries",
        "grep" => "matches",
        "glob" => "files",
        _ => "results",
    };

    const PREVIEW: usize = 3;
    if count <= PREVIEW {
        format!("{} {} — {}", count, label, result)
    } else {
        let preview: Vec<&str> = lines.iter().take(PREVIEW).copied().collect();
        format!(
            "{} {} — {} ... ({} more)",
            count,
            label,
            preview.join(", "),
            count - PREVIEW
        )
    }
}

/// Format tool call arguments as a compact single-line summary.
pub fn format_args_summary(arguments: &serde_json::Value) -> String {
    match arguments {
        serde_json::Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(key, val)| {
                    let val_str = match val {
                        serde_json::Value::String(s) => {
                            if s.len() > 60 {
                                format!("\"{}...\"", &s[..57])
                            } else {
                                format!("\"{}\"", s)
                            }
                        }
                        other => other.to_string(),
                    };
                    format!("{}={}", key, val_str)
                })
                .collect();
            parts.join(", ")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_ls_error_passthrough() {
        let result = summarize_listing_result("Error: Path '/nope' does not exist", "ls");
        assert_eq!(result, "Error: Path '/nope' does not exist");
    }

    #[test]
    fn test_summarize_ls_empty_dir() {
        let result = summarize_listing_result("Directory is empty", "ls");
        assert_eq!(result, "Directory is empty");
    }

    #[test]
    fn test_summarize_ls_no_matches() {
        let result = summarize_listing_result("No matches found for pattern 'xyz'", "grep");
        assert_eq!(result, "No matches found for pattern 'xyz'");
    }

    #[test]
    fn test_summarize_ls_few_entries() {
        let result = summarize_listing_result("Cargo.toml\nCargo.lock\nsrc", "ls");
        assert_eq!(result, "3 entries — Cargo.toml\nCargo.lock\nsrc");
    }

    #[test]
    fn test_summarize_ls_many_entries() {
        let entries: Vec<String> = (0..10).map(|i| format!("file{i}")).collect();
        let input = entries.join("\n");
        let result = summarize_listing_result(&input, "ls");
        assert_eq!(result, "10 entries — file0, file1, file2 ... (7 more)");
    }

    #[test]
    fn test_summarize_grep_many_matches() {
        let matches: Vec<String> = (1..=5).map(|i| format!("src/main.rs:{}:foo", i)).collect();
        let input = matches.join("\n");
        let result = summarize_listing_result(&input, "grep");
        assert_eq!(
            result,
            "5 matches — src/main.rs:1:foo, src/main.rs:2:foo, src/main.rs:3:foo ... (2 more)"
        );
    }

    #[test]
    fn test_summarize_glob_no_files() {
        let result = summarize_listing_result("No files found matching pattern '*.xyz'", "glob");
        assert_eq!(result, "No files found matching pattern '*.xyz'");
    }

    #[test]
    fn test_summarize_glob_many_files() {
        let files: Vec<String> = (0..6).map(|i| format!("src/file{i}.rs")).collect();
        let input = files.join("\n");
        let result = summarize_listing_result(&input, "glob");
        assert_eq!(
            result,
            "6 files — src/file0.rs, src/file1.rs, src/file2.rs ... (3 more)"
        );
    }

    #[test]
    fn test_summarize_single_entry() {
        let result = summarize_listing_result("Cargo.toml", "ls");
        assert_eq!(result, "1 entries — Cargo.toml");
    }
}
