use std::io::Write;

use tokio::sync::Mutex;

use tinyharness_lib::{
    config::load_settings,
    mode::AgentMode,
    provider::{Message, Role, ToolCall},
    session::Session,
    tools::SignalEvent,
    tools::ToolManager,
};

use crate::commands::compact::execute_compact;
use crate::style::*;
use crate::ui::confirm::Confirmation;

use super::safety::is_safe_command;

/// Handle tool calls from the assistant response.
///
/// Returns `Ok(true)` if tool results were added to messages (the caller should
/// continue the inner loop), or `Ok(false)` if no tool calls were present.
#[allow(clippy::too_many_arguments)]
pub async fn handle_tool_calls<W: Write>(
    tool_calls: &[ToolCall],
    response_content: &str,
    messages: &mut Vec<Message>,
    tool_manager: &ToolManager,
    dispatcher: &mut crate::commands::CommandDispatcher,
    stdout: &mut W,
    auto_accept: &mut bool,
    session: &mut Session,
    provider: std::sync::Arc<Mutex<dyn tinyharness_lib::provider::Provider + Send + Sync>>,
    interrupted: &std::sync::atomic::AtomicBool,
) -> Result<bool, Box<dyn std::error::Error>> {
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
        if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            interrupted.store(false, std::sync::atomic::Ordering::SeqCst);
            writeln!(
                stdout,
                "\n{}⚠ Tool execution interrupted by user.{}",
                ORANGE, RESET
            )?;
            stdout.flush()?;
            return Ok(true);
        }

        // Signal tools are handled specially by the agent loop — they have their
        // own user interaction (question prompts the user, switch_mode changes
        // mode, auto_compact compacts) so they skip the generic confirmation gate.
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
                            std::sync::Arc::clone(&provider),
                        )
                        .await?;
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
                });
                session.append_message(messages.last().unwrap());
            }
            continue;
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
            let args_summary = super::display::format_args_summary(&call.function.arguments);
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

        // Generic tool execution
        execute_generic_tool(call, tool_manager, messages, session, stdout, auto_accepted).await;
    }

    Ok(true)
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
) -> Result<(bool, bool), Box<dyn std::error::Error>> {
    if !needs_confirmation {
        return Ok((true, false));
    }

    // Check for auto-accept mode (but still validate run commands)
    if *auto_accept {
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

    match crate::ui::confirm::prompt_tool_confirmation(stdout, call)? {
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
async fn execute_generic_tool<W: Write>(
    call: &ToolCall,
    tool_manager: &ToolManager,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
    auto_accepted: bool,
) {
    if auto_accepted {
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

    // Capture start time for duration tracking
    let start_time = std::time::Instant::now();

    let result = tool_manager
        .execute_tool_call(&call.function.name, &call.function.arguments)
        .await;

    let duration_ms = start_time.elapsed().as_millis() as u64;

    // Log to audit if this is a "run" command
    if call.function.name == "run"
        && let Some(cmd) = call
            .function
            .arguments
            .get("command")
            .and_then(|v| v.as_str())
    {
        let exit_code = if result.starts_with("Error:") { -1 } else { 0 };
        let session_id = session.id().to_string();
        crate::commands::audit::log_command(
            &session_id,
            cmd,
            exit_code,
            auto_accepted,
            duration_ms,
        );
    }

    // For tools that return potentially large listings, show only a summary
    match call.function.name.as_str() {
        "read" => {
            let summary = result.lines().next().unwrap_or("(empty result)");
            writeln!(stdout, "    {}", summary).unwrap();
        }
        "ls" | "grep" | "glob" => {
            let summary = super::display::summarize_listing_result(&result, &call.function.name);
            writeln!(stdout, "    {}", summary).unwrap();
        }
        _ => {
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
    dispatcher: &mut crate::commands::CommandDispatcher,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
) -> Result<(), Box<dyn std::error::Error>> {
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
) -> Result<(), Box<dyn std::error::Error>> {
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
    writeln!(stdout, "  │")?;
    writeln!(
        stdout,
        "  │   {}Enter anything else to skip with a custom answer{}",
        DIM, RESET
    )?;
    writeln!(stdout, "  └{}──────────────────────────────{}", BOLD, RESET)?;

    let answer_count = answers.len();
    write!(
        stdout,
        "  {}Your choice (1-{} or type to skip): {}",
        BOLD, answer_count, RESET
    )?;
    stdout.flush()?;

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .expect("Failed to read line");
    let input = input.trim();

    // Determine if the user selected an option or skipped with custom input
    let (selected_answer, is_skip) = if input.is_empty() {
        ("Skipped (no answer provided)".to_string(), true)
    } else if let Ok(num) = input.parse::<usize>() {
        if num >= 1 && num <= answer_count {
            (answers[num - 1].clone(), false)
        } else {
            // Out-of-range number: treat as a skip with free-form input
            (format!("Skipped (user entered: {})", input), true)
        }
    } else {
        let input_lower = input.to_lowercase();
        match answers.iter().find(|a| a.to_lowercase() == input_lower) {
            Some(a) => (a.clone(), false),
            None => (input.to_string(), true), // Free-form answer (skip)
        }
    };

    if is_skip {
        writeln!(
            stdout,
            "  {}⊘{} Skipped with: {}{}{}",
            ORANGE, RESET, BOLD, selected_answer, RESET
        )?;
    } else {
        writeln!(
            stdout,
            "  {}✓{} Selected: {}{}{}",
            GREEN, RESET, BOLD, selected_answer, RESET
        )?;
    }
    stdout.flush()?;

    let result_content = if is_skip {
        format!(
            "User skipped the provided options for the question '{}' and entered a custom answer: '{}'.\n\nUse this answer to continue helping the user.",
            question_text, selected_answer
        )
    } else {
        format!(
            "User answered the question '{}' with: '{}'.\n\nUse this answer to continue helping the user.",
            question_text, selected_answer
        )
    };

    messages.push(Message {
        role: Role::Tool,
        content: result_content,
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
    provider: std::sync::Arc<Mutex<dyn tinyharness_lib::provider::Provider + Send + Sync>>,
) -> Result<(), Box<dyn std::error::Error>> {
    writeln!(
        stdout,
        "\n{}  {}▶ auto_compact{} Compacting conversation history...",
        DIM, CYAN, RESET
    )?;
    stdout.flush()?;

    let mut provider_guard = provider.lock().await;

    match execute_compact(&mut *provider_guard, messages, focus).await {
        Ok(()) => {
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
