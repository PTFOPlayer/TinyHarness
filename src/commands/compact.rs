use tinyharness_lib::{
    config::load_settings,
    provider::{Message, Provider, Role},
    token::{estimate_conversation_tokens, estimate_tokens},
};

use crate::commands::registry::{Command, CommandContext, CommandResult};
use crate::style::*;

use std::future::Future;
use std::pin::Pin;

// ── Command trait implementation ────────────────────────────────────────────

pub struct CompactCommand;

impl Command for CompactCommand {
    fn name(&self) -> &'static str {
        "/compact"
    }

    fn description(&self) -> &'static str {
        "Summarize conversation history to free context space. Optionally specify a focus area."
    }

    fn usage(&self) -> &'static str {
        "/compact [focus]"
    }

    fn execute<'a>(
        &'a self,
        raw_arg: Option<&str>,
        ctx: &'a mut CommandContext,
        messages: &'a mut Vec<Message>,
    ) -> Pin<Box<dyn Future<Output = Result<CommandResult, String>> + Send + 'a>> {
        let focus = raw_arg.unwrap_or("").to_string();
        let provider = ctx.provider.clone();

        Box::pin(async move {
            let mut p = provider.lock().await;
            execute_compact(&mut *p, messages, &focus).await?;
            Ok(CommandResult::Ok)
        })
    }
}

// ── Core implementation ─────────────────────────────────────────────────────

/// Maximum characters per message when formatting for summarization.
const MAX_CHARS_PER_MESSAGE: usize = 2000;

/// Fraction of the context window to use as the token budget per chunk.
/// We leave room for the system prompt, summarization instructions, and the response.
const CHUNK_BUDGET_FRACTION: f64 = 0.6;

/// Minimum number of messages per chunk (don't split finer than this).
const MIN_MESSAGES_PER_CHUNK: usize = 10;

/// Maximum number of messages per chunk (capped at 250, but the 60% context
/// budget will naturally limit how many messages actually fit).
const MAX_MESSAGES_PER_CHUNK: usize = 250;

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

