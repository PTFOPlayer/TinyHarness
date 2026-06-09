use std::io::Write;

use tinyharness_lib::provider::{Message, Provider, Role, TokenUsage};
use tinyharness_ui::output::Output;

use crate::async_command;
use crate::commands::registry::CommandResult;
use tinyharness_ui::style::*;

// ── Command trait implementation ────────────────────────────────────────────

async_command!(
    CompactCommand,
    "/compact",
    "Summarize conversation history to free context space. Optionally specify a focus area.",
    "/compact [focus]",
    |raw_arg, ctx, messages| {
        let focus = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();
        async move {
            let mut p = provider.lock().await;
            match execute_compact(&mut ctx.output, &mut *p, messages, &focus).await {
                Ok(tokens) => {
                    ctx.compaction_token_usage = tokens;
                    Ok(CommandResult::Ok)
                }
                Err(e) => Err(e),
            }
        }
    }
);

// ── Core implementation ─────────────────────────────────────────────────────

/// Maximum characters per message when formatting for summarization.
const MAX_CHARS_PER_MESSAGE: usize = 2000;

/// Minimum number of messages per chunk (don't split finer than this).
const MIN_MESSAGES_PER_CHUNK: usize = 10;

/// Format a slice of messages into the text representation used for summarization.
///
/// Each message is prefixed with its role and truncated to `MAX_CHARS_PER_MESSAGE`
/// characters if it's very long.
fn format_messages_for_summary(messages: &[&Message]) -> String {
    let mut text = String::new();
    for msg in messages {
        let role_str = match msg.role {
            Role::System => "SYSTEM",
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::Tool => "TOOL",
        };
        let content = if msg.content.len() > MAX_CHARS_PER_MESSAGE {
            let truncate_at = msg.content.floor_char_boundary(MAX_CHARS_PER_MESSAGE);
            format!(
                "{}... [truncated, {} chars total]",
                &msg.content[..truncate_at],
                msg.content.len()
            )
        } else {
            msg.content.clone()
        };
        text.push_str(&format!("[{}]: {}\n\n", role_str, content));
    }
    text
}

/// Build the focus instruction suffix for the summarization prompt.
fn focus_instruction(focus: &str) -> String {
    if focus.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nPay special attention to preserving information about: {}",
            focus
        )
    }
}

/// Call the LLM to summarize a text, returning the generated summary
/// and the token usage reported by the provider.
///
/// Uses a dedicated system prompt for summarization and streams the response.
async fn call_llm_summarize(
    provider: &mut dyn Provider,
    text_to_summarize: &str,
    focus: &str,
    is_merge: bool,
) -> Result<(String, Option<TokenUsage>), String> {
    let summarization_prompt = if is_merge {
        format!(
            "Merge the following conversation summaries into a single coherent summary. \
             Preserve all important decisions, code changes, file paths, error messages, \
             and technical details from each stage. Remove redundancy between stages. \
             Focus on facts: what was discussed, what was decided, what was changed.{}",
            focus_instruction(focus),
        )
    } else {
        format!(
            "Summarize the following conversation history concisely. \
             Preserve all important decisions, code changes, file paths, error messages, \
             and technical details. Omit pleasantries and redundant information. \
             Focus on facts: what was discussed, what was decided, what was changed.{}",
            focus_instruction(focus),
        )
    };

    let summarization_messages = vec![
        Message {
            role: Role::System,
            content: "You are a helpful assistant that creates concise, accurate summaries of conversations. \
                       Preserve all technical details: file paths, code snippets, error messages, decisions made, \
                       and current task status. Do NOT add information that was not in the original conversation."
                .to_string(),
            tool_calls: vec![], images: vec![],
        },
        Message {
            role: Role::User,
            content: format!("{}\n\n{}", summarization_prompt, text_to_summarize),
            tool_calls: vec![], images: vec![],
        },
    ];

    let tools = vec![];
    let mut recv = provider.chat(summarization_messages, tools).await?;

    let mut summary_content = String::new();
    let mut done = false;
    let mut token_usage: Option<TokenUsage> = None;
    while let Some(msg) = recv.recv().await {
        if !msg.message.content.is_empty() {
            summary_content.push_str(&msg.message.content);
        }
        if msg.usage.is_some() {
            token_usage = msg.usage;
        }
        if msg.done {
            done = true;
            break;
        }
    }

    if !done || summary_content.is_empty() {
        return Err(
            "Failed to generate conversation summary. The conversation was not modified."
                .to_string(),
        );
    }

    Ok((summary_content, token_usage))
}

