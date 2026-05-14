pub mod display;
pub mod input;
pub mod safety;
pub mod tools;

use std::{
    error::Error,
    io,
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use rustyline::Editor;
use tokio::sync::Mutex;

use tinyharness_lib::{
    provider::{Message, Provider, Role},
    session::{Session, SessionStore},
    tools::ToolManager,
};

use crate::style::*;
use crate::{
    commands::{CommandDispatcher, CommandResult, init},
    ui::input::CommandHelper,
};

pub use display::{
    format_args_summary, print_context_load_warning, print_conversation_history,
    summarize_listing_result,
};
pub use input::read_multiline_input;
pub use safety::{is_safe_command, strip_safe_descriptor_redirections};
pub use tools::handle_tool_calls;

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
        let mut auto_accept = false;
        let mut token_usage: Option<tinyharness_lib::provider::TokenUsage> = None;

        loop {
            // Filter tools based on current mode
            let tools = tool_manager.tools_for_mode(dispatcher.current_mode);

            // Call the provider — it returns a receiver for streaming chunks
            let mut recv = {
                let mut provider = provider.lock().await;
                provider.chat(messages.clone(), tools).await
            };

            let mut response_content = String::new();
            let mut tool_calls: Vec<tinyharness_lib::provider::ToolCall> = Vec::new();
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

                // Save any partial content as an assistant message
                if !response_content.is_empty() {
                    messages.push(Message {
                        role: Role::Assistant,
                        content: response_content,
                        tool_calls: vec![],
                    });
                    session.append_message(messages.last().unwrap());
                } else {
                    // No content — remove the user message
                    messages.pop();
                }
                break; // Back to input prompt
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

        // Display token usage after turn is complete
        display::display_token_usage(messages, token_usage.as_ref(), &mut stdout)?;
    }

    // Save history on exit
    rl.save_history(&history_path).ok();

    Ok(())
}
