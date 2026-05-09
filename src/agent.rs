use std::{
    error::Error,
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use rustyline::Editor;
use tokio::sync::Mutex;

use tinyharness_lib::{
    config::load_settings,
    mode::AgentMode,
    provider::{Message, Provider, Role, TokenUsage, ToolCall},
    session::{Session, SessionStore},
    token::{
        ContextWindowSize, check_context_warning, estimate_conversation_tokens, estimate_tokens,
        format_token_count,
    },
    tools::SignalEvent,
    tools::ToolManager,
};

use crate::style::*;
use crate::{
    commands::{CommandDispatcher, CommandResult, compact::execute_compact, init},
    ui::confirm::prompt_tool_confirmation,
    ui::input::CommandHelper,
};

pub async fn run_agent_loop(
    provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    tool_manager: ToolManager,
    messages: &mut Vec<Message>,
    dispatcher: &mut CommandDispatcher,
    session: &mut Session,
    interrupted: &Arc<AtomicBool>,
) -> Result<(), Box<dyn Error>> {
    let mut stdout = io::stdout();
    stdout.write_all(
        format!(
            "{}╔════════════════════════════════════════════════════════╗{}\n",
            BOX_COLOR, RESET
        )
        .as_bytes(),
    )?;
    stdout.write_all(
        format!(
            "{}║{}           {}TinyHarness AI Assistant{}                     {}║{}\n",
            BOX_COLOR, RESET, BOLD, TITLE_COLOR, BOX_COLOR, RESET
        )
        .as_bytes(),
    )?;
    stdout.write_all(
        format!(
            "{}╚════════════════════════════════════════════════════════╝{}\n\n",
            BOX_COLOR, RESET
        )
        .as_bytes(),
    )?;
    stdout.write_all(
        format!(
            "{}Tip:{} Type {} to see available commands\n\n",
            GRAY, RESET, "/help"
        )
        .as_bytes(),
    )?;
    stdout.flush()?;

    // If resuming a session with existing messages, print the conversation history
    // so the user can see where they left off.
    if messages.len() > 1 {
        print_conversation_history(messages, &mut stdout)?;
    }

    let helper = CommandHelper::new();
    let history_dir = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/share/tinyharness"))
        .unwrap_or_else(|_| std::path::PathBuf::from(".tinyharness_history"));
    std::fs::create_dir_all(&history_dir).ok();
    let history_path = history_dir.join("history.txt");
    let mut rl = Editor::new()?;
    rl.set_helper(Some(helper));
    rl.load_history(&history_path).ok();

    loop {
        // Clear any stale interrupt flag from a previous turn.
        // The flag may be set from Ctrl+C during rustyline's blocking read,
        // which we handle below by showing a hint and continuing.
        interrupted.store(false, Ordering::SeqCst);

        let mode_label = dispatcher.current_mode.to_string();
        let msg_count = messages.len();
        let pinned_count = dispatcher.file_context.pinned_file_count();
        let context_info = if pinned_count > 0 {
            format!("{} msgs,{}{}{} pinned", msg_count, BLUE, pinned_count, GRAY)
        } else {
            format!("{} msgs", msg_count)
        };
        let prompt = format!(
            "{}[{}]{} {}{}> {}{}",
            BOLD, mode_label, RESET, GRAY, context_info, BLUE, RESET
        );
        let readline = rl.readline(&prompt);
        let user_input = match readline {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                rl.add_history_entry(&trimmed)?;
                trimmed
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                // Ctrl+C during input — just clear the flag (set by our handler)
                // and show a hint. Don't exit the program.
                interrupted.store(false, Ordering::SeqCst);
                stdout.write_all("\n".as_bytes())?;
                stdout.write_all(
                    format!(
                        "{}Use {}/exit{} or {}{}Ctrl+D{} to exit.\n",
                        GRAY, BLUE, GRAY, GRAY, BOLD, RESET
                    )
                    .as_bytes(),
                )?;
                stdout.flush()?;
                continue;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                stdout.write_all("\n".as_bytes())?;
                break;
            }
            Err(err) => {
                eprintln!("{}Error reading input: {}{}", RED, err, RESET);
                break;
            }
        };

        if user_input.starts_with('/') {
            match CommandDispatcher::parse(&user_input) {
                Some(cmd) => {
                    match dispatcher.dispatch(cmd, messages).await {
                        Ok(CommandResult::Ok) => {}
                        Ok(CommandResult::SwitchSession(id_prefix)) => {
                            let store = SessionStore::default_path();
                            match store.find_by_prefix(&id_prefix) {
                                Ok(full_id) => {
                                    // Flush current session before switching
                                    session.flush();
                                    match store.load(&full_id) {
                                        Ok((new_session, loaded_msgs)) => {
                                            let meta = new_session.meta();
                                            let name = meta.name.as_deref().unwrap_or("unnamed");
                                            eprintln!(
                                                "{}Switched to session {}{}{} — {}{}{} ({} messages, {}){}",
                                                BOLD,
                                                BLUE,
                                                &meta.id[..12],
                                                RESET,
                                                BOLD,
                                                name,
                                                RESET,
                                                meta.message_count,
                                                meta.mode,
                                                RESET
                                            );
                                            *session = new_session;
                                            *messages = loaded_msgs;
                                            // Update dispatcher mode and session ID to match loaded session
                                            dispatcher.current_mode = session.meta().mode;
                                            dispatcher.session_id = Some(session.id().to_string());
                                            // Ensure system prompt reflects current context
                                            dispatcher.refresh_system_prompt(messages);
                                            // Print loaded conversation history
                                            print_conversation_history(messages, &mut stdout)?;
                                        }
                                        Err(e) => {
                                            eprintln!("{}{}{}", RED, e, RESET);
                                        }
                                    }
                                }
                                Err(e) => {
                                    eprintln!("{}{}{}", RED, e, RESET);
                                }
                            }
                        }
                        Ok(CommandResult::RenameSession(new_name)) => {
                            session.set_name(new_name.clone());
                            eprintln!("{}Session renamed to {}{}{}", BOLD, BLUE, new_name, RESET);
                        }
                        Ok(CommandResult::Init(result)) => match &result {
                            init::InitResult::Created { path } => {
                                eprintln!(
                                    "{}  Created {}{}{} — workspace context refreshed.{}",
                                    GREEN,
                                    BLUE,
                                    path.display(),
                                    GREEN,
                                    RESET
                                );
                            }
                            init::InitResult::Updated { path } => {
                                eprintln!(
                                    "{}  Updated {}{}{} — workspace context refreshed.{}",
                                    GREEN,
                                    BLUE,
                                    path.display(),
                                    GREEN,
                                    RESET
                                );
                            }
                        },
                        Err(e) => {
                            eprintln!("{}{}{}", RED, e, RESET);
                        }
                    }
                    if dispatcher.exit_requested {
                        break;
                    }
                }
                None => {
                    eprintln!(
                        "{}Unknown command: {}{}{}\n  Type {}/help{} for available commands.{}",
                        RED, BLUE, user_input, RED, BLUE, RED, RESET
                    );
                }
            }
            continue;
        }

        messages.push(Message {
            role: Role::User,
            content: user_input.clone(),
            tool_calls: vec![],
        });

        // Auto-save: user message
        session.append_message(messages.last().unwrap());

        // auto_accept persists across all agent iterations within this user turn,
        // so that pressing 'a' once auto-accepts all subsequent tool calls.
        let mut auto_accept = false;
        let mut token_usage: Option<TokenUsage> = None;

        loop {
            // Filter tools based on current mode
            let tools = tool_manager.tools_for_mode(dispatcher.current_mode);

            // Call the provider — it returns a receiver for streaming chunks
            let mut recv = {
                let mut provider = provider.lock().await;
                provider.chat(messages.clone(), tools).await
            };

            let mut response_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut received_done = false;
            let mut is_error = false;
            let mut was_interrupted = false;

            stdout.write_all(ORANGE.as_bytes())?;

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
                                }

                                if msg.is_error {
                                    is_error = true;
                                }

                                // Capture token usage from the final response
                                if msg.done && token_usage.is_none() {
                                    token_usage = msg.usage.clone();
                                }

                                if !msg.message.content.is_empty() {
                                    response_content.push_str(&msg.message.content);
                                    stdout.write_all(msg.message.content.as_bytes())?;
                                    stdout.flush()?;
                                }

                                if received_done {
                                    break;
                                }
                            }
                            None => {
                                // Channel closed — treat as end of stream
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {
                        // Check if the user pressed Ctrl+C
                        if interrupted.load(Ordering::SeqCst) {
                            was_interrupted = true;
                            break;
                        }
                    }
                }
            }

            // Handle user interrupt (Ctrl+C during generation)
            if was_interrupted {
                interrupted.store(false, Ordering::SeqCst);
                stdout.write_all(RESET.as_bytes())?;
                writeln!(
                    stdout,
                    "\n{}⚠ Generation interrupted by user.{}",
                    ORANGE, RESET
                )?;
                stdout.flush()?;

                // Save any partial content as an assistant message so the
                // conversation remains coherent for future turns.
                if !response_content.is_empty() {
                    messages.push(Message {
                        role: Role::Assistant,
                        content: response_content,
                        tool_calls: vec![],
                    });
                    session.append_message(messages.last().unwrap());
                } else {
                    // No content was generated — remove the user message so
                    // the next retry isn't confused by a dangling prompt.
                    messages.pop();
                }
                break; // Break out of the inner generation loop, back to input prompt
            }

            if !received_done || is_error {
                let error_detail = if is_error {
                    response_content.clone()
                } else {
                    "Provider request was interrupted.".to_string()
                };

                stdout.write_all(RESET.as_bytes())?;
                stdout.write_all(
                    format!("\n{}⚠ Request failed.{} {}\n", RED, RESET, error_detail).as_bytes(),
                )?;

                // Ask the user if they want to retry
                let should_retry;
                loop {
                    write!(stdout, "{}Retry request? [Y/n]{} ", BOLD, RESET)?;
                    stdout.flush()?;

                    let mut answer = String::new();
                    io::stdin()
                        .read_line(&mut answer)
                        .expect("Failed to read line");
                    let answer = answer.trim().to_lowercase();

                    if answer.is_empty() || answer == "y" || answer == "yes" {
                        should_retry = true;
                        break;
                    } else if answer == "n" || answer == "no" {
                        should_retry = false;
                        break;
                    } else {
                        writeln!(stdout, "{}Please answer y or n.{}", GRAY, RESET)?;
                        continue;
                    }
                }

                if should_retry {
                    continue;
                } else {
                    messages.pop();
                    break;
                }
            }

            stdout.write_all(RESET.as_bytes())?;

            if handle_tool_calls(
                &tool_calls,
                &response_content,
                messages,
                &tool_manager,
                dispatcher,
                &mut stdout,
                &mut auto_accept,
                session,
                Arc::clone(&provider),
                interrupted,
            )
            .await?
            {
                continue;
            }

            messages.push(Message {
                role: Role::Assistant,
                content: response_content,
                tool_calls: vec![],
            });
            session.append_message(messages.last().unwrap());

            break;
        }

        // Display token usage after turn is complete (always show, with estimation if needed)
        let estimated_total = estimate_conversation_tokens(messages);

        // Use configured context limit for warnings, or fall back to default
        let settings = load_settings();
        let context_size = settings
            .context_limit
            .map(ContextWindowSize::Custom)
            .unwrap_or_else(ContextWindowSize::default_size);
        let usage_pct = context_size.usage_percentage(estimated_total);

        let is_estimated = token_usage.is_none();
        let (prompt_tokens, completion_tokens, total_tokens) = if let Some(usage) = &token_usage {
            (
                usage.prompt_tokens,
                usage.completion_tokens,
                usage.total_tokens,
            )
        } else {
            // Estimate if provider didn't return usage
            let prompt_est =
                estimate_conversation_tokens(&messages[..messages.len().saturating_sub(1)]);
            let last_msg = messages.last().map(|m| m.content.as_str()).unwrap_or("");
            let completion_est = estimate_tokens(last_msg);
            (prompt_est, completion_est, prompt_est + completion_est)
        };

        let estimation_marker = if is_estimated { " (estimated)" } else { "" };

        writeln!(
            stdout,
            "\n{}{}Tokens{}{}:{} prompt={}, completion={}, total={} | {}{}{} of context ({:.1}%){}",
            DIM,
            BOLD,
            if is_estimated { YELLOW } else { RESET },
            estimation_marker,
            RESET,
            prompt_tokens,
            completion_tokens,
            total_tokens,
            if usage_pct >= 90.0 {
                RED
            } else if usage_pct >= 70.0 {
                YELLOW
            } else {
                GRAY
            },
            format_token_count(estimated_total),
            RESET,
            usage_pct,
            RESET
        )?;

        // Show context warning if needed
        if let Some(warning) = check_context_warning(estimated_total, context_size) {
            let (icon, color) = if warning.is_critical() {
                ("⚠", RED)
            } else {
                ("⚠", YELLOW)
            };
            writeln!(
                stdout,
                "{}{}{} Context window {:.1}% full. Consider using {}/compact{} to free space.{}",
                icon,
                color,
                BOLD,
                warning.percentage(),
                BLUE,
                color,
                RESET
            )?;
        }

        stdout.write_all("\n".as_bytes())?;
        stdout.flush()?;
    }

    // Save history on exit
    rl.save_history(&history_path).ok();

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_tool_calls<W: Write>(
    tool_calls: &[ToolCall],
    response_content: &str,
    messages: &mut Vec<Message>,
    tool_manager: &ToolManager,
    dispatcher: &mut CommandDispatcher,
    stdout: &mut W,
    auto_accept: &mut bool,
    session: &mut Session,
    provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    interrupted: &Arc<AtomicBool>,
) -> Result<bool, Box<dyn Error>> {
    if tool_calls.is_empty() {
        return Ok(false);
    }

    let tool_count = tool_calls.len();
    writeln!(stdout, "\n{}  {} tool call(s){}", DIM, tool_count, RESET)?;

    messages.push(Message {
        role: Role::Assistant,
        content: response_content.to_string(),
        tool_calls: tool_calls.to_vec(),
    });
    session.append_message(messages.last().unwrap());

    for call in tool_calls {
        // Check for interrupt between tool calls
        if interrupted.load(Ordering::SeqCst) {
            interrupted.store(false, Ordering::SeqCst);
            writeln!(
                stdout,
                "\n{}⚠ Tool execution interrupted by user.{}",
                ORANGE, RESET
            )?;
            stdout.flush()?;
            return Ok(true); // Signal that there are tool results in the conversation
        }

        let needs_confirmation = tool_manager.needs_approval(&call.function.name);

        // Confirmation step
        if !confirm_tool_call(call, needs_confirmation, auto_accept, stdout)? {
            // User denied — record the denial and skip
            let args_summary = format_args_summary(&call.function.arguments);
            messages.push(Message {
                role: Role::System,
                content: format!(
                    "The user denied the '{}' tool call with arguments: {}\n\nTell the user you cannot proceed with that action unless they approve it.",
                    call.function.name, args_summary
                ),
                tool_calls: vec![],
            });
            session.append_message(messages.last().unwrap());
            continue;
        }

        // Signal tools are handled specially by the agent loop — they don't
        // go through generic execution. Parse the call into a structured
        // SignalEvent and dispatch accordingly.
        if tool_manager.is_signal_tool(&call.function.name) {
            if let Some(event) =
                tool_manager.parse_signal_event(&call.function.name, &call.function.arguments)
            {
                match event {
                    SignalEvent::SwitchMode { mode } => {
                        handle_switch_mode(mode, dispatcher, messages, session, stdout)?;
                    }
                    SignalEvent::Question { question, answers } => {
                        handle_question(&question, &answers, messages, session, stdout)?;
                    }
                    SignalEvent::AutoCompact { focus } => {
                        handle_auto_compact(
                            &focus,
                            messages,
                            session,
                            stdout,
                            Arc::clone(&provider),
                        )
                        .await?;
                    }
                }
            } else {
                // Failed to parse signal arguments — record an error message.
                messages.push(Message {
                    role: Role::Tool,
                    content: format!(
                        "Error: Could not parse arguments for signal tool '{}'.",
                        call.function.name
                    ),
                    tool_calls: vec![],
                });
                session.append_message(messages.last().unwrap());
            }
            continue;
        }

        // Generic tool execution
        execute_generic_tool(call, tool_manager, messages, session, stdout).await;
    }

    Ok(true)
}

