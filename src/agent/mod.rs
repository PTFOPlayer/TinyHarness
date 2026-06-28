pub mod command_result;
pub mod confirm;
pub mod display;
pub mod input;
pub mod safety;
pub mod setup;
pub mod signal;
pub mod tool_result;
pub mod tools;
pub mod tui_loop;

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
    provider::{Message, Provider, Role},
    session::Session,
    token::ContextWindowSize,
    tools::ToolManager,
};
use tinyharness_ui::output::Output;

use crate::commands::{CommandContext, CommandResult, build_registry};
use tinyharness_ui::style::*;
use tinyharness_ui::ui::input::CommandHelper;

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
    initial_prompt: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    // Build the command registry once at startup
    let registry = build_registry();

    let mut stdout = Output::stdout();
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

    let helper = CommandHelper::with_commands(
        registry
            .command_names()
            .into_iter()
            .map(|s| s.to_string())
            .collect(),
        registry.subcommands(),
    );
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

    // If the user passed --prompt / -p, treat it as the first user message.
    // We materialize it into a local `pending_user_input` buffer that the
    // input-reading block below drains on the first iteration. This lets the
    // rest of the loop body (the LLM-call flow, tool handling, etc.) run
    // unmodified for the initial turn.
    let mut pending_user_input: Option<String> = initial_prompt.map(|s| s.to_string());

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

        // Include current model name next to the mode label.
        // When no model is known yet (e.g. Sockudo before the first
        // response), omit the suffix entirely rather than showing a
        // placeholder like "?".
        let model_suffix = {
            let p = provider.lock().await;
            match p.current_model() {
                Some(name) => format!(" {DIM}{name}{RESET}"),
                None => String::new(),
            }
        };

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

        // Read input with support for multi-line continuation. If we have
        // a pending user input (e.g. from --prompt on the first iteration),
        // use that and skip rustyline.
        let user_input = if let Some(pending) = pending_user_input.take() {
            Some(pending)
        } else {
            read_multiline_input(
                &mut rl,
                &prompt,
                &continuation_prompt,
                interrupted,
                &mut stdout,
            )?
        };

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
                    if let Some(usage) = command_result::apply_ok(ctx, session) {
                        last_known_token_usage = Some(usage);
                    }
                }
                Ok(CommandResult::SwitchSession(id_prefix)) => {
                    let info =
                        command_result::apply_switch_session(&id_prefix, ctx, messages, session);
                    if info.is_error {
                        let mut err_out = Output::stderr();
                        let _ = writeln!(err_out, "{RED}{}{RESET}", info.description);
                    } else {
                        let mut err_out = Output::stderr();
                        let _ = writeln!(err_out, "{BOLD}{}{RESET}", info.description);
                        last_known_token_usage = session.meta().token_usage.clone();
                        print_conversation_history(messages, &mut stdout)?;
                        print_context_load_warning(
                            messages,
                            session.meta().token_usage.as_ref().map(|u| u.total_tokens),
                            &mut stdout,
                        )?;
                    }
                }
                Ok(CommandResult::RenameSession(new_name)) => {
                    let info = command_result::apply_rename_session(&new_name, session);
                    let mut err_out = Output::stderr();
                    let _ = writeln!(err_out, "{BOLD}{}{RESET}", info.description);
                }
                Ok(CommandResult::Init(result)) => {
                    let info = command_result::apply_init(&result, ctx, messages);
                    let mut err_out = Output::stderr();
                    let _ = writeln!(err_out, "{GREEN}{}{RESET}", info.description);
                }
                Ok(CommandResult::SkillUse(skill_name)) => {
                    let info = command_result::apply_skill_use(&skill_name, ctx, messages, session);
                    let mut err_out = Output::stderr();
                    if info.is_error {
                        let _ = writeln!(err_out, "{RED}⚠ {}{RESET}", info.description);
                    } else {
                        let _ = writeln!(err_out, "{BOLD}⚡ {}{RESET}", info.description);
                    }
                }
                Ok(CommandResult::SkillUnload(skill_name)) => {
                    let info =
                        command_result::apply_skill_unload(&skill_name, ctx, messages, session);
                    let mut err_out = Output::stderr();
                    let _ = writeln!(err_out, "{BOLD}{}{RESET}", info.description);
                }
                Err(e) => {
                    let mut err_out = Output::stderr();
                    let _ = writeln!(err_out, "{RED}{e}{RESET}");
                }
            }
            if ctx.exit_requested {
                break;
            }
            continue;
        }

        let pending_images = std::mem::take(&mut ctx.pending_images);
        messages.push(Message {
            role: Role::User,
            content: user_input.clone(),
            tool_calls: vec![],
            tool_call_id: None,
            images: pending_images,
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
                                        write!(stdout, "\n{DIM}{THINK_COLOR}[thinking] ")?;
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

                        // Show spinner animation while waiting for first chunk.
                        // When show_thinking is enabled, skip the spinner — the
                        // model's actual thinking content will stream in instead.
                        if waiting_for_first_chunk && !ctx.show_thinking {
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
                        tool_call_id: None,
                        images: vec![],
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
                tool_call_id: None,
                images: vec![],
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
