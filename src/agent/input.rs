use std::io::Write;

use rustyline::Editor;

use tinyharness_ui::output::Output;
use tinyharness_ui::style::*;
use tinyharness_ui::ui::input::CommandHelper;

/// Read input from the user with support for multi-line continuation.
///
/// Uses rustyline's validator to detect incomplete input (trailing backslash,
/// unclosed code fences, etc.) and shows a continuation prompt for additional lines.
///
/// Ctrl+C at the prompt follows the classic REPL convention: the first press
/// discards the current (possibly multi-line) input and re-prompts, a second
/// consecutive press exits.
///
/// Returns:
/// - `Ok(Some(String))` - Complete input received
/// - `Ok(None)` - EOF (Ctrl+D), a second consecutive Ctrl+C, or unrecoverable error
/// - `Err(...)` - Read error
pub fn read_multiline_input(
    rl: &mut Editor<CommandHelper, rustyline::history::DefaultHistory>,
    prompt: &str,
    continuation_prompt: &str,
    interrupted: &std::sync::atomic::AtomicBool,
    stdout: &mut impl Write,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let mut input = String::new();
    let mut is_first_line = true;
    let mut consecutive_interrupts = 0u32;

    loop {
        let current_prompt = if is_first_line {
            prompt
        } else {
            continuation_prompt
        };

        let readline = rl.readline(current_prompt);

        match readline {
            Ok(line) => {
                consecutive_interrupts = 0;
                if is_first_line {
                    input = line;
                } else {
                    input.push('\n');
                    input.push_str(&line);
                }

                let trimmed = input.trim_end();
                let ends_with_backslash = trimmed.ends_with('\\');
                let fence_count = input.matches("```").count();
                let has_unclosed_fence = fence_count % 2 == 1;
                let backtick_count = input.matches('`').count();
                let has_unclosed_backtick = backtick_count % 2 == 1;

                if ends_with_backslash || has_unclosed_fence || has_unclosed_backtick {
                    is_first_line = false;
                    continue;
                }

                return Ok(Some(input));
            }
            Err(rustyline::error::ReadlineError::Interrupted) => {
                // Clear the shared flag so a Ctrl+C at the prompt doesn't leak
                // into the next generation turn.
                interrupted.store(false, std::sync::atomic::Ordering::SeqCst);
                consecutive_interrupts += 1;
                if consecutive_interrupts >= 2 {
                    // Second consecutive Ctrl+C — exit.
                    stdout.write_all("\n".as_bytes())?;
                    stdout.flush()?;
                    return Ok(None);
                }
                // First Ctrl+C — discard the current (possibly multi-line)
                // input and re-prompt from scratch. rustyline does not echo
                // anything for Ctrl+C in raw mode, so print it ourselves.
                input.clear();
                is_first_line = true;
                stdout.write_all("^C\n".as_bytes())?;
                stdout.write_all(
                    format!(
                        "{GRAY}(input cleared — press Ctrl+C again, {BLUE}/exit{GRAY}, or Ctrl+D to exit){RESET}\n"
                    )
                    .as_bytes(),
                )?;
                stdout.flush()?;
                continue;
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                stdout.write_all("\n".as_bytes())?;
                return Ok(None);
            }
            Err(err) => {
                let mut err_out = Output::stderr();
                let _ = writeln!(err_out, "{RED}Error reading input: {err}{RESET}");
                return Ok(None);
            }
        }
    }
}
