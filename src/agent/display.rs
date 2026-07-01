use std::io::Write;

use tinyharness_lib::{
    config::load_settings,
    provider::{Message, Role},
    token::{ContextWindowSize, check_context_warning, format_token_count},
};

use tinyharness_ui::style::*;

/// Print a warning if the loaded session's conversation has many messages.
///
/// When token usage from the provider is available, uses that for precise
/// context window threshold checks. Otherwise, only warns on excessive
/// message counts.
pub fn print_context_load_warning<W: Write>(
    messages: &[Message],
    known_tokens: Option<u32>,
    stdout: &mut W,
) -> Result<(), Box<dyn std::error::Error>> {
    if messages.len() <= 1 {
        return Ok(());
    }

    let settings = load_settings();
    let context_size = settings
        .context_limit
        .map(ContextWindowSize::Custom)
        .unwrap_or_else(ContextWindowSize::default_size);

    match known_tokens {
        Some(tokens) => {
            let usage_pct = context_size.usage_percentage(tokens);

            if usage_pct >= 90.0 {
                writeln!(
                    stdout,
                    "\n{}⚠ This session has {} messages ({}{}{}) — exceeds the context window!{}",
                    RED,
                    messages.len(),
                    BOLD,
                    format_token_count(tokens),
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
                    format_token_count(tokens),
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
        }
        None => {
            // No provider token data yet — warn based on message count alone.
            // 200+ messages is likely too many for a default 8K context.
            if messages.len() >= 200 {
                writeln!(
                    stdout,
                    "\n{}⚠ This session has {} messages — may exceed the context window.{}",
                    YELLOW,
                    messages.len(),
                    RESET
                )?;
                writeln!(
                    stdout,
                    "{}  Use {}/compact{} to free context space.{}",
                    YELLOW, BOLD, YELLOW, RESET
                )?;
            }
        }
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
                        writeln!(
                            stdout,
                            "{BG_DIM}  {DIM}▶ {WHITE}{name}{DIM}{FILL_EOL}{RESET}",
                            name = tc.function.name
                        )?;
                    }
                }
                writeln!(stdout)?;
            }
            Role::Tool => {
                let tool_name = msg.content.split('\'').nth(1).unwrap_or("tool");
                let result_body = extract_tool_result_body(&msg.content, tool_name);

                if tool_name == "read" {
                    let summary = result_body.lines().next().unwrap_or("(empty result)");
                    writeln!(
                        stdout,
                        "{BG_DIM}      {DIM}{summary}{FILL_EOL}{RESET}",
                        summary = summary
                    )?;
                } else if matches!(tool_name, "ls" | "grep" | "glob") {
                    let summary = summarize_listing_result(result_body, tool_name);
                    writeln!(
                        stdout,
                        "{BG_DIM}      {DIM}{summary}{FILL_EOL}{RESET}",
                        summary = summary
                    )?;
                } else {
                    tinyharness_ui::ui::wrap::write_wrapped_lines(
                        stdout,
                        result_body,
                        &format!("{BG_DIM}      "),
                        &format!("      {BG_DIM}{DIM}"),
                        tinyharness_ui::ui::wrap::MAX_LINE_WIDTH,
                        true,
                    )?;
                }
                writeln!(stdout, "{RESET}")?;
            }
        }
    }

    stdout.flush()?;
    Ok(())
}

/// Format a compact context status line (pi-style).
///
/// Returns a string like: `5 msgs · 1.2K/8K (15%) · 3 tools · 5.2K total · 2 pinned`
/// Colors are applied based on usage thresholds.
/// When no token usage is available, shows `?/8K` in dim gray.
pub fn format_context_status(
    msg_count: usize,
    pinned_count: usize,
    token_usage: Option<&tinyharness_lib::provider::TokenUsage>,
    context_size: ContextWindowSize,
    tool_call_count: u64,
    total_tokens_used: u64,
) -> String {
    let max_str = format_token_count(context_size.tokens());

    let (used_str, usage_pct, pct_color) = match token_usage {
        Some(usage) => {
            let pct = context_size.usage_percentage(usage.total_tokens);
            let color = if pct >= 90.0 {
                RED
            } else if pct >= 70.0 {
                YELLOW
            } else {
                GRAY
            };
            (format_token_count(usage.total_tokens), Some(pct), color)
        }
        None => ("?".to_string(), None, GRAY),
    };

    let mut parts = vec![format!("{} msgs", msg_count)];
    if let Some(pct) = usage_pct {
        parts.push(format!(
            "{}{}/{}{} ({:.0}%){}",
            pct_color, used_str, max_str, pct_color, pct, RESET
        ));
    } else {
        parts.push(format!("{}{}/{}{}{}", GRAY, used_str, max_str, GRAY, RESET));
    }
    if tool_call_count > 0 {
        parts.push(format!("{}{} tools{}", GRAY, tool_call_count, RESET));
    }
    if total_tokens_used > 0 {
        parts.push(format!(
            "{}{} total{}",
            GRAY,
            format_token_count(total_tokens_used as u32),
            RESET
        ));
    }
    if pinned_count > 0 {
        parts.push(format!("{}{} pinned{}", BLUE, pinned_count, RESET));
    }

    format!(
        "{}{}{}",
        DIM,
        parts.join(&format!(" {}·{} ", DIM, RESET)),
        RESET
    )
}

