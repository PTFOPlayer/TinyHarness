// ── Shared Tool Result Types ───────────────────────────────────────────────
//
// Generic tool result struct and batching logic shared between CLI and TUI loops.

use tinyharness_lib::{image::ImageAttachment, provider::Message};

/// Result from executing a generic (non-signal) tool call.
///
/// Used by both CLI and TUI agent loops to track tool execution results
/// before batching them into a single `Role::Tool` message.
pub struct GenericToolResult {
    /// Formatted content for batching into the conversation message.
    pub content: String,
    /// If this was an auditable tool (run/write/edit), the tool name.
    pub audit_tool_name: Option<String>,
    /// For auditable tools: the primary argument (command for "run", path for "write"/"edit").
    pub audit_detail: Option<String>,
    /// Duration of the tool execution in milliseconds.
    pub duration_ms: u64,
    /// Whether the tool returned an error.
    pub is_error: bool,
    /// Images loaded by the tool (e.g. when reading an image file).
    pub images: Vec<ImageAttachment>,
}

/// Batch multiple generic tool results into a single `Role::Tool` message.
///
/// For a single result, the content is used directly. For multiple results,
/// they're joined with separators and a count header.
///
/// Returns `None` if the results list is empty.
pub fn batch_tool_results(results: Vec<GenericToolResult>) -> Option<Message> {
    if results.is_empty() {
        return None;
    }

    let batched_content = if results.len() == 1 {
        results[0].content.clone()
    } else {
        format!(
            "Multiple tool results ({} total):\n\n{}",
            results.len(),
            results
                .iter()
                .map(|r| r.content.as_str())
                .collect::<Vec<_>>()
                .join("\n\n---\n\n")
        )
    };

    // Collect images from all tool results
    let all_images: Vec<ImageAttachment> = results.into_iter().flat_map(|r| r.images).collect();

    Some(Message {
        role: tinyharness_lib::provider::Role::Tool,
        content: format!(
            "Tool results:\n{}\n\nUse these results to continue helping the user.",
            batched_content
        ),
        tool_calls: vec![],
        tool_call_id: None,
        images: all_images,
    })
}

/// Build audit detail for a tool call.
///
/// Returns `(tool_name, detail)` for auditable tools, or `(None, None)` otherwise.
pub fn audit_info_for_tool(
    call: &tinyharness_lib::provider::ToolCall,
) -> (Option<String>, Option<String>) {
    match call.function.name.as_str() {
        "run" => (
            Some("run".to_string()),
            call.function
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        ),
        "write" => (
            Some("write".to_string()),
            call.function
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        ),
        "edit" => (
            Some("edit".to_string()),
            call.function
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        ),
        _ => (None, None),
    }
}

/// Log a tool call to the audit log.
pub fn log_tool_audit(
    session_id: &str,
    call: &tinyharness_lib::provider::ToolCall,
    auto_accepted: bool,
    duration_ms: u64,
    is_error: bool,
) {
    let (audit_tool_name, audit_detail) = audit_info_for_tool(call);
    if let Some(tool_name) = audit_tool_name {
        let exit_code = if is_error { -1 } else { 0 };
        crate::commands::audit::log_command(
            session_id,
            &tool_name,
            audit_detail.as_deref().unwrap_or(""),
            exit_code,
            auto_accepted,
            duration_ms,
        );
    }
}

/// Compute a plain-text diff for a tool call (edit or write).
///
/// Returns `None` for non-edit/write tools, or if the diff is empty.
/// Returns `Some(diff_string)` with the diff content otherwise.
///
/// This is used for both:
/// - Confirmation previews (before the tool executes)
/// - Display content (after the tool executes, to show what changed)
pub fn compute_tool_diff(tool_name: &str, arguments: &serde_json::Value) -> Option<String> {
    match tool_name {
        "edit" => {
            let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let old_str = arguments
                .get("old_str")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new_str = arguments
                .get("new_str")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let diff =
                tinyharness_ui::ui::diff::compute_edit_diff_from_path(path, old_str, new_str);
            if diff.is_empty() { None } else { Some(diff) }
        }
        "write" => {
            let path = arguments.get("path").and_then(|v| v.as_str()).unwrap_or("");
            let content = arguments
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let diff = tinyharness_ui::ui::diff::compute_write_diff_plain(path, content);
            if diff.is_empty() { None } else { Some(diff) }
        }
        _ => None,
    }
}

/// Build the display content for a tool result, appending a diff for edit/write tools.
///
/// If the tool is edit or write and the diff is non-empty, the diff is prepended
/// to the result. Otherwise, the result is returned as-is.
pub fn tool_display_content(
    tool_name: &str,
    arguments: &serde_json::Value,
    result: &str,
    is_error: bool,
) -> String {
    if is_error {
        return result.to_string();
    }
    match compute_tool_diff(tool_name, arguments) {
        Some(diff) if !diff.is_empty() => format!("{}\n{}", diff.trim_end(), result),
        _ => result.to_string(),
    }
}