/// Determine whether a tool call is allowed to proceed.
/// Returns `true` if the call is approved (either by the user or auto-accept).
/// Returns `false` if the user denied the call.
fn confirm_tool_call<W: Write>(
    call: &ToolCall,
    needs_confirmation: bool,
    auto_accept: &mut bool,
    stdout: &mut W,
) -> Result<bool, Box<dyn Error>> {
    use crate::ui::confirm::Confirmation;

    if !needs_confirmation {
        return Ok(true);
    }

    // "run" always requires confirmation even in auto-accept mode
    if *auto_accept && call.function.name != "run" {
        writeln!(
            stdout,
            "  {}▶ {}{} (auto-accepted)",
            DIM, call.function.name, RESET
        )?;
        return Ok(true);
    }

    match prompt_tool_confirmation(stdout, call)? {
        Confirmation::No => {
            stdout.write_all(format!("  {}Skipped{}{}\n", ORANGE, RESET, BOLD).as_bytes())?;
            stdout.flush()?;
            Ok(false)
        }
        Confirmation::AutoAccept => {
            *auto_accept = true;
            writeln!(
                stdout,
                "  {}Auto-accept enabled for the rest of this turn{}",
                GREEN, RESET
            )?;
            Ok(true)
        }
        Confirmation::Yes => Ok(true),
    }
}

