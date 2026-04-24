use std::collections::HashMap;
use ollama_rs::generation::tools::ToolInfo;

pub struct Tool {
    pub name: String,
    pub function: fn(HashMap<String, String>) -> String,
    pub tool_info: ToolInfo,
}

impl Tool {
    pub fn tool_info(&self) -> &ToolInfo {
        &self.tool_info
    }
}

pub fn execute_tool_call(tool: &Tool, arguments: &serde_json::Value) -> String {
    let args: HashMap<String, String> = arguments
        .as_object()
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();
    
    (tool.function)(args)
}
