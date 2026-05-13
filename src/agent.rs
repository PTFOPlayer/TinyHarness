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

/// Read input from the user with support for multi-line continuation.
///
/// Uses rustyline's validator to detect incomplete input (trailing backslash,
/// unclosed code fences, etc.) and shows a continuation prompt for additional lines.
///
/// Returns:
/// - `Ok(Some(String))` - Complete input received
/// - `Ok(None)` - EOF (Ctrl+D) or unrecoverable error
/// - `Err(...)` - Read error
fn read_multiline_input<W: Write>(
    rl: &mut Editor<CommandHelper, rustyline::history::DefaultHistory>,
    prompt: &str,
    continuation_prompt: &str,
    interrupted: &Arc<AtomicBool>,
    stdout: &mut W,
) -> Result<Option<String>, Box<dyn Error>> {
    let mut input = String::new();
    let mut is_first_line = true;

    loop {
        let current_prompt = if is_first_line {
            prompt
        } else {
            continuation_prompt
        };

        let readline = rl.readline(current_prompt);

        match readline {
            Ok(line) => {
                if is_first_line {
                    input = line;
                } else {
                    // Append continuation line with newline
                    input.push('\n');
                    input.push_str(&line);
                }

                // Check if the validator considers this complete
                // We need to manually check since rustyline handles this internally
                let trimmed = input.trim_end();
                let ends_with_backslash = trimmed.ends_with('\\');
                let fence_count = input.matches("```").count();
                let has_unclosed_fence = fence_count % 2 == 1;
                let backtick_count = input.matches('`').count();
                let has_unclosed_backtick = backtick_count % 2 == 1;

                if ends_with_backslash || has_unclosed_fence || has_unclosed_backtick {
                    // Incomplete - continue reading
                    is_first_line = false;
                    continue;
                }

                // Complete input
                return Ok(Some(input));
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
                return Ok(None); // Return None to continue the main loop
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                stdout.write_all("\n".as_bytes())?;
                return Ok(None); // EOF - signal to exit
            }
            Err(err) => {
                eprintln!("{}Error reading input: {}{}", RED, err, RESET);
                return Ok(None);
            }
        }
    }
}

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

    // Warn if the loaded session is near or over the context window limit.
    print_context_load_warning(messages, &mut stdout)?;

    let helper = CommandHelper::new();
    let history_dir = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/share/tinyharness"))
        .unwrap_or_else(|_| std::path::PathBuf::from(".tinyharness_history"));
    std::fs::create_dir_all(&history_dir).ok();
    let history_path = history_dir.join("history.txt");
    let mut rl = Editor::new()?;
    rl.set_helper(Some(helper));
    rl.load_history(&history_path).ok();

    // Configure multi-line input:
    // - Ctrl+J inserts a newline
    // - Enter submits the input
    // - Validator detects incomplete input (trailing \, unclosed fences) and shows continuation prompt
    rl.bind_sequence(
        rustyline::KeyEvent(rustyline::KeyCode::Char('j'), rustyline::Modifiers::CTRL),
        rustyline::EventHandler::Simple(rustyline::Cmd::Newline),
    );

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
        let continuation_prompt = format!(
            "{}[{}]{} {}{}...> {}{}",
            BOLD, mode_label, RESET, GRAY, context_info, BLUE, RESET
        );

        // Read input with support for multi-line continuation
        let user_input = read_multiline_input(
            &mut rl,
            &prompt,
            &continuation_prompt,
            interrupted,
            &mut stdout,
        )?;

        if user_input.is_none() {
            // EOF or error - exit
            break;
        }

        let user_input = user_input.unwrap();
        let trimmed = user_input.trim().to_string();
        if trimmed.is_empty() {
            continue;
        }
        rl.add_history_entry(&trimmed)?;

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
                                            // Warn if the loaded session is near or over the context window limit
                                            print_context_load_warning(messages, &mut stdout)?;
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

        // Load settings to check auto_accept_safe_commands preference and safe/denied commands
        let settings = load_settings();
        let auto_accept_safe_commands = settings.auto_accept_safe_commands;
        let safe_commands = settings.get_safe_commands();
        let denied_commands = settings.get_denied_commands();

        // Confirmation step
        let (approved, auto_accepted) = confirm_tool_call(
            call,
            needs_confirmation,
            auto_accept,
            stdout,
            auto_accept_safe_commands,
            &safe_commands,
            &denied_commands,
        )?;

        if !approved {
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
        execute_generic_tool(call, tool_manager, messages, session, stdout, auto_accepted).await;
    }

    Ok(true)
}

