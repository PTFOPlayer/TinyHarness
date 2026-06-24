use std::io::Write;

use tokio::sync::Mutex;

use tinyharness_lib::{
    config::load_settings,
    image::ImageAttachment,
    provider::{Message, Role, ToolCall},
    session::Session,
    tools::SignalEvent,
    tools::ToolManager,
};

use crate::commands::CommandContext;
use tinyharness_ui::style::*;
use tinyharness_ui::ui::confirm::Confirmation;

use super::confirm::ConfirmationDecision;
use super::signal::{self, SignalResult};
use super::tool_result::{
    GenericToolResult, audit_info_for_tool, batch_tool_results, ensure_tool_call_ids,
    log_tool_audit,
};

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

    // Ensure every tool call has a non-empty id (some providers omit them).
    // We clone so we can mutate, and use the cloned slice for the rest of the
    // function so tool_call_ids are consistent between assistant message and
    // tool result messages.
    let mut tool_calls: Vec<ToolCall> = tool_calls.to_vec();
    ensure_tool_call_ids(&mut tool_calls);
    let tool_calls = tool_calls; // immutable from here

    let tool_count = tool_calls.len();
    writeln!(
        stdout,
        "\n{BG_TOOL}  {WHITE}{count} tool call(s){FILL_EOL}{RESET}",
        count = tool_count
    )?;

    messages.push(Message {
        role: Role::Assistant,
        content: response_content.to_string(),
        tool_calls: tool_calls.clone(),
        tool_call_id: None,
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
                let tc_id = call.id.as_deref().unwrap_or("");
                // Question signal requires user interaction — handle it directly
                // since the shared signal module can't do CLI I/O.
                match &event {
                    SignalEvent::Question { question, answers } => {
                        handle_question_cli(question, answers, messages, session, stdout, tc_id)?;
                    }
                    _ => {
                        let result = signal::handle_signal_event(
                            &event, messages, session, ctx, &provider, tc_id,
                        )
                        .await;
                        render_signal_result_cli(&result, stdout)?;
                    }
                }
            } else {
                let tc_id = call.id.as_deref().unwrap_or("");
                signal::apply_signal_parse_error(&call.function.name, messages, session, tc_id);
            }
            continue;
        }

        let needs_confirmation = tool_manager.needs_approval(&call.function.name);

        // Load settings to check auto_accept_safe_commands preference and safe/denied commands
        let settings = load_settings();
        let auto_accept_safe_commands = settings.auto_accept_safe_commands;
        let safe_commands = settings.get_safe_commands();
        let denied_commands = settings.get_denied_commands();

        // Confirmation step using shared decision logic
        let decision = super::confirm::decide_tool_confirmation(
            &call,
            *auto_accept,
            auto_accept_safe_commands,
            &safe_commands,
            &denied_commands,
            needs_confirmation,
        );

        let (approved, auto_accepted) = match decision {
            ConfirmationDecision::AutoApproved { auto_accepted: aa } => (true, aa),
            ConfirmationDecision::NeedsConfirmation => {
                match tinyharness_ui::ui::confirm::prompt_tool_confirmation(stdout, &call)? {
                    Confirmation::No => {
                        stdout.write_all(
                            format!("  {}Skipped{}{}\n", ORANGE, RESET, BOLD).as_bytes(),
                        )?;
                        stdout.flush()?;
                        (false, false)
                    }
                    Confirmation::AutoAccept => {
                        *auto_accept = true;
                        writeln!(
                            stdout,
                            "  {}Auto-accept enabled for the rest of this turn{}",
                            GREEN, RESET
                        )?;
                        (true, true)
                    }
                    Confirmation::Yes => (true, false),
                }
            }
            ConfirmationDecision::Denied => (false, false),
        };

        if !approved {
            let args_summary = super::display::format_args_summary(&call.function.arguments);
            messages.push(Message {
                role: Role::Tool,
                content: format!(
                    "[Tool denied] The user denied the '{}' tool call with arguments: {}\n\nTell the user you cannot proceed with that action unless they approve it.",
                    call.function.name, args_summary
                ),
                tool_call_id: call.id.clone(),
                tool_calls: vec![], images: vec![],
            });
            session.append_message(messages.last().expect("just pushed a message"));
            continue;
        }

        // Generic tool execution — collect result for batching
        let result = execute_generic_tool(&call, tool_manager, stdout, auto_accepted).await;

        // Log to audit if this was an auditable tool (run/write/edit)
        log_tool_audit(
            session.id(),
            &call,
            auto_accepted,
            result.duration_ms,
            result.is_error,
        );

        generic_tool_results.push(GenericToolResult {
            tool_call_id: call.id.clone().unwrap_or_default(),
            ..result
        });
    }

    // Push individual tool result messages (one per tool call)
    let tool_msgs = batch_tool_results(generic_tool_results);
    for msg in tool_msgs {
        messages.push(msg);
        session.append_message(messages.last().expect("just pushed a message"));
    }

    Ok(true)
}

