use std::fs;

use crate::provider::Message;

pub fn execute_save(path: &str, messages: &[Message]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(messages).map_err(|e| format!("Failed to serialize: {}", e))?;
    fs::write(path, &json).map_err(|e| format!("Failed to write file '{}': {}", path, e))?;
    println!(
        "{}Saved {} messages to {}{}{}",
        crate::BOLD,
        messages.len(),
        crate::BLUE,
        path,
        crate::RESET
    );
    Ok(())
}

pub fn execute_load(path: &str) -> Result<Vec<Message>, String> {
    let json = fs::read_to_string(path).map_err(|e| format!("Failed to read file '{}': {}", path, e))?;
    let messages: Vec<Message> =
        serde_json::from_str(&json).map_err(|e| format!("Failed to parse '{}': {}", path, e))?;
    println!(
        "{}Loaded {} messages from {}{}{}",
        crate::BOLD,
        messages.len(),
        crate::BLUE,
        path,
        crate::RESET
    );
    Ok(messages)
}
