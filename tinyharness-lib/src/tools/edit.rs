use std::collections::HashMap;
use std::fs;
use std::path::Path;

use crate::extract_args;
use crate::path_guard::PathGuard;
use crate::tools::read::read_string_guarded;
use crate::tools::tool::{BoxFuture, ToolCategory, build_string_params_schema, make_tool};

pub fn edit_tool_entry(guard: Option<PathGuard>) -> crate::tools::tool::Tool {
    make_tool(
        "edit",
        "Edit a file by finding an exact string and replacing it with new text. The old_str must appear exactly once in the file. Use this for targeted edits instead of rewriting the entire file.",
        ToolCategory::Destructive,
        build_string_params_schema(
            &[
                ("path", "The absolute path to the file to edit"),
                (
                    "old_str",
                    "The exact string to find in the file (must appear exactly once)",
                ),
                ("new_str", "The replacement string"),
            ],
            &[],
        ),
        move |args| {
            let guard = guard.clone();
            Box::pin(edit_tool_inner(args, guard))
        },
    )
}

pub fn edit_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(edit_tool_inner(args, None))
}

async fn edit_tool_inner(args: HashMap<String, String>, guard: Option<PathGuard>) -> String {
    extract_args!(args, path, old_str, new_str);

    // Read the file (through the guard's verified descriptor when active)
    let content = match read_string_guarded(&path, guard.as_ref()) {
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

    // Write the file (through the guard's verified descriptor when active)
    let result = match &guard {
        Some(g) => g.write_contained(Path::new(&path), &new_content),
        None => fs::write(&path, &new_content).map_err(|e| e.to_string()),
    };
    match result {
        Ok(_) => format!("Edited '{}' ({} chars replaced).", path, old_str.len()),
        Err(e) => format!("Error: Failed to write file '{}': {}", path, e),
    }
}
