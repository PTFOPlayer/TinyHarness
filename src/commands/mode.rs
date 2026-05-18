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
        println!(
            "{}Current mode: {}{}{}",
            BOLD, BLUE, ctx.current_mode, RESET
        );
        return Ok(CommandResult::Ok);
    }

    let new_mode: AgentMode = mode_str.parse()?;

    match ctx.switch_mode(new_mode, messages) {
        Ok(()) => {
            println!("{}Switched to {} mode.{}", BOLD, BLUE, RESET);
        }
        Err(msg) => {
            println!("{}{}{}", ORANGE, msg, RESET);
        }
    }

    Ok(CommandResult::Ok)
}
