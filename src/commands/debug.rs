use std::io::Write;

use serde_json::{Value, json};
use tinyharness_lib::config::{load_settings, save_settings};
use tinyharness_lib::provider::{Message, Role};
use tinyharness_ui::style::*;

use crate::commands::registry::{CommandContext, CommandResult};

// ── Core implementation ─────────────────────────────────────────────────────

/// Parsed `/debug` arguments.
struct DebugArgs {
    /// Output file path (None = auto-generate).
    path: Option<String>,
    /// Whether to emit JSON instead of plain text.
    json: bool,
}

/// Parse the raw argument string from `/debug`.
///
/// Supports:
/// - `/debug`               → text log, auto path
/// - `/debug --json`        → JSON, auto path
/// - `/debug /path/to/file` → text log at path
/// - `/debug --json /path`  → JSON at path
/// - `/debug /path --json`  → JSON at path
fn parse_args(arg: Option<&str>) -> DebugArgs {
    let mut json = false;
    let mut path: Option<String> = None;

    if let Some(a) = arg
        && !a.is_empty()
    {
        for part in a.split_whitespace() {
            if part == "--json" || part == "-j" {
                json = true;
            } else {
                path = Some(part.to_string());
            }
        }
    }

    DebugArgs { path, json }
}

pub fn execute(
    ctx: &mut CommandContext,
    arg: Option<&str>,
    messages: &[Message],
) -> Result<CommandResult, String> {
    let args = parse_args(arg);

    let path = match &args.path {
        Some(p) if !p.is_empty() => p.clone(),
        _ => {
            // Default: save next to the session data directory
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            let dir = std::path::PathBuf::from(home).join(".local/share/tinyharness");
            let timestamp = chrono_now_or_fallback();
            let ext = if args.json { "json" } else { "log" };
            dir.join(format!("debug-{}.{}", timestamp, ext))
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

    if args.json {
        let json_value = build_json_dump(ctx, messages);
        let pretty = serde_json::to_string_pretty(&json_value)
            .map_err(|e| format!("Failed to serialize JSON: {}", e))?;
        writeln!(file, "{pretty}").unwrap();
    } else {
        write_text_dump(&mut file, ctx, messages);
    }

    // Persist the current show_thinking state so it survives restarts.
    let mut settings = load_settings();
    if settings.show_thinking != ctx.show_thinking {
        settings.show_thinking = ctx.show_thinking;
        save_settings(&settings);
    }

    let _ = writeln!(
        ctx.output,
        "{GREEN}Dumped debug info to {}{RESET}",
        file_path.display(),
    );

    Ok(CommandResult::Ok)
}

// ── Text dump ───────────────────────────────────────────────────────────────

fn write_text_dump(file: &mut std::fs::File, ctx: &CommandContext, messages: &[Message]) {
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

    // ── Provider diagnostics ───────────────────────────────────────────────
    writeln!(file, "=== Provider Diagnostics ===").unwrap();
    dump_provider_diagnostics(file, ctx);
    writeln!(file).unwrap();

    // ── Token usage ────────────────────────────────────────────────────────
    writeln!(file, "=== Token Usage ===").unwrap();
    dump_token_usage(file, ctx, messages);
    writeln!(file).unwrap();

    // ── Session metadata ───────────────────────────────────────────────────
    if let Some(session_id) = &ctx.session_id {
        writeln!(file, "=== Session Metadata ===").unwrap();
        dump_session_metadata(file, session_id);
        writeln!(file).unwrap();
    }

    // ── Configuration snapshot ─────────────────────────────────────────────
    writeln!(file, "=== Configuration Snapshot ===").unwrap();
    dump_configuration_snapshot(file);
    writeln!(file).unwrap();

    // ── Pending images ─────────────────────────────────────────────────────
    writeln!(file, "=== Pending Images ===").unwrap();
    dump_pending_images(file, ctx);
    writeln!(file).unwrap();

    // ── Command lists ──────────────────────────────────────────────────────
    writeln!(file, "=== Command Auto-Accept Lists ===").unwrap();
    dump_command_lists(file);
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

    // ── Skills ──────────────────────────────────────────────────────────────
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

        // Thinking/reasoning chain (if present) — shown before content since
        // the model reasons before producing its answer.
        let has_thinking = msg.thinking.as_ref().is_some_and(|t| !t.is_empty());

        if has_thinking {
            writeln!(file, "[Thinking]").unwrap();
            writeln!(file, "{}", msg.thinking.as_ref().unwrap()).unwrap();
            writeln!(file, "[End Thinking]").unwrap();
            writeln!(file).unwrap();
        }

        // Content (may be very long, dump in full)
        // Use [Response] markers when thinking was present, so the
        // separation between reasoning and answer is unambiguous.
        if has_thinking {
            writeln!(file, "[Response]").unwrap();
            writeln!(file, "{}", msg.content).unwrap();
            writeln!(file, "[End Response]").unwrap();
        } else {
            writeln!(file, "{}", msg.content).unwrap();
        }

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
}

// ── JSON dump ───────────────────────────────────────────────────────────────

/// Build the complete debug dump as a structured JSON value.
///
/// The `thinking` and `content` fields are separate properties on each
/// message object, giving unambiguous separation between reasoning and
/// response without the need for text markers.
fn build_json_dump(ctx: &CommandContext, messages: &[Message]) -> Value {
    let settings = load_settings();

    // ── Session info ───────────────────────────────────────────────────────
    let session_info = json!({
        "mode": ctx.current_mode,
        "session_id": ctx.session_id,
        "show_thinking": ctx.show_thinking,
    });

    // ── Provider diagnostics ──────────────────────────────────────────────
    let provider_diagnostics = json!({
        "provider_kind": settings.last_provider.to_string(),
        "provider_url": settings.get_current_url(),
        "current_model": settings.get_current_model(),
        "timeout_secs": settings.effective_timeout_secs(),
        "max_retries": settings.effective_max_retries(),
        "think_type": settings.ollama_think_type.to_string(),
        "api_key_configured": settings.ollama_api_key.is_some(),
    });

    // ── Token usage ────────────────────────────────────────────────────────
    let token_usage = build_json_token_usage(ctx, messages);

    // ── Session metadata ───────────────────────────────────────────────────
    let session_metadata = ctx
        .session_id
        .as_ref()
        .and_then(|id| build_json_session_metadata(id));

    // ── Configuration snapshot ─────────────────────────────────────────────
    let configuration = serde_json::to_value(&settings).unwrap_or(Value::Null);

    // ── Pending images ─────────────────────────────────────────────────────
    let pending_images: Vec<Value> = ctx
        .pending_images
        .iter()
        .enumerate()
        .map(|(i, img)| {
            json!({
                "index": i + 1,
                "path": img.path.display().to_string(),
                "size_bytes": img.size_bytes,
                "mime_type": img.mime_type,
            })
        })
        .collect();

    // ── Command auto-accept lists ──────────────────────────────────────────
    let safe: Vec<String> = settings
        .safe_command_prefixes
        .clone()
        .unwrap_or_else(tinyharness_lib::config::get_default_safe_commands);
    let denied: Vec<String> = settings.denied_command_prefixes.clone().unwrap_or_default();

    let command_auto_accept = json!({
        "auto_accept_mode": settings.auto_accept_mode.to_string(),
        "auto_compact_enabled": settings.auto_compact_enabled,
        "questions_enabled": settings.questions_enabled,
        "safe_command_prefixes": safe,
        "denied_command_prefixes": denied,
    });

    // ── System prompt source ──────────────────────────────────────────────
    let mode = ctx.current_mode;
    let prompts_dir = &ctx.prompts_dir;

    let header_source = if mode.uses_header() {
        let header_path = prompts_dir.join("header.md");
        match std::fs::read_to_string(&header_path) {
            Ok(content) if !content.trim().is_empty() => json!({
                "source": "custom_file",
                "path": header_path.display().to_string(),
                "bytes": content.len(),
            }),
            _ => json!({"source": "hardcoded_default"}),
        }
    } else {
        Value::Null
    };

    let mode_path = prompts_dir.join(mode.prompts_filename());
    let mode_prompt_source = match std::fs::read_to_string(&mode_path) {
        Ok(content) if !content.trim().is_empty() => json!({
            "source": "custom_file",
            "path": mode_path.display().to_string(),
            "bytes": content.len(),
        }),
        _ => json!({
            "source": "hardcoded_default",
            "filename": mode.prompts_filename(),
        }),
    };

    let system_prompt = json!({
        "header": header_source,
        "mode_prompt": mode_prompt_source,
        "assembled": ctx.build_system_prompt(),
    });

    // ── Workspace context ──────────────────────────────────────────────────
    let wctx = &ctx.workspace_ctx;
    let project_md = match &wctx.project_md {
        Some((filename, content)) => json!({
            "filename": filename,
            "bytes": content.len(),
            "content": content,
        }),
        None => Value::Null,
    };

    let additional_mds: Vec<Value> = wctx
        .additional_project_mds
        .iter()
        .map(|(name, content)| {
            json!({ "filename": name, "bytes": content.len(), "content": content })
        })
        .collect();

    let workspace_context = json!({
        "root": wctx.root.display().to_string(),
        "project_type": wctx.project_type,
        "project_name": wctx.project_name,
        "is_git_repo": wctx.is_git_repo,
        "build_command": wctx.build_command,
        "test_command": wctx.test_command,
        "project_md": project_md,
        "additional_project_mds": additional_mds,
        "formatted": wctx.format(),
    });

    // ── Pinned files ───────────────────────────────────────────────────────
    let pinned_summaries = ctx.file_context.pinned_file_summaries();
    let pinned_files: Vec<Value> = pinned_summaries
        .iter()
        .map(|(path, lines, bytes)| {
            json!({
                "path": path,
                "lines": lines,
                "bytes": bytes,
            })
        })
        .collect();

    // ── Skills ──────────────────────────────────────────────────────────────
    let discovered_skills: Vec<Value> = ctx
        .skill_registry
        .skills
        .iter()
        .map(|skill| {
            json!({
                "name": skill.name,
                "description": skill.description,
                "source": skill.source,
                "disable_model_invocation": skill.disable_model_invocation,
                "path": skill.path.display().to_string(),
            })
        })
        .collect();

    let active_skills: Vec<Value> = ctx
        .active_skills
        .iter()
        .filter_map(|name| {
            ctx.skill_registry.get(name).map(|skill| {
                json!({
                    "name": skill.name,
                    "description": skill.description,
                    "source": skill.source,
                    "path": skill.path.display().to_string(),
                    "content": skill.content,
                })
            })
        })
        .collect();

    let skills = json!({
        "discovered": discovered_skills,
        "active": active_skills,
    });

    // ── Messages ───────────────────────────────────────────────────────────
    // Message already serializes with `thinking` and `content` as separate
    // fields, so JSON gives natural separation between thinking and response.
    let messages_json: Vec<Value> = messages
        .iter()
        .map(|m| {
            let tool_calls: Vec<Value> = m
                .tool_calls
                .iter()
                .map(|tc| {
                    json!({
                        "id": tc.id,
                        "name": tc.function.name,
                        "arguments": tc.function.arguments,
                    })
                })
                .collect();

            json!({
                "role": m.role,
                "content": m.content,
                "thinking": m.thinking,
                "tool_calls": tool_calls,
                "tool_call_id": m.tool_call_id,
                "has_images": !m.images.is_empty(),
                "image_count": m.images.len(),
            })
        })
        .collect();

    json!({
        "version": 1,
        "generated_at": chrono_now_or_fallback(),
        "session_info": session_info,
        "provider_diagnostics": provider_diagnostics,
        "token_usage": token_usage,
        "session_metadata": session_metadata,
        "configuration": configuration,
        "pending_images": pending_images,
        "command_auto_accept": command_auto_accept,
        "system_prompt": system_prompt,
        "workspace_context": workspace_context,
        "pinned_files": pinned_files,
        "skills": skills,
        "messages": messages_json,
        "message_count": messages.len(),
    })
}

fn build_json_token_usage(ctx: &CommandContext, messages: &[Message]) -> Value {
    let last_known = ctx.compaction_token_usage.as_ref().map(|u| {
        json!({
            "prompt_tokens": u.prompt_tokens,
            "completion_tokens": u.completion_tokens,
            "total_tokens": u.total_tokens,
        })
    });

    let (cumulative_tool_calls, cumulative_total_tokens) = if let Some(session_id) = &ctx.session_id
    {
        let store = tinyharness_lib::session::SessionStore::default_path();
        store
            .load(session_id)
            .map(|(session, _)| {
                let meta = session.meta();
                (meta.total_tool_calls, meta.total_tokens_used)
            })
            .unwrap_or((0, 0))
    } else {
        (0, 0)
    };

    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    let estimated_tokens = total_chars / 4;

    json!({
        "last_known": last_known,
        "cumulative_tool_calls": cumulative_tool_calls,
        "cumulative_total_tokens": cumulative_total_tokens,
        "estimated_context_tokens": estimated_tokens,
        "estimated_context_chars": total_chars,
    })
}

fn build_json_session_metadata(session_id: &str) -> Option<Value> {
    use tinyharness_lib::session::SessionStore;

    let store = SessionStore::default_path();
    match store.load(session_id) {
        Ok((session, _)) => {
            let meta = session.meta();
            Some(json!({
                "session_id": meta.id,
                "working_dir": meta.working_dir,
                "created_at": format_timestamp(meta.created_at),
                "updated_at": format_timestamp(meta.updated_at),
                "mode": meta.mode.to_string(),
                "provider": meta.provider.to_string(),
                "model": meta.model,
                "name": meta.name,
                "message_count": meta.message_count,
                "token_usage": meta.token_usage.as_ref().map(|u| json!({
                    "prompt_tokens": u.prompt_tokens,
                    "completion_tokens": u.completion_tokens,
                    "total_tokens": u.total_tokens,
                })),
                "cumulative_tool_calls": meta.total_tool_calls,
                "cumulative_total_tokens": meta.total_tokens_used,
            }))
        }
        Err(e) => Some(json!({
            "error": e.to_string(),
        })),
    }
}

// ── Diagnostic helpers ───────────────────────────────────────────────────────

fn dump_provider_diagnostics(file: &mut std::fs::File, _ctx: &CommandContext) {
    let settings = load_settings();

    writeln!(file, "Provider kind: {}", settings.last_provider).unwrap();

    if let Some(url) = settings.get_current_url() {
        writeln!(file, "Provider URL: {}", url).unwrap();
    } else {
        writeln!(file, "Provider URL: (not set)").unwrap();
    }

    writeln!(
        file,
        "Current model: {}",
        settings.get_current_model().unwrap_or("(none)")
    )
    .unwrap();

    writeln!(file, "Timeout: {}s", settings.effective_timeout_secs()).unwrap();
    writeln!(file, "Max retries: {}", settings.effective_max_retries()).unwrap();
    writeln!(file, "Think type: {}", settings.ollama_think_type).unwrap();
    writeln!(
        file,
        "API key configured: {}",
        settings.ollama_api_key.is_some()
    )
    .unwrap();
}

fn dump_token_usage(file: &mut std::fs::File, ctx: &CommandContext, messages: &[Message]) {
    // Last known token usage from compaction or provider response.
    if let Some(usage) = &ctx.compaction_token_usage {
        writeln!(file, "Last known prompt tokens: {}", usage.prompt_tokens).unwrap();
        writeln!(
            file,
            "Last known completion tokens: {}",
            usage.completion_tokens
        )
        .unwrap();
        writeln!(file, "Last known total tokens: {}", usage.total_tokens).unwrap();
    } else {
        writeln!(file, "Last known token usage: (none)").unwrap();
    }

    // Cumulative session stats from session metadata.
    let store = tinyharness_lib::session::SessionStore::default_path();
    if let Some(session_id) = &ctx.session_id
        && let Ok((session, _)) = store.load(session_id)
    {
        let meta = session.meta();
        writeln!(file, "Cumulative tool calls: {}", meta.total_tool_calls).unwrap();
        writeln!(
            file,
            "Cumulative total tokens used: {}",
            meta.total_tokens_used
        )
        .unwrap();
    }

    // Rough estimate: ~4 characters per token across all message contents.
    let total_chars: usize = messages.iter().map(|m| m.content.len()).sum();
    let estimated_tokens = total_chars / 4;
    writeln!(
        file,
        "Estimated context tokens (chars/4): {} ({} chars)",
        estimated_tokens, total_chars
    )
    .unwrap();

    if let Some(limit) = ctx.workspace_ctx.additional_project_mds.first() {
        let _ = limit; // suppress unused warning if context_limit isn't implemented yet
    }
}

fn dump_session_metadata(file: &mut std::fs::File, session_id: &str) {
    use tinyharness_lib::session::SessionStore;

    let store = SessionStore::default_path();
    match store.load(session_id) {
        Ok((session, _messages)) => {
            let meta = session.meta();
            writeln!(file, "Session ID: {}", meta.id).unwrap();
            writeln!(file, "Working directory: {}", meta.working_dir).unwrap();
            writeln!(file, "Created: {}", format_timestamp(meta.created_at)).unwrap();
            writeln!(file, "Last updated: {}", format_timestamp(meta.updated_at)).unwrap();
            writeln!(file, "Stored mode: {}", meta.mode).unwrap();
            writeln!(file, "Stored provider: {}", meta.provider).unwrap();
            writeln!(
                file,
                "Stored model: {}",
                meta.model.as_deref().unwrap_or("(none)")
            )
            .unwrap();
            if let Some(name) = &meta.name {
                writeln!(file, "Session name: {}", name).unwrap();
            }
            writeln!(file, "Message count: {}", meta.message_count).unwrap();
            if let Some(usage) = &meta.token_usage {
                writeln!(
                    file,
                    "Stored token usage — prompt: {}, completion: {}, total: {}",
                    usage.prompt_tokens, usage.completion_tokens, usage.total_tokens
                )
                .unwrap();
            }
            writeln!(file, "Cumulative tool calls: {}", meta.total_tool_calls).unwrap();
            writeln!(
                file,
                "Cumulative total tokens used: {}",
                meta.total_tokens_used
            )
            .unwrap();
        }
        Err(e) => {
            writeln!(file, "Failed to load session metadata: {}", e).unwrap();
        }
    }
}

fn dump_configuration_snapshot(file: &mut std::fs::File) {
    use tinyharness_lib::config::SettingsStore;

    let settings = load_settings();
    let store = SettingsStore::default_path();
    writeln!(file, "Settings file: {}", store.path().display()).unwrap();

    // Dump settings as pretty-printed JSON for diagnostics.
    match serde_json::to_string_pretty(&settings) {
        Ok(json) => {
            writeln!(file, "Settings snapshot:").unwrap();
            writeln!(file, "{}", json).unwrap();
        }
        Err(e) => {
            writeln!(file, "Failed to serialize settings: {}", e).unwrap();
        }
    }
}

fn dump_pending_images(file: &mut std::fs::File, ctx: &CommandContext) {
    if ctx.pending_images.is_empty() {
        writeln!(file, "No images pending attachment.").unwrap();
        return;
    }

    writeln!(file, "Pending images: {}", ctx.pending_images.len()).unwrap();
    for (i, img) in ctx.pending_images.iter().enumerate() {
        writeln!(
            file,
            "  [{}] {} ({} bytes, {})",
            i + 1,
            img.path.display(),
            img.size_bytes,
            img.mime_type
        )
        .unwrap();
    }
}

fn dump_command_lists(file: &mut std::fs::File) {
    use tinyharness_lib::config::get_default_safe_commands;

    let settings = load_settings();

    let safe: Vec<String> = settings
        .safe_command_prefixes
        .clone()
        .unwrap_or_else(get_default_safe_commands);
    let denied: Vec<String> = settings.denied_command_prefixes.clone().unwrap_or_default();

    writeln!(file, "Auto-accept mode: {}", settings.auto_accept_mode).unwrap();
    writeln!(
        file,
        "Auto-compact enabled: {}",
        settings.auto_compact_enabled
    )
    .unwrap();
    writeln!(file, "Questions enabled: {}", settings.questions_enabled).unwrap();
    writeln!(file, "Safe command prefixes ({}):", safe.len()).unwrap();
    for cmd in &safe {
        writeln!(file, "  - {}", cmd).unwrap();
    }
    writeln!(file, "Denied command prefixes ({}):", denied.len()).unwrap();
    for cmd in &denied {
        writeln!(file, "  - {}", cmd).unwrap();
    }
}

fn format_timestamp(unix_secs: u64) -> String {
    use std::time::{Duration, UNIX_EPOCH};
    let dt = std::time::SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(unix_secs))
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs());

    if let Some(secs) = dt {
        let days = secs / 86400;
        let time_of_day = secs % 86400;
        let hours = time_of_day / 3600;
        let minutes = (time_of_day % 3600) / 60;
        let seconds = time_of_day % 60;
        let year = 1970 + days / 365;
        format!(
            "{}-{:02}-{:02} {:02}:{:02}:{:02}",
            year,
            (days % 365) / 30 + 1,
            days % 30 + 1,
            hours,
            minutes,
            seconds
        )
    } else {
        unix_secs.to_string()
    }
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
                tinyharness_lib::provider::AnyProvider::build(
                    tinyharness_lib::config::ProviderKind::Ollama,
                    "http://localhost:11434".to_string(),
                    None,
                    120,
                    0,
                    tinyharness_lib::config::OllamaThinkType::Off,
                    tinyharness_lib::provider::SockudoCredentials::default(),
                )
                .expect("valid test Ollama URL"),
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
                    id: None,
                    function: tinyharness_lib::provider::ToolCallFunction {
                        name: "read".to_string(),
                        arguments: serde_json::json!({"path": "/tmp/test.rs"}),
                        thought_signature: None,
                    },
                }],
                tool_call_id: None,
                images: vec![],
                thinking: None,
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
        assert!(content.contains("=== Provider Diagnostics ==="));
        assert!(content.contains("=== Token Usage ==="));
        assert!(content.contains("=== Configuration Snapshot ==="));
        assert!(content.contains("=== Pending Images ==="));
        assert!(content.contains("=== Command Auto-Accept Lists ==="));
        assert!(content.contains("=== System Prompt Source ==="));
        assert!(content.contains("=== Workspace Context ==="));
        assert!(content.contains("=== Pinned Files ==="));
        assert!(content.contains("=== Skills ==="));
    }

    #[test]
    fn test_execute_includes_thinking() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: "Here is my answer.".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            images: vec![],
            thinking: Some("Let me reason about this...".to_string()),
        }];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug-thinking.log");
        let path_str = path.to_string_lossy().to_string();

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&path_str), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("[Thinking]"));
        assert!(content.contains("[End Thinking]"));
        assert!(content.contains("[Response]"));
        assert!(content.contains("[End Response]"));
        assert!(content.contains("Let me reason about this..."));
        assert!(content.contains("Here is my answer."));
    }

    #[test]
    fn test_json_dumps_valid_json() {
        let messages = vec![
            Message::simple(Role::System, "You are helpful."),
            Message::simple(Role::User, "Hello"),
            Message::simple(Role::Assistant, "Hi there!"),
        ];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug.json");
        let arg = format!("--json {}", path.to_string_lossy());

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&arg), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&content).expect("output must be valid JSON");

        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["message_count"], 3);
        assert_eq!(parsed["messages"][0]["role"], "System");
        assert_eq!(parsed["messages"][0]["content"], "You are helpful.");
        assert_eq!(parsed["messages"][1]["role"], "User");
        assert_eq!(parsed["messages"][2]["content"], "Hi there!");
    }

    #[test]
    fn test_json_separates_thinking_and_response() {
        let messages = vec![Message {
            role: Role::Assistant,
            content: "Here is my answer.".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            images: vec![],
            thinking: Some("Let me reason about this...".to_string()),
        }];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug-thinking.json");
        let arg = format!("--json {}", path.to_string_lossy());

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&arg), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&content).expect("output must be valid JSON");

        // thinking and content are separate JSON fields
        assert_eq!(
            parsed["messages"][0]["thinking"],
            "Let me reason about this..."
        );
        assert_eq!(parsed["messages"][0]["content"], "Here is my answer.");
    }

    #[test]
    fn test_json_includes_tool_calls() {
        use tinyharness_lib::provider::ToolCall;

        let messages = vec![
            Message::simple(Role::User, "Read the file"),
            Message {
                role: Role::Assistant,
                content: "I'll read that file.".to_string(),
                tool_calls: vec![ToolCall {
                    id: Some("call_1".to_string()),
                    function: tinyharness_lib::provider::ToolCallFunction {
                        name: "read".to_string(),
                        arguments: serde_json::json!({"path": "/tmp/test.rs"}),
                        thought_signature: None,
                    },
                }],
                tool_call_id: None,
                images: vec![],
                thinking: None,
            },
            Message::simple(Role::Tool, "file contents here"),
        ];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug-tools.json");
        let arg = format!("--json {}", path.to_string_lossy());

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&arg), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&content).expect("output must be valid JSON");

        assert_eq!(parsed["messages"][1]["tool_calls"][0]["name"], "read");
        assert_eq!(parsed["messages"][1]["tool_calls"][0]["id"], "call_1");
    }

    #[test]
    fn test_json_includes_top_level_sections() {
        let messages = vec![Message::simple(Role::User, "test")];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug-sections.json");
        let arg = format!("--json {}", path.to_string_lossy());

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&arg), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: Value = serde_json::from_str(&content).expect("output must be valid JSON");

        // All major sections should be present
        assert!(parsed.get("session_info").is_some());
        assert!(parsed.get("provider_diagnostics").is_some());
        assert!(parsed.get("token_usage").is_some());
        assert!(parsed.get("configuration").is_some());
        assert!(parsed.get("pending_images").is_some());
        assert!(parsed.get("command_auto_accept").is_some());
        assert!(parsed.get("system_prompt").is_some());
        assert!(parsed.get("workspace_context").is_some());
        assert!(parsed.get("pinned_files").is_some());
        assert!(parsed.get("skills").is_some());
        assert!(parsed.get("messages").is_some());
    }

    #[test]
    fn test_json_flag_after_path() {
        let messages = vec![Message::simple(Role::User, "test")];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug-order.json");
        let arg = format!("{} --json", path.to_string_lossy());

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&arg), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        serde_json::from_str::<Value>(&content).expect("output must be valid JSON");
    }

    #[test]
    fn test_short_json_flag() {
        let messages = vec![Message::simple(Role::User, "test")];

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("debug-short.json");
        let arg = format!("-j {}", path.to_string_lossy());

        let mut ctx = make_test_ctx();
        let result = execute(&mut ctx, Some(&arg), &messages);
        assert!(result.is_ok());

        let content = std::fs::read_to_string(&path).unwrap();
        serde_json::from_str::<Value>(&content).expect("output must be valid JSON");
    }

    #[test]
    fn test_parse_args_defaults() {
        let args = parse_args(None);
        assert!(!args.json);
        assert!(args.path.is_none());
    }

    #[test]
    fn test_parse_args_json_only() {
        let args = parse_args(Some("--json"));
        assert!(args.json);
        assert!(args.path.is_none());
    }

    #[test]
    fn test_parse_args_path_and_json() {
        let args = parse_args(Some("/tmp/debug.json --json"));
        assert!(args.json);
        assert_eq!(args.path.as_deref(), Some("/tmp/debug.json"));

        let args = parse_args(Some("--json /tmp/debug.json"));
        assert!(args.json);
        assert_eq!(args.path.as_deref(), Some("/tmp/debug.json"));
    }

    #[test]
    fn test_parse_args_path_only() {
        let args = parse_args(Some("/tmp/debug.log"));
        assert!(!args.json);
        assert_eq!(args.path.as_deref(), Some("/tmp/debug.log"));
    }
}