/// Strip shell descriptor redirections that are considered safe:
/// - `2>&1`  — redirect stderr to stdout
/// - `N>/dev/null` — redirect a file descriptor to /dev/null
///
/// These don't write to arbitrary files so they're safe for auto-accept.
fn strip_safe_descriptor_redirections(command: &str) -> String {
    // Remove `2>&1` (and `1>&2`, `0>&1`, etc. — any single-digit descriptor redirect)
    let re = regex::Regex::new(r"\d>&\d").unwrap();
    let result = re.replace_all(command, "");
    // Remove `N>/dev/null` patterns (e.g. `2>/dev/null`, `2> /dev/null`)
    let re = regex::Regex::new(r"\d>\s*/dev/null").unwrap();
    re.replace_all(&result, "").to_string()
}

/// Check if a shell command is safe to auto-accept (read-only operations).
/// Returns `true` if the command only uses safe, read-only utilities AND
/// does not match any prefix in the denied list.
fn is_safe_command(command: &str, safe_commands: &[String], denied_commands: &[String]) -> bool {
    let command = command.trim();

    // Accept empty commands
    if command.is_empty() {
        return true;
    }

    // Check the denied list first — it takes priority over the safe list.
    // If a command matches a denied prefix, it is always blocked from auto-accept.
    for denied in denied_commands {
        if command.starts_with(denied) {
            let rest = &command[denied.len()..];
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('=') {
                return false;
            }
        }
    }

    // Reject commands with newlines — could hide a second command
    if command.contains('\n') {
        return false;
    }

    // Reject commands with semicolons — `cd /path ; rm -rf /` bypasses safety
    if command.contains(';') {
        return false;
    }

    // Strip safe descriptor redirections early so they don't trigger
    // false positives in later checks:
    //   - `2>&1` contains `&` (would be flagged as background operator)
    //   - `2>/dev/null` contains `>` (would be flagged as file redirection)
    // These patterns are safe — they only redirect stderr, not arbitrary files.
    let stripped = strip_safe_descriptor_redirections(command);

    // Reject commands with single `&` (background operator) — `sleep 1 & rm -rf /`
    // `&&` is a logical AND and is handled separately below.
    // Check for any `&` that is not part of `&&`.
    let bytes = stripped.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'&' {
            // Check if this is part of `&&`
            if i + 1 < bytes.len() && bytes[i + 1] == b'&' {
                i += 2; // skip the `&&`
            } else {
                return false; // lone `&` — background operator
            }
        } else {
            i += 1;
        }
    }

    // Handle `&&` and `||` chaining: models often write `cd /path && ls`
    // Split on `&&` / `||` and check that every part is individually safe.
    // This must come before the `|` check since `||` contains `|`.
    if stripped.contains("&&") || stripped.contains("||") {
        let parts: Vec<&str> = stripped
            .split("&&")
            .flat_map(|part| part.split("||"))
            .collect();
        return parts
            .iter()
            .all(|part| is_safe_command(part, safe_commands, denied_commands));
    }

    // Reject commands with pipes, command substitution, or remaining redirections.
    // Safe descriptor redirections (2>&1, 2>/dev/null) have already been stripped.
    if stripped.contains('|') || stripped.contains("$(") || stripped.contains("`") {
        return false;
    }

    if stripped.contains('>') || stripped.contains('<') {
        return false;
    }

    // Check if command starts with any safe prefix followed by a word boundary
    // (space, end-of-string, or `=`). This prevents `cdx` from matching `cd`.
    // Use the ORIGINAL command (not stripped) for prefix matching, since the
    // stripped version has safe redirections removed.
    for safe_cmd in safe_commands {
        if command.starts_with(safe_cmd) {
            let rest = &command[safe_cmd.len()..];
            if rest.is_empty() || rest.starts_with(' ') || rest.starts_with('=') {
                return true;
            }
        }
    }

    false
}