/// Determine how many messages fit within a chunk for cascading compaction.
///
/// Uses a fixed message count approach: splits into chunks of up to 200 messages
/// each. For very small budgets, still ensures at least MIN_MESSAGES_PER_CHUNK.
fn messages_per_chunk(messages_len: usize) -> usize {
    // When the conversation has 500+ messages to summarize, use 200-message chunks.
    // For shorter conversations, just use half the messages.
    let chunk_size = if messages_len >= 500 {
        200
    } else {
        messages_len / 2
    };

    chunk_size.max(MIN_MESSAGES_PER_CHUNK).min(messages_len)
}

/// Context needed for the message reconstruction step after compaction.
struct CompactContext {
    system_msg: Option<Message>,
    keep_from: usize,
    original_count: usize,
}

/// Compact the conversation history by summarizing older messages.
///
/// For short conversations, this uses a single LLM call (single-pass compaction).
/// For long conversations that would exceed the context window, it uses cascading
/// multi-stage compaction: the intermediate messages are split into chunks, each
/// chunk is summarized separately, and then the chunk summaries are merged into
/// a final summary.
///
/// This keeps the system prompt and the most recent messages intact,
/// while replacing all intermediate messages with a single summary message.
pub async fn execute_compact(
    out: &mut Output,
    provider: &mut dyn Provider,
    messages: &mut Vec<Message>,
    focus: &str,
) -> Result<Option<TokenUsage>, String> {
    if messages.len() <= 6 {
        let _ = writeln!(
            out,
            "{ORANGE}Not enough messages to compact (only {} messages).{RESET}",
            messages.len(),
        );
        return Ok(None);
    }

    let original_count = messages.len();

    // Keep the system prompt (always the first message)
    let system_msg = messages.first().cloned();

    // Keep the last 4 messages intact (recent context).
    const KEEP_RECENT: usize = 4;
    let keep_from = messages.len().saturating_sub(KEEP_RECENT).max(1);

    // Messages to summarize: everything between system prompt and keep_from.
    // We clone these so we can release the borrow on `messages` before
    // passing it mutably to the compaction functions.
    let to_summarize: Vec<Message> = messages[1..keep_from].to_vec();

    if to_summarize.is_empty() {
        let _ = writeln!(
            out,
            "{ORANGE}Nothing to compact — recent messages are the only ones present.{RESET}",
        );
        return Ok(None);
    }

    let ctx = CompactContext {
        system_msg,
        keep_from,
        original_count,
    };

    // Decide: single-pass or cascade based on message count.
    // For up to 200 intermediate messages, use a single summarization pass.
    // Beyond that, use cascading multi-stage compaction.
    const SINGLE_PASS_LIMIT: usize = 200;
    if to_summarize.len() <= SINGLE_PASS_LIMIT {
        compact_single_pass(out, provider, &to_summarize, messages, &ctx, focus).await
    } else {
        compact_cascade(out, provider, &to_summarize, messages, &ctx, focus).await
    }
}

/// Single-pass compaction: summarize all intermediate messages in one LLM call.
async fn compact_single_pass(
    out: &mut Output,
    provider: &mut dyn Provider,
    to_summarize: &[Message],
    messages: &mut Vec<Message>,
    ctx: &CompactContext,
    focus: &str,
) -> Result<Option<TokenUsage>, String> {
    let refs: Vec<&Message> = to_summarize.iter().collect();
    let summary_text = format_messages_for_summary(&refs);

    let _ = writeln!(
        out,
        "{BOLD}Compacting {} messages...{RESET}",
        to_summarize.len(),
    );

    let (summary_content, token_usage) =
        call_llm_summarize(provider, &summary_text, focus, false).await?;
    reconstruct_messages(out, messages, ctx, &summary_content);
    Ok(token_usage)
}

