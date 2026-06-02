use std::io::Write;

use tokio::sync::Mutex;

use tinyharness_lib::{
    config::load_settings,
    image::ImageAttachment,
    mode::AgentMode,
    provider::{Message, Role, ToolCall},
    session::Session,
    tools::SignalEvent,
    tools::ToolManager,
};

use crate::commands::CommandContext;
use crate::commands::compact::execute_compact;
use tinyharness_ui::style::*;
use tinyharness_ui::ui::confirm::Confirmation;

use super::safety::is_safe_command;

/// Result from executing a generic tool call.
struct GenericToolResult {
    /// Formatted content for batching into the conversation message.
    content: String,
    /// If this was an auditable tool (run/write/edit), the tool name.
    audit_tool_name: Option<String>,
    /// For auditable tools: the primary argument (command for "run", path for "write"/"edit").
    audit_detail: Option<String>,
    /// Duration of the tool execution in milliseconds.
    duration_ms: u64,
    /// Whether the tool returned an error.
    is_error: bool,
    /// Images loaded by the tool (e.g. when reading an image file).
    images: Vec<ImageAttachment>,
}

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
    ctx: &mut CommandContext,
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
    writeln!(
        stdout,
        "\n{BG_TOOL}  {WHITE}{count} tool call(s){FILL_EOL}{RESET}",
        count = tool_count
    )?;

    messages.push(Message {
        role: Role::Assistant,
        content: response_content.to_string(),
        tool_calls: tool_calls.to_vec(),
        images: vec![],
    });
    session.append_message(messages.last().expect("just pushed a message"));

    // Collect generic tool results to batch them into a single message
    let mut generic_tool_results: Vec<GenericToolResult> = Vec::new();

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
                        handle_switch_mode(mode, ctx, messages, session, stdout)?;
                    }
                    SignalEvent::Question { question, answers } => {
                        handle_question(&question, &answers, messages, session, stdout)?;
                    }
                    SignalEvent::AutoCompact { focus } => {
                        handle_auto_compact(
                            &focus,
                            messages,
                            session,
                            ctx,
                            stdout,
                            std::sync::Arc::clone(&provider),
                        )
                        .await?;
                    }
                    SignalEvent::InvokeSkill { skill_name } => {
                        // Clone skill info to avoid borrowing ctx while calling it mutably
                        let skill_result = {
                            let registry = &ctx.skill_registry;
                            registry
                                .get(&skill_name)
                                .map(|s| (s.name.clone(), s.description.clone()))
                        };
                        handle_invoke_skill(
                            &skill_name,
                            &skill_result,
                            ctx,
                            messages,
                            session,
                            stdout,
                        )?;
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
                tool_calls: vec![], images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
            continue;
        }

        // Generic tool execution — collect result for batching
        let result = execute_generic_tool(call, tool_manager, stdout, auto_accepted).await;

        // Log to audit if this was an auditable tool (run/write/edit)
        if let Some(ref tool_name) = result.audit_tool_name {
            let exit_code = if result.is_error { -1 } else { 0 };
            let session_id = session.id().to_string();
            crate::commands::audit::log_command(
                &session_id,
                tool_name,
                result.audit_detail.as_deref().unwrap_or(""),
                exit_code,
                auto_accepted,
                result.duration_ms,
            );
        }

        generic_tool_results.push(result);
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

        // Collect images from all tool results (e.g. read tool on image files)
        let all_images: Vec<ImageAttachment> = generic_tool_results
            .into_iter()
            .flat_map(|r| r.images)
            .collect();

        messages.push(Message {
            role: Role::Tool,
            content: format!(
                "Tool results:\n{}\n\nUse these results to continue helping the user.",
                batched_content
            ),
            tool_calls: vec![],
            images: all_images,
        });
        session.append_message(messages.last().expect("just pushed a message"));
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

    match tinyharness_ui::ui::confirm::prompt_tool_confirmation(stdout, call)? {
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
/// Format a duration in milliseconds as a human-readable string.
/// Under 1 second: "42ms", 1-59 seconds: "1.2s", 60+ seconds: "1m 23s"
fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms ", ms)
    } else if ms < 60_000 {
        format!("{:.1}s ", ms as f64 / 1000.0)
    } else {
        let mins = ms / 60_000;
        let secs = (ms % 60_000) / 1000;
        format!("{}m {}s ", mins, secs)
    }
}

