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

use crate::commands::{CommandContext, CommandResult, build_registry};

use super::command_result;
use super::confirm::ConfirmationDecision;
use super::display::format_args_summary_tui;
use super::signal::{self, SignalResult};
use super::tool_result::{
    GenericToolResult, audit_info_for_tool, batch_tool_results, compute_tool_diff,
    ensure_tool_call_ids, log_tool_audit, tool_display_content,
};

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

/// Render a signal result to the TUI via channel events.
fn render_signal_result_tui(result: &SignalResult, agent_event_tx: &mpsc::Sender<TuiAgentEvent>) {
    match result {
        SignalResult::SwitchMode {
            old_mode: _,
            new_mode,
            already_in,
        } => {
            if *already_in {
                let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                    "Already in '{}' mode. No change was made.",
                    new_mode
                )));
            } else {
                let _ = agent_event_tx.send(TuiAgentEvent::ModeChanged(new_mode.to_string()));
            }
        }
        SignalResult::AutoCompact {
            focus: _,
            success,
            error,
        } => {
            if *success {
                // The captured output from execute_compact already includes
                // the detailed compaction result (message counts, stages, etc.),
                // so we don't need to send a duplicate success message here.
            } else if let Some(e) = error {
                let _ = agent_event_tx
                    .send(TuiAgentEvent::Error(format!("Auto-compact failed: {}", e)));
            }
        }
        SignalResult::InvokeSkill {
            name,
            description,
            already_active,
            found,
        } => {
            if *already_active {
                let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                    "Skill '{}' is already active",
                    name
                )));
            } else if *found {
                let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(format!(
                    "Skill activated: {} — {}",
                    name, description
                )));
            } else {
                let _ = agent_event_tx
                    .send(TuiAgentEvent::Error(format!("Skill '{}' not found", name)));
            }
        }
        SignalResult::Question { .. } => {
            // Question is handled separately in the TUI loop
        }
        SignalResult::ParseError { tool_name } => {
            let _ = agent_event_tx.send(TuiAgentEvent::Error(format!(
                "Could not parse arguments for signal tool '{}'",
                tool_name
            )));
        }
    }
}

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

    // Send initial state to the TUI.
    // When no model is known yet (e.g. Sockudo before the first
    // response), don't send a model name — the TUI will display
    // an empty/placeholder until the first response reports it.
    let model_name = {
        let p = provider.lock().await;
        p.current_model()
    };
    if let Some(name) = model_name {
        let _ = agent_event_tx.send(TuiAgentEvent::ModelChanged(name));
    }
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
    context_size: ContextWindowSize,
) {
    match dispatch_command_to_tui(input, ctx, messages, registry, agent_event_tx).await {
        Ok(CommandResult::Ok) => {
            // Update token usage from compaction side-channel
            if let Some(usage) = command_result::apply_ok(ctx, session) {
                *last_known_token_usage = Some(usage.clone());
                // Send updated token count to TUI
                let _ = agent_event_tx.send(TuiAgentEvent::TokenUpdate {
                    count: usage.total_tokens as u64,
                    limit: Some(context_size.tokens() as u64),
                });
                send_context_warning_if_needed(usage.total_tokens, context_size, agent_event_tx);
            }
            // Send mode update if it changed
            let _ = agent_event_tx.send(TuiAgentEvent::ModeChanged(ctx.current_mode.to_string()));
        }
        Ok(CommandResult::SwitchSession(id_prefix)) => {
            let info = command_result::apply_switch_session(&id_prefix, ctx, messages, session);
            if info.is_error {
                let _ = agent_event_tx.send(TuiAgentEvent::Error(info.description));
            } else {
                let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(info.description));
                *last_known_token_usage = session.meta().token_usage.clone();
                let _ =
                    agent_event_tx.send(TuiAgentEvent::ModeChanged(ctx.current_mode.to_string()));
            }
        }
        Ok(CommandResult::RenameSession(new_name)) => {
            let info = command_result::apply_rename_session(&new_name, session);
            let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(info.description));
        }
        Ok(CommandResult::Init(result)) => {
            let info = command_result::apply_init(&result, ctx, messages);
            let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(info.description));
        }
        Ok(CommandResult::SkillUse(skill_name)) => {
            let info = command_result::apply_skill_use(&skill_name, ctx, messages, session);
            if info.is_error {
                let _ = agent_event_tx.send(TuiAgentEvent::Error(info.description));
            } else {
                let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(info.description));
            }
        }
        Ok(CommandResult::SkillUnload(skill_name)) => {
            let info = command_result::apply_skill_unload(&skill_name, ctx, messages, session);
            let _ = agent_event_tx.send(TuiAgentEvent::SystemMessage(info.description));
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
        tool_call_id: None,
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
                                tool_call_id: None,
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

        // After streaming, check if the provider's model has been updated
        // (e.g. Sockudo discovers the actual model from the worker's
        // response). Notify the TUI so the status bar reflects it.
        {
            let p = provider.lock().await;
            if let Some(model) = p.current_model() {
                let _ = agent_event_tx.send(TuiAgentEvent::ModelChanged(model));
            }
        }

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
            // Ensure every tool call has a non-empty id (some providers omit them)
            let mut tool_calls = tool_calls;
            ensure_tool_call_ids(&mut tool_calls);

            // Push the assistant message with tool calls
            messages.push(Message {
                role: Role::Assistant,
                content: response_content.clone(),
                tool_calls: tool_calls.clone(),
                tool_call_id: None,
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
                context_size,
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
            tool_call_id: None,
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
    context_size: ContextWindowSize,
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
                // Question signal requires user interaction — handle via TUI channel
                match &event {
                    SignalEvent::Question { question, answers } => {
                        let tc_id = call.id.as_deref().unwrap_or("");
                        // Validate question
                        if let Some(error) = signal::validate_question(question, answers) {
                            signal::apply_question_error(error, messages, session, tc_id);
                            continue;
                        }

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

                        let is_skip = answer.starts_with("Skipped");
                        signal::apply_question_answer(
                            question, &answer, is_skip, messages, session, tc_id,
                        );
                    }
                    _ => {
                        let tc_id = call.id.as_deref().unwrap_or("");
                        // Swap ctx.output with a capture writer so that any
                        // output produced by the signal handler (e.g.
                        // auto_compact's compaction progress) is captured
                        // instead of going to raw stdout (which is invisible
                        // and corrupts the TUI in alternate-screen mode).
                        let capture = CaptureWriter::new();
                        let captured_output = Output::new(Box::new(capture.clone()));
                        let original_output = std::mem::replace(&mut ctx.output, captured_output);

                        let result = signal::handle_signal_event(
                            &event, messages, session, ctx, provider, tc_id,
                        )
                        .await;

                        // Restore the original output writer
                        ctx.output = original_output;

                        // Send any captured output as a system message
                        let output_bytes = capture.take_output();
                        let output_text = String::from_utf8_lossy(&output_bytes);
                        let stripped = strip_ansi_sgr(&output_text);
                        if !stripped.is_empty() {
                            let _ = agent_event_tx
                                .send(TuiAgentEvent::SystemMessage(stripped.to_string()));
                        }

                        render_signal_result_tui(&result, agent_event_tx);

                        // If auto_compact succeeded, send updated token usage
                        // to the TUI so the status bar refreshes.
                        if matches!(result, SignalResult::AutoCompact { success: true, .. })
                            && let Some(usage) = ctx.compaction_token_usage.take()
                        {
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
                }
            } else {
                let tc_id = call.id.as_deref().unwrap_or("");
                signal::apply_signal_parse_error(&call.function.name, messages, session, tc_id);
            }
            continue;
        }

        let needs_confirmation = tool_manager.needs_approval(&call.function.name);

        // Use shared decision logic for confirmation
        let decision = super::confirm::decide_tool_confirmation(
            call,
            *auto_accept,
            auto_accept_safe_commands,
            &safe_commands,
            &denied_commands,
            needs_confirmation,
        );

        let (approved, auto_accepted) = match decision {
            ConfirmationDecision::AutoApproved { auto_accepted: aa } => (true, aa),
            ConfirmationDecision::NeedsConfirmation => {
                // Ask the user via the TUI confirmation flow
                let args_summary =
                    format_args_summary_tui(&call.function.name, &call.function.arguments);
                let diff_preview = compute_tool_diff(&call.function.name, &call.function.arguments);
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
            ConfirmationDecision::Denied => (false, false),
        };

        if !approved {
            let args_summary =
                format_args_summary_tui(&call.function.name, &call.function.arguments);
            messages.push(Message {
                role: Role::Tool,
                content: format!(
                    "The user denied the '{}' tool call with arguments: {}",
                    call.function.name, args_summary
                ),
                tool_calls: vec![],
                tool_call_id: call.id.clone(),
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

        let display_content = tool_display_content(
            &call.function.name,
            &call.function.arguments,
            &result,
            is_error,
        );

        let _ = agent_event_tx.send(TuiAgentEvent::ToolResult {
            name: call.function.name.clone(),
            content: display_content,
            is_error,
        });

        // Log to audit if this was an auditable tool
        log_tool_audit(session.id(), call, auto_accepted, duration_ms, is_error);

        // Collect result for batching
        let (audit_tool_name, audit_detail) = audit_info_for_tool(call);
        generic_tool_results.push(GenericToolResult {
            content: format!("### {} Tool Result\n\n{}", call.function.name, result),
            tool_call_id: call.id.clone().unwrap_or_default(),
            audit_tool_name,
            audit_detail,
            duration_ms,
            is_error,
            images: vec![],
        });
    }

    // Push individual tool result messages (one per tool call)
    let tool_msgs = batch_tool_results(generic_tool_results);
    for msg in tool_msgs {
        messages.push(msg);
        session.append_message(messages.last().expect("just pushed a message"));
    }

    true
}
