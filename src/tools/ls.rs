use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::tools::tool::{BoxFuture, Tool, build_string_params_schema, make_tool, require_arg};

pub fn ls_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let path = match require_arg(&args, "path") {
            Ok(p) => p,
            Err(e) => return e,
        };

        let dir_path = Path::new(&path);

        if !dir_path.exists() {
            return format!("Error: Path '{}' does not exist", path);
        }

        if !dir_path.is_dir() {
            return format!("Error: '{}' is not a directory", path);
        }

        let entries = match fs::read_dir(&path) {
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
    })
}

pub fn ls_tool_entry() -> Tool {
    make_tool(
        "ls",
        "List directory contents. Returns a newline-separated list of files and directories in the specified path.",
        build_string_params_schema(&[("path", "The directory path to list")], &[]),
        ls_tool,
    )
}