/// Execute a generic tool call, display the result summary, and record the
/// tool result as a message in the conversation.
async fn execute_generic_tool<W: Write>(
    call: &ToolCall,
    tool_manager: &ToolManager,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
) {
    stdout
        .write_all(
            format!(
                "  {}▶ {}{} Executing {}...\n",
                CYAN, RESET, call.function.name, RESET
            )
            .as_bytes(),
        )
        .unwrap();
    stdout.flush().unwrap();
    let result = tool_manager
        .execute_tool_call(&call.function.name, &call.function.arguments)
        .await;

    // For tools that return potentially large listings, show only a summary
    // line to keep the terminal clean. The full content is still sent to the
    // LLM in the message below.
    match call.function.name.as_str() {
        "read" => {
            let summary = result.lines().next().unwrap_or("(empty result)");
            writeln!(stdout, "    {}", summary).unwrap();
        }
        "ls" | "grep" | "glob" | "git_status" | "git_diff" => {
            let summary = summarize_listing_result(&result, &call.function.name);
            writeln!(stdout, "    {}", summary).unwrap();
        }
        _ => {
            // Display the full result, multiline, with lines wrapped at word boundaries
            crate::ui::wrap::write_wrapped_lines(
                stdout,
                &result,
                "    ",
                "      ",
                crate::ui::wrap::MAX_LINE_WIDTH,
            )
            .unwrap();
        }
    }
    stdout.flush().unwrap();
    messages.push(Message {
        role: Role::Tool,
        content: format!(
            "Tool '{}' result:\n{}\n\nUse this result to continue helping the user.",
            call.function.name, result
        ),
        tool_calls: vec![],
    });
    session.append_message(messages.last().unwrap());
}

/// Handle the switch_mode signal: update dispatcher and system prompt.
fn handle_switch_mode<W: Write>(
    new_mode: AgentMode,
    dispatcher: &mut CommandDispatcher,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
) -> Result<(), Box<dyn Error>> {
    let old_mode = dispatcher.current_mode;
    match dispatcher.switch_mode(new_mode, messages) {
        Ok(()) => {
            session.set_mode(new_mode);

            writeln!(
                stdout,
                "\n{}{}🔄 Mode switched: {} → {}{}",
                BOLD, BLUE, old_mode, new_mode, RESET
            )?;
            stdout.flush()?;

            messages.push(Message {
                role: Role::Tool,
                content: format!(
                    "SUCCESS: Mode switched from '{}' to '{}'. The assistant is now in {} mode and will use the appropriate toolset and behavior.",
                    old_mode, new_mode, new_mode
                ),
                tool_calls: vec![],
            });
            session.append_message(messages.last().unwrap());
        }
        Err(msg) => {
            writeln!(stdout, "  {}{}{}", ORANGE, msg, RESET)?;
            messages.push(Message {
                role: Role::Tool,
                content: format!("Already in '{}' mode. No change was made.", new_mode),
                tool_calls: vec![],
            });
            session.append_message(messages.last().unwrap());
        }
    }
    Ok(())
}

/// Handle the question signal: display options and prompt user for a choice.
fn handle_question<W: Write>(
    question_text: &str,
    answers: &[String],
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
) -> Result<(), Box<dyn Error>> {
    if question_text.is_empty() {
        messages.push(Message {
            role: Role::Tool,
            content: "Error: 'question' argument is required for the question tool.".to_string(),
            tool_calls: vec![],
        });
        session.append_message(messages.last().unwrap());
        return Ok(());
    }

    if answers.is_empty() {
        messages.push(Message {
            role: Role::Tool,
            content:
                "Error: 'answers' argument must contain at least one option for the question tool."
                    .to_string(),
            tool_calls: vec![],
        });
        session.append_message(messages.last().unwrap());
        return Ok(());
    }

    // Display the question and options
    writeln!(
        stdout,
        "\n{}  ┌─── {}❓ Question {}─────{}",
        BOLD, CYAN, BOLD, RESET
    )?;
    writeln!(stdout, "  │ {}{}{}", BOLD, question_text, RESET)?;
    writeln!(stdout, "  │")?;
    for (i, answer) in answers.iter().enumerate() {
        writeln!(
            stdout,
            "  │   {}{}.{}) {} {}{}",
            GREEN,
            i + 1,
            RESET,
            BOLD,
            answer,
            RESET
        )?;
    }
    writeln!(stdout, "  └{}──────────────────────────────{}", BOLD, RESET)?;

    let answer_count = answers.len();
    write!(
        stdout,
        "  {}Your choice (1-{}): {}",
        BOLD, answer_count, RESET
    )?;
    stdout.flush()?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .expect("Failed to read line");
    let input = input.trim();

    // Parse the user's choice — by number or by text match
    let selected_answer = if let Ok(num) = input.parse::<usize>() {
        if num >= 1 && num <= answer_count {
            answers[num - 1].clone()
        } else {
            format!("Invalid choice: {} (valid range: 1-{})", num, answer_count)
        }
    } else if !input.is_empty() {
        // Try to match by text (case-insensitive)
        let input_lower = input.to_lowercase();
        match answers.iter().find(|a| a.to_lowercase() == input_lower) {
            Some(a) => a.clone(),
            None => input.to_string(), // Allow free-form answer
        }
    } else {
        "No answer provided (empty input)".to_string()
    };

    writeln!(
        stdout,
        "  {}✓{} Selected: {}{}{}",
        GREEN, RESET, BOLD, selected_answer, RESET
    )?;
    stdout.flush()?;

    messages.push(Message {
        role: Role::Tool,
        content: format!(
            "User answered the question '{}' with: '{}'.\n\nUse this answer to continue helping the user.",
            question_text, selected_answer
        ),
        tool_calls: vec![],
    });
    session.append_message(messages.last().unwrap());
    Ok(())
}

/// Handle the auto_compact signal: trigger conversation compaction.
async fn handle_auto_compact<W: Write>(
    focus: &str,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
    provider: Arc<Mutex<dyn Provider + Send + Sync>>,
) -> Result<(), Box<dyn Error>> {
    writeln!(
        stdout,
        "\n{}  {}▶ auto_compact{} Compacting conversation history...",
        DIM, CYAN, RESET
    )?;
    stdout.flush()?;

    // Lock the provider and perform compaction
    let mut provider_guard = provider.lock().await;

    match execute_compact(&mut *provider_guard, messages, focus).await {
        Ok(()) => {
            // Compaction successful
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
            });
            session.append_message(messages.last().unwrap());
        }
        Err(e) => {
            messages.push(Message {
                role: Role::Tool,
                content: format!(
                    "Auto-compact failed: {}. The conversation was not modified.",
                    e
                ),
                tool_calls: vec![],
            });
            session.append_message(messages.last().unwrap());
        }
    }

    Ok(())
}

/// Extract the result body from a tool result message.
fn extract_tool_result_body<'a>(content: &'a str, tool_name: &str) -> &'a str {
    let prefix = format!("Tool '{tool_name}' result:\n");
    content
        .strip_prefix(&prefix)
        .or_else(|| content.strip_prefix(&prefix)) // Redundant but kept for safety
        .unwrap_or(content)
}

