use crate::commands::registry::CommandResult;

pub fn execute(name: &str) -> Result<CommandResult, String> {
    if name.is_empty() {
        return Err(
            "Usage: /rename <name> — give the current session a descriptive name".to_string(),
        );
    }

    Ok(CommandResult::RenameSession(name.to_string()))
}
