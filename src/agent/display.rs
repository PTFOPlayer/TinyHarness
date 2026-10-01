use std::io::Write;

use tinyharness_lib::{
    config::load_settings,
    provider::{Message, Role},
    token::{ContextWindowSize, check_context_warning, format_token_count},
};

use tinyharness_ui::style::*;
use tinyharness_ui::ui::tool_result::{ToolStatus, tool_subject};

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

/// Separate assistant output from user input without coloring the prose.
pub fn write_assistant_header<W: Write>(stdout: &mut W) -> std::io::Result<()> {
    writeln!(
        stdout,
        "\n{RESET}{DIM}──{RESET} {BOLD}{CYAN}Assistant{RESET} {DIM}{}{RESET}\n",
        "─".repeat(28)
    )?;
    stdout.flush()
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

    // tool_call_id → tool name, populated from Assistant messages so Tool
    // result messages can be rendered with the right tool-specific body.
    let mut tool_names: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    // tool_call_id → (tool name, subject), so the result card can name what
    // the call acted on — arguments are not persisted on the Tool message.
    let mut pending_cards: std::collections::HashMap<String, (String, Option<String>)> =
        std::collections::HashMap::new();

    for msg in messages {
        match msg.role {
            Role::System => {}
            Role::User => {
                writeln!(stdout, "{BOLD}{BLUE}You >{RESET} {}", msg.content)?;
            }
            Role::Assistant => {
                if !msg.content.is_empty() || !msg.tool_calls.is_empty() {
                    write_assistant_header(stdout)?;
                }
                if !msg.content.is_empty() {
                    // Render prose with the same markdown styling as live
                    // streaming so a reloaded session looks identical to the
                    // original run.
                    write!(stdout, "{ASSISTANT_TEXT}")?;
                    let rendered = tinyharness_ui::ui::markdown::render_markdown(&msg.content);
                    stdout
                        .write_all(rendered.strip_suffix('\n').unwrap_or(&rendered).as_bytes())?;
                    writeln!(stdout, "{RESET}")?;
                    // Blank line after prose. Tool-only turns skip it: the
                    // assistant header already ends on one, and a doubled gap
                    // would separate the header from its own cards.
                    writeln!(stdout)?;
                }
                if !msg.tool_calls.is_empty() {
                    for tc in &msg.tool_calls {
                        // Remember tool names so the following Tool messages
                        // can render with the correct tool-specific body.
                        if let Some(id) = &tc.id {
                            tool_names.insert(id.clone(), tc.function.name.clone());
                        }
                        // Replay shows the call and its result as one card:
                        // the stored Tool message carries the outcome, so the
                        // header is emitted there rather than as a separate
                        // "▶ name" line.
                        pending_cards.insert(
                            tc.id.clone().unwrap_or_default(),
                            (
                                tc.function.name.clone(),
                                tool_subject(&tc.function.arguments),
                            ),
                        );
                    }
                }
            }
            Role::Tool => {
                let (tool_name, result_body) =
                    split_tool_result(&msg.content, &tool_names, &msg.tool_call_id);
                let subject = msg
                    .tool_call_id
                    .as_deref()
                    .and_then(|id| pending_cards.get(id))
                    .and_then(|(_, subject)| subject.clone());
                let denied = result_body.starts_with("[Tool denied]");
                tinyharness_ui::ui::tool_result::write_tool_card(
                    stdout,
                    &tool_name,
                    subject.as_deref(),
                    if denied {
                        ToolStatus::Denied
                    } else if result_body.starts_with("Error:") {
                        ToolStatus::Error
                    } else {
                        ToolStatus::Ok
                    },
                    None,
                    result_body,
                )?;
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

/// Split a stored tool result into `(tool_name, result_body)`.
///
/// Tool results are persisted as `### {tool}\n\n{result}`. When the tool name
/// can't be recovered from the prefix, falls back to the assistant's
/// `tool_calls` map (id → name) or the message's own `tool_call_id`, so older
/// sessions with different prefixes still resolve the tool name.
fn split_tool_result<'a>(
    content: &'a str,
    tool_names: &std::collections::HashMap<String, String>,
    tool_call_id: &Option<String>,
) -> (String, &'a str) {
    if let Some(rest) = content.strip_prefix("### ")
        && let Some((name, body)) = rest.split_once("\n\n")
    {
        return (name.trim().to_string(), body);
    }
    // Fallbacks: the originating call's name, looked up by id.
    let fallback = tool_call_id
        .as_deref()
        .and_then(|id| tool_names.get(id))
        .cloned()
        .unwrap_or_else(|| "tool".to_string());
    (fallback, content)
}

/// Format tool call arguments as a compact single-line summary.
pub fn format_args_summary(arguments: &serde_json::Value) -> String {
    format_args_summary_impl(arguments, true)
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
    use std::collections::HashMap;

    use super::*;

    /// Strip ANSI escape sequences so assertions can match rendered text.
    fn strip_ansi(s: &str) -> String {
        let re = regex::Regex::new(r"\x1b\[[0-9;]*[mK]").unwrap();
        re.replace_all(s, "").to_string()
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

    #[test]
    fn test_history_labels_separate_user_and_assistant() {
        let messages = vec![
            Message::simple(Role::User, "question"),
            Message::simple(Role::Assistant, "answer"),
        ];
        let mut out = Vec::new();
        print_conversation_history(&messages, &mut out).unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(out.contains(&format!("You >{RESET} question\n")));
        assert!(out.contains("Assistant"));
        assert!(out.find("question").unwrap() < out.find("Assistant").unwrap());
        assert!(out.find("Assistant").unwrap() < out.find("answer").unwrap());
        assert!(out.ends_with(&format!("answer{RESET}\n\n")));
    }

    #[test]
    fn test_history_assistant_message_exact_newlines_without_trailing() {
        // Content WITHOUT a trailing newline: the markdown renderer adds one,
        // and the final writeln! adds the message-terminating newline. The
        // result must be exactly one blank line after the message — never two.
        let msgs = vec![Message::simple(Role::Assistant, "hello world")];
        let mut buf = Vec::new();
        print_conversation_history(&msgs, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let mut header = Vec::new();
        write_assistant_header(&mut header).unwrap();
        let header = String::from_utf8(header).unwrap();
        assert_eq!(
            out,
            format!("{header}{ASSISTANT_TEXT}hello world{RESET}\n\n")
        );
    }

    #[test]
    fn test_history_assistant_message_exact_newlines_with_trailing() {
        // Content WITH a trailing newline must yield the same output as
        // without it — no doubled blank line between history messages.
        let msgs = vec![Message::simple(Role::Assistant, "hello world\n")];
        let mut buf = Vec::new();
        print_conversation_history(&msgs, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let mut header = Vec::new();
        write_assistant_header(&mut header).unwrap();
        let header = String::from_utf8(header).unwrap();
        assert_eq!(
            out,
            format!("{header}{ASSISTANT_TEXT}hello world{RESET}\n\n")
        );
    }

    #[test]
    fn test_history_multi_paragraph_no_doubled_blanks() {
        // Two assistant messages in a row: exactly one blank line separates
        // each message block.
        let msgs = vec![
            Message::simple(Role::Assistant, "first"),
            Message::simple(Role::Assistant, "second"),
        ];
        let mut buf = Vec::new();
        print_conversation_history(&msgs, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        let mut header = Vec::new();
        write_assistant_header(&mut header).unwrap();
        let header = String::from_utf8(header).unwrap();
        assert_eq!(
            out,
            format!(
                "{header}{ASSISTANT_TEXT}first{RESET}\n\n{header}{ASSISTANT_TEXT}second{RESET}\n\n"
            )
        );
    }

    #[test]
    fn test_history_multiline_content_rendered_as_markdown() {
        // Headings get markdown-styled on reload, matching live streaming.
        let msgs = vec![Message::simple(Role::Assistant, "# Title\nbody")];
        let mut buf = Vec::new();
        print_conversation_history(&msgs, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains(BOLD), "heading should be bold: {out:?}");
        assert!(out.contains("body"));
        assert!(out.ends_with(&format!("{RESET}\n\n")));
    }

    #[test]
    fn test_split_tool_result_stored_prefix() {
        let names = HashMap::new();
        let content = "### read\n\n(2 lines)\nfoo\nbar";
        let (name, body) = split_tool_result(content, &names, &None);
        assert_eq!(name, "read");
        assert_eq!(body, "(2 lines)\nfoo\nbar");
    }

    #[test]
    fn test_split_tool_result_falls_back_to_call_id() {
        let mut names = HashMap::new();
        names.insert("call_0".to_string(), "run".to_string());
        let content = "plain output without prefix";
        let (name, body) = split_tool_result(content, &names, &Some("call_0".to_string()));
        assert_eq!(name, "run");
        assert_eq!(body, "plain output without prefix");
    }

    #[test]
    fn test_history_tool_result_uses_call_name_and_caps_body() {
        use tinyharness_lib::provider::{ToolCall, ToolCallFunction};
        let assistant = Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: Some("call_0".to_string()),
                function: ToolCallFunction {
                    name: "read".to_string(),
                    arguments: serde_json::json!({"path": "src/lib.rs"}),
                    thought_signature: None,
                },
            }],
            tool_call_id: None,
            images: vec![],
            thinking: None,
        };
        let mut tool = Message::simple(Role::Tool, "### read\n\n(5 lines)\nl1\nl2\nl3\nl4\nl5");
        tool.tool_call_id = Some("call_0".to_string());
        let msgs = vec![assistant, tool];
        let mut buf = Vec::new();
        print_conversation_history(&msgs, &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("read"), "tool name should render: {out:?}");
        assert!(
            out.contains("src/lib.rs"),
            "the call's subject should carry over from the assistant message: {out:?}"
        );
        assert!(out.contains("(5 lines)"), "meta line should render");
        let plain = strip_ansi(&out);
        assert!(
            plain.contains("╰ +2 more lines"),
            "body should cap at 3: {plain:?}"
        );
        assert!(!plain.contains("l4"), "hidden lines must not render");
    }

    #[test]
    fn test_history_denied_call_renders_as_denied_card() {
        use tinyharness_lib::provider::{ToolCall, ToolCallFunction};
        let assistant = Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: vec![ToolCall {
                id: Some("call_0".to_string()),
                function: ToolCallFunction {
                    name: "run".to_string(),
                    arguments: serde_json::json!({"command": "rm -rf /"}),
                    thought_signature: None,
                },
            }],
            tool_call_id: None,
            images: vec![],
            thinking: None,
        };
        let mut tool = Message::simple(
            Role::Tool,
            "### run\n\n[Tool denied] The user denied the 'run' tool call",
        );
        tool.tool_call_id = Some("call_0".to_string());
        let mut buf = Vec::new();
        print_conversation_history(&[assistant, tool], &mut buf).unwrap();
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains('⊘'),
            "denied calls use the deny glyph: {out:?}"
        );
        assert!(out.contains("rm -rf /"), "subject should render");
        assert!(!out.contains('●'), "a denial is not a success");
    }

    #[test]
    fn test_split_tool_result_unknown_defaults_to_tool() {
        let names = HashMap::new();
        let (name, body) = split_tool_result("hello", &names, &None);
        assert_eq!(name, "tool");
        assert_eq!(body, "hello");
    }
}
