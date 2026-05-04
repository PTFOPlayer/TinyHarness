use crate::provider::{Message, Role};
use crate::style::*;

/// Compact the conversation history by summarizing older messages.
///
/// This keeps the system prompt and the most recent messages intact,
/// while replacing all intermediate messages with a single summary message.
/// The provider is used to generate the summary.
pub async fn execute_compact(
    provider: &mut dyn crate::provider::Provider,
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
    // This ensures we preserve the most recent exchange regardless of role.
    const KEEP_RECENT: usize = 4;
    let keep_from = messages.len().saturating_sub(KEEP_RECENT).max(1);

    // Messages to summarize: everything between system prompt and keep_from
    let to_summarize: Vec<&Message> = messages[1..keep_from].iter().collect();

    if to_summarize.is_empty() {
        println!(
            "{}Nothing to compact — recent messages are the only ones present.{}",
            ORANGE, RESET
        );
        return Ok(());
    }

    // Build a summary of the conversation to compact
    let mut summary_text = String::new();
    for msg in &to_summarize {
        let role_str = match msg.role {
            Role::System => "SYSTEM",
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::Tool => "TOOL",
        };
        // Truncate very long messages
        let content = if msg.content.len() > 2000 {
            format!(
                "{}... [truncated, {} chars total]",
                &msg.content[..2000],
                msg.content.len()
            )
        } else {
            msg.content.clone()
        };
        summary_text.push_str(&format!("[{}]: {}\n\n", role_str, content));
    }

    let focus_instruction = if focus.is_empty() {
        String::new()
    } else {
        format!(
            "\n\nPay special attention to preserving information about: {}",
            focus
        )
    };

    let summarization_prompt = format!(
        "Summarize the following conversation history concisely. \
         Preserve all important decisions, code changes, file paths, error messages, \
         and technical details. Omit pleasantries and redundant information. \
         Focus on facts: what was discussed, what was decided, what was changed.{}",
        focus_instruction,
    );

    // Build messages for the summarization request
    let summarization_messages = vec![
        Message {
            role: Role::System,
            content: "You are a helpful assistant that creates concise, accurate summaries of conversations. \
                       Preserve all technical details: file paths, code snippets, error messages, decisions made, \
                       and current task status. Do NOT add information that was not in the original conversation.".to_string(),
            tool_calls: vec![],
        },
        Message {
            role: Role::User,
            content: format!("{}\n\n{}", summarization_prompt, summary_text),
            tool_calls: vec![],
        },
    ];

    println!(
        "{}Compacting {} messages...{}",
        BOLD,
        to_summarize.len(),
        RESET
    );

    // Use the provider to generate a summary
    let (send, mut recv) = tokio::sync::mpsc::channel::<crate::provider::ChatMessageResponse>(1024);

    // We need no tools for summarization
    let tools = vec![];

    provider
        .chat(
            summarization_messages,
            "Summarize the conversation".to_string(),
            send,
            tools,
        )
        .await;

    // Collect the summary
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

    // Reconstruct the messages
    let mut new_messages = Vec::new();

    // Keep system prompt
    if let Some(sys) = system_msg {
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
    for msg in messages.drain(keep_from..) {
        new_messages.push(msg);
    }

    *messages = new_messages;
    let new_count = messages.len();

    println!(
        "{}Compacted: {} messages → {} messages{}",
        GREEN, original_count, new_count, RESET
    );

    Ok(())
}
