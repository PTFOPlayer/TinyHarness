use std::io::Write;

use tinyharness_lib::provider::{Message, Role};
use tinyharness_ui::style::*;

use crate::commands::registry::{CommandContext, CommandResult};

// ── Core implementation ─────────────────────────────────────────────────────

pub fn execute(
    ctx: &mut CommandContext,
    arg: Option<&str>,
    messages: &[Message],
) -> Result<CommandResult, String> {
    let path = match arg {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => {
            // Default: save next to the session data directory
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            let dir = std::path::PathBuf::from(home).join(".local/share/tinyharness");
            let timestamp = chrono_now_or_fallback();
            dir.join(format!("debug-{}.log", timestamp))
                .to_string_lossy()
                .to_string()
        }
    };

    let file_path = std::path::PathBuf::from(&path);

    // Create parent directory if needed
    if let Some(parent) = file_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory '{}': {}", parent.display(), e))?;
    }

    let mut file = std::fs::File::create(&file_path)
        .map_err(|e| format!("Failed to create file '{}': {}", file_path.display(), e))?;

    // ── Header ────────────────────────────────────────────────────────────
    writeln!(file, "=== TinyHarness Debug Dump ===").unwrap();
    writeln!(file).unwrap();

    // ── Session info ───────────────────────────────────────────────────────
    writeln!(file, "=== Session Info ===").unwrap();
    writeln!(file, "Mode: {}", ctx.current_mode).unwrap();
    writeln!(
        file,
        "Session ID: {}",
        ctx.session_id.as_deref().unwrap_or("(none)")
    )
    .unwrap();
    writeln!(file, "Show thinking: {}", ctx.show_thinking).unwrap();
    writeln!(file).unwrap();

    // ── System prompt source ───────────────────────────────────────────────
    writeln!(file, "=== System Prompt Source ===").unwrap();
    let mode = ctx.current_mode;
    let prompts_dir = &ctx.prompts_dir;

    // Check header source
    if mode.uses_header() {
        let header_path = prompts_dir.join("header.md");
        match std::fs::read_to_string(&header_path) {
            Ok(content) if !content.trim().is_empty() => {
                writeln!(
                    file,
                    "Header: custom file ({}, {} bytes)",
                    header_path.display(),
                    content.len()
                )
                .unwrap();
            }
            _ => {
                writeln!(file, "Header: hardcoded default").unwrap();
            }
        }
    }

    // Check mode prompt source
    let mode_path = prompts_dir.join(mode.prompts_filename());
    match std::fs::read_to_string(&mode_path) {
        Ok(content) if !content.trim().is_empty() => {
            writeln!(
                file,
                "Mode prompt: custom file ({}, {} bytes)",
                mode_path.display(),
                content.len()
            )
            .unwrap();
        }
        _ => {
            writeln!(
                file,
                "Mode prompt: hardcoded default ({})",
                mode.prompts_filename()
            )
            .unwrap();
        }
    }

    // Show the assembled system prompt
    writeln!(file).unwrap();
    writeln!(file, "--- Assembled System Prompt ---").unwrap();
    writeln!(file, "{}", ctx.build_system_prompt()).unwrap();
    writeln!(file, "--- End System Prompt ---").unwrap();
    writeln!(file).unwrap();

    // ── Workspace context ──────────────────────────────────────────────────
    writeln!(file, "=== Workspace Context ===").unwrap();
    let wctx = &ctx.workspace_ctx;
    writeln!(file, "Root: {}", wctx.root.display()).unwrap();
    writeln!(file, "Project type: {}", wctx.project_type).unwrap();
    writeln!(file, "Project name: {}", wctx.project_name).unwrap();
    writeln!(file, "Git repo: {}", wctx.is_git_repo).unwrap();
    writeln!(file, "Build command: {}", wctx.build_command).unwrap();
    writeln!(file, "Test command: {}", wctx.test_command).unwrap();

    // Project MD
    match &wctx.project_md {
        Some((filename, content)) => {
            writeln!(
                file,
                "Project instructions: {} ({} bytes)",
                filename,
                content.len()
            )
            .unwrap();
        }
        None => {
            writeln!(file, "Project instructions: (none found)").unwrap();
        }
    }

    if !wctx.additional_project_mds.is_empty() {
        writeln!(file, "Additional project MD files:").unwrap();
        for (name, content) in &wctx.additional_project_mds {
            writeln!(file, "  - {} ({} bytes)", name, content.len()).unwrap();
        }
    }

    writeln!(file).unwrap();
    writeln!(file, "--- Formatted workspace context ---").unwrap();
    writeln!(file, "{}", wctx.format()).unwrap();
    writeln!(file, "--- End workspace context ---").unwrap();
    writeln!(file).unwrap();

    // ── Pinned files ───────────────────────────────────────────────────────
    writeln!(file, "=== Pinned Files ===").unwrap();
    let pinned_summaries = ctx.file_context.pinned_file_summaries();
    if pinned_summaries.is_empty() {
        writeln!(file, "(no files pinned)").unwrap();
    } else {
        writeln!(file, "Pinned file count: {}", pinned_summaries.len()).unwrap();
        for (path, lines, bytes) in &pinned_summaries {
            writeln!(file, "  - {} ({} lines, {} bytes)", path, lines, bytes).unwrap();
        }
    }
    writeln!(file).unwrap();

    // ── Skills ──────────────────────────────────────────────────────��──────
    writeln!(file, "=== Skills ===").unwrap();
    let all_skills = &ctx.skill_registry.skills;
    if all_skills.is_empty() {
        writeln!(file, "No skills discovered.").unwrap();
    } else {
        writeln!(file, "Discovered skills ({}):", all_skills.len()).unwrap();
        for skill in all_skills {
            let auto = if skill.disable_model_invocation {
                "manual-only"
            } else {
                "auto-invocable"
            };
            writeln!(
                file,
                "  - {} [{}] ({:?}) — {}",
                skill.name, auto, skill.source, skill.description
            )
            .unwrap();
        }
    }

    if ctx.active_skills.is_empty() {
        writeln!(file, "Active skills: (none)").unwrap();
    } else {
        writeln!(file, "Active skills: {}", ctx.active_skills.join(", ")).unwrap();
        // Include full content of active skills
        for name in &ctx.active_skills {
            if let Some(skill) = ctx.skill_registry.get(name) {
                writeln!(file).unwrap();
                writeln!(file, "--- Active skill: {} ---", skill.name).unwrap();
                writeln!(file, "Description: {}", skill.description).unwrap();
                writeln!(file, "Path: {}", skill.path.display()).unwrap();
                writeln!(file, "Source: {:?}", skill.source).unwrap();
                writeln!(file).unwrap();
                writeln!(file, "{}", skill.content).unwrap();
                writeln!(file, "--- End skill: {} ---", skill.name).unwrap();
            }
        }
    }
    writeln!(file).unwrap();

    // ── Messages ───────────────────────────────────────────────────────────
    writeln!(file, "=== Messages ===").unwrap();
    writeln!(file, "Messages in context: {}", messages.len()).unwrap();
    writeln!(file).unwrap();

    // Dump each message
    for (i, msg) in messages.iter().enumerate() {
        let role_str = match msg.role {
            Role::System => "SYSTEM",
            Role::User => "USER",
            Role::Assistant => "ASSISTANT",
            Role::Tool => "TOOL",
        };

        writeln!(file, "--- Message {} [{}] ---", i + 1, role_str).unwrap();

        // Content (may be very long, dump in full)
        writeln!(file, "{}", msg.content).unwrap();

        // Tool calls
        if !msg.tool_calls.is_empty() {
            writeln!(file).unwrap();
            writeln!(file, "[Tool Calls]").unwrap();
            for tc in &msg.tool_calls {
                writeln!(file, "  - {}({})", tc.function.name, tc.function.arguments).unwrap();
            }
        }

        // Images
        if !msg.images.is_empty() {
            writeln!(file).unwrap();
            writeln!(file, "[{} image(s) attached]", msg.images.len()).unwrap();
        }

        writeln!(file).unwrap();
    }

    let _ = writeln!(
        ctx.output,
        "{GREEN}Dumped debug info to {}{RESET}",
        file_path.display(),
    );

    Ok(CommandResult::Ok)
}