/// Print the conversation history from loaded messages so the user can see
/// what was discussed in the resumed session. The format mimics the live
/// conversation experience so it feels like you're picking up where you left off.
fn print_conversation_history<W: Write>(
    messages: &[Message],
    stdout: &mut W,
) -> Result<(), Box<dyn Error>> {
    if messages.is_empty() {
        return Ok(());
    }

    for msg in messages {
        match msg.role {
            Role::System => {} // System prompt is internal — skip it in the replay.
            Role::User => {
                writeln!(stdout, "{}> {}{}", BLUE, msg.content, RESET)?;
            }
            Role::Assistant => {
                if !msg.content.is_empty() {
                    write!(stdout, "{}", ORANGE)?;
                    stdout.write_all(msg.content.as_bytes())?;
                    writeln!(stdout, "{}", RESET)?;
                }
                if !msg.tool_calls.is_empty() {
                    for tc in &msg.tool_calls {
                        writeln!(stdout, "{}  {}▶{} {}", DIM, CYAN, RESET, tc.function.name)?;
                    }
                }
                writeln!(stdout)?;
            }
            Role::Tool => {
                let tool_name = msg.content.split('\'').nth(1).unwrap_or("tool");
                let result_body = extract_tool_result_body(&msg.content, tool_name);

                if tool_name == "read" {
                    let summary = result_body.lines().next().unwrap_or("(empty result)");
                    writeln!(stdout, "    {}", summary)?;
                } else if matches!(
                    tool_name,
                    "ls" | "grep" | "glob" | "git_status" | "git_diff"
                ) {
                    let summary = summarize_listing_result(result_body, tool_name);
                    writeln!(stdout, "    {}", summary)?;
                } else {
                    crate::ui::wrap::write_wrapped_lines(
                        stdout,
                        result_body,
                        "    ",
                        "      ",
                        crate::ui::wrap::MAX_LINE_WIDTH,
                    )?;
                }
            }
        }
    }

    stdout.flush()?;
    Ok(())
}