/// Determine whether a tool call is allowed to proceed.
/// Returns `(approved, auto_accepted)` where:
/// - `approved` is true if the call should proceed
/// - `auto_accepted` is true if it was auto-accepted (no "Executing" header needed)
fn confirm_tool_call<W: Write>(
    call: &ToolCall,
    needs_confirmation: bool,
    auto_accept: &mut bool,
    stdout: &mut W,
    auto_accept_safe_commands: bool,
    safe_commands: &[String],
    denied_commands: &[String],
) -> Result<(bool, bool), Box<dyn Error>> {
    use crate::ui::confirm::Confirmation;

    if !needs_confirmation {
        return Ok((true, false));
    }

    // Check for auto-accept mode (but still validate run commands)
    if *auto_accept {
        // For "run" tool, check if the command is safe
        if call.function.name == "run" {
            if let Some(cmd_value) = call.function.arguments.get("command")
                && let Some(cmd_str) = cmd_value.as_str()
                && is_safe_command(cmd_str, safe_commands, denied_commands)
            {
                return Ok((true, true));
            }
            // Unsafe run command - still require confirmation even in auto-accept mode
        } else {
            // Other tools can be auto-accepted
            return Ok((true, true));
        }
    }

    // Check for auto-accept of safe commands (configurable via settings)
    if auto_accept_safe_commands
        && call.function.name == "run"
        && let Some(cmd_value) = call.function.arguments.get("command")
        && let Some(cmd_str) = cmd_value.as_str()
        && is_safe_command(cmd_str, safe_commands, denied_commands)
    {
        return Ok((true, true));
    }

    match prompt_tool_confirmation(stdout, call)? {
        Confirmation::No => {
            stdout.write_all(format!("  {}Skipped{}{}\n", ORANGE, RESET, BOLD).as_bytes())?;
            stdout.flush()?;
            Ok((false, false))
        }
        Confirmation::AutoAccept => {
            *auto_accept = true;
            writeln!(
                stdout,
                "  {}Auto-accept enabled for the rest of this turn{}",
                GREEN, RESET
            )?;
            Ok((true, true))
        }
        Confirmation::Yes => Ok((true, false)),
    }
}

