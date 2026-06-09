// ── Shared Command Result Handling ─────────────────────────────────────────
//
// When slash commands return `CommandResult` variants, both CLI and TUI loops
// need to apply the same state mutations (session switches, skill activation,
// etc.). This module extracts that shared logic.
//
// Each handler returns a `CommandResultInfo` describing what happened, so
// callers can render appropriate output for their UI.

use tinyharness_lib::{
    context::WorkspaceContext,
    provider::{Message, Role},
    session::Session,
};

use crate::commands::CommandContext;

/// Information about what happened when a command result was applied.
///
/// Callers use this to render appropriate output (CLI: ANSI text, TUI: events).
#[derive(Debug)]
pub struct CommandResultInfo {
    /// A human-readable summary of what happened.
    pub description: String,
    /// Whether this was an error.
    pub is_error: bool,
}

/// Apply a `SwitchSession` command result.
///
/// Performs the session switch (flush, load, update context), returns
/// a description of what happened.
pub fn apply_switch_session(
    id_prefix: &str,
    ctx: &mut CommandContext,
    messages: &mut Vec<Message>,
    session: &mut Session,
) -> CommandResultInfo {
    let store = tinyharness_lib::session::SessionStore::default_path();
    match store.find_by_prefix(id_prefix) {
        Ok(full_id) => {
            session.flush();
            match store.load(&full_id) {
                Ok((new_session, loaded_msgs)) => {
                    let id_short = full_id[..12].to_string();
                    let name = new_session
                        .meta()
                        .name
                        .clone()
                        .unwrap_or_else(|| "unnamed".to_string());
                    let msg_count = new_session.meta().message_count;
                    let mode = new_session.meta().mode;

                    *session = new_session;
                    *messages = loaded_msgs;
                    ctx.current_mode = session.meta().mode;
                    ctx.session_id = Some(session.id().to_string());
                    ctx.refresh_system_prompt(messages);

                    CommandResultInfo {
                        description: format!(
                            "Switched to session {} — {} ({} messages, {})",
                            id_short, name, msg_count, mode
                        ),
                        is_error: false,
                    }
                }
                Err(e) => CommandResultInfo {
                    description: e.to_string(),
                    is_error: true,
                },
            }
        }
        Err(e) => CommandResultInfo {
            description: e.to_string(),
            is_error: true,
        },
    }
}

/// Apply a `RenameSession` command result.
pub fn apply_rename_session(new_name: &str, session: &mut Session) -> CommandResultInfo {
    session.set_name(new_name.to_string());
    CommandResultInfo {
        description: format!("Session renamed to {}", new_name),
        is_error: false,
    }
}

/// Apply an `Init` command result.
pub fn apply_init(
    result: &crate::commands::init::InitResult,
    ctx: &mut CommandContext,
    messages: &mut [Message],
) -> CommandResultInfo {
    ctx.workspace_ctx = WorkspaceContext::collect();
    ctx.refresh_system_prompt(messages);
    match result {
        crate::commands::init::InitResult::Created { path } => CommandResultInfo {
            description: format!("Created {} — workspace context refreshed.", path.display()),
            is_error: false,
        },
        crate::commands::init::InitResult::Updated { path } => CommandResultInfo {
            description: format!("Updated {} — workspace context refreshed.", path.display()),
            is_error: false,
        },
    }
}

/// Apply a `SkillUse` command result.
pub fn apply_skill_use(
    skill_name: &str,
    ctx: &mut CommandContext,
    messages: &mut Vec<Message>,
    session: &mut Session,
) -> CommandResultInfo {
    // Prevent duplicate activation
    if ctx
        .active_skills
        .iter()
        .any(|s| s.eq_ignore_ascii_case(skill_name))
    {
        return CommandResultInfo {
            description: format!(
                "Skill '{}' is already active. Use /unload {} to deactivate it.",
                skill_name, skill_name
            ),
            is_error: false, // not an error per se, but a warning
        };
    }

    match ctx.skill_registry.get(skill_name) {
        Some(skill) => {
            ctx.active_skills.push(skill.name.clone());
            messages.push(Message {
                role: Role::User,
                content: format!("/use {}", skill_name),
                tool_calls: vec![],
                images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
            ctx.refresh_system_prompt(messages);

            CommandResultInfo {
                description: format!("Skill activated: {} — {}", skill_name, skill.description),
                is_error: false,
            }
        }
        None => CommandResultInfo {
            description: format!(
                "Skill '{}' not found — it may have been removed.",
                skill_name
            ),
            is_error: true,
        },
    }
}

/// Apply a `SkillUnload` command result.
pub fn apply_skill_unload(
    skill_name: &str,
    ctx: &mut CommandContext,
    messages: &mut Vec<Message>,
    session: &mut Session,
) -> CommandResultInfo {
    let pos = ctx
        .active_skills
        .iter()
        .position(|s| s.eq_ignore_ascii_case(skill_name));

    match pos {
        Some(idx) => {
            let removed = ctx.active_skills.remove(idx);
            messages.push(Message {
                role: Role::User,
                content: format!("/unload {}", skill_name),
                tool_calls: vec![],
                images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
            ctx.refresh_system_prompt(messages);

            CommandResultInfo {
                description: format!("Skill deactivated: {}", removed),
                is_error: false,
            }
        }
        None => CommandResultInfo {
            description: format!("Skill '{}' is not active.", skill_name),
            is_error: false,
        },
    }
}

/// Apply a generic `CommandResult::Ok` — update token usage if needed.
///
/// Returns the updated token usage, if any.
pub fn apply_ok(
    ctx: &mut CommandContext,
    session: &mut Session,
) -> Option<tinyharness_lib::provider::TokenUsage> {
    let usage = ctx.compaction_token_usage.take();
    if let Some(ref u) = usage {
        session.set_token_usage(u.clone());
    }
    usage
}
