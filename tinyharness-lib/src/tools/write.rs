use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::extract_args;
use crate::tools::tool::{BoxFuture, ToolCategory, build_string_params_schema, make_tool};

pub fn write_tool_entry() -> crate::tools::tool::Tool {
    make_tool(
        "write",
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Creates parent directories automatically.",
        ToolCategory::Destructive,
        build_string_params_schema(
            &[
                ("path", "The absolute path to the file to write"),
                ("content", "The text content to write to the file"),
            ],
            &[],
        ),
        |args| Box::pin(write_tool(args)),
    )
}

pub fn write_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        extract_args!(args, path, content);

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
