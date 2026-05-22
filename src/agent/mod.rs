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
    config::load_settings,
    context::WorkspaceContext,
    mode::AgentMode,
    provider::{Message, Provider, Role},
    session::{Session, SessionStore},
    token::ContextWindowSize,
    tools::ToolManager,
};

use crate::style::*;
use crate::{
    commands::{CommandContext, CommandResult, build_registry, init},
    ui::input::CommandHelper,
};

pub use display::{
    format_args_summary, format_context_status, print_context_load_warning,
    print_conversation_history, summarize_listing_result,
};
pub use input::read_multiline_input;
pub use safety::{is_safe_command, strip_safe_descriptor_redirections};
pub use tools::handle_tool_calls;

pub async fn run_agent_loop(
    provider: Arc<Mutex<dyn Provider + Send + Sync>>,
    tool_manager: ToolManager,
    messages: &mut Vec<Message>,
    ctx: &mut CommandContext,
    session: &mut Session,
    interrupted: &Arc<AtomicBool>,
) -> Result<(), Box<dyn Error>> {
    // Build the command registry once at startup
    let registry = build_registry();

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

    // Warn if near/over the context window limit, using last known
    // provider token count from the session (or None for fresh sessions).
    print_context_load_warning(
        messages,
        session.meta().token_usage.as_ref().map(|u| u.total_tokens),
        &mut stdout,
    )?;

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

    let settings = load_settings();
    let context_size = settings
        .context_limit
        .map(ContextWindowSize::Custom)
        .unwrap_or_else(ContextWindowSize::default_size);

    // Track the last known token count from the LLM provider.
    // Providers report total_tokens (prompt + completion) on every call
    // (Ollama: prompt_eval_count + eval_count, OpenAI-compat: usage.total_tokens).
    // We always display the last known value — it's the best data available
    // and gets refreshed on every provider response. Seed from the session
    // metadata so that old sessions don't show "?" on restart.
    let mut last_known_token_usage: Option<tinyharness_lib::provider::TokenUsage> =
        session.meta().token_usage.clone();

    loop {
        // Clear any stale interrupt flag from a previous turn.
        interrupted.store(false, Ordering::SeqCst);

        let mode_label = ctx.current_mode.to_string();
        let mode_color = match ctx.current_mode {
            AgentMode::Casual => GREEN,
            AgentMode::Planning => YELLOW,
            AgentMode::Agent => CYAN,
            AgentMode::Research => ORANGE,
        };
        let pinned_count = ctx.file_context.pinned_file_count();

        // Only use provider-reported token counts, never estimate.
        // The provider's total_tokens (prompt + completion) reflects the
        // context that will be sent on the next turn. It remains valid
        // even after the assistant reply is appended. Tool calls may add
        // extra messages not yet accounted for, but the count will be
        // refreshed on the next provider call — until then, it's still
        // the best number we have.
        let token_usage_for_status = last_known_token_usage.as_ref();

        let status_line = display::format_context_status(
            messages.len(),
            pinned_count,
            token_usage_for_status,
            context_size,
        );

        // Include session name in prompt if available
        let session_name = session.meta().name.as_deref().unwrap_or("unnamed");
        let session_suffix = format!(" {DIM}({}){RESET}", session_name);

        // Include current model name next to the mode label
        let model_name = {
            let p = provider.lock().await;
            p.current_model().unwrap_or_else(|| "?".to_string())
        };
        let model_suffix = format!(" {DIM}{}{RESET}", model_name);

        let prompt = format!(
            "{}{}{}\n{}[{}]{}{}> {}{}",
            status_line,
            session_suffix,
            RESET,
            mode_color,
            mode_label,
            RESET,
            model_suffix,
            BLUE,
            RESET
        );
        let continuation_prompt = format!(
            "{}[{}]{}{}...> {}{}",
            mode_color, mode_label, RESET, model_suffix, BLUE, RESET
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
            match registry.dispatch(&user_input, ctx, messages).await {
                Ok(CommandResult::Ok) => {
                    // Update token usage from compaction side-channel.
                    if let Some(usage) = ctx.compaction_token_usage.take() {
                        last_known_token_usage = Some(usage.clone());
                        session.set_token_usage(usage);
                    }
                }
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
                                    // Update context mode, session ID, and token usage
                                    // from the loaded session
                                    ctx.current_mode = session.meta().mode;
                                    ctx.session_id = Some(session.id().to_string());
                                    last_known_token_usage = session.meta().token_usage.clone();
                                    // Ensure system prompt reflects current context
                                    ctx.refresh_system_prompt(messages);
                                    // Print loaded conversation history
                                    print_conversation_history(messages, &mut stdout)?;
                                    // Warn if the loaded session is near or over the context window limit,
                                    // using the stored provider token count from the session's metadata.
                                    print_context_load_warning(
                                        messages,
                                        session.meta().token_usage.as_ref().map(|u| u.total_tokens),
                                        &mut stdout,
                                    )?;
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
                Ok(CommandResult::Init(result)) => {
                    // Refresh workspace context since the project instruction file may have changed
                    ctx.workspace_ctx = WorkspaceContext::collect();
                    ctx.refresh_system_prompt(messages);

                    match &result {
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
                    }
                }
                Ok(CommandResult::SkillUse(skill_name)) => {
                    // Prevent duplicate activation
                    if ctx
                        .active_skills
                        .iter()
                        .any(|s| s.eq_ignore_ascii_case(&skill_name))
                    {
                        eprintln!(
                            "{}⚠ Skill '{}' is already active.{} Use {}/unload {}{} to deactivate it.",
                            ORANGE, skill_name, RESET, BOLD, skill_name, RESET
                        );
                        continue;
                    }
                    match ctx.skill_registry.get(&skill_name) {
                        Some(skill) => {
                            eprintln!(
                                "{}⚡ Skill activated: {}{}{} — {}{}",
                                BOLD, CYAN, skill_name, RESET, skill.description, RESET
                            );
                            // Track the active skill
                            ctx.active_skills.push(skill.name.clone());
                            // Inject a user message indicating skill activation
                            messages.push(Message {
                                role: Role::User,
                                content: format!("/use {}", skill_name),
                                tool_calls: vec![],
                            });
                            session.append_message(messages.last().expect("just pushed a message"));
                            // Refresh system prompt to include the active skill
                            ctx.refresh_system_prompt(messages);
                        }
                        None => {
                            eprintln!(
                                "{}⚠ Skill '{}' not found — it may have been removed.{}",
                                RED, skill_name, RESET
                            );
                        }
                    }
                }
                Ok(CommandResult::SkillUnload(skill_name)) => {
                    // Find and remove the skill from active list
                    let pos = ctx
                        .active_skills
                        .iter()
                        .position(|s| s.eq_ignore_ascii_case(&skill_name));
                    match pos {
                        Some(idx) => {
                            let removed = ctx.active_skills.remove(idx);
                            eprintln!(
                                "{}Skill deactivated: {}{}{}{}",
                                BOLD, CYAN, removed, RESET, RESET
                            );
                            // Inject a user message indicating skill deactivation
                            messages.push(Message {
                                role: Role::User,
                                content: format!("/unload {}", skill_name),
                                tool_calls: vec![],
                            });
                            session.append_message(messages.last().expect("just pushed a message"));
                            // Refresh system prompt to remove the skill
                            ctx.refresh_system_prompt(messages);
                        }
                        None => {
                            // Should not happen since dispatch validates this
                            eprintln!("{}⚠ Skill '{}' is not active.{}", ORANGE, skill_name, RESET);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("{}{}{}", RED, e, RESET);
                }
            }
            if ctx.exit_requested {
                break;
            }
            continue;
        }

        messages.push(Message {
            role: Role::User,
            content: user_input.clone(),
            tool_calls: vec![],
        });

        // Auto-save: user message
        session.append_message(messages.last().expect("just pushed a message"));

        // auto_accept persists across all agent iterations within this user turn,
        let mut auto_accept = false;

        loop {
            // Filter tools based on current mode
            let tools = tool_manager.tools_for_mode(ctx.current_mode);

            // Call the provider — it returns a receiver for streaming chunks
            let mut recv = {
                let mut provider = provider.lock().await;
                match provider.chat(messages.clone(), tools).await {
                    Ok(recv) => recv,
                    Err(e) => {
                        stdout.write_all(RESET.as_bytes())?;
                        writeln!(
                            stdout,
                            "\n{}⚠ Failed to start request: {}{}\n",
                            RED, e, RESET
                        )?;
                        // Remove the user message we just added
                        messages.pop();
                        break; // Back to input prompt
                    }
                }
            };

            let mut response_content = String::new();
            let mut tool_calls: Vec<tinyharness_lib::provider::ToolCall> = Vec::new();
            let mut received_done = false;
            let mut is_error = false;
            let mut was_interrupted = false;

            // Thinking chain tracking: accumulate thinking delta and track
            // whether we've shown the header, so we only print new content.
            let mut thinking_content = String::new();
            let mut thinking_header_shown = false;

            // Spinner state: animate while waiting for the first content chunk
            let mut spinner_idx: usize = 0;
            let mut waiting_for_first_chunk = true;
            let mut has_shown_spinner = false;

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
                                    // Capture the ground-truth prompt token count from the
                                    // LLM provider (Ollama: prompt_eval_count, OpenAI-compat: usage).
                                    if let Some(ref usage) = msg.usage {
                                        last_known_token_usage = Some(usage.clone());
                                        // Persist in session so restarts don't lose the count.
                                        session.set_token_usage(usage.clone());
                                    }
                                }

                                if msg.is_error {
                                    is_error = true;
                                }

                                // Display thinking/reasoning chain if enabled and present
                                if let Some(ref thinking) = msg.message.thinking
                                    && ctx.show_thinking
                                    && !thinking.is_empty()
                                {
                                    // Clear spinner before first output (thinking or content)
                                    if waiting_for_first_chunk && has_shown_spinner {
                                        write!(stdout, "\r{CLEAR_LINE}")?;
                                        stdout.flush()?;
                                        waiting_for_first_chunk = false;
                                    } else {
                                        waiting_for_first_chunk = false;
                                    }

                                    // Show [thinking] header once, before the first delta
                                    if !thinking_header_shown {
                                        write!(stdout, "{DIM}{THINK_COLOR}[thinking] ")?;
                                        thinking_header_shown = true;
                                    }

                                    // Each chunk's `thinking` is a delta — print only the new part
                                    write!(stdout, "{thinking}")?;
                                    thinking_content.push_str(thinking);
                                    stdout.flush()?;
                                }

                                if !msg.message.content.is_empty() {
                                    // Clear spinner before first content
                                    if waiting_for_first_chunk && has_shown_spinner {
                                        // Erase the spinner line: move to start, clear line
                                        write!(stdout, "\r{CLEAR_LINE}")?;
                                        stdout.flush()?;
                                        waiting_for_first_chunk = false;
                                    } else {
                                        waiting_for_first_chunk = false;
                                    }

                                    // Transition from thinking to content: close styling
                                    if thinking_header_shown {
                                        writeln!(stdout, "{RESET}")?;
                                        thinking_header_shown = false;
                                    }

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

                        // Show spinner animation while waiting for first chunk
                        if waiting_for_first_chunk {
                            let frame = SPINNER_FRAMES[spinner_idx % SPINNER_FRAMES.len()];
                            spinner_idx += 1;
                            if has_shown_spinner {
                                write!(stdout, "\r{DIM}{frame} Thinking...{RESET}")?;
                            } else {
                                write!(stdout, "{DIM}{frame} Thinking...{RESET}")?;
                                has_shown_spinner = true;
                            }
                            stdout.flush()?;
                        }
                    }
                }
            }

            // Clear spinner if still showing when stream ends
            if waiting_for_first_chunk && has_shown_spinner {
                write!(stdout, "\r{CLEAR_LINE}")?;
                stdout.flush()?;
            }

            // Close thinking styling if stream ended while still in thinking mode
            if thinking_header_shown {
                writeln!(stdout, "{RESET}")?;
                stdout.flush()?;
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
                    session.append_message(messages.last().expect("just pushed a message"));
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
                ctx,
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
            session.append_message(messages.last().expect("just pushed a message"));

            break;
        }

        // Blank line after agent response for visual separation
        writeln!(stdout)?;
    }

    // Save history on exit
    rl.save_history(&history_path).ok();

    Ok(())
}