async fn execute_generic_tool<W: Write>(
    call: &ToolCall,
    tool_manager: &ToolManager,
    stdout: &mut W,
    auto_accepted: bool,
) -> GenericToolResult {
    // Show the "Executing..." header line
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
                    "{BG_DIM}  {DIM}▶ {WHITE}{name}{DIM} (auto-accepted){FILL_EOL}{RESET}",
                    name = call.function.name
                )
                .unwrap();
                writeln!(
                    stdout,
                    "{BG_DIM}      {BRIGHT_CYAN}{cmd}{FILL_EOL}{RESET}",
                    cmd = cmd
                )
                .unwrap();
            } else {
                writeln!(
                    stdout,
                    "{BG_DIM}  {DIM}▶ {WHITE}{name}{DIM} (auto-accepted){FILL_EOL}{RESET}",
                    name = call.function.name
                )
                .unwrap();
            }
        } else {
            writeln!(
                stdout,
                "{BG_DIM}  {DIM}▶ {WHITE}{name}{DIM} (auto-accepted){FILL_EOL}{RESET}",
                name = call.function.name
            )
            .unwrap();
        }
    } else {
        writeln!(
            stdout,
            "{BG_DIM}  {DIM}▶ {WHITE}{name}{DIM} Executing...{FILL_EOL}{RESET}",
            name = call.function.name
        )
        .unwrap();
    }
    stdout.flush().unwrap();

    // Spinner state for tool execution animation
    let mut spinner_idx: usize = 0;
    let mut has_shown_spinner = false;

    // Capture start time for duration tracking
    let start_time = std::time::Instant::now();

    // Execute tool call with spinner animation using tokio::pin! and select!
    let tool_fut = tool_manager.execute_tool_call(&call.function.name, &call.function.arguments);
    tokio::pin!(tool_fut);

    let result = loop {
        tokio::select! {
            result = &mut tool_fut => {
                // Tool finished — clear spinner line if we showed one
                if has_shown_spinner {
                    write!(stdout, "\r{CLEAR_LINE}{RESET}").unwrap();
                    stdout.flush().unwrap();
                }
                break result;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(80)) => {
                // Animate spinner
                let frame = SPINNER_FRAMES[spinner_idx % SPINNER_FRAMES.len()];
                spinner_idx += 1;
                if has_shown_spinner {
                    write!(stdout, "\r{DIM}{frame} {RESET}").unwrap();
                } else {
                    write!(stdout, "{DIM}{frame} {RESET}").unwrap();
                    has_shown_spinner = true;
                }
                stdout.flush().unwrap();
            }
        }
    };

    let duration_ms = start_time.elapsed().as_millis() as u64;

    // For tools that return potentially large listings, show only a summary
    match call.function.name.as_str() {
        "read" => {
            let is_error = result.starts_with("Error:");
            let summary = result.lines().next().unwrap_or("(empty result)");
            let indicator = if is_error { RED } else { GREEN };
            let icon = if is_error { "✗" } else { "✓" };
            let summary_color = if is_error { RED } else { DIM };
            writeln!(
                stdout,
                "{BG_DIM}  {indicator}{icon}{RESET}{BG_DIM} {DIM}{name}{RESET}{BG_DIM} {duration}{summary_color}{summary}{FILL_EOL}{RESET}",
                indicator = indicator,
                icon = icon,
                name = call.function.name,
                duration = format_duration(duration_ms),
                summary_color = summary_color,
                summary = summary
            )
            .unwrap();
        }
        "ls" | "grep" | "glob" => {
            let is_error = result.starts_with("Error:");
            let summary = super::display::summarize_listing_result(&result, &call.function.name);
            let indicator = if is_error { RED } else { GREEN };
            let icon = if is_error { "✗" } else { "✓" };
            let summary_color = if is_error { RED } else { DIM };
            writeln!(
                stdout,
                "{BG_DIM}  {indicator}{icon}{RESET}{BG_DIM} {DIM}{name}{RESET}{BG_DIM} {duration}{summary_color}{summary}{FILL_EOL}{RESET}",
                indicator = indicator,
                icon = icon,
                name = call.function.name,
                duration = format_duration(duration_ms),
                summary_color = summary_color,
                summary = summary
            )
            .unwrap();
        }
        _ => {
            let is_error = result.starts_with("Error:");
            let indicator = if is_error { RED } else { GREEN };
            let icon = if is_error { "✗" } else { "✓" };

            if is_error {
                // Compact single-line error: truncate to fit one line
                let error_msg = result.lines().next().unwrap_or("Error");
                // Truncate at 80 chars to keep the line compact
                let max_err_len = 80;
                let truncated = if error_msg.len() > max_err_len {
                    let cut = error_msg.floor_char_boundary(max_err_len - 1);
                    format!("{}…", &error_msg[..cut])
                } else {
                    error_msg.to_string()
                };
                writeln!(
                    stdout,
                    "{BG_DIM}  {indicator}{icon}{RESET}{BG_DIM} {DIM}{name}{RESET}{BG_DIM} {duration}{RED}{truncated}{FILL_EOL}{RESET}",
                    indicator = indicator,
                    icon = icon,
                    name = call.function.name,
                    duration = format_duration(duration_ms),
                    truncated = truncated,
                )
                .unwrap();
            } else {
                writeln!(
                    stdout,
                    "{BG_DIM}  {indicator}{icon}{RESET}{BG_DIM} {DIM}{name}{RESET}{BG_DIM} {duration}",
                    indicator = indicator,
                    icon = icon,
                    name = call.function.name,
                    duration = format_duration(duration_ms),
                )
                .unwrap();
                tinyharness_ui::ui::wrap::write_wrapped_lines(
                    stdout,
                    &result,
                    &format!("{BG_DIM}      "),
                    &format!("      {BG_DIM}{DIM}"),
                    tinyharness_ui::ui::wrap::MAX_LINE_WIDTH,
                    true, // fill background to end of line
                )
                .unwrap();
            }
        }
    }
    writeln!(stdout, "{RESET}").unwrap();
    stdout.flush().unwrap();

    // Capture audit-relevant info before returning
    let (audit_tool_name, audit_detail) = match call.function.name.as_str() {
        "run" => (
            Some("run".to_string()),
            call.function
                .arguments
                .get("command")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        ),
        "write" => (
            Some("write".to_string()),
            call.function
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        ),
        "edit" => (
            Some("edit".to_string()),
            call.function
                .arguments
                .get("path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
        ),
        _ => (None, None),
    };
    let is_error = result.starts_with("Error:");

    // For read tool on image files, load the image data for the model to view.
    // The read tool prefixes image results with "[IMAGE] path" so we can detect them.
    let images = if call.function.name == "read" && result.starts_with("[IMAGE]") {
        let image_path = result
            .lines()
            .next()
            .and_then(|l| l.strip_prefix("[IMAGE] "))
            .unwrap_or("");
        if !image_path.is_empty() {
            match ImageAttachment::load_from_str(image_path) {
                Ok(img) => {
                    // Also populate dimensions if the read tool detected them
                    let mut img = img;
                    if img.dimensions.is_none() {
                        img.dimensions =
                            tinyharness_lib::tools::read::detect_image_dimensions(image_path);
                    }
                    vec![img]
                }
                Err(_) => vec![],
            }
        } else {
            vec![]
        }
    } else {
        vec![]
    };

    GenericToolResult {
        content: format!("### {} Tool Result\n\n{}", call.function.name, result),
        audit_tool_name,
        audit_detail,
        duration_ms,
        is_error,
        images,
    }
}

