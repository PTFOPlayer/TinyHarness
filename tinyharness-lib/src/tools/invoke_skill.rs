use std::collections::HashMap;

use crate::tools::tool::{BoxFuture, Tool, ToolCategory, make_tool};

/// Stub function — the invoke_skill tool is handled specially in the agent loop
/// (similar to switch_mode and question), so this function is never actually called.
fn invoke_skill_tool_stub(_args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        "Error: invoke_skill tool should be handled by the agent loop, not executed directly."
            .to_string()
    })
}

/// Build the JSON Schema for the invoke_skill tool.
fn build_invoke_skill_schema() -> schemars::Schema {
    let schema_value = serde_json::json!({
        "type": "object",
        "properties": {
            "skill_name": {
                "type": "string",
                "description": "The name of the skill to invoke. Use the exact skill name from the available skills list."
            }
        },
        "required": ["skill_name"],
        "additionalProperties": false
    });

    serde_json::from_value(schema_value)
        .unwrap_or_else(|_| serde_json::from_value(serde_json::json!(true)).unwrap())
}

pub fn invoke_skill_tool_entry() -> Tool {
    make_tool(
        "invoke_skill",
        "Invoke a skill by name to activate its instructions. Skills provide specialized knowledge and instructions for specific tasks. The skill's full instructions will be injected into the conversation, guiding your behavior for the current task.",
        ToolCategory::Signal,
        build_invoke_skill_schema(),
        invoke_skill_tool_stub,
    )
}
