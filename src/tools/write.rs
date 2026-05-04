use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::tools::tool::{BoxFuture, Tool, build_string_params_schema, make_tool, require_arg};

pub fn write_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let path = match require_arg(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };

        let content = match require_arg(&args, "content") {
            Ok(c) => c,
            Err(e) => return e,
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
    make_tool(
        "write",
        "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Creates parent directories automatically.",
        build_string_params_schema(
            &[
                ("path", "The absolute path to the file to write"),
                ("content", "The text content to write to the file"),
            ],
            &[],
        ),
        write_tool,
    )
}
