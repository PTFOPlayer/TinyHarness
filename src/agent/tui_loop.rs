// ── TUI Agent Loop ─────────────────────────────────────────────────────────────
//
// Background agent loop for TUI mode. Communicates with the TUI via channels:
// - Receives `TuiUserAction` (user messages, confirmations) from the TUI
// - Sends `TuiAgentEvent` (streaming text, tool calls, status) to the TUI
//
// This is a simplified version of the CLI agent loop that:
// - Reads user input from a channel instead of rustyline
// - Sends UI updates to a channel instead of writing to stdout
// - Auto-approves read-only tool calls (no interactive confirmation)
// - Handles slash commands via the command registry

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use tokio::sync::Mutex;

use tinyharness_lib::{
    config::load_settings,
    provider::{Message, Provider, Role},
    session::Session,
    token::{ContextWindowSize, check_context_warning},
    tools::{SignalEvent, ToolManager},
};
use tinyharness_ui::output::Output;
use tinyharness_ui::tui::{TuiAgentEvent, TuiUserAction};

use crate::commands::compact::execute_compact;
use crate::commands::{CommandContext, CommandResult, build_registry};

use super::display::format_args_summary_tui;
use super::safety::is_safe_command;

/// Strip common ANSI SGR escape sequences from a string.
///
/// In TUI mode, command output contains ANSI color/style codes meant for a
/// terminal. Since the TUI renders its own styling, we strip these codes before
/// sending the output as a system message.
fn strip_ansi_sgr(s: &str) -> String {
    // Strip CSI sequences: ESC [ ... m  (SGR) and ESC [ ... letter (other CSI)
    let re = regex::Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap();
    re.replace_all(s, "").to_string()
}

/// Check context window usage and send a warning event if thresholds are exceeded.
fn send_context_warning_if_needed(
    token_count: u32,
    context_size: ContextWindowSize,
    agent_event_tx: &mpsc::Sender<TuiAgentEvent>,
) {
    if let Some(warning) = check_context_warning(token_count, context_size) {
        let _ = agent_event_tx.send(TuiAgentEvent::ContextWarning {
            percentage: warning.percentage(),
            critical: warning.is_critical(),
        });
    }
}

/// A writer that captures output into a shared buffer, allowing the captured
/// text to be retrieved later. Used in TUI mode to intercept command output
/// that would otherwise go to stdout (which is invisible in alternate-screen mode).
#[derive(Clone)]
struct CaptureWriter {
    buf: Arc<std::sync::Mutex<Vec<u8>>>,
}

