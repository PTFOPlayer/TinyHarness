use std::collections::HashMap;
use std::fs;

use crate::provider::{ToolFunctionInfo, ToolInfo, ToolType};
use crate::tools::tool::{BoxFuture, Tool, build_string_params_schema};

pub fn edit_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let path = match args.get("path") {
            Some(p) => p.clone(),
            None => return "Error: 'path' argument is required".to_string(),
        };

        let old_str = match args.get("old_str") {
            Some(s) => s.clone(),
            None => return "Error: 'old_str' argument is required".to_string(),
        };

        let new_str = match args.get("new_str") {
            Some(s) => s.clone(),
            None => return "Error: 'new_str' argument is required".to_string(),
        };

        // Read the file
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => return format!("Error: Failed to read file '{}': {}", path, e),
        };

        // Find the old_str in the content
        let count = content.matches(&old_str).count();

        if count == 0 {
            return format!(
                "Error: 'old_str' not found in '{}'. The exact text to replace must appear in the file.",
                path
            );
        }

        if count > 1 {
            return format!(
                "Error: 'old_str' appears {} times in '{}'. The old_str must appear exactly once. Found {} occurrences.",
                count, path, count
            );
        }

        // Perform the replacement
        let new_content = content.replace(&old_str, &new_str);

        // Write the file
        match fs::write(&path, &new_content) {
            Ok(_) => format!(
                "Successfully edited '{}'. Replaced 1 occurrence ({} chars replaced).",
                path,
                old_str.len()
            ),
            Err(e) => format!("Error: Failed to write file '{}': {}", path, e),
        }
    })
}

pub fn edit_tool_entry() -> Tool {
    let tool_info = ToolInfo {
        tool_type: ToolType::Function,
        function: ToolFunctionInfo {
            name: "edit".to_string(),
            description: "Edit a file by finding an exact string and replacing it with new text. The old_str must appear exactly once in the file. Use this for targeted edits instead of rewriting the entire file.".to_string(),
            parameters: build_string_params_schema(
                &[
                    ("path", "The absolute path to the file to edit"),
                    ("old_str", "The exact string to find in the file (must appear exactly once)"),
                    ("new_str", "The replacement string"),
                ],
                &[],
            ),
        },
    };

    Tool {
        function: Box::new(edit_tool),
        tool_info,
    }
}
