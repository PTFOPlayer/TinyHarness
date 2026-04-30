use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::provider::{ToolFunctionInfo, ToolInfo, ToolType};
use crate::tools::tool::{build_string_params_schema, BoxFuture, Tool};

pub fn write_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let path = match args.get("path") {
            Some(p) => p.clone(),
            None => return "Error: 'path' argument is required".to_string(),
        };

        let content = match args.get("content") {
            Some(c) => c.clone(),
            None => return "Error: 'content' argument is required".to_string(),
        };

        // Create parent directories if they don't exist
        if let Some(parent) = Path::new(&path).parent()
            && !parent.as_os_str().is_empty()
            && let Err(e) = fs::create_dir_all(parent)
        {
            return format!("Error: Failed to create parent directories: {}", e);
        }

        match fs::write(&path, &content) {
            Ok(_) => format!("Successfully wrote {} bytes to '{}'", content.len(), path),
            Err(e) => format!("Error: Failed to write file: {}", e),
        }
    })
}

pub fn write_tool_entry() -> Tool {
    let tool_info = ToolInfo {
        tool_type: ToolType::Function,
        function: ToolFunctionInfo {
            name: "write".to_string(),
            description: "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Creates parent directories automatically.".to_string(),
            parameters: build_string_params_schema(
                &[
                    ("path", "The absolute path to the file to write"),
                    ("content", "The text content to write to the file"),
                ],
                &[],
            ),
        },
    };

    Tool {
        function: Box::new(write_tool),
        tool_info,
    }
}
