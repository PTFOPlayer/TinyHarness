use std::{
    error::Error,
    io::{self, Write},
    sync::Arc,
};

use rustyline::Editor;
use tokio::sync::{Mutex, mpsc};

use crate::style::*;
use crate::{
    commands::{CommandDispatcher, CommandResult, init},
    mode::AgentMode,
    provider::{ChatMessageResponse, Message, Provider, Role, ToolCall, ToolInfo},
    session::Session,
    tools::ToolManager,
    ui::confirm::prompt_tool_confirmation,
    ui::input::CommandHelper,
};

pub async fn run_agent_loop(
    provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    tool_manager: ToolManager,
    ollama_tools: Vec<ToolInfo>,
    messages: &mut Vec<Message>,
    dispatcher: &mut CommandDispatcher,
    session: &mut Session,
) -> Result<(), Box<dyn Error>> {
    let (send, mut recv) = mpsc::channel::<ChatMessageResponse>(1024);

    let mut stdout = io::stdout();
    stdout.write("╔════════════════════════════════════════════════════════╗\n".as_bytes())?;
    stdout.write("║           TinyHarness AI Assistant                     ║\n".as_bytes())?;
    stdout.write("╚════════════════════════════════════════════════════════╝\n\n".as_bytes())?;
    stdout.write(
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
        let mode_label = dispatcher.current_mode.to_string();
        let msg_count = messages.len();
        let pinned_count = dispatcher.file_context.pinned_file_count();
        let context_info = if pinned_count > 0 {
            format!("{} msgs,{}{}{} pinned", msg_count, BLUE, pinned_count, GRAY)
        } else {
            format!("{} msgs", msg_count)
        };
        let prompt = format!("{}[{}]{} {}{}> {}{}", BOLD, mode_label, RESET, GRAY, context_info, BLUE, RESET);
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
                stdout.write("\n".as_bytes())?;
                stdout.write(
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
                stdout.write("\n".as_bytes())?;
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
                            match Session::find_by_prefix(&id_prefix) {
                                Ok(full_id) => {
                                    // Flush current session before switching
                                    session.flush();
                                    match Session::load(&full_id) {
                                        Ok((new_session, loaded_msgs)) => {
                                            let meta = new_session.meta();
                                            let name = meta.name.as_deref().unwrap_or("unnamed");
                                            eprintln!(
                                                "{}Switched to session {}{}{} — {}{}{} ({} messages, {}){}",
                                                BOLD, BLUE, &meta.id[..12], RESET,
                                                BOLD, name, RESET,
                                                meta.message_count, meta.mode, RESET
                                            );
                                            *session = new_session;
                                            *messages = loaded_msgs;
                                            // Update dispatcher mode and session ID to match loaded session
                                            dispatcher.current_mode = session.meta().mode;
                                            dispatcher.session_id = Some(session.id().to_string());
                                            // Ensure system prompt reflects current context
                                            dispatcher.refresh_system_prompt(messages);
                                            // Print loaded conversation history
                                            print_conversation_history(&messages, &mut stdout)?;
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
                            eprintln!(
                                "{}Session renamed to {}{}{}",
                                BOLD, BLUE, new_name, RESET
                            );
                        }
                        Ok(CommandResult::Init(result)) => {
                            match &result {
                                init::InitResult::Created { path } => {
                                    eprintln!(
                                        "{}  Created {}{}{} — workspace context refreshed.{}",
                                        GREEN, BLUE, path.display(), GREEN, RESET
                                    );
                                }
                                init::InitResult::Updated { path } => {
                                    eprintln!(
                                        "{}  Updated {}{}{} — workspace context refreshed.{}",
                                        GREEN, BLUE, path.display(), GREEN, RESET
                                    );
                                }
                            }
                        }
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

        // Drain any leftover messages in the channel
        while let Ok(_) = recv.try_recv() {}

        // auto_accept persists across all agent iterations within this user turn,
        // so that pressing 'a' once auto-accepts all subsequent tool calls.
        let mut auto_accept = false;

        loop {
            let messages_cloned = messages.clone();
            let send_cloned = send.clone();
            let provider_cloned = Arc::clone(&provider);
            // Filter tools based on current mode
            let tools = match dispatcher.current_mode {
                AgentMode::Agent => ollama_tools.clone(),
                AgentMode::Planning => tool_manager.get_readonly_tools(),
                AgentMode::Casual => Vec::new(),
                AgentMode::Research => tool_manager.get_research_tools(),
            };
            let cloned_user_input = user_input.clone();
            tokio::spawn(async move {
                let mut provider = provider_cloned.lock().await;
                provider
                    .chat(messages_cloned, cloned_user_input, send_cloned, tools)
                    .await;
            });

            let mut response_content = String::new();
            let mut tool_calls: Vec<ToolCall> = Vec::new();
            let mut received_done = false;

            stdout.write(ORANGE.as_bytes())?;

            while let Some(msg) = recv.recv().await {
                if !msg.message.tool_calls.is_empty() {
                    tool_calls = msg.message.tool_calls.clone();
                }

                if msg.done {
                    received_done = true;
                }

                if !msg.message.content.is_empty() {
                    response_content.push_str(&msg.message.content);
                    stdout.write(format!("{}", msg.message.content).as_bytes())?;
                    stdout.flush()?;
                }

                if received_done {
                    break;
                }
            }

            if !received_done {
                stdout.write(RESET.as_bytes())?;
                stdout.write(
                    format!(
                        "{}Error:{} Provider request failed or was interrupted.\n",
                        RED, RESET
                    )
                    .as_bytes(),
                )?;
                messages.pop();
                break;
            }

            stdout.write(RESET.as_bytes())?;

            if handle_tool_calls(
                &tool_calls,
                &response_content,
                messages,
                &tool_manager,
                dispatcher,
                &mut stdout,
                &mut auto_accept,
                session,
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

            // Warn if conversation is getting long
            if messages.len() > 30 && messages.len() % 10 == 0 {
                writeln!(
                    stdout,
                    "\n{}{}⚠ Context is getting large ({} messages). Consider using {}/compact{} to summarize.{}",
                    YELLOW, BOLD, messages.len(), BLUE, YELLOW, RESET
                )?;
            }

            break;
        }

        stdout.write("\n".as_bytes())?;
        stdout.flush()?;
    }

    // Save history on exit
    rl.save_history(&history_path).ok();

    Ok(())
}

async fn handle_tool_calls<W: Write>(
    tool_calls: &[ToolCall],
    response_content: &str,
    messages: &mut Vec<Message>,
    tool_manager: &ToolManager,
    dispatcher: &mut CommandDispatcher,
    stdout: &mut W,
    auto_accept: &mut bool,
    session: &mut Session,
) -> Result<bool, Box<dyn Error>> {
    if tool_calls.is_empty() {
        return Ok(false);
    }

    let tool_count = tool_calls.len();
    writeln!(
        stdout,
        "\n{}  {} tool call(s){}",
        DIM, tool_count, RESET
    )?;

    messages.push(Message {
        role: Role::Assistant,
        content: response_content.to_string(),
        tool_calls: tool_calls.to_vec(),
    });
    session.append_message(messages.last().unwrap());

    let sensitive_tools = ["run", "write", "edit", "switch_mode"];

    for call in tool_calls {
        let needs_confirmation = sensitive_tools.contains(&call.function.name.as_str());

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

        // Special handling: switch_mode tool performs an actual mode switch through the dispatcher.
        if call.function.name == "switch_mode" {
            handle_switch_mode(call, dispatcher, messages, session, stdout)?;
            continue;
        }

        // Special handling: question tool prompts the user for an answer interactively.
        if call.function.name == "question" {
            handle_question(call, messages, session, stdout)?;
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
            "  {}{} {}{} (auto-accepted)",
            DIM, "▶", call.function.name, RESET
        )?;
        return Ok(true);
    }

    match prompt_tool_confirmation(stdout, call)? {
        Confirmation::No => {
            stdout
                .write(format!("  {}Skipped{}{}\n", ORANGE, RESET, BOLD).as_bytes())?;
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
    stdout.write(format!("  {}▶{} Executing {}...\n", CYAN, RESET, call.function.name).as_bytes()).unwrap();
    stdout.flush().unwrap();
    let result = tool_manager.execute_tool_call(&call.function.name, &call.function.arguments).await;

    // Display the full result, multiline, with lines wrapped at word boundaries
    let max_line_width = 120;
    for line in result.lines() {
        if line.is_empty() {
            writeln!(stdout).unwrap();
            continue;
        }
        if line.len() <= max_line_width {
            writeln!(stdout, "    {}", line).unwrap();
        } else {
            // Word-wrap long lines
            let mut remaining = line;
            let mut first = true;
            while !remaining.is_empty() {
                let indent = if first { "    " } else { "      " };
                if remaining.len() <= max_line_width - indent.len() {
                    writeln!(stdout, "{}{}", indent, remaining).unwrap();
                    break;
                }
                // Find the last space within the width limit
                let chunk_end = remaining.floor_char_boundary(max_line_width - indent.len());
                let chunk = &remaining[..chunk_end];
                let split_at = chunk.rfind(' ').unwrap_or(chunk_end);
                if split_at == 0 {
                    // No space found at all — just break at the width limit
                    writeln!(stdout, "{}{}", indent, &remaining[..chunk_end]).unwrap();
                    remaining = remaining[chunk_end..].trim_start();
                } else {
                    writeln!(stdout, "{}{}", indent, &chunk[..split_at]).unwrap();
                    remaining = remaining[chunk[..split_at].len()..].trim_start();
                }
                first = false;
            }
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

/// Handle the switch_mode tool: parse mode, update dispatcher, and update system prompt.
fn handle_switch_mode<W: Write>(
    call: &ToolCall,
    dispatcher: &mut CommandDispatcher,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
) -> Result<(), Box<dyn Error>> {
    let mode_str = call.function.arguments
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    if mode_str.is_empty() {
        messages.push(Message {
            role: Role::Tool,
            content: "Error: 'mode' argument is required for switch_mode. Valid values: casual, planning, agent, research".to_string(),
            tool_calls: vec![],
        });
        session.append_message(messages.last().unwrap());
        return Ok(());
    }

    match mode_str.parse::<AgentMode>() {
        Ok(new_mode) => {
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
                    writeln!(
                        stdout,
                        "  {}{}{}",
                        ORANGE, msg, RESET
                    )?;
                    messages.push(Message {
                        role: Role::Tool,
                        content: format!(
                            "Already in '{}' mode. No change was made.",
                            new_mode
                        ),
                        tool_calls: vec![],
                    });
                    session.append_message(messages.last().unwrap());
                }
            }
        }
        Err(e) => {
            writeln!(stdout, "  {}Error: {}{}", RED, e, RESET)?;
            messages.push(Message {
                role: Role::Tool,
                content: format!("Error: {}. Valid modes: casual, planning, agent, research", e),
                tool_calls: vec![],
            });
            session.append_message(messages.last().unwrap());
        }
    }
    Ok(())
}

/// Handle the question tool: display options and prompt user for a choice.
fn handle_question<W: Write>(
    call: &ToolCall,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
) -> Result<(), Box<dyn Error>> {
    let question_text = call.function.arguments
        .get("question")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();

    let answers: Vec<String> = call.function.arguments
        .get("answers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

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
            content: "Error: 'answers' argument must contain at least one option for the question tool.".to_string(),
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
    writeln!(
        stdout,
        "  │ {}{}{}",
        BOLD, question_text, RESET
    )?;
    writeln!(stdout, "  │")?;
    for (i, answer) in answers.iter().enumerate() {
        writeln!(
            stdout,
            "  │   {}{}.{}) {} {}{}",
            GREEN, i + 1, RESET, BOLD, answer, RESET
        )?;
    }
    writeln!(
        stdout,
        "  └{}──────────────────────────────{}",
        BOLD, RESET
    )?;

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
            Role::System => {
                // System prompt is internal — skip it in the replay.
            }
            Role::User => {
                writeln!(stdout, "{}> {}{}", BLUE, msg.content, RESET)?;
            }
            Role::Assistant => {
                if !msg.content.is_empty() {
                    write!(stdout, "{}", ORANGE)?;
                    stdout.write(msg.content.as_bytes())?;
                    writeln!(stdout, "{}", RESET)?;
                }
                if !msg.tool_calls.is_empty() {
                    for tc in &msg.tool_calls {
                        writeln!(
                            stdout,
                            "{}  {}▶{} {}",
                            DIM, CYAN, RESET, tc.function.name
                        )?;
                    }
                }
                writeln!(stdout)?;
            }
            Role::Tool => {
                // Tool results are shown inline during live use as
                //   ▶ Executing <name>...
                //   multiline output
                // Find the tool name embedded in the content (format: "Tool '<name>' result:\n...")
                let tool_name = msg.content.split('\'').nth(1).unwrap_or("tool");
                let result_body = msg.content
                    .strip_prefix(&format!("Tool '{}' result:\n", tool_name))
                    .or_else(|| msg.content.strip_prefix(&format!("Tool '{}' result:\n", tool_name)))
                    .unwrap_or(&msg.content);

                // Display full result, multiline with word-wrap
                let max_line_width = 120;
                for line in result_body.lines() {
                    if line.is_empty() {
                        writeln!(stdout, "    ")?;
                        continue;
                    }
                    if line.len() <= max_line_width - 4 {
                        writeln!(stdout, "    {}", line)?;
                    } else {
                        // Word-wrap long lines
                        let mut remaining = line;
                        let mut first = true;
                        while !remaining.is_empty() {
                            let indent = if first { "    " } else { "      " };
                            let avail = max_line_width - indent.len();
                            if remaining.len() <= avail {
                                writeln!(stdout, "{}{}", indent, remaining)?;
                                break;
                            }
                            let chunk_end = remaining.floor_char_boundary(avail);
                            let chunk = &remaining[..chunk_end];
                            let split_at = chunk.rfind(' ').unwrap_or(chunk_end);
                            if split_at == 0 {
                                writeln!(stdout, "{}{}", indent, &remaining[..chunk_end])?;
                                remaining = remaining[chunk_end..].trim_start();
                            } else {
                                writeln!(stdout, "{}{}", indent, &chunk[..split_at])?;
                                remaining = remaining[chunk[..split_at].len()..].trim_start();
                            }
                            first = false;
                        }
                    }
                }
            }
        }
    }

    stdout.flush()?;
    Ok(())
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