/// Display a pi-style context status line.
///
/// Shows a compact dim line with message count,
/// token usage from the provider, context percentage, and pinned file count.
/// Also emits a warning if the context window is nearing capacity.
pub fn display_context_status<W: Write>(
    messages: &[Message],
    pinned_count: usize,
    token_usage: Option<&tinyharness_lib::provider::TokenUsage>,
    stdout: &mut W,
    tool_call_count: u64,
    total_tokens_used: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let settings = load_settings();
    let context_size = settings
        .context_limit
        .map(ContextWindowSize::Custom)
        .unwrap_or_else(ContextWindowSize::default_size);

    // Only use provider-reported token count.
    // If the LLM hasn't reported usage yet (first prompt of a new session),
    // we display "?" for the token count rather than making up numbers.
    let status = format_context_status(
        messages.len(),
        pinned_count,
        token_usage,
        context_size,
        tool_call_count,
        total_tokens_used,
    );
    writeln!(stdout, "{}", status)?;

    // Show context warning if we have real token data
    if let Some(usage) = token_usage
        && let Some(warning) = check_context_warning(usage.total_tokens, context_size)
    {
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
    format_args_summary_impl(arguments, true)
}

/// Format tool call arguments for TUI display — shows full commands for `run`
/// and full paths for `edit`/`write`, without truncating important args.
pub fn format_args_summary_tui(tool_name: &str, arguments: &serde_json::Value) -> String {
    // For run/edit/write, show the key argument in full and summarize the rest
    match tool_name {
        "run" => {
            if let Some(cmd) = arguments.get("command").and_then(|v| v.as_str()) {
                let cwd = arguments
                    .get("cwd")
                    .and_then(|v| v.as_str())
                    .map(|c| format!(", cwd={}", c))
                    .unwrap_or_default();
                let timeout = arguments
                    .get("timeout")
                    .and_then(|v| v.as_str())
                    .map(|t| format!(", timeout={}", t))
                    .unwrap_or_default();
                format!("command=\"{}\"{}{}", cmd, cwd, timeout)
            } else {
                format_args_summary_impl(arguments, false)
            }
        }
        "edit" => {
            if let Some(path) = arguments.get("path").and_then(|v| v.as_str()) {
                format!("path=\"{}\"", path)
            } else {
                format_args_summary_impl(arguments, false)
            }
        }
        "write" => {
            if let Some(path) = arguments.get("path").and_then(|v| v.as_str()) {
                let content_len = arguments
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(|c| c.len())
                    .unwrap_or(0);
                format!("path=\"{}\", content=({} bytes)", path, content_len)
            } else {
                format_args_summary_impl(arguments, false)
            }
        }
        "read" => {
            if let Some(path) = arguments.get("path").and_then(|v| v.as_str()) {
                let from_to = arguments
                    .get("from")
                    .and_then(|v| v.as_str())
                    .map(|f| {
                        let to = arguments
                            .get("to")
                            .and_then(|v| v.as_str())
                            .map(|t| format!("-{}", t))
                            .unwrap_or_default();
                        format!(":{}{}", f, to)
                    })
                    .unwrap_or_default();
                format!("path=\"{}{}\"", path, from_to)
            } else {
                format_args_summary_impl(arguments, false)
            }
        }
        "grep" | "glob" => {
            // Show pattern in full, truncate if very long
            if let Some(pattern) = arguments.get("pattern").and_then(|v| v.as_str()) {
                let path = arguments
                    .get("path")
                    .and_then(|v| v.as_str())
                    .map(|p| format!(", path={}", p))
                    .unwrap_or_default();
                format!("pattern=\"{}\"{}", pattern, path)
            } else {
                format_args_summary_impl(arguments, false)
            }
        }
        _ => format_args_summary_impl(arguments, false),
    }
}

/// Internal implementation of argument formatting with optional truncation.
fn format_args_summary_impl(arguments: &serde_json::Value, truncate: bool) -> String {
    match arguments {
        serde_json::Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(key, val)| {
                    let val_str = match val {
                        serde_json::Value::String(s) => {
                            if truncate && s.len() > 60 {
                                let truncate_at = s.floor_char_boundary(57);
                                format!("\"{}...\"", &s[..truncate_at])
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

    #[test]
    fn test_format_args_summary_short_string() {
        let args = serde_json::json!({"path": "/tmp/test.rs", "content": "hello"});
        let result = format_args_summary(&args);
        assert!(result.contains("path="));
        assert!(result.contains("content="));
    }

    #[test]
    fn test_format_args_summary_long_string_truncation() {
        let long_val = "x".repeat(100);
        let args = serde_json::json!({"content": long_val});
        let result = format_args_summary(&args);
        assert!(result.contains("..."));
        // Should be truncated, not 100 chars long in the value
        assert!(result.len() < 120);
    }

    #[test]
    fn test_format_args_summary_multibyte_utf8_safe() {
        // Multi-byte UTF-8 characters should not panic when truncated
        let emoji_val = "🎉".repeat(30); // 30 * 4 bytes = 120 bytes
        let args = serde_json::json!({"content": emoji_val});
        let result = format_args_summary(&args);
        // Should not panic and should contain the truncation marker
        assert!(result.contains("content="));
    }

    #[test]
    fn test_format_context_status_low_usage() {
        // 500 tokens out of 8K = ~6%
        let usage = tinyharness_lib::provider::TokenUsage {
            prompt_tokens: 400,
            completion_tokens: 100,
            total_tokens: 500,
        };
        let result = format_context_status(5, 0, Some(&usage), ContextWindowSize::Small8K, 0, 0);
        // Should contain "5 msgs", token info, and percentage
        assert!(result.contains("5 msgs"));
        assert!(result.contains("500"));
        assert!(result.contains("8.2K")); // 8192 tokens = 8.2K
        assert!(result.contains("6%"));
        // No pinned info when count is 0
        assert!(!result.contains("pinned"));
        // No tools/total when 0
        assert!(!result.contains("tools"));
        assert!(!result.contains("total"));
    }

    #[test]
    fn test_format_context_status_with_pinned() {
        let usage = tinyharness_lib::provider::TokenUsage {
            prompt_tokens: 1500,
            completion_tokens: 500,
            total_tokens: 2000,
        };
        let result = format_context_status(10, 3, Some(&usage), ContextWindowSize::Small8K, 0, 0);
        assert!(result.contains("10 msgs"));
        assert!(result.contains("3 pinned"));
        assert!(result.contains("2.0K/8.2K")); // 2000 = 2.0K, 8192 = 8.2K
    }

    #[test]
    fn test_format_context_status_with_tools_and_total() {
        let usage = tinyharness_lib::provider::TokenUsage {
            prompt_tokens: 1500,
            completion_tokens: 500,
            total_tokens: 2000,
        };
        let result =
            format_context_status(10, 0, Some(&usage), ContextWindowSize::Small8K, 3, 5200);
        assert!(result.contains("3 tools"));
        assert!(result.contains("5.2K total"));
    }

    #[test]
    fn test_format_context_status_high_usage_warning_color() {
        // 90%+ should use RED color code
        let usage = tinyharness_lib::provider::TokenUsage {
            prompt_tokens: 7000,
            completion_tokens: 500,
            total_tokens: 7500,
        };
        let result = format_context_status(20, 0, Some(&usage), ContextWindowSize::Small8K, 0, 0);
        assert!(result.contains("20 msgs"));
        assert!(result.contains(RED));
    }

    #[test]
    fn test_format_context_status_medium_usage_warning_color() {
        // 70-89% should use YELLOW color code
        let usage = tinyharness_lib::provider::TokenUsage {
            prompt_tokens: 5000,
            completion_tokens: 1000,
            total_tokens: 6000,
        };
        let result = format_context_status(10, 0, Some(&usage), ContextWindowSize::Small8K, 0, 0);
        assert!(result.contains(YELLOW));
    }

    #[test]
    fn test_format_context_status_no_usage() {
        // When no token usage is available, show "?"
        let result = format_context_status(5, 0, None, ContextWindowSize::Small8K, 0, 0);
        assert!(result.contains("5 msgs"));
        assert!(result.contains("?/8.2K")); // unknown / 8192
        // Should have dim gray color for the token part
        assert!(result.contains(GRAY));
    }
}
