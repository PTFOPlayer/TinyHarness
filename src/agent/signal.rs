// ── Shared Signal Handling ──────────────────────────────────────────────────
//
// Signal tools (switch_mode, question, auto_compact, invoke_skill) produce
// structured side-effects on the conversation state. Both the CLI and TUI loops
// handle the same state mutations — the only difference is how they report
// results to the user.
//
// This module extracts the business logic into pure functions that return
// `SignalResult`, so both loops can share the mutation code and only differ
// in rendering.

use tinyharness_lib::{
    mode::AgentMode,
    provider::{Message, Role},
    session::Session,
    tools::SignalEvent,
};
use tokio::sync::Mutex;

use crate::commands::CommandContext;
use crate::commands::compact::execute_compact;

// ── SignalResult ─────────────────────────────────────────────────────────────

/// Structured result from handling a signal tool event.
///
/// Contains the state mutations that have already been applied (messages pushed,
/// session updated, mode changed) along with information the caller needs for
/// rendering to the user.
#[derive(Debug)]
pub enum SignalResult {
    SwitchMode {
        old_mode: AgentMode,
        new_mode: AgentMode,
        already_in: bool,
    },
    Question {
        /// The answer selected or entered by the user.
        /// `None` means the question was skipped (no answer provided).
        answer: Option<String>,
        /// Whether the user selected one of the provided options (vs free-form).
        selected_provided: bool,
    },
    AutoCompact {
        focus: String,
        success: bool,
        error: Option<String>,
    },
    InvokeSkill {
        name: String,
        description: String,
        already_active: bool,
        found: bool,
    },
    /// The signal event arguments could not be parsed.
    ParseError { tool_name: String },
}

// ── Handle signal event ─────────────────────────────────────────────────────

/// Handle a signal tool event, mutating conversation state and returning a
/// structured result for the caller to render.
///
/// This function:
/// - Pushes appropriate `Message`s into the conversation
/// - Appends them to the session
/// - Updates `CommandContext` state (mode, skills, compaction token usage)
/// - Refreshes the system prompt when needed
///
/// The caller is responsible for rendering the result to the user (CLI: ANSI
/// output; TUI: channel events).
#[allow(clippy::too_many_arguments)]
pub async fn handle_signal_event(
    event: &SignalEvent,
    messages: &mut Vec<Message>,
    session: &mut Session,
    ctx: &mut CommandContext,
    provider: &std::sync::Arc<Mutex<dyn tinyharness_lib::provider::Provider + Send + Sync>>,
    tool_call_id: &str,
) -> SignalResult {
    match event {
        SignalEvent::SwitchMode { mode } => {
            let old_mode = ctx.current_mode;
            match ctx.switch_mode(*mode, messages) {
                Ok(()) => {
                    session.set_mode(*mode);
                    messages.push(Message {
                        role: Role::Tool,
                        content: format!(
                            "SUCCESS: Mode switched from '{}' to '{}'. The assistant is now in {} mode and will use the appropriate toolset and behavior.",
                            old_mode, mode, mode
                        ),
                        tool_calls: vec![],
                        tool_call_id: Some(tool_call_id.to_string()),
                        images: vec![],
                    });
                    session.append_message(messages.last().expect("just pushed a message"));
                    SignalResult::SwitchMode {
                        old_mode,
                        new_mode: *mode,
                        already_in: false,
                    }
                }
                Err(_msg) => {
                    messages.push(Message {
                        role: Role::Tool,
                        content: format!("Already in '{}' mode. No change was made.", mode),
                        tool_calls: vec![],
                        tool_call_id: Some(tool_call_id.to_string()),
                        images: vec![],
                    });
                    session.append_message(messages.last().expect("just pushed a message"));
                    SignalResult::SwitchMode {
                        old_mode,
                        new_mode: *mode,
                        already_in: true,
                    }
                }
            }
        }

        SignalEvent::Question { question, answers } => {
            // Question signals require user interaction (prompting the user for
            // input) which can't be done here — the caller must handle the I/O
            // directly. Validate the question and return an error if invalid,
            // otherwise return a marker result indicating the caller should
            // handle the question.
            if let Some(error) = validate_question(question, answers) {
                apply_question_error(error, messages, session, tool_call_id);
            }
            // The caller should handle the question before calling this function.
            // This branch should never be reached in practice.
            SignalResult::Question {
                answer: None,
                selected_provided: false,
            }
        }

        SignalEvent::AutoCompact { focus } => {
            let mut provider_guard = provider.lock().await;
            match execute_compact(&mut ctx.output, &mut *provider_guard, messages, focus).await {
                Ok(token_usage) => {
                    if let Some(usage) = token_usage.clone() {
                        ctx.compaction_token_usage = Some(usage.clone());
                        session.set_token_usage(usage);
                    }
                    messages.push(Message {
                        role: Role::Tool,
                        content: format!(
                            "Conversation compacted successfully. Focus: '{}'.",
                            if focus.is_empty() {
                                "general summary"
                            } else {
                                focus
                            }
                        ),
                        tool_calls: vec![],
                        tool_call_id: Some(tool_call_id.to_string()),
                        images: vec![],
                    });
                    session.append_message(messages.last().expect("just pushed a message"));
                    SignalResult::AutoCompact {
                        focus: focus.clone(),
                        success: true,
                        error: None,
                    }
                }
                Err(e) => {
                    messages.push(Message {
                        role: Role::Tool,
                        content: format!(
                            "Auto-compact failed: {}. The conversation was not modified.",
                            e
                        ),
                        tool_calls: vec![],
                        tool_call_id: Some(tool_call_id.to_string()),
                        images: vec![],
                    });
                    session.append_message(messages.last().expect("just pushed a message"));
                    SignalResult::AutoCompact {
                        focus: focus.clone(),
                        success: false,
                        error: Some(e.to_string()),
                    }
                }
            }
        }

        SignalEvent::InvokeSkill { skill_name } => {
            let skill_result = {
                let registry = &ctx.skill_registry;
                registry
                    .get(skill_name)
                    .map(|s| (s.name.clone(), s.description.clone()))
            };
            match skill_result {
                Some((name, description)) => {
                    if ctx
                        .active_skills
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(&name))
                    {
                        messages.push(Message {
                            role: Role::Tool,
                            content: format!("Skill '{}' is already active. Its instructions are already in effect.", name),
                            tool_calls: vec![],
                            tool_call_id: Some(tool_call_id.to_string()),
                            images: vec![],
                        });
                        session.append_message(messages.last().expect("just pushed a message"));
                        SignalResult::InvokeSkill {
                            name,
                            description,
                            already_active: true,
                            found: true,
                        }
                    } else {
                        ctx.active_skills.push(name.clone());
                        messages.push(Message {
                            role: Role::User,
                            content: format!("/use {}", skill_name),
                            tool_calls: vec![],
                            tool_call_id: None,
                            images: vec![],
                        });
                        session.append_message(messages.last().expect("just pushed a message"));
                        ctx.refresh_system_prompt(messages);
                        SignalResult::InvokeSkill {
                            name,
                            description,
                            already_active: false,
                            found: true,
                        }
                    }
                }
                None => {
                    let available = ctx
                        .skill_registry
                        .skills
                        .iter()
                        .map(|s| s.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ");
                    messages.push(Message {
                        role: Role::Tool,
                        content: format!(
                            "Error: Skill '{}' not found. Available skills: {}. Use /skills to list them.",
                            skill_name, available
                        ),
                        tool_calls: vec![],
                        tool_call_id: Some(tool_call_id.to_string()),
                        images: vec![],
                    });
                    session.append_message(messages.last().expect("just pushed a message"));
                    SignalResult::InvokeSkill {
                        name: skill_name.clone(),
                        description: String::new(),
                        already_active: false,
                        found: false,
                    }
                }
            }
        }
    }
}

/// Apply the user's answer to a question signal event.
///
/// This is called after the caller has obtained the user's answer through
/// its own I/O mechanism (CLI prompt or TUI channel).
pub fn apply_question_answer(
    question: &str,
    answer: &str,
    is_skip: bool,
    messages: &mut Vec<Message>,
    session: &mut Session,
    tool_call_id: &str,
) {
    let result_content = if is_skip {
        format!(
            "User skipped the provided options for the question '{}' and entered a custom answer: '{}'.\n\nUse this answer to continue helping the user.",
            question, answer
        )
    } else {
        format!(
            "User answered the question '{}' with: '{}'.\n\nUse this answer to continue helping the user.",
            question, answer
        )
    };
    messages.push(Message {
        role: Role::Tool,
        content: result_content,
        tool_calls: vec![],
        tool_call_id: Some(tool_call_id.to_string()),
        images: vec![],
    });
    session.append_message(messages.last().expect("just pushed a message"));
}

/// Handle the question signal event for validation.
///
/// Returns `Some(error_message)` if validation fails (empty question or no answers),
/// `None` if the question is valid and the caller should proceed with user interaction.
pub fn validate_question(question: &str, answers: &[String]) -> Option<String> {
    if question.is_empty() {
        Some("Error: 'question' argument is required for the question tool.".to_string())
    } else if answers.is_empty() {
        Some(
            "Error: 'answers' argument must contain at least one option for the question tool."
                .to_string(),
        )
    } else {
        None
    }
}

/// Apply validation error for question signal as a message.
pub fn apply_question_error(
    error: String,
    messages: &mut Vec<Message>,
    session: &mut Session,
    tool_call_id: &str,
) {
    messages.push(Message {
        role: Role::Tool,
        content: error,
        tool_calls: vec![],
        tool_call_id: Some(tool_call_id.to_string()),
        images: vec![],
    });
    session.append_message(messages.last().expect("just pushed a message"));
}

/// Apply a parse error for an unparseable signal tool.
pub fn apply_signal_parse_error(
    tool_name: &str,
    messages: &mut Vec<Message>,
    session: &mut Session,
    tool_call_id: &str,
) {
    messages.push(Message {
        role: Role::Tool,
        content: format!(
            "Error: Could not parse arguments for signal tool '{}'.",
            tool_name
        ),
        tool_calls: vec![],
        tool_call_id: Some(tool_call_id.to_string()),
        images: vec![],
    });
    session.append_message(messages.last().expect("just pushed a message"));
}