/// Multi-stage cascade compaction: split intermediate messages into chunks,
/// summarize each chunk, then merge the summaries.
async fn compact_cascade(
    out: &mut Output,
    provider: &mut dyn Provider,
    to_summarize: &[Message],
    messages: &mut Vec<Message>,
    ctx: &CompactContext,
    focus: &str,
) -> Result<Option<TokenUsage>, String> {
    let chunk_size = messages_per_chunk(to_summarize.len());
    let total_stages = to_summarize.len().div_ceil(chunk_size);

    let _ = writeln!(
        out,
        "{BOLD}Cascading compaction: {} intermediate messages → {} stages ({} messages/stage){RESET}",
        to_summarize.len(),
        total_stages,
        chunk_size,
    );

    let mut summaries: Vec<String> = Vec::new();
    let mut start = 0;

    for stage in 0..total_stages {
        let end = (start + chunk_size).min(to_summarize.len());
        let chunk: Vec<&Message> = to_summarize[start..end].iter().collect();

        let _ = writeln!(
            out,
            "{BOLD}  Stage {}/{}: Compacting messages {}–{} ({} messages)...{RESET}",
            stage + 1,
            total_stages,
            start + 1,
            end,
            chunk.len(),
        );

        let chunk_text = format_messages_for_summary(&chunk);

        match call_llm_summarize(provider, &chunk_text, focus, false).await {
            Ok((summary, _)) => summaries.push(summary),
            Err(e) => {
                let _ = writeln!(
                    out,
                    "{ORANGE}  Stage {}/{} failed: {e}{RESET} — continuing with remaining stages.",
                    stage + 1,
                    total_stages,
                );
            }
        }

        start = end;
    }

    if summaries.is_empty() {
        return Err("All compaction stages failed. The conversation was not modified.".to_string());
    }

    // Merge summaries if there are multiple
    let (final_summary, token_usage) = if summaries.len() > 1 {
        let _ = writeln!(
            out,
            "{BOLD}  Merging {} summaries into final summary...{RESET}",
            summaries.len(),
        );

        let merged_text = summaries.join("\n\n---\n\n");

        match call_llm_summarize(provider, &merged_text, focus, true).await {
            Ok((merged, usage)) => (merged, usage),
            Err(_) => {
                // Merge failed — fall back to concatenating raw summaries
                let _ = writeln!(
                    out,
                    "{ORANGE}  Merge stage failed — using concatenated summaries as fallback.{RESET}",
                );
                let mut concatenated = String::from(
                    "[Compacted in multiple stages — each section is a summary of a conversation segment]\n\n",
                );
                for (i, summary) in summaries.iter().enumerate() {
                    concatenated.push_str(&format!("--- Stage {} ---\n{}\n\n", i + 1, summary));
                }
                (concatenated, None)
            }
        }
    } else {
        // Only one summary was produced (all other stages failed, or only one chunk)
        (summaries.into_iter().next().unwrap(), None)
    };

    reconstruct_messages(out, messages, ctx, &final_summary);
    Ok(token_usage)
}

