use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::provider::{ToolFunctionInfo, ToolInfo, ToolType};
use crate::tools::tool::{build_string_params_schema, Tool};

pub fn ls_tool(args: HashMap<String, String>) -> String {
    let path = match args.get("path") {
        Some(p) => p,
        None => return "Error: 'path' argument is required".to_string(),
    };

    let dir_path = Path::new(path);

    if !dir_path.exists() {
        return format!("Error: Path '{}' does not exist", path);
    }

    if !dir_path.is_dir() {
        return format!("Error: '{}' is not a directory", path);
    }

    let entries = match fs::read_dir(dir_path) {
        Ok(e) => e,
        Err(e) => return format!("Error: Failed to read directory: {}", e),
    };

    let mut files: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| {
            let file_name = entry.file_name();
            file_name.to_string_lossy().to_string()
        })
        .collect();

    files.sort();

    if files.is_empty() {
        return "Directory is empty".to_string();
    }

    files.join("\n")
}

pub fn ls_tool_entry() -> Tool {
    let tool_info = ToolInfo {
        tool_type: ToolType::Function,
        function: ToolFunctionInfo {
            name: "ls".to_string(),
            description: "List directory contents. Returns a newline-separated list of files and directories in the specified path.".to_string(),
            parameters: build_string_params_schema(
                &[("path", "The directory path to list")],
                &[],
            ),
        },
    };

    Tool {
        name: "ls".to_string(),
        function: ls_tool,
        tool_info,
    }
}
