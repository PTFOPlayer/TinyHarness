use std::io::Write;

use rustyline::Editor;

use crate::style::*;
use crate::ui::input::CommandHelper;

/// Read input from the user with support for multi-line continuation.
///
/// Uses rustyline's validator to detect incomplete input (trailing backslash,
/// unclosed code fences, etc.) and shows a continuation prompt for additional lines.
///
/// Returns:
/// - `Ok(Some(String))` - Complete input received
/// - `Ok(None)` - EOF (Ctrl+D) or unrecoverable error
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
                interrupted.store(false, std::sync::atomic::Ordering::SeqCst);
                stdout.write_all("\n".as_bytes())?;
                stdout.write_all(
                    format!(
                        "{}Use {}/exit{} or {}{}Ctrl+D{} to exit.\n",
                        GRAY, BLUE, GRAY, GRAY, BOLD, RESET
                    )
                    .as_bytes(),
                )?;
                stdout.flush()?;
                return Ok(None);
            }
            Err(rustyline::error::ReadlineError::Eof) => {
                stdout.write_all("\n".as_bytes())?;
                return Ok(None);
            }
            Err(err) => {
                eprintln!("{}Error reading input: {}{}", RED, err, RESET);
                return Ok(None);
            }
        }
    }
}
