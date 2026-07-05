// ── Shared Tool Result Types ───────────────────────────────────────────────
//
// Generic tool result struct and batching logic shared between CLI and TUI loops.

use tinyharness_lib::{image::ImageAttachment, provider::Message};

/// Ensure every `ToolCall` in the slice has a non-empty `id`.
///
/// Some providers (Ollama, Sockudo) return tool calls without `id`. OpenAI-
/// compatible APIs require a non-empty id that matches between the assistant
/// message and the tool result. This function synthesizes stable ids
/// (`call_0`, `call_1`, …) for any tool calls that are missing one, mutating
/// the slice in place.
pub fn ensure_tool_call_ids(tool_calls: &mut [tinyharness_lib::provider::ToolCall]) {
    for (i, tc) in tool_calls.iter_mut().enumerate() {
        if tc.id.as_deref().map(|s| s.is_empty()).unwrap_or(true) {
            tc.id = Some(format!("call_{}", i));
        }
    }
}

/// Result from executing a generic (non-signal) tool call.
///
/// Used by both CLI and TUI agent loops to track tool execution results
/// before batching them into a single `Role::Tool` message.
pub struct GenericToolResult {
    /// Formatted content for the conversation message.
    pub content: String,
    /// The `tool_call_id` from the originating assistant tool call.
    /// OpenAI-compatible APIs require this to match the assistant's tool call id.
    pub tool_call_id: String,
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

/// Convert tool results into individual `Role::Tool` messages, one per result.
///
/// OpenAI-compatible APIs require one `Role::Tool` message per tool call, each
/// with a `tool_call_id` matching the originating assistant tool call.
///
/// Returns an empty vector if the results list is empty.
pub fn batch_tool_results(results: Vec<GenericToolResult>) -> Vec<Message> {
    results
        .into_iter()
        .map(|r| {
            // Collect images from this tool result
            let images = r.images;
            Message {
                role: tinyharness_lib::provider::Role::Tool,
                content: r.content,
                tool_calls: vec![],
                tool_call_id: Some(r.tool_call_id),
                images,
            }
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tinyharness_lib::provider::{ToolCall, ToolCallFunction};

    fn make_call(name: &str, args: serde_json::Value) -> ToolCall {
        ToolCall {
            id: Some("test-id".to_string()),
            function: ToolCallFunction {
                name: name.to_string(),
                arguments: args,
                thought_signature: None,
            },
        }
    }

    #[test]
    fn ensure_tool_call_ids_assigns_missing_ids() {
        let mut calls = vec![
            ToolCall {
                id: None,
                function: ToolCallFunction {
                    name: "ls".to_string(),
                    arguments: json!({}),
                    thought_signature: None,
                },
            },
            ToolCall {
                id: Some("".to_string()),
                function: ToolCallFunction {
                    name: "cat".to_string(),
                    arguments: json!({}),
                    thought_signature: None,
                },
            },
            ToolCall {
                id: Some("existing".to_string()),
                function: ToolCallFunction {
                    name: "grep".to_string(),
                    arguments: json!({}),
                    thought_signature: None,
                },
            },
        ];
        ensure_tool_call_ids(&mut calls);
        assert_eq!(calls[0].id, Some("call_0".to_string()));
        assert_eq!(calls[1].id, Some("call_1".to_string()));
        assert_eq!(calls[2].id, Some("existing".to_string()));
    }

    #[test]
    fn ensure_tool_call_ids_empty_slice() {
        let mut calls: Vec<ToolCall> = vec![];
        ensure_tool_call_ids(&mut calls);
        assert!(calls.is_empty());
    }

    #[test]
    fn batch_tool_results_creates_tool_messages() {
        let results = vec![
            GenericToolResult {
                content: "result 1".to_string(),
                tool_call_id: "call_0".to_string(),
                audit_tool_name: None,
                audit_detail: None,
                duration_ms: 10,
                is_error: false,
                images: vec![],
            },
            GenericToolResult {
                content: "result 2".to_string(),
                tool_call_id: "call_1".to_string(),
                audit_tool_name: None,
                audit_detail: None,
                duration_ms: 5,
                is_error: false,
                images: vec![],
            },
        ];
        let messages = batch_tool_results(results);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].role, tinyharness_lib::provider::Role::Tool);
        assert_eq!(messages[0].content, "result 1");
        assert_eq!(messages[0].tool_call_id, Some("call_0".to_string()));
        assert_eq!(messages[1].content, "result 2");
        assert_eq!(messages[1].tool_call_id, Some("call_1".to_string()));
    }

    #[test]
    fn batch_tool_results_empty() {
        let messages = batch_tool_results(vec![]);
        assert!(messages.is_empty());
    }

    #[test]
    fn audit_info_for_run() {
        let call = make_call("run", json!({"command": "ls -la"}));
        let (tool, detail) = audit_info_for_tool(&call);
        assert_eq!(tool, Some("run".to_string()));
        assert_eq!(detail, Some("ls -la".to_string()));
    }

    #[test]
    fn audit_info_for_write() {
        let call = make_call("write", json!({"path": "/tmp/test.rs", "content": "hi"}));
        let (tool, detail) = audit_info_for_tool(&call);
        assert_eq!(tool, Some("write".to_string()));
        assert_eq!(detail, Some("/tmp/test.rs".to_string()));
    }

    #[test]
    fn audit_info_for_edit() {
        let call = make_call(
            "edit",
            json!({"path": "/tmp/test.rs", "old_str": "a", "new_str": "b"}),
        );
        let (tool, detail) = audit_info_for_tool(&call);
        assert_eq!(tool, Some("edit".to_string()));
        assert_eq!(detail, Some("/tmp/test.rs".to_string()));
    }

    #[test]
    fn audit_info_for_read_only_tool() {
        let call = make_call("ls", json!({"path": "/tmp"}));
        let (tool, detail) = audit_info_for_tool(&call);
        assert_eq!(tool, None);
        assert_eq!(detail, None);
    }

    #[test]
    fn tool_display_content_for_non_edit_tool() {
        let result = tool_display_content("ls", &json!({}), "file1\nfile2", false);
        assert_eq!(result, "file1\nfile2");
    }

    #[test]
    fn tool_display_content_error_passes_through() {
        let result = tool_display_content(
            "edit",
            &json!({"path": "/nonexistent"}),
            "Error: file not found",
            true,
        );
        assert_eq!(result, "Error: file not found");
    }
}