impl CaptureWriter {
    fn new() -> Self {
        Self {
            buf: Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn take_output(&self) -> Vec<u8> {
        let mut buf = self.buf.lock().unwrap();
        std::mem::take(&mut *buf)
    }
}

impl std::io::Write for CaptureWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        let mut buf = self.buf.lock().unwrap();
        buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

// Safety: CaptureWriter uses Arc<Mutex<Vec<u8>>> which is safe to send across threads.
// The Mutex<Vec<u8>> is always locked for short durations (single write/flush calls).
unsafe impl Send for CaptureWriter {}

/// Execute a slash command with output captured and redirected to the TUI.
///
/// This replaces `ctx.output` with a buffer-backed writer, dispatches the
/// command, then sends whatever the command wrote (with ANSI codes stripped)
/// as `TuiAgentEvent::SystemMessage` events. This ensures slash command output
/// is visible in the TUI conversation pane instead of being written to the
/// raw terminal (which would be invisible or garbled in alternate-screen mode).
#[allow(clippy::too_many_arguments)]
async fn dispatch_command_to_tui(
    input: &str,
    ctx: &mut CommandContext,
    messages: &mut Vec<Message>,
    registry: &crate::commands::CommandRegistry,
    agent_event_tx: &mpsc::Sender<TuiAgentEvent>,
) -> Result<CommandResult, String> {
    // Swap ctx.output with a buffer-backed writer to capture command output
    let capture = CaptureWriter::new();
    let captured_output = Output::new(Box::new(capture.clone()));
    let original_output = std::mem::replace(&mut ctx.output, captured_output);

    let result = registry.dispatch(input, ctx, messages).await;

    // Replace ctx.output back and extract the captured bytes
    let _restored = std::mem::replace(&mut ctx.output, original_output);

    let output_bytes = capture.take_output();
    let output_text = String::from_utf8_lossy(&output_bytes);
    let stripped = strip_ansi_sgr(&output_text);

    if !stripped.is_empty() {
        let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(stripped.to_string()));
    }

    result
}

/// Spinner frames used during tool execution (same as CLI mode).
#[allow(dead_code)]
const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Background agent loop for TUI mode.
///
/// This function runs in a background tokio task. It:
/// 1. Waits for user messages from the TUI input bar
/// 2. Processes slash commands locally
/// 3. Sends messages to the LLM provider
/// 4. Streams responses back to the TUI
/// 5. Handles tool calls (auto-approving read-only ones)
///
/// It communicates with the TUI via `mpsc` channels.
#[allow(clippy::too_many_arguments)]
pub async fn run_tui_agent_loop(
    provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    tool_manager: ToolManager,
    mut messages: Vec<Message>,
    mut ctx: CommandContext,
    mut session: Session,
    interrupted: Arc<AtomicBool>,
    initial_prompt: Option<String>,
    mut user_action_rx: mpsc::Receiver<TuiUserAction>,
    agent_event_tx: mpsc::Sender<TuiAgentEvent>,
) -> Result<(), String> {
    let registry = build_registry();

    let settings = load_settings();
    let context_size = settings
        .context_limit
        .map(ContextWindowSize::Custom)
        .unwrap_or_else(ContextWindowSize::default_size);

    let mut last_known_token_usage: Option<tinyharness_lib::provider::TokenUsage> =
        session.meta().token_usage.clone();

    // Send initial state to the TUI
    let model_name = {
        let p = provider.lock().await;
        p.current_model().unwrap_or_else(|| "unknown".to_string())
    };
    let _ = agent_event_tx.send(TuiAgentEvent::ModelChanged(model_name));
    let _ = agent_event_tx.send(TuiAgentEvent::ModeChanged(ctx.current_mode.to_string()));

    if let Some(ref usage) = last_known_token_usage {
        let _ = agent_event_tx.send(TuiAgentEvent::TokenUpdate {
            count: usage.total_tokens as u64,
            limit: Some(context_size.tokens() as u64),
        });
        send_context_warning_if_needed(usage.total_tokens, context_size, &agent_event_tx);
    }

    // Handle initial prompt (from --prompt flag)
    if let Some(prompt) = initial_prompt
        && !prompt.trim().is_empty()
    {
        // Process the initial prompt as if the user sent it
        process_user_message(
            &prompt,
            &mut messages,
            &mut ctx,
            &mut session,
            &provider,
            &tool_manager,
            &registry,
            &interrupted,
            &agent_event_tx,
            &mut user_action_rx,
            &mut last_known_token_usage,
            context_size,
        )
        .await;
    }

    // Main loop: wait for user actions from the TUI
    loop {
        let action = match user_action_rx.recv() {
            Ok(action) => action,
            Err(_) => {
                // Channel closed — TUI exited
                let _ = agent_event_tx.send(TuiAgentEvent::Done);
                break;
            }
        };

        match action {
            TuiUserAction::Quit => {
                let _ = agent_event_tx.send(TuiAgentEvent::Done);
                break;
            }
            TuiUserAction::SendMessage(text) => {
                if text.trim().is_empty() {
                    continue;
                }

                // Check if it's a slash command
                if text.starts_with('/') {
                    process_slash_command(
                        &text,
                        &mut messages,
                        &mut ctx,
                        &mut session,
                        &provider,
                        &registry,
                        &interrupted,
                        &agent_event_tx,
                        &mut last_known_token_usage,
                        context_size,
                    )
                    .await;

                    if ctx.exit_requested {
                        let _ = agent_event_tx.send(TuiAgentEvent::Done);
                        break;
                    }
                    continue;
                }

                process_user_message(
                    &text,
                    &mut messages,
                    &mut ctx,
                    &mut session,
                    &provider,
                    &tool_manager,
                    &registry,
                    &interrupted,
                    &agent_event_tx,
                    &mut user_action_rx,
                    &mut last_known_token_usage,
                    context_size,
                )
                .await;
            }
            TuiUserAction::ConfirmResponse { .. } => {
                // Confirmation responses are handled inline during tool execution
                // This branch shouldn't normally be reached in the main loop
                // since confirmations are handled synchronously in tool processing
            }
            TuiUserAction::QuestionAnswer(_) => {
                // Same as above — handled inline during question signal processing
            }
            TuiUserAction::Interrupt => {
                interrupted.store(true, Ordering::SeqCst);
            }
        }
    }

    // Flush session on exit
    session.flush();

    Ok(())
}

/// Process a slash command in TUI mode.
#[allow(clippy::too_many_arguments)]
async fn process_slash_command(
    input: &str,
    messages: &mut Vec<Message>,
    ctx: &mut CommandContext,
    session: &mut Session,
    _provider: &Arc<Mutex<dyn Provider + Send + Sync>>,
    registry: &crate::commands::CommandRegistry,
    _interrupted: &Arc<AtomicBool>,
    agent_event_tx: &mpsc::Sender<TuiAgentEvent>,
    last_known_token_usage: &mut Option<tinyharness_lib::provider::TokenUsage>,
    _context_size: ContextWindowSize,
) {
    match dispatch_command_to_tui(input, ctx, messages, registry, agent_event_tx).await {
        Ok(CommandResult::Ok) => {
            // Update token usage from compaction side-channel
            if let Some(usage) = ctx.compaction_token_usage.take() {
                *last_known_token_usage = Some(usage.clone());
                session.set_token_usage(usage);
            }
            // Send mode update if it changed
            let _ = agent_event_tx.send(TuiAgentEvent::ModeChanged(ctx.current_mode.to_string()));
        }
        Ok(CommandResult::SwitchSession(id_prefix)) => {
            let store = tinyharness_lib::session::SessionStore::default_path();
            match store.find_by_prefix(&id_prefix) {
                Ok(full_id) => {
                    session.flush();
                    match store.load(&full_id) {
                        Ok((new_session, loaded_msgs)) => {
                            *session = new_session;
                            *messages = loaded_msgs;
                            ctx.current_mode = session.meta().mode;
                            ctx.session_id = Some(session.id().to_string());
                            *last_known_token_usage = session.meta().token_usage.clone();
                            ctx.refresh_system_prompt(messages);

                            let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                                "Switched to session {}",
                                &full_id[..12]
                            )));
                            let _ = agent_event_tx
                                .send(TuiAgentEvent::ModeChanged(ctx.current_mode.to_string()));
                        }
                        Err(e) => {
                            let _ = agent_event_tx.send(TuiAgentEvent::Error(format!("{}", e)));
                        }
                    }
                }
                Err(e) => {
                    let _ = agent_event_tx.send(TuiAgentEvent::Error(format!("{}", e)));
                }
            }
        }
        Ok(CommandResult::RenameSession(new_name)) => {
            session.set_name(new_name.clone());
            let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                "Session renamed to {}",
                new_name
            )));
        }
        Ok(CommandResult::Init(result)) => {
            ctx.workspace_ctx = tinyharness_lib::context::WorkspaceContext::collect();
            ctx.refresh_system_prompt(messages);
            let msg = match &result {
                crate::commands::init::InitResult::Created { path } => {
                    format!("Created {}", path.display())
                }
                crate::commands::init::InitResult::Updated { path } => {
                    format!("Updated {}", path.display())
                }
            };
            let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(msg));
        }
        Ok(CommandResult::SkillUse(skill_name)) => {
            if ctx
                .active_skills
                .iter()
                .any(|s| s.eq_ignore_ascii_case(&skill_name))
            {
                let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                    "Skill '{}' is already active",
                    skill_name
                )));
                return;
            }
            match ctx.skill_registry.get(&skill_name) {
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
                    let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                        "Skill activated: {} — {}",
                        skill_name, skill.description
                    )));
                }
                None => {
                    let _ = agent_event_tx.send(TuiAgentEvent::Error(format!(
                        "Skill '{}' not found",
                        skill_name
                    )));
                }
            }
        }
        Ok(CommandResult::SkillUnload(skill_name)) => {
            let pos = ctx
                .active_skills
                .iter()
                .position(|s| s.eq_ignore_ascii_case(&skill_name));
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
                    let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                        "Skill deactivated: {}",
                        removed
                    )));
                }
                None => {
                    let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                        "Skill '{}' is not active",
                        skill_name
                    )));
                }
            }
        }
        Err(e) => {
            let _ = agent_event_tx.send(TuiAgentEvent::Error(e));
        }
    }
}