/// Handle the question signal in CLI mode: display options and prompt user.
fn handle_question_cli<W: Write>(
    question: &str,
    answers: &[String],
    messages: &mut Vec<Message>,
    session: &mut Session,
    stdout: &mut W,
    tool_call_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Validate
    if let Some(error) = signal::validate_question(question, answers) {
        signal::apply_question_error(error, messages, session, tool_call_id);
        return Ok(());
    }

    // Display the question and options
    writeln!(
        stdout,
        "\n{}  ┌─── {}❓ Question {}─────{}",
        BOLD, CYAN, BOLD, RESET
    )?;
    writeln!(stdout, "  │ {}{}{}", BOLD, question, RESET)?;
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
            (format!("Skipped (user entered: {})", input), true)
        }
    } else {
        let input_lower = input.to_lowercase();
        match answers.iter().find(|a| a.to_lowercase() == input_lower) {
            Some(a) => (a.clone(), false),
            None => (input.to_string(), true),
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

    signal::apply_question_answer(
        question,
        &selected_answer,
        is_skip,
        messages,
        session,
        tool_call_id,
    );
    Ok(())
}

/// Render a signal result to CLI stdout with ANSI styling.
fn render_signal_result_cli<W: Write>(
    result: &SignalResult,
    stdout: &mut W,
) -> Result<(), Box<dyn std::error::Error>> {
    match result {
        SignalResult::SwitchMode {
            old_mode,
            new_mode,
            already_in,
        } => {
            if *already_in {
                writeln!(
                    stdout,
                    "  {ORANGE}Already in '{new_mode}' mode. No change was made.{RESET}",
                )?;
            } else {
                writeln!(
                    stdout,
                    "\n{}{}🔄 Mode switched: {} → {}{}",
                    BOLD, BLUE, old_mode, new_mode, RESET
                )?;
            }
            stdout.flush()?;
        }
        SignalResult::AutoCompact {
            focus: _,
            success,
            error,
        } => {
            if *success {
                writeln!(
                    stdout,
                    "\n{}  {}▶ auto_compact{} Compacting conversation history...",
                    DIM, CYAN, RESET
                )?;
            } else if let Some(e) = error {
                writeln!(stdout, "\n{}⚠ Auto-compact failed: {}{}", RED, e, RESET)?;
            }
            stdout.flush()?;
        }
        SignalResult::InvokeSkill {
            name,
            description,
            already_active,
            found,
        } => {
            if *already_active {
                writeln!(
                    stdout,
                    "\n{}⚠ Skill '{}' is already active.{}",
                    ORANGE, name, RESET
                )?;
            } else if *found {
                writeln!(
                    stdout,
                    "\n{}{}⚡ Skill activated: {}{}{} — {}{}",
                    BOLD, CYAN, BOLD, name, RESET, description, RESET
                )?;
            } else {
                writeln!(
                    stdout,
                    "\n{}⚠ Skill '{}' not found — it may have been removed.{}",
                    RED, name, RESET
                )?;
            }
            stdout.flush()?;
        }
        SignalResult::Question { .. } => {
            // Question is handled separately via handle_question_cli
        }
        SignalResult::ParseError { tool_name } => {
            writeln!(
                stdout,
                "\n{}⚠ Could not parse arguments for signal tool '{}'.{}",
                RED, tool_name, RESET
            )?;
        }
    }
    Ok(())
}

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
    let (audit_tool_name, audit_detail) = audit_info_for_tool(call);
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
        tool_call_id: String::new(), // set by caller
        audit_tool_name,
        audit_detail,
        duration_ms,
        is_error,
        images,
    }
}
