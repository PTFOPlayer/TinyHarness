use std::io::Write;

use tinyharness_lib::mode::AgentMode;
use tinyharness_lib::provider::Message;

use crate::commands::registry::{CommandContext, CommandResult};
use crate::style::*;

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