/// Process a user message in TUI mode: send to LLM, stream response, handle tools.
#[allow(clippy::too_many_arguments)]
async fn process_user_message(
    text: &str,
    messages: &mut Vec<Message>,
    ctx: &mut CommandContext,
    session: &mut Session,
    provider: &Arc<Mutex<dyn Provider + Send + Sync>>,
    tool_manager: &ToolManager,
    _registry: &crate::commands::CommandRegistry,
    interrupted: &Arc<AtomicBool>,
    agent_event_tx: &mpsc::Sender<TuiAgentEvent>,
    user_action_rx: &mut mpsc::Receiver<TuiUserAction>,
    last_known_token_usage: &mut Option<tinyharness_lib::provider::TokenUsage>,
    context_size: ContextWindowSize,
) {
    let pending_images = std::mem::take(&mut ctx.pending_images);
    messages.push(Message {
        role: Role::User,
        content: text.to_string(),
        tool_calls: vec![],
        images: pending_images,
    });

    // Auto-save: user message
    session.append_message(messages.last().expect("just pushed a message"));

    let mut auto_accept = false;

    loop {
        // Clear interrupt flag for this turn
        interrupted.store(false, Ordering::SeqCst);

        // Filter tools based on current mode
        let tools = tool_manager.tools_for_mode(ctx.current_mode);

        // Call the provider
        let mut recv = {
            let mut p = provider.lock().await;
            match p.chat(messages.clone(), tools).await {
                Ok(recv) => recv,
                Err(e) => {
                    let _ = agent_event_tx.send(TuiAgentEvent::Error(format!(
                        "Failed to start request: {}",
                        e
                    )));
                    // Remove the user message we just added
                    messages.pop();
                    return;
                }
            }
        };

        let mut response_content = String::new();
        let mut tool_calls: Vec<tinyharness_lib::provider::ToolCall> = Vec::new();
        let mut received_done = false;
        let mut is_error = false;

        // Notify TUI that streaming has started
        let _ = agent_event_tx.send(TuiAgentEvent::StreamingStarted);

        loop {
            tokio::select! {
                msg = recv.recv() => {
                    match msg {
                        Some(msg) => {
                            if !msg.message.tool_calls.is_empty() {
                                tool_calls = msg.message.tool_calls.clone();
                            }

                            if msg.done {
                                received_done = true;
                                if let Some(ref usage) = msg.usage {
                                    *last_known_token_usage = Some(usage.clone());
                                    session.set_token_usage(usage.clone());
                                    let _ = agent_event_tx.send(TuiAgentEvent::TokenUpdate {
                                        count: usage.total_tokens as u64,
                                        limit: Some(context_size.tokens() as u64),
                                    });
                                    send_context_warning_if_needed(
                                        usage.total_tokens,
                                        context_size,
                                        agent_event_tx,
                                    );
                                }
                            }

                            if msg.is_error {
                                is_error = true;
                            }

                            // Send thinking content if present
                            if let Some(ref thinking) = msg.message.thinking
                                && !thinking.is_empty()
                                && ctx.show_thinking
                            {
                                let _ = agent_event_tx.send(TuiAgentEvent::StreamingThinking(
                                    thinking.clone(),
                                ));
                            }

                            // Send content chunks
                            if !msg.message.content.is_empty() {
                                response_content.push_str(&msg.message.content);
                                let _ = agent_event_tx.send(TuiAgentEvent::StreamingText(
                                    msg.message.content.clone(),
                                ));
                            }

                            if received_done {
                                break;
                            }
                        }
                        None => {
                            // Channel closed
                            break;
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                    // Check for interrupt
                    if interrupted.load(Ordering::SeqCst) {
                        interrupted.store(false, Ordering::SeqCst);
                        let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(
                            "Generation interrupted by user.".to_string(),
                        ));

                        // Save partial response
                        if !response_content.is_empty() {
                            messages.push(Message {
                                role: Role::Assistant,
                                content: response_content,
                                tool_calls: vec![],
                                images: vec![],
                            });
                            session.append_message(messages.last().expect("just pushed a message"));
                        } else {
                            messages.pop();
                        }

                        let _ = agent_event_tx.send(TuiAgentEvent::StreamingDone);
                        return;
                    }
                }
            }
        }

        // Finish streaming
        let _ = agent_event_tx.send(TuiAgentEvent::StreamingDone);

        if !received_done || is_error {
            let error_detail = if is_error {
                response_content.clone()
            } else {
                "Provider request was interrupted.".to_string()
            };
            let _ = agent_event_tx.send(TuiAgentEvent::Error(error_detail));
            messages.pop();
            return;
        }

        // Handle tool calls
        if !tool_calls.is_empty() {
            // Push the assistant message with tool calls
            messages.push(Message {
                role: Role::Assistant,
                content: response_content.clone(),
                tool_calls: tool_calls.clone(),
                images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));

            // Process tool calls
            let has_more_tools = handle_tui_tool_calls(
                &tool_calls,
                messages,
                tool_manager,
                ctx,
                session,
                provider,
                interrupted,
                agent_event_tx,
                user_action_rx,
                &mut auto_accept,
            )
            .await;

            if has_more_tools {
                continue; // Loop back to call provider again with tool results
            }

            // No more tool calls — we're done
            return;
        }

        // No tool calls — push the final assistant message
        messages.push(Message {
            role: Role::Assistant,
            content: response_content,
            tool_calls: vec![],
            images: vec![],
        });
        session.append_message(messages.last().expect("just pushed a message"));
        return;
    }
}

/// Handle tool calls in TUI mode.
///
/// Returns `true` if tool results were added (the caller should loop back
/// to call the provider again), or `false` if there were no tool calls.
#[allow(clippy::too_many_arguments)]
async fn handle_tui_tool_calls(
    tool_calls: &[tinyharness_lib::provider::ToolCall],
    messages: &mut Vec<Message>,
    tool_manager: &ToolManager,
    ctx: &mut CommandContext,
    session: &mut Session,
    provider: &Arc<Mutex<dyn Provider + Send + Sync>>,
    interrupted: &Arc<AtomicBool>,
    agent_event_tx: &mpsc::Sender<TuiAgentEvent>,
    user_action_rx: &mut mpsc::Receiver<TuiUserAction>,
    auto_accept: &mut bool,
) -> bool {
    if tool_calls.is_empty() {
        return false;
    }

    let tool_count = tool_calls.len();
    let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
        "{} tool call(s)",
        tool_count
    )));

    let settings = load_settings();
    let auto_accept_safe_commands = settings.auto_accept_safe_commands;
    let safe_commands = settings.get_safe_commands();
    let denied_commands = settings.get_denied_commands();

    // Collect generic tool results
    let mut generic_tool_results: Vec<GenericToolResult> = Vec::new();

    for call in tool_calls {
        // Check for interrupt
        if interrupted.load(Ordering::SeqCst) {
            interrupted.store(false, Ordering::SeqCst);
            let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(
                "Tool execution interrupted by user.".to_string(),
            ));
            return true;
        }

        // Signal tools handled specially
        if tool_manager.is_signal_tool(&call.function.name) {
            if let Some(event) =
                tool_manager.parse_signal_event(&call.function.name, &call.function.arguments)
            {
                match event {
                    SignalEvent::SwitchMode { mode } => {
                        let old_mode = ctx.current_mode;
                        match ctx.switch_mode(mode, messages) {
                            Ok(()) => {
                                session.set_mode(mode);
                                let _ = agent_event_tx
                                    .send(TuiAgentEvent::ModeChanged(mode.to_string()));
                                let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                                    "Mode switched: {} → {}",
                                    old_mode, mode
                                )));
                                messages.push(Message {
                                    role: Role::Tool,
                                    content: format!(
                                        "SUCCESS: Mode switched from '{}' to '{}'.",
                                        old_mode, mode
                                    ),
                                    tool_calls: vec![],
                                    images: vec![],
                                });
                                session.append_message(
                                    messages.last().expect("just pushed a message"),
                                );
                            }
                            Err(msg) => {
                                let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(msg));
                                messages.push(Message {
                                    role: Role::Tool,
                                    content: format!(
                                        "Already in '{}' mode. No change was made.",
                                        mode
                                    ),
                                    tool_calls: vec![],
                                    images: vec![],
                                });
                                session.append_message(
                                    messages.last().expect("just pushed a message"),
                                );
                            }
                        }
                    }
                    SignalEvent::Question { question, answers } => {
                        // Send the question to the TUI for user interaction
                        let _ = agent_event_tx.send(TuiAgentEvent::Question {
                            question: question.clone(),
                            answers: answers.clone(),
                        });

                        // Wait for the user's answer
                        let answer = loop {
                            match user_action_rx.recv() {
                                Ok(TuiUserAction::QuestionAnswer(ans)) => {
                                    break ans;
                                }
                                Ok(TuiUserAction::Interrupt) => {
                                    interrupted.store(true, Ordering::SeqCst);
                                    break "Skipped (interrupted)".to_string();
                                }
                                Ok(TuiUserAction::Quit) => {
                                    let _ = agent_event_tx.send(TuiAgentEvent::Done);
                                    return false;
                                }
                                Ok(_) => {
                                    // Ignore other actions while waiting for answer
                                    continue;
                                }
                                Err(_) => {
                                    // Channel closed
                                    break "Skipped (channel closed)".to_string();
                                }
                            }
                        };

                        messages.push(Message {
                            role: Role::Tool,
                            content: format!(
                                "User answered the question '{}' with: '{}'.",
                                question, answer
                            ),
                            tool_calls: vec![],
                            images: vec![],
                        });
                        session.append_message(messages.last().expect("just pushed a message"));
                    }
                    SignalEvent::AutoCompact { focus } => {
                        let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(
                            "Compacting conversation history...".to_string(),
                        ));
                        let mut provider_guard = provider.lock().await;
                        match execute_compact(
                            &mut ctx.output,
                            &mut *provider_guard,
                            messages,
                            &focus,
                        )
                        .await
                        {
                            Ok(token_usage) => {
                                if let Some(usage) = token_usage.clone() {
                                    ctx.compaction_token_usage = Some(usage.clone());
                                    session.set_token_usage(usage);
                                }
                                let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(
                                    "Conversation compacted successfully.".to_string(),
                                ));
                                messages.push(Message {
                                    role: Role::Tool,
                                    content: format!(
                                        "Conversation compacted successfully. Focus: '{}'.",
                                        if focus.is_empty() {
                                            "general summary"
                                        } else {
                                            &focus
                                        }
                                    ),
                                    tool_calls: vec![],
                                    images: vec![],
                                });
                                session.append_message(
                                    messages.last().expect("just pushed a message"),
                                );
                            }
                            Err(e) => {
                                let _ = agent_event_tx.send(TuiAgentEvent::Error(format!(
                                    "Auto-compact failed: {}",
                                    e
                                )));
                                messages.push(Message {
                                    role: Role::Tool,
                                    content: format!(
                                        "Auto-compact failed: {}. The conversation was not modified.",
                                        e
                                    ),
                                    tool_calls: vec![],
                                    images: vec![],
                                });
                                session.append_message(
                                    messages.last().expect("just pushed a message"),
                                );
                            }
                        }
                    }
                    SignalEvent::InvokeSkill { skill_name } => {
                        let skill_result = {
                            let registry = &ctx.skill_registry;
                            registry
                                .get(&skill_name)
                                .map(|s| (s.name.clone(), s.description.clone()))
                        };
                        match skill_result {
                            Some((name, description)) => {
                                if ctx
                                    .active_skills
                                    .iter()
                                    .any(|s| s.eq_ignore_ascii_case(&name))
                                {
                                    let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(
                                        format!("Skill '{}' is already active", name),
                                    ));
                                    messages.push(Message {
                                        role: Role::Tool,
                                        content: format!("Skill '{}' is already active.", name),
                                        tool_calls: vec![],
                                        images: vec![],
                                    });
                                    session.append_message(
                                        messages.last().expect("just pushed a message"),
                                    );
                                } else {
                                    ctx.active_skills.push(name.clone());
                                    messages.push(Message {
                                        role: Role::User,
                                        content: format!("/use {}", skill_name),
                                        tool_calls: vec![],
                                        images: vec![],
                                    });
                                    session.append_message(
                                        messages.last().expect("just pushed a message"),
                                    );
                                    ctx.refresh_system_prompt(messages);
                                    let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(
                                        format!("Skill activated: {} — {}", name, description),
                                    ));
                                }
                            }
                            None => {
                                let _ = agent_event_tx.send(TuiAgentEvent::Error(format!(
                                    "Skill '{}' not found",
                                    skill_name
                                )));
                                messages.push(Message {
                                    role: Role::Tool,
                                    content: format!("Error: Skill '{}' not found.", skill_name),
                                    tool_calls: vec![],
                                    images: vec![],
                                });
                                session.append_message(
                                    messages.last().expect("just pushed a message"),
                                );
                            }
                        }
                    }
                }
            } else {
                messages.push(Message {
                    role: Role::Tool,
                    content: format!(
                        "Error: Could not parse arguments for signal tool '{}'.",
                        call.function.name
                    ),
                    tool_calls: vec![],
                    images: vec![],
                });
                session.append_message(messages.last().expect("just pushed a message"));
            }
            continue;
        }

        let needs_confirmation = tool_manager.needs_approval(&call.function.name);

        // Determine approval
        let (approved, auto_accepted) = if !needs_confirmation {
            (true, false)
        } else if *auto_accept {
            // Auto-accept mode — check if it's a safe command
            if call.function.name == "run" {
                if let Some(cmd_value) = call.function.arguments.get("command")
                    && let Some(cmd_str) = cmd_value.as_str()
                    && is_safe_command(cmd_str, &safe_commands, &denied_commands)
                {
                    (true, true)
                } else {
                    // Unsafe run command — still require confirmation even in auto-accept mode
                    // Ask the user via the TUI confirmation flow
                    let args_summary =
                        format_args_summary_tui(&call.function.name, &call.function.arguments);
                    let diff_preview =
                        compute_diff_preview(&call.function.name, &call.function.arguments);
                    let _ = agent_event_tx.send(TuiAgentEvent::ConfirmTool {
                        name: call.function.name.clone(),
                        args_summary: args_summary.clone(),
                        needs_approval: true,
                        diff_preview,
                    });
                    // Wait for user response
                    loop {
                        match user_action_rx.recv() {
                            Ok(TuiUserAction::ConfirmResponse {
                                approved,
                                auto_accept: resp_auto_accept,
                            }) => {
                                if resp_auto_accept {
                                    *auto_accept = true;
                                }
                                break (approved, resp_auto_accept);
                            }
                            Ok(TuiUserAction::Interrupt) => {
                                interrupted.store(true, Ordering::SeqCst);
                                break (false, false);
                            }
                            Ok(TuiUserAction::Quit) => {
                                let _ = agent_event_tx.send(TuiAgentEvent::Done);
                                return false;
                            }
                            Ok(_) => {
                                // Ignore other actions while waiting for confirmation
                                continue;
                            }
                            Err(_) => {
                                // Channel closed
                                return false;
                            }
                        }
                    }
                }
            } else {
                (true, true)
            }
        } else if auto_accept_safe_commands
            && call.function.name == "run"
            && let Some(cmd_value) = call.function.arguments.get("command")
            && let Some(cmd_str) = cmd_value.as_str()
            && is_safe_command(cmd_str, &safe_commands, &denied_commands)
        {
            (true, true)
        } else {
            // Needs confirmation — ask the user via the TUI confirmation flow
            let args_summary =
                format_args_summary_tui(&call.function.name, &call.function.arguments);
            let diff_preview = compute_diff_preview(&call.function.name, &call.function.arguments);
            let _ = agent_event_tx.send(TuiAgentEvent::ConfirmTool {
                name: call.function.name.clone(),
                args_summary: args_summary.clone(),
                needs_approval: true,
                diff_preview,
            });
            // Wait for user response
            loop {
                match user_action_rx.recv() {
                    Ok(TuiUserAction::ConfirmResponse {
                        approved,
                        auto_accept: resp_auto_accept,
                    }) => {
                        if resp_auto_accept {
                            *auto_accept = true;
                        }
                        break (approved, resp_auto_accept);
                    }
                    Ok(TuiUserAction::Interrupt) => {
                        interrupted.store(true, Ordering::SeqCst);
                        break (false, false);
                    }
                    Ok(TuiUserAction::Quit) => {
                        let _ = agent_event_tx.send(TuiAgentEvent::Done);
                        return false;
                    }
                    Ok(_) => {
                        // Ignore other actions while waiting for confirmation
                        continue;
                    }
                    Err(_) => {
                        // Channel closed
                        return false;
                    }
                }
            }
        };

        if !approved {
            let args_summary =
                format_args_summary_tui(&call.function.name, &call.function.arguments);
            messages.push(Message {
                role: Role::System,
                content: format!(
                    "The user denied the '{}' tool call with arguments: {}",
                    call.function.name, args_summary
                ),
                tool_calls: vec![],
                images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
            continue;
        }

        // Notify TUI about tool call
        let args_summary = format_args_summary_tui(&call.function.name, &call.function.arguments);
        let _ = agent_event_tx.send(TuiAgentEvent::ToolCall {
            name: call.function.name.clone(),
            args_summary: args_summary.clone(),
        });

        // Execute the tool
        let start_time = std::time::Instant::now();
        let result = tool_manager
            .execute_tool_call(&call.function.name, &call.function.arguments)
            .await;
        let duration_ms = start_time.elapsed().as_millis() as u64;

        let is_error = result.starts_with("Error:");

        // For edit/write tools, compute a diff and include it in the TUI display
        let display_content = if !is_error {
            match call.function.name.as_str() {
                "edit" => {
                    let path = call
                        .function
                        .arguments
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let old_str = call
                        .function
                        .arguments
                        .get("old_str")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let new_str = call
                        .function
                        .arguments
                        .get("new_str")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let diff = tinyharness_ui::ui::diff::compute_edit_diff_from_path(
                        path, old_str, new_str,
                    );
                    if diff.is_empty() {
                        result.clone()
                    } else {
                        format!("{}\n{}", diff.trim_end(), result)
                    }
                }
                "write" => {
                    let path = call
                        .function
                        .arguments
                        .get("path")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let content = call
                        .function
                        .arguments
                        .get("content")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let diff = tinyharness_ui::ui::diff::compute_write_diff_plain(path, content);
                    if diff.is_empty() {
                        result.clone()
                    } else {
                        format!("{}\n{}", diff.trim_end(), result)
                    }
                }
                _ => result.clone(),
            }
        } else {
            result.clone()
        };

        let _ = agent_event_tx.send(TuiAgentEvent::ToolResult {
            name: call.function.name.clone(),
            content: display_content,
            is_error,
        });

        // Log to audit if this was an auditable tool
        if matches!(call.function.name.as_str(), "run" | "write" | "edit") {
            let audit_detail = call
                .function
                .arguments
                .get(if call.function.name == "run" {
                    "command"
                } else {
                    "path"
                })
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let exit_code = if is_error { -1 } else { 0 };
            crate::commands::audit::log_command(
                session.id(),
                &call.function.name,
                audit_detail.as_deref().unwrap_or(""),
                exit_code,
                auto_accepted,
                duration_ms,
            );
        }

        // Collect result for batching
        generic_tool_results.push(GenericToolResult {
            content: format!("### {} Tool Result\n\n{}", call.function.name, result),
            audit_tool_name: if matches!(call.function.name.as_str(), "run" | "write" | "edit") {
                Some(call.function.name.clone())
            } else {
                None
            },
            audit_detail: call
                .function
                .arguments
                .get(if call.function.name == "run" {
                    "command"
                } else {
                    "path"
                })
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            duration_ms,
            is_error,
        });
    }

    // Batch all generic tool results into a single message
    if !generic_tool_results.is_empty() {
        let batched_content = if generic_tool_results.len() == 1 {
            generic_tool_results[0].content.clone()
        } else {
            format!(
                "Multiple tool results ({} total):\n\n{}",
                generic_tool_results.len(),
                generic_tool_results
                    .iter()
                    .map(|r| r.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n")
            )
        };

        messages.push(Message {
            role: Role::Tool,
            content: batched_content,
            tool_calls: vec![],
            images: vec![],
        });
        session.append_message(messages.last().expect("just pushed a message"));
    }

    true
}

/// Compute a plain-text diff preview for a destructive tool call (edit/write).
///
/// Returns `Some(diff_string)` for edit and write tools, `None` otherwise.
/// The diff is computed *before* the tool is executed so the user can review
/// the pending changes before confirming.
fn compute_diff_preview(tool_name: &str, arguments: &serde_json::Value) -> Option<String> {
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

/// Result from executing a generic tool call in TUI mode.
#[allow(dead_code)]
struct GenericToolResult {
    content: String,
    audit_tool_name: Option<String>,
    audit_detail: Option<String>,
    duration_ms: u64,
    is_error: bool,
}
