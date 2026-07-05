use std::io::Write;

use tinyharness_lib::mode::AgentMode;
use tinyharness_lib::provider::Message;

use crate::commands::registry::{CommandContext, CommandResult};
use tinyharness_ui::style::*;

/// Execute the /mode command.
pub fn execute(
    arg: Option<&str>,
    ctx: &mut CommandContext,
    messages: &mut [Message],
) -> Result<CommandResult, String> {
    let mode_str = arg.unwrap_or("");

    if mode_str.is_empty() {
        let _ = writeln!(
            ctx.output,
            "{BOLD}Current mode: {BLUE}{}{RESET}",
            ctx.current_mode,
        );
        return Ok(CommandResult::Ok);
    }

    let new_mode: AgentMode = mode_str.parse()?;

    match ctx.switch_mode(new_mode, messages) {
        Ok(()) => {
            let _ = writeln!(
                ctx.output,
                "{BOLD}Switched to {BLUE}{new_mode}{RESET} mode."
            );
        }
        Err(msg) => {
            let _ = writeln!(ctx.output, "{ORANGE}{msg}{RESET}");
        }
    }

    Ok(CommandResult::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{
        captured_output, captured_string, make_context, make_messages, strip_ansi,
    };

    #[test]
    fn mode_no_arg_shows_current_mode() {
        let (mut ctx, _mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;
        let mut messages = make_messages("test");

        let result = execute(None, &mut ctx, &mut messages).unwrap();
        assert!(matches!(result, CommandResult::Ok));

        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Current mode:"));
        assert!(plain.contains("casual"));
    }

    #[test]
    fn mode_switch_to_agent() {
        let (mut ctx, _mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;
        let mut messages = make_messages("test");

        let result = execute(Some("agent"), &mut ctx, &mut messages).unwrap();
        assert!(matches!(result, CommandResult::Ok));

        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Switched to"));
        assert!(plain.contains("agent"));
        assert_eq!(ctx.current_mode, tinyharness_lib::mode::AgentMode::Agent);
    }

    #[test]
    fn mode_switch_same_mode_shows_warning() {
        let (mut ctx, _mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;
        let mut messages = make_messages("test");

        // Default mode is casual
        let result = execute(Some("casual"), &mut ctx, &mut messages).unwrap();
        assert!(matches!(result, CommandResult::Ok));

        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("Already in"));
    }

    #[test]
    fn mode_invalid_mode_returns_error() {
        let (mut ctx, _mock) = make_context();
        let mut messages = make_messages("test");

        let result = execute(Some("invalidmode"), &mut ctx, &mut messages);
        assert!(result.is_err());
    }

    #[test]
    fn mode_switch_to_planning() {
        let (mut ctx, _mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;
        let mut messages = make_messages("test");

        let result = execute(Some("planning"), &mut ctx, &mut messages).unwrap();
        assert!(matches!(result, CommandResult::Ok));

        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("planning"));
        assert_eq!(ctx.current_mode, tinyharness_lib::mode::AgentMode::Planning);
    }

    #[test]
    fn mode_switch_to_research() {
        let (mut ctx, _mock) = make_context();
        let (output, buf) = captured_output();
        ctx.output = output;
        let mut messages = make_messages("test");

        let result = execute(Some("research"), &mut ctx, &mut messages).unwrap();
        assert!(matches!(result, CommandResult::Ok));

        let plain = strip_ansi(&captured_string(&buf));
        assert!(plain.contains("research"));
        assert_eq!(ctx.current_mode, tinyharness_lib::mode::AgentMode::Research);
    }
}