/// Reconstruct the messages vector after compaction:
/// [system_prompt, summary_message, ...recent_messages]
fn reconstruct_messages(
    out: &mut Output,
    messages: &mut Vec<Message>,
    ctx: &CompactContext,
    summary_content: &str,
) {
    let mut new_messages = Vec::new();

    // Keep system prompt
    if let Some(sys) = ctx.system_msg.clone() {
        new_messages.push(sys);
    }

    // Add the compaction summary as a system message
    new_messages.push(Message {
        role: Role::System,
        content: format!(
            "[Previous conversation summary]\n{}\n[End of summary — all details above have been compacted. \
             If the user references something from before, it may be in this summary.]",
            summary_content.trim()
        ),
        tool_calls: vec![], images: vec![],
    });

    // Keep all recent messages from keep_from onward
    for msg in messages.drain(ctx.keep_from..) {
        new_messages.push(msg);
    }

    let original_count = ctx.original_count;
    *messages = new_messages;
    let new_count = messages.len();

    let _ = writeln!(
        out,
        "{GREEN}Compacted: {original_count} messages → {new_count} messages{RESET}",
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_messages_truncation() {
        let long_content = "x".repeat(3000);
        let msg = Message {
            role: Role::User,
            content: long_content,
            tool_calls: vec![],
            images: vec![],
        };
        let formatted = format_messages_for_summary(&[&msg]);
        assert!(formatted.contains("[USER]:"));
        assert!(formatted.contains("[truncated, 3000 chars total]"));
        // The truncated content should be at most MAX_CHARS_PER_MESSAGE chars
        let content_start = formatted.find("[USER]: ").unwrap() + 7;
        let trunc_marker = formatted.find("... [truncated").unwrap();
        assert!(trunc_marker - content_start <= MAX_CHARS_PER_MESSAGE + 10); // +10 for safety margin
    }

    #[test]
    fn test_format_messages_short() {
        let msg = Message {
            role: Role::Assistant,
            content: "Hello world".to_string(),
            tool_calls: vec![],
            images: vec![],
        };
        let formatted = format_messages_for_summary(&[&msg]);
        assert!(formatted.contains("[ASSISTANT]: Hello world"));
    }

    #[test]
    fn test_format_messages_all_roles() {
        let msgs = [
            Message {
                role: Role::System,
                content: "sys".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::User,
                content: "usr".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "ast".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::Tool,
                content: "tol".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
        ];
        let refs: Vec<&Message> = msgs.iter().collect();
        let formatted = format_messages_for_summary(&refs);
        assert!(formatted.contains("[SYSTEM]: sys"));
        assert!(formatted.contains("[USER]: usr"));
        assert!(formatted.contains("[ASSISTANT]: ast"));
        assert!(formatted.contains("[TOOL]: tol"));
    }

    #[test]
    fn test_messages_per_chunk_small() {
        // For 50 messages, half is 25
        assert_eq!(messages_per_chunk(50), 25);
    }

    #[test]
    fn test_messages_per_chunk_large() {
        // For 1000 messages, capped at 200
        assert_eq!(messages_per_chunk(1000), 200);
    }

    #[test]
    fn test_messages_per_chunk_minimum() {
        // For very few messages, should be at least MIN_MESSAGES_PER_CHUNK
        let size = messages_per_chunk(15);
        assert!(size >= MIN_MESSAGES_PER_CHUNK);
        assert!(size <= 15); // can't exceed the total
    }

    #[test]
    fn test_focus_instruction() {
        assert!(focus_instruction("").is_empty());
        assert!(focus_instruction("Rust patterns").contains("Rust patterns"));
        assert!(focus_instruction("Rust patterns").starts_with("\n\n"));
    }

    #[test]
    fn test_reconstruct_messages() {
        let system = Message {
            role: Role::System,
            content: "You are helpful.".to_string(),
            tool_calls: vec![],
            images: vec![],
        };
        let mut messages = vec![
            system.clone(),
            Message {
                role: Role::User,
                content: "msg1".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "msg2".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::User,
                content: "msg3".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "msg4".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::User,
                content: "recent1".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "recent2".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::User,
                content: "recent3".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "recent4".to_string(),
                tool_calls: vec![],
                images: vec![],
            },
        ];
        let ctx = CompactContext {
            system_msg: Some(system),
            keep_from: 5, // keep messages[5..] (last 4)
            original_count: 9,
        };

        let mut out = Output::new(Box::new(Vec::new()));
        reconstruct_messages(&mut out, &mut messages, &ctx, "This is a summary.");

        // Should have: system + summary + 4 recent = 6 messages
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content, "You are helpful.");
        assert_eq!(messages[1].role, Role::System);
        assert!(messages[1].content.contains("This is a summary."));
        assert_eq!(messages[2].content, "recent1");
        assert_eq!(messages[5].content, "recent4");
    }
}