/// Execute a generic tool call, display the result summary, and record the
/// tool result as a message in the conversation.
///
/// When `auto_accepted` is true, the tool was auto-accepted and a brief
/// status line is printed instead of the full "Executing..." header.
async fn execute_generic_tool<W: Write>(
    call: &ToolCall,
    tool_manager: &ToolManager,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
    auto_accepted: bool,
) {
    if auto_accepted {
        // Brief status line for auto-accepted commands — show what was accepted
        if call.function.name == "run" {
            if let Some(cmd) = call
                .function
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
            {
                writeln!(
                    stdout,
                    "  {}▶ {}{} (auto-accepted){}",
                    DIM, call.function.name, RESET, RESET
                )
                .unwrap();
                writeln!(stdout, "    {}{}{}", CYAN, cmd, RESET).unwrap();
            } else {
                writeln!(
                    stdout,
                    "  {}▶ {}{} (auto-accepted){}",
                    DIM, call.function.name, RESET, RESET
                )
                .unwrap();
            }
        } else {
            writeln!(
                stdout,
                "  {}▶ {}{} (auto-accepted){}",
                DIM, call.function.name, RESET, RESET
            )
            .unwrap();
        }
    } else {
        stdout
            .write_all(
                format!(
                    "  {}▶ {}{} Executing {}...\n",
                    CYAN, RESET, call.function.name, RESET
                )
                .as_bytes(),
            )
            .unwrap();
    }
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
        "ls" | "grep" | "glob" => {
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

/// Print a warning if the loaded session's conversation is near or exceeding
/// the context window limit. This helps the user understand why the model may
/// behave oddly and nudges them to use `/compact`.
fn print_context_load_warning<W: Write>(
    messages: &[Message],
    stdout: &mut W,
) -> Result<(), Box<dyn Error>> {
    if messages.len() <= 1 {
        return Ok(());
    }

    let estimated_tokens = estimate_conversation_tokens(messages);
    let settings = load_settings();
    let context_size = settings
        .context_limit
        .map(ContextWindowSize::Custom)
        .unwrap_or_else(ContextWindowSize::default_size);
    let usage_pct = context_size.usage_percentage(estimated_tokens);

    if usage_pct >= 90.0 {
        // Exceeds or is very close to the context window
        writeln!(
            stdout,
            "\n{}⚠ This session has {} messages ({}{}{}) — exceeds the context window!{}",
            RED,
            messages.len(),
            BOLD,
            format_token_count(estimated_tokens),
            RED,
            RESET
        )?;
        writeln!(
            stdout,
            "{}  The conversation may not work properly until you compact it.{}",
            RED, RESET
        )?;
        writeln!(
            stdout,
            "{}  Use {}/compact{} [focus] to summarize older messages.{}",
            RED, BOLD, RED, RESET
        )?;
    } else if usage_pct >= 70.0 {
        // Warning zone
        writeln!(
            stdout,
            "\n{}⚠ This session has {} messages ({}{}{}, {:.1}% of context).{}",
            YELLOW,
            messages.len(),
            BOLD,
            format_token_count(estimated_tokens),
            YELLOW,
            usage_pct,
            RESET
        )?;
        writeln!(
            stdout,
            "{}  Consider using {}/compact{} to free context space.{}",
            YELLOW, BOLD, YELLOW, RESET
        )?;
    }

    stdout.flush()?;
    Ok(())
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
                } else if matches!(tool_name, "ls" | "grep" | "glob") {
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

    #[test]
    fn test_is_safe_command_basic() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(is_safe_command("ls", &safe_commands, &[]));
        assert!(is_safe_command("ls -la", &safe_commands, &[]));
        assert!(is_safe_command("grep foo bar.txt", &safe_commands, &[]));
        assert!(is_safe_command("find . -name '*.rs'", &safe_commands, &[]));
        assert!(is_safe_command("cat README.md", &safe_commands, &[]));
        assert!(is_safe_command("pwd", &safe_commands, &[]));
        assert!(is_safe_command("git status", &safe_commands, &[]));
        assert!(is_safe_command("git diff", &safe_commands, &[]));
        assert!(is_safe_command("cargo tree", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_unsafe() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(!is_safe_command("rm -rf /", &safe_commands, &[]));
        assert!(!is_safe_command(
            "echo hello > file.txt",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command("cat file | grep foo", &safe_commands, &[]));
        assert!(!is_safe_command("$(whoami)", &safe_commands, &[]));
        assert!(!is_safe_command("echo `whoami`", &safe_commands, &[]));
        assert!(!is_safe_command("cargo build", &safe_commands, &[]));
        assert!(!is_safe_command("cargo test", &safe_commands, &[]));
        assert!(!is_safe_command("git commit", &safe_commands, &[]));
        assert!(!is_safe_command("git push", &safe_commands, &[]));
        assert!(!is_safe_command("cargo run", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_with_whitespace() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        assert!(is_safe_command("  ls  ", &safe_commands, &[]));
        assert!(is_safe_command("\tgrep foo\n", &safe_commands, &[]));
        assert!(!is_safe_command("  rm -rf  ", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_custom_list() {
        let custom_commands = vec!["ls".to_string(), "custom-cmd".to_string()];
        assert!(is_safe_command("ls", &custom_commands, &[]));
        assert!(is_safe_command("custom-cmd arg", &custom_commands, &[]));
        assert!(!is_safe_command("grep foo", &custom_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_with_chain() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        // cd && safe-command should be safe
        assert!(is_safe_command("cd /some/path && ls", &safe_commands, &[]));
        assert!(is_safe_command(
            "cd /project && git status",
            &safe_commands,
            &[]
        ));
        assert!(is_safe_command(
            "cd /project && git diff",
            &safe_commands,
            &[]
        ));
        assert!(is_safe_command("pwd && ls", &safe_commands, &[]));
        // All parts must be safe
        assert!(!is_safe_command(
            "cd /path && rm -rf /",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command("ls && cargo build", &safe_commands, &[]));
        assert!(!is_safe_command("ls && git push", &safe_commands, &[]));
        // Pipe inside a chained part is still rejected
        assert!(!is_safe_command(
            "cd /path && cat file | grep foo",
            &safe_commands,
            &[]
        ));
        // Redirection inside a chained part is still rejected
        assert!(!is_safe_command(
            "cd /path && echo hello > file.txt",
            &safe_commands,
            &[]
        ));
    }

    #[test]
    fn test_is_safe_command_semicolon_separator() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        // Semicolons are always rejected — they allow chaining arbitrary commands
        assert!(!is_safe_command("cd /path ; rm -rf /", &safe_commands, &[]));
        assert!(!is_safe_command("ls ; pwd", &safe_commands, &[]));
        assert!(!is_safe_command("ls;rm -rf /", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_background_ampersand() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        // Single `&` (background) is a command separator and always rejected
        assert!(!is_safe_command("sleep 1 & rm -rf /", &safe_commands, &[]));
        assert!(!is_safe_command("ls & pwd", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_newline_separator() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        // Newlines inside a command could hide a second command
        assert!(!is_safe_command("cd /path\nrm -rf /", &safe_commands, &[]));
        assert!(!is_safe_command("ls\npwd", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_or_chain() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        // `||` chaining: all parts must be safe
        assert!(is_safe_command("cd /path || ls", &safe_commands, &[]));
        assert!(is_safe_command("pwd || ls", &safe_commands, &[]));
        assert!(!is_safe_command(
            "cd /path || rm -rf /",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command("ls || cargo build", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_mixed_chains() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        // Mix of && and ||
        assert!(is_safe_command(
            "cd /path && ls || pwd",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command(
            "cd /path && ls || rm -rf /",
            &safe_commands,
            &[]
        ));
    }

    #[test]
    fn test_is_safe_command_word_boundary() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();
        // Prefix must be followed by a word boundary (space or end-of-string)
        assert!(is_safe_command("ls", &safe_commands, &[]));
        assert!(is_safe_command("ls -la", &safe_commands, &[]));
        // Fake commands that start with a safe prefix but aren't that command
        assert!(!is_safe_command("lsx", &safe_commands, &[]));
        assert!(!is_safe_command("cdx", &safe_commands, &[]));
        assert!(!is_safe_command("catt", &safe_commands, &[]));
    }

    #[test]
    fn test_is_safe_command_descriptor_redirection() {
        // Use a custom list that includes "cargo test" to test descriptor redirection
        let mut safe_commands = tinyharness_lib::config::get_default_safe_commands();
        safe_commands.push("cargo test".to_string());

        // `2>&1` is a safe descriptor redirection — just redirects stderr to stdout
        assert!(is_safe_command("cargo test 2>&1", &safe_commands, &[]));
        assert!(is_safe_command("ls -la 2>&1", &safe_commands, &[]));
        // `2>/dev/null` is safe — suppresses stderr output
        assert!(is_safe_command(
            "cargo test 2>/dev/null",
            &safe_commands,
            &[]
        ));
        assert!(is_safe_command(
            "find . -name '*.rs' 2>/dev/null",
            &safe_commands,
            &[]
        ));
        // `2> /dev/null` with space is also safe
        assert!(is_safe_command("ls 2> /dev/null", &safe_commands, &[]));
        // `cd /path && cargo test 2>&1` — all parts safe
        assert!(is_safe_command(
            "cd /path && cargo test 2>&1",
            &safe_commands,
            &[]
        ));
        // File redirections to actual files are still unsafe
        assert!(!is_safe_command(
            "echo hello > file.txt",
            &safe_commands,
            &[]
        ));
        assert!(!is_safe_command(
            "cat file > output.txt",
            &safe_commands,
            &[]
        ));
        // Input redirection is still unsafe
        assert!(!is_safe_command("cat < input.txt", &safe_commands, &[]));
    }

    #[test]
    fn test_strip_safe_descriptor_redirections() {
        // Basic 2>&1 removal
        assert_eq!(strip_safe_descriptor_redirections("ls 2>&1"), "ls ");
        // Multiple descriptor redirections
        assert_eq!(
            strip_safe_descriptor_redirections("cmd 2>&1 1>/dev/null"),
            "cmd  "
        );
        // 2>/dev/null removal
        assert_eq!(
            strip_safe_descriptor_redirections("find . 2>/dev/null"),
            "find . "
        );
        // 2> /dev/null with space
        assert_eq!(
            strip_safe_descriptor_redirections("find . 2> /dev/null"),
            "find . "
        );
        // Should NOT strip regular file redirections
        assert_eq!(
            strip_safe_descriptor_redirections("echo hello > file.txt"),
            "echo hello > file.txt"
        );
    }

    #[test]
    fn test_is_safe_command_denied_list() {
        let safe_commands = tinyharness_lib::config::get_default_safe_commands();

        // Without denied list, "git status" is safe
        assert!(is_safe_command("git status", &safe_commands, &[]));

        // With denied list, "git push" is blocked even though "git" prefix
        // would otherwise match safe commands like "git status"
        let denied = vec!["git push".to_string()];
        assert!(is_safe_command("git status", &safe_commands, &denied));
        assert!(!is_safe_command("git push", &safe_commands, &denied));
        assert!(!is_safe_command(
            "git push origin main",
            &safe_commands,
            &denied
        ));

        // Deny "cargo" to block all cargo subcommands
        let denied_cargo = vec!["cargo".to_string()];
        assert!(!is_safe_command(
            "cargo build",
            &safe_commands,
            &denied_cargo
        ));
        assert!(!is_safe_command(
            "cargo test",
            &safe_commands,
            &denied_cargo
        ));
        // "cargo tree" was safe, but now denied
        assert!(!is_safe_command(
            "cargo tree",
            &safe_commands,
            &denied_cargo
        ));

        // Deny list uses prefix matching like safe list
        let denied_echo = vec!["echo".to_string()];
        assert!(!is_safe_command("echo hello", &safe_commands, &denied_echo));
        // "echox" should NOT be denied — prefix must match at word boundary
        // (actually "echox" wouldn't match "echo" prefix since there's no space/end after)
        // Wait, "echox" starts with "echo" but rest is "x" which doesn't start with space
        // So it would NOT be denied. But it also wouldn't be safe since "echox" isn't in safe list.
        // Let's test with something that IS in safe list
        let denied_ps = vec!["ps".to_string()];
        assert!(!is_safe_command("ps", &safe_commands, &denied_ps));
        assert!(!is_safe_command("ps aux", &safe_commands, &denied_ps));
        // "psx" is not denied (word boundary) and not safe (not in list)
        assert!(!is_safe_command("psx", &safe_commands, &denied_ps));

        // Denied list works with && chains
        let denied_git_push = vec!["git push".to_string()];
        // "git push" part is denied, so whole chain is denied
        assert!(!is_safe_command(
            "cd /path && git push",
            &safe_commands,
            &denied_git_push
        ));
        // "git status" is still safe in a chain
        assert!(is_safe_command(
            "cd /path && git status",
            &safe_commands,
            &denied_git_push
        ));
    }
}
