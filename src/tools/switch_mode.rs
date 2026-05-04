use std::collections::HashMap;

use crate::mode::AgentMode;
use crate::provider::{ToolFunctionInfo, ToolInfo, ToolType};
use crate::tools::tool::{BoxFuture, Tool, build_string_params_schema};

pub fn switch_mode_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let mode_str = match args.get("mode") {
            Some(m) => m.trim().to_string(),
            None => return "Error: 'mode' argument is required. Valid values: casual, planning, agent, research".to_string(),
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
    let tool_info = ToolInfo {
        tool_type: ToolType::Function,
        function: ToolFunctionInfo {
            name: "switch_mode".to_string(),
            description: "Switch the assistant to a different operating mode. Use 'planning' to analyze and plan without making changes. Use 'agent' to write code and execute commands (escalate from planning). Use 'research' to search the web. Use 'casual' for general conversation.".to_string(),
            parameters: build_string_params_schema(
                &[("mode", "The mode to switch to: 'casual', 'planning', 'agent', or 'research'")],
                &[],
            ),
        },
    };

    Tool {
        function: Box::new(switch_mode_tool),
        tool_info,
    }
}
