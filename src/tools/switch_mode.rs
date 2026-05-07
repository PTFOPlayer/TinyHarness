use std::collections::HashMap;

use crate::define_tool;
use crate::extract_args;
use crate::mode::AgentMode;
use crate::tools::tool::BoxFuture;

pub fn switch_mode_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        extract_args!(args, mode);

        let mode_str = mode.trim().to_string();

        // Validate the mode string
        match mode_str.parse::<AgentMode>() {
            Ok(parsed_mode) => format!(
                "SUCCESS: Mode switched to '{}'. The assistant is now in {} mode and will use the appropriate toolset and behavior.",
                parsed_mode, parsed_mode
            ),
            Err(e) => {
                format!(
                    "Error: {}. Valid modes: casual, planning, agent, research",
                    e
                )
            }
        }
    })
}

define_tool!(
    switch_mode_tool_entry, "switch_mode",
    "Switch the assistant to a different operating mode. Use 'planning' to analyze and plan without making changes. Use 'agent' to write code and execute commands (escalate from planning). Use 'research' to search the web. Use 'casual' for general conversation.",
    required: [("mode", "The mode to switch to: 'casual', 'planning', 'agent', or 'research'")],
    handler: switch_mode_tool
);