/// Produce a one-line summary for listing tools (ls, grep, glob) so the
/// terminal isn't flooded with potentially large output. Error and empty
/// messages are passed through verbatim; otherwise the count of result lines
/// is shown along with the first few entries as a preview.
fn summarize_listing_result(result: &str, tool_name: &str) -> String {
    // Pass through error messages and empty-result messages as-is.
    if result.starts_with("Error:") || result.starts_with("No ") || result == "Directory is empty" {
        return result.to_string();
    }

    let lines: Vec<&str> = result.lines().collect();
    let count = lines.len();
    let label = match tool_name {
        "ls" => "entries",
        "grep" => "matches",
        "glob" => "files",
        "git_status" => "status lines",
        "git_diff" => "diff lines",
        _ => "results",
    };

    // Show a preview of the first few entries alongside the total count.
    const PREVIEW: usize = 3;
    if count <= PREVIEW {
        format!("{} {} — {}", count, label, result)
    } else {
        let preview: Vec<&str> = lines.iter().take(PREVIEW).copied().collect();
        format!(
            "{} {} — {} ... ({} more)",
            count,
            label,
            preview.join(", "),
            count - PREVIEW
        )
    }
}

/// Format tool call arguments as a compact single-line summary.
fn format_args_summary(arguments: &serde_json::Value) -> String {
    match arguments {
        serde_json::Value::Object(map) => {
            let parts: Vec<String> = map
                .iter()
                .map(|(key, val)| {
                    let val_str = match val {
                        serde_json::Value::String(s) => {
                            if s.len() > 60 {
                                format!("\"{}...\"", &s[..57])
                            } else {
                                format!("\"{}\"", s)
                            }
                        }
                        other => other.to_string(),
                    };
                    format!("{}={}", key, val_str)
                })
                .collect();
            parts.join(", ")
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_summarize_ls_error_passthrough() {
        let result = summarize_listing_result("Error: Path '/nope' does not exist", "ls");
        assert_eq!(result, "Error: Path '/nope' does not exist");
    }

    #[test]
    fn test_summarize_ls_empty_dir() {
        let result = summarize_listing_result("Directory is empty", "ls");
        assert_eq!(result, "Directory is empty");
    }

    #[test]
    fn test_summarize_ls_no_matches() {
        let result = summarize_listing_result("No matches found for pattern 'xyz'", "grep");
        assert_eq!(result, "No matches found for pattern 'xyz'");
    }

    #[test]
    fn test_summarize_ls_few_entries() {
        let result = summarize_listing_result("Cargo.toml\nCargo.lock\nsrc", "ls");
        assert_eq!(result, "3 entries — Cargo.toml\nCargo.lock\nsrc");
    }

    #[test]
    fn test_summarize_ls_many_entries() {
        let entries: Vec<String> = (0..10).map(|i| format!("file{i}")).collect();
        let input = entries.join("\n");
        let result = summarize_listing_result(&input, "ls");
        assert_eq!(result, "10 entries — file0, file1, file2 ... (7 more)");
    }

    #[test]
    fn test_summarize_grep_many_matches() {
        let matches: Vec<String> = (1..=5).map(|i| format!("src/main.rs:{}:foo", i)).collect();
        let input = matches.join("\n");
        let result = summarize_listing_result(&input, "grep");
        assert_eq!(
            result,
            "5 matches — src/main.rs:1:foo, src/main.rs:2:foo, src/main.rs:3:foo ... (2 more)"
        );
    }

    #[test]
    fn test_summarize_glob_no_files() {
        let result = summarize_listing_result("No files found matching pattern '*.xyz'", "glob");
        assert_eq!(result, "No files found matching pattern '*.xyz'");
    }

    #[test]
    fn test_summarize_glob_many_files() {
        let files: Vec<String> = (0..6).map(|i| format!("src/file{i}.rs")).collect();
        let input = files.join("\n");
        let result = summarize_listing_result(&input, "glob");
        assert_eq!(
            result,
            "6 files — src/file0.rs, src/file1.rs, src/file2.rs ... (3 more)"
        );
    }

    #[test]
    fn test_summarize_single_entry() {
        let result = summarize_listing_result("Cargo.toml", "ls");
        assert_eq!(result, "1 entries — Cargo.toml");
    }
}
