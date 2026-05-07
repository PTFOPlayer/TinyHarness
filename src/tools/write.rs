use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::define_tool;
use crate::extract_args;
use crate::tools::tool::BoxFuture;

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

define_tool!(
    write_tool_entry, "write",
    "Write content to a file. Creates the file if it doesn't exist, overwrites if it does. Creates parent directories automatically.",
    required: [
        ("path", "The absolute path to the file to write"),
        ("content", "The text content to write to the file"),
    ],
    handler: write_tool
);
