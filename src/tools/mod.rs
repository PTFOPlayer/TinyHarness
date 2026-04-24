pub mod ls;
pub mod tool;
pub mod read;

use crate::tools::tool::Tool;

pub struct ToolManager {
    tools: Vec<Tool>,
}

impl ToolManager {
    pub fn new() -> Self {
        ToolManager { tools: vec![] }
    }

    pub fn register_tool(&mut self, tool: Tool) {
        self.tools.push(tool);
    }

    pub fn get_ollama_tools(&self) -> Vec<ollama_rs::generation::tools::ToolInfo> {
        self.tools.iter().map(|t| t.tool_info.clone()).collect()
    }

    pub fn execute_tool_call(&self, tool_name: &str, arguments: &serde_json::Value) -> String {
        if let Some(tool) = self.tools.iter().find(|t| t.name == tool_name) {
            tool::execute_tool_call(tool, arguments)
        } else {
            format!("Error: Tool '{}' not found", tool_name)
        }
    }
}