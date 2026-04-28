use crate::style::*;
use std::fs;

use serde::{Deserialize, Serialize};

use crate::{mode::AgentMode, provider::Message};

#[derive(Serialize, Deserialize)]
struct ConversationData {
    mode: AgentMode,
    messages: Vec<Message>,
}

pub fn execute_save(path: &str, messages: &[Message], mode: AgentMode) -> Result<(), String> {
    let data = ConversationData {
        mode,
        messages: messages.to_vec(),
    };
    let json =
        serde_json::to_string_pretty(&data).map_err(|e| format!("Failed to serialize: {}", e))?;
    fs::write(path, &json).map_err(|e| format!("Failed to write file '{}': {}", path, e))?;
    println!(
        "{}Saved {} messages ({}) to {}{}{}",
        BOLD,
        messages.len(),
        mode,
        BLUE,
        path,
        RESET
    );
    Ok(())
}

pub fn execute_load(path: &str) -> Result<(AgentMode, Vec<Message>), String> {
    let json =
        fs::read_to_string(path).map_err(|e| format!("Failed to read file '{}': {}", path, e))?;

    // Try new format first (with mode), fall back to old format (just messages)
    if let Ok(data) = serde_json::from_str::<ConversationData>(&json) {
        println!(
            "{}Loaded {} messages ({}) from {}{}{}",
            BOLD,
            data.messages.len(),
            data.mode,
            BLUE,
            path,
            RESET
        );
        return Ok((data.mode, data.messages));
    }

    // Fallback: old format (just a Vec<Message>)
    let messages: Vec<Message> =
        serde_json::from_str(&json).map_err(|e| format!("Failed to parse '{}': {}", path, e))?;
    println!(
        "{}Loaded {} messages from {}{}{}",
        BOLD,
        messages.len(),
        BLUE,
        path,
        RESET
    );
    Ok((AgentMode::Casual, messages))
}
