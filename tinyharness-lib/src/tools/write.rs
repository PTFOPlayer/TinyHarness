use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::extract_args;
use crate::path_guard::PathGuard;
use crate::tools::tool::{BoxFuture, ToolCategory, build_string_params_schema, make_tool};

pub fn write_tool_entry(guard: Option<PathGuard>) -> crate::tools::tool::Tool {
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
        move |args| {
            let guard = guard.clone();
            Box::pin(write_tool_inner(args, guard))
        },
    )
}

pub fn write_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(write_tool_inner(args, None))
}

async fn write_tool_inner(args: HashMap<String, String>, guard: Option<PathGuard>) -> String {
    extract_args!(args, path, content);

    // Create parent directories if they don't exist. Through the guard each
    // new directory is verified via its open descriptor, so a symlink swapped
    // in after the argument check cannot redirect creation outside the root.
    if let Some(parent) = Path::new(&path).parent()
        && !parent.as_os_str().is_empty()
    {
        let result = match &guard {
            Some(g) => g.create_dir_all_contained(parent),
            None => fs::create_dir_all(parent).map_err(|e| e.to_string()),
        };
        if let Err(e) = result {
            return format!("Error: Failed to create parent directories: {}", e);
        }
    }

    let result = match &guard {
        Some(g) => g.write_contained(Path::new(&path), &content),
        None => fs::write(&path, &content).map_err(|e| e.to_string()),
    };
    match result {
        Ok(_) => format!("Wrote {} bytes to '{}'.", content.len(), path),
        Err(e) => format!("Error: Failed to write file: {}", e),
    }
}
