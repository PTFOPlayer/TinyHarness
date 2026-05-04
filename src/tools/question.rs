use std::collections::HashMap;

use crate::tools::tool::{BoxFuture, Tool, make_tool};

/// Stub function — the question tool is handled specially in the agent loop
/// (similar to switch_mode), so this function is never actually called.
/// It exists only to satisfy the Tool struct's function field requirement.
fn question_tool_stub(_args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        "Error: question tool should be handled by the agent loop, not executed directly."
            .to_string()
    })
}

/// Build a custom JSON Schema for the question tool.
/// This is needed because the "answers" parameter is an array of strings,
/// not a simple string — so `build_string_params_schema` cannot be used.
fn build_question_schema() -> schemars::Schema {
    let schema_value = serde_json::json!({
        "type": "object",
        "properties": {
            "question": {
                "type": "string",
                "description": "The question to ask the user about an implementation detail or decision"
            },
            "answers": {
                "type": "array",
                "items": {
                    "type": "string"
                },
                "description": "A list of possible answers the user can choose from. Must contain at least one option."
            }
        },
        "required": ["question", "answers"],
        "additionalProperties": false
    });

    serde_json::from_value(schema_value)
        .unwrap_or_else(|_| serde_json::from_value(serde_json::json!(true)).unwrap())
}

pub fn question_tool_entry() -> Tool {
    make_tool(
        "question",
        "Ask the user a question with a list of possible answers. Use this when you need clarification about implementation details, design decisions, or any choice that affects how you should proceed. The tool will present the question and options to the user, and return their selected answer.",
        build_question_schema(),
        question_tool_stub,
    )
}
