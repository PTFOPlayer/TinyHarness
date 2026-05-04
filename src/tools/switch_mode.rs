use std::collections::HashMap;

use crate::mode::AgentMode;
use crate::tools::tool::{BoxFuture, Tool, build_string_params_schema, make_tool, require_arg};

pub fn switch_mode_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let mode_str = match require_arg(&args, "mode") {
            Ok(m) => m.trim().to_string(),
            Err(_) => return "Error: 'mode' argument is required. Valid values: casual, planning, agent, research".to_string(),
        };

        // Validate the mode string
        match mode_str.parse::<AgentMode>() {
            Ok(mode) => {
                format!(
                    "SUCCESS: Mode switched to '{}'. The assistant is now in {} mode and will use the appropriate toolset and behavior.",
                    mode, mode
                )
            }
            Err(e) => {
                format!(
                    "Error: {}. Valid modes: casual, planning, agent, research",
                    e
                )
            }
        }
    })
}

pub fn switch_mode_tool_entry() -> Tool {
    make_tool(
        "switch_mode",
        "Switch the assistant to a different operating mode. Use 'planning' to analyze and plan without making changes. Use 'agent' to write code and execute commands (escalate from planning). Use 'research' to search the web. Use 'casual' for general conversation.",
        build_string_params_schema(
            &[(
                "mode",
                "The mode to switch to: 'casual', 'planning', 'agent', or 'research'",
            )],
            &[],
        ),
        switch_mode_tool,
    )
}