/// Generate a timestamp string for the default filename.
/// Falls back to a counter-based name if the system time is unavailable.
fn chrono_now_or_fallback() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Format as YYYYMMDD-HHMMSS-ish using simple arithmetic
    let days = now / 86400;
    let time_of_day = now % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    // Days since epoch → approximate year
    let year = 1970 + days / 365;
    format!("{}-{:02}{:02}{:02}", year, hours, minutes, seconds)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tinyharness_lib::provider::Message;

    fn make_test_ctx() -> CommandContext {
        use std::sync::Arc;
        use tokio::sync::Mutex;

        // Create a minimal CommandContext for testing.
        CommandContext::new(
            Arc::new(Mutex::new(
                tinyharness_lib::provider::ollama::OllamaProvider::new(
                    "http://localhost:11434".to_string(),
                    120,
                    0,
                    tinyharness_lib::config::OllamaThinkType::Off,
                ),
            )),
            tinyharness_lib::context::WorkspaceContext::collect(),
            std::path::PathBuf::from("/tmp/tinyharness-prompts-test"),
        )
    }

    #[test]
    fn test_execute_dumps_messages() {
        let messages = vec![
            Message::simple(Role::System, "You are helpful."),
            Message::simple(Role::User, "Hello"),
            Message::simple(Role::Assistant, "Hi there!"),
        ];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug-test.log");
        let path_str = path.to_string_lossy().to_string();

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&path_str), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("Messages in context: 3"));
        assert!(content.contains("[SYSTEM]"));
        assert!(content.contains("[USER]"));
        assert!(content.contains("[ASSISTANT]"));
        assert!(content.contains("You are helpful."));
        assert!(content.contains("Hello"));
        assert!(content.contains("Hi there!"));
    }

    #[test]
    fn test_execute_with_tool_calls() {
        use tinyharness_lib::provider::ToolCall;

        let messages = vec![
            Message::simple(Role::User, "Read the file"),
            Message {
                role: Role::Assistant,
                content: "I'll read that file.".to_string(),
                tool_calls: vec![ToolCall {
                    function: tinyharness_lib::provider::ToolCallFunction {
                        name: "read".to_string(),
                        arguments: serde_json::json!({"path": "/tmp/test.rs"}),
                        thought_signature: None,
                    },
                }],
                images: vec![],
            },
            Message::simple(Role::Tool, "file contents here"),
        ];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug-tools.log");
        let path_str = path.to_string_lossy().to_string();

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&path_str), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[Tool Calls]"));
        assert!(content.contains("read"));
    }

    #[test]
    fn test_execute_includes_session_info() {
        let messages = vec![Message::simple(Role::User, "test")];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug-session.log");
        let path_str = path.to_string_lossy().to_string();

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&path_str), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("=== Session Info ==="));
        assert!(content.contains("Mode:"));
        assert!(content.contains("=== System Prompt Source ==="));
        assert!(content.contains("=== Workspace Context ==="));
        assert!(content.contains("=== Pinned Files ==="));
        assert!(content.contains("=== Skills ==="));
    }
}