/// Call the LLM to summarize a text, returning the generated summary.
///
/// Uses a dedicated system prompt for summarization and streams the response.
async fn call_llm_summarize(
    provider: &mut dyn Provider,
    text_to_summarize: &str,
    focus: &str,
    is_merge: bool,
) -> Result<String, String> {
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
            tool_calls: vec![],
        },
        Message {
            role: Role::User,
            content: format!("{}\n\n{}", summarization_prompt, text_to_summarize),
            tool_calls: vec![],
        },
    ];

    let tools = vec![];
    let mut recv = provider.chat(summarization_messages, tools).await?;

    let mut summary_content = String::new();
    let mut done = false;
    while let Some(msg) = recv.recv().await {
        if !msg.message.content.is_empty() {
            summary_content.push_str(&msg.message.content);
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

    Ok(summary_content)
}

/// Estimate how many messages fit within the token budget per chunk.
///
/// Walks the messages from oldest to newest, accumulating estimated tokens,
/// and returns the number of messages that fit within the budget. The result
/// is clamped to [`MIN_MESSAGES_PER_CHUNK`, `MAX_MESSAGES_PER_CHUNK`].
fn estimate_messages_per_chunk(messages: &[&Message], token_budget: u32) -> usize {
    let mut count = 0;
    let mut tokens_used = 0u32;

    for msg in messages {
        let msg_tokens = estimate_tokens(&msg.content);
        if tokens_used + msg_tokens > token_budget && count >= MIN_MESSAGES_PER_CHUNK {
            break;
        }
        tokens_used += msg_tokens;
        count += 1;

        if count >= MAX_MESSAGES_PER_CHUNK {
            break;
        }
    }

    // Ensure at least MIN_MESSAGES_PER_CHUNK per chunk
    count.max(MIN_MESSAGES_PER_CHUNK).min(messages.len())
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
    provider: &mut dyn Provider,
    messages: &mut Vec<Message>,
    focus: &str,
) -> Result<(), String> {
    if messages.len() <= 6 {
        println!(
            "{}Not enough messages to compact (only {} messages).{}",
            ORANGE,
            messages.len(),
            RESET
        );
        return Ok(());
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
        println!(
            "{}Nothing to compact — recent messages are the only ones present.{}",
            ORANGE, RESET
        );
        return Ok(());
    }

    let ctx = CompactContext {
        system_msg,
        keep_from,
        original_count,
    };

    // Estimate tokens for the intermediate messages
    let total_tokens = estimate_conversation_tokens(&to_summarize);

    // Determine token budget per chunk based on context window size
    let settings = load_settings();
    let context_tokens = settings.context_limit.unwrap_or(8192); // Default 8K context
    let budget_per_chunk = (context_tokens as f64 * CHUNK_BUDGET_FRACTION) as u32;

    // Decide: single-pass or cascade?
    if total_tokens <= budget_per_chunk {
        compact_single_pass(provider, &to_summarize, messages, &ctx, focus).await
    } else {
        compact_cascade(
            provider,
            &to_summarize,
            messages,
            &ctx,
            focus,
            budget_per_chunk,
        )
        .await
    }
}

/// Single-pass compaction: summarize all intermediate messages in one LLM call.
async fn compact_single_pass(
    provider: &mut dyn Provider,
    to_summarize: &[Message],
    messages: &mut Vec<Message>,
    ctx: &CompactContext,
    focus: &str,
) -> Result<(), String> {
    let refs: Vec<&Message> = to_summarize.iter().collect();
    let summary_text = format_messages_for_summary(&refs);

    println!(
        "{}Compacting {} messages...{}",
        BOLD,
        to_summarize.len(),
        RESET
    );

    let summary_content = call_llm_summarize(provider, &summary_text, focus, false).await?;
    reconstruct_messages(messages, ctx, &summary_content);
    Ok(())
}

/// Multi-stage cascade compaction: split intermediate messages into chunks,
/// summarize each chunk, then merge the summaries.
async fn compact_cascade(
    provider: &mut dyn Provider,
    to_summarize: &[Message],
    messages: &mut Vec<Message>,
    ctx: &CompactContext,
    focus: &str,
    budget_per_chunk: u32,
) -> Result<(), String> {
    let refs: Vec<&Message> = to_summarize.iter().collect();
    let chunk_size = estimate_messages_per_chunk(&refs, budget_per_chunk);
    let total_stages = to_summarize.len().div_ceil(chunk_size);

    println!(
        "{}Cascading compaction: {} intermediate messages → {} stages ({} messages/stage){}",
        BOLD,
        to_summarize.len(),
        total_stages,
        chunk_size,
        RESET
    );

    let mut summaries: Vec<String> = Vec::new();
    let mut start = 0;

    for stage in 0..total_stages {
        let end = (start + chunk_size).min(to_summarize.len());
        let chunk: Vec<&Message> = to_summarize[start..end].iter().collect();

        println!(
            "{}  Stage {}/{}: Compacting messages {}–{} ({} messages)...{}",
            BOLD,
            stage + 1,
            total_stages,
            start + 1,
            end,
            chunk.len(),
            RESET
        );

        let chunk_text = format_messages_for_summary(&chunk);

        match call_llm_summarize(provider, &chunk_text, focus, false).await {
            Ok(summary) => summaries.push(summary),
            Err(e) => {
                // If a stage fails, try to continue with remaining chunks.
                // We'll attempt to merge whatever we have.
                eprintln!(
                    "{}  Stage {}/{} failed: {}{} — continuing with remaining stages.",
                    ORANGE,
                    stage + 1,
                    total_stages,
                    e,
                    RESET
                );
            }
        }

        start = end;
    }

    if summaries.is_empty() {
        return Err("All compaction stages failed. The conversation was not modified.".to_string());
    }

    // Merge summaries if there are multiple
    let final_summary = if summaries.len() > 1 {
        println!(
            "{}  Merging {} summaries into final summary...{}",
            BOLD,
            summaries.len(),
            RESET
        );

        let merged_text = summaries.join("\n\n---\n\n");

        match call_llm_summarize(provider, &merged_text, focus, true).await {
            Ok(merged) => merged,
            Err(_) => {
                // Merge failed — fall back to concatenating raw summaries
                eprintln!(
                    "{}  Merge stage failed — using concatenated summaries as fallback.{}",
                    ORANGE, RESET
                );
                let mut concatenated = String::from(
                    "[Compacted in multiple stages — each section is a summary of a conversation segment]\n\n",
                );
                for (i, summary) in summaries.iter().enumerate() {
                    concatenated.push_str(&format!("--- Stage {} ---\n{}\n\n", i + 1, summary));
                }
                concatenated
            }
        }
    } else {
        // Only one summary was produced (all other stages failed, or only one chunk)
        summaries.into_iter().next().unwrap()
    };

    reconstruct_messages(messages, ctx, &final_summary);
    Ok(())
}

/// Reconstruct the messages vector after compaction:
/// [system_prompt, summary_message, ...recent_messages]
fn reconstruct_messages(messages: &mut Vec<Message>, ctx: &CompactContext, summary_content: &str) {
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
        tool_calls: vec![],
    });

    // Keep all recent messages from keep_from onward
    for msg in messages.drain(ctx.keep_from..) {
        new_messages.push(msg);
    }

    let original_count = ctx.original_count;
    *messages = new_messages;
    let new_count = messages.len();

    println!(
        "{}Compacted: {} messages → {} messages{}",
        GREEN, original_count, new_count, RESET
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
        };
        let formatted = format_messages_for_summary(&[&msg]);
        assert!(formatted.contains("[ASSISTANT]: Hello world"));
    }

    #[test]
    fn test_format_messages_all_roles() {
        let msgs = vec![
            Message {
                role: Role::System,
                content: "sys".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::User,
                content: "usr".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "ast".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::Tool,
                content: "tol".to_string(),
                tool_calls: vec![],
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
    fn test_estimate_messages_per_chunk_small_budget() {
        // With a tiny budget, we should still return at least MIN_MESSAGES_PER_CHUNK
        let msgs: Vec<Message> = (0..50)
            .map(|_| Message {
                role: Role::User,
                content: "a".repeat(1000), // ~250 tokens each
                tool_calls: vec![],
            })
            .collect();
        let refs: Vec<&Message> = msgs.iter().collect();
        let chunk_size = estimate_messages_per_chunk(&refs, 100); // very small budget
        assert!(chunk_size >= MIN_MESSAGES_PER_CHUNK);
    }

    #[test]
    fn test_estimate_messages_per_chunk_large_budget() {
        // With a huge budget, all messages should fit in one chunk
        let msgs: Vec<Message> = (0..20)
            .map(|_| Message {
                role: Role::User,
                content: "short".to_string(), // ~1-2 tokens each
                tool_calls: vec![],
            })
            .collect();
        let refs: Vec<&Message> = msgs.iter().collect();
        let chunk_size = estimate_messages_per_chunk(&refs, 100_000);
        assert_eq!(chunk_size, 20); // All fit in one chunk
    }

    #[test]
    fn test_estimate_messages_per_chunk_respects_max() {
        // Even with a huge budget, cap at MAX_MESSAGES_PER_CHUNK
        let msgs: Vec<Message> = (0..500)
            .map(|_| Message {
                role: Role::User,
                content: "x".to_string(),
                tool_calls: vec![],
            })
            .collect();
        let refs: Vec<&Message> = msgs.iter().collect();
        let chunk_size = estimate_messages_per_chunk(&refs, 1_000_000);
        // Should cap at MAX_MESSAGES_PER_CHUNK (250) even with unlimited budget
        assert_eq!(chunk_size, 250);
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
        };
        let mut messages = vec![
            system.clone(),
            Message {
                role: Role::User,
                content: "msg1".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "msg2".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::User,
                content: "msg3".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "msg4".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::User,
                content: "recent1".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "recent2".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::User,
                content: "recent3".to_string(),
                tool_calls: vec![],
            },
            Message {
                role: Role::Assistant,
                content: "recent4".to_string(),
                tool_calls: vec![],
            },
        ];
        let ctx = CompactContext {
            system_msg: Some(system),
            keep_from: 5, // keep messages[5..] (last 4)
            original_count: 9,
        };

        reconstruct_messages(&mut messages, &ctx, "This is a summary.");

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