/// Handle the switch_mode signal: update context and system prompt.
fn handle_switch_mode<W: Write>(
    new_mode: AgentMode,
    ctx: &mut CommandContext,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
) -> Result<(), Box<dyn std::error::Error>> {
    let old_mode = ctx.current_mode;
    match ctx.switch_mode(new_mode, messages) {
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
                tool_calls: vec![], images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
        }
        Err(msg) => {
            writeln!(stdout, "  {}{}{}", ORANGE, msg, RESET)?;
            messages.push(Message {
                role: Role::Tool,
                content: format!("Already in '{}' mode. No change was made.", new_mode),
                tool_calls: vec![],
                images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
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
            images: vec![],
        });
        session.append_message(messages.last().expect("just pushed a message"));
        return Ok(());
    }

    if answers.is_empty() {
        messages.push(Message {
            role: Role::Tool,
            content:
                "Error: 'answers' argument must contain at least one option for the question tool."
                    .to_string(),
            tool_calls: vec![],
            images: vec![],
        });
        session.append_message(messages.last().expect("just pushed a message"));
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
        images: vec![],
    });
    session.append_message(messages.last().expect("just pushed a message"));
    Ok(())
}

/// Handle the auto_compact signal: trigger conversation compaction.
async fn handle_auto_compact<W: Write>(
    focus: &str,
    messages: &mut Vec<Message>,
    session: &mut Session,
    ctx: &mut CommandContext,
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

    match execute_compact(&mut ctx.output, &mut *provider_guard, messages, focus).await {
        Ok(token_usage) => {
            // Propagate token usage the same way /compact does:
            // 1) store in CommandContext so the agent loop updates the display
            // 2) persist in session metadata so it survives restarts
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
                images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
        }
        Err(e) => {
            messages.push(Message {
                role: Role::Tool,
                content: format!(
                    "Auto-compact failed: {}. The conversation was not modified.",
                    e
                ),
                tool_calls: vec![],
                images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
        }
    }

    Ok(())
}

/// Handle the invoke_skill signal: activate a skill by name.
///
/// When the model invokes a skill, we look it up in the skill registry,
/// display a confirmation message, track it as active, and refresh the
/// system prompt to include the skill's instructions.
///
/// `skill_result` is `Some((name, description))` if the skill was found,
/// or `None` if not found. This avoids borrowing the context while also
/// calling it mutably.
fn handle_invoke_skill<W: Write>(
    skill_name: &str,
    skill_result: &Option<(String, String)>,
    ctx: &mut CommandContext,
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
) -> Result<(), Box<dyn std::error::Error>> {
    match skill_result {
        Some((name, description)) => {
            // Prevent duplicate activation
            if ctx
                .active_skills
                .iter()
                .any(|s| s.eq_ignore_ascii_case(name))
            {
                writeln!(
                    stdout,
                    "\n{}⚠ Skill '{}' is already active.{}",
                    ORANGE, name, RESET
                )?;
                stdout.flush()?;

                messages.push(Message {
                    role: Role::Tool,
                    content: format!(
                        "Skill '{}' is already active. Its instructions are already in effect.",
                        name
                    ),
                    tool_calls: vec![],
                    images: vec![],
                });
                session.append_message(messages.last().expect("just pushed a message"));
                return Ok(());
            }

            writeln!(
                stdout,
                "\n{}{}⚡ Skill activated: {}{}{} — {}{}",
                BOLD, CYAN, BOLD, name, RESET, description, RESET
            )?;
            stdout.flush()?;

            // Track the active skill
            ctx.active_skills.push(name.clone());

            messages.push(Message {
                role: Role::Tool,
                content: format!(
                    "SUCCESS: Skill '{}' activated. The skill's instructions are now in effect.",
                    name
                ),
                tool_calls: vec![],
                images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));

            // Refresh system prompt to include the active skill
            ctx.refresh_system_prompt(messages);
        }
        None => {
            let available = ctx
                .skill_registry
                .skills
                .iter()
                .map(|s| s.name.clone())
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                stdout,
                "\n{}⚠ Skill '{}' not found.{} Use {}/skills{} to list available skills.",
                ORANGE, skill_name, RESET, BOLD, RESET
            )?;
            stdout.flush()?;

            messages.push(Message {
                role: Role::Tool,
                content: format!(
                    "Error: Skill '{}' not found. Available skills: {}. Use /skills to list them.",
                    skill_name, available
                ),
                tool_calls: vec![],
                images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
        }
    }
    Ok(())
}
