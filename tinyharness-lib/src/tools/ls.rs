use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::extract_args;
use crate::path_guard::PathGuard;
use crate::tools::tool::{BoxFuture, ToolCategory, build_string_params_schema, make_tool};

pub fn ls_tool_entry(guard: Option<PathGuard>) -> crate::tools::tool::Tool {
    make_tool(
        "ls",
        "List directory contents. Returns a newline-separated list of files and directories in the specified path.",
        ToolCategory::ReadOnly,
        build_string_params_schema(&[("path", "The directory path to list")], &[]),
        move |args| {
            let guard = guard.clone();
            Box::pin(ls_tool_inner(args, guard))
        },
    )
}

pub fn ls_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(ls_tool_inner(args, None))
}

async fn ls_tool_inner(args: HashMap<String, String>, guard: Option<PathGuard>) -> String {
    extract_args!(args, path);

    let dir_path = Path::new(&path);

    if !dir_path.exists() {
        return format!("Error: Path '{}' does not exist", path);
    }

    if !dir_path.is_dir() {
        return format!("Error: '{}' is not a directory", path);
    }

    // When a guard is active, verify the open descriptor before listing, so a
    // symlink swapped in after the argument check cannot turn this into a
    // listing of a directory outside the allowed roots.
    if let Some(g) = &guard
        && let Err(e) = g.open_dir_contained(dir_path)
    {
        return e;
    }

    let entries = match fs::read_dir(&path) {
            Ok(e) => e,
            Err(e) => return format!("Error: Failed to read directory: {}", e),
        };

        let mut files: Vec<String> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| {
                let file_name = entry.file_name();
                let name = file_name.to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                if is_dir { format!("{}/", name) } else { name }
            })
            .collect();

        files.sort();

        if files.is_empty() {
            return "Directory is empty".to_string();
        }

        files.join("\n")
}
