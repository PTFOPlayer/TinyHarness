use std::collections::HashMap;

use crate::tools::tool::{BoxFuture, Tool, build_string_params_schema, make_tool, require_arg};

pub fn glob_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let pattern = match require_arg(&args, "pattern") {
            Ok(p) => p,
            Err(e) => return e,
        };

        let max_results: usize = args
            .get("max_results")
            .and_then(|m| m.parse().ok())
            .unwrap_or(100);

        let glob_pattern = if pattern.starts_with('/')
            || pattern.starts_with("./")
            || pattern.starts_with("../")
        {
            pattern.clone()
        } else {
            // If it's a bare pattern like "**/*.rs", prepend "./"
            if pattern.starts_with("**") {
                format!("./{}", pattern)
            } else {
                format!("./**/{}", pattern)
            }
        };

        let mut results: Vec<String> = match glob::glob(&glob_pattern) {
            Ok(entries) => entries
                .filter_map(|entry| entry.ok())
                .map(|p| p.to_string_lossy().to_string())
                .collect(),
            Err(e) => return format!("Error: Invalid glob pattern '{}': {}", pattern, e),
        };

        results.sort();

        if results.is_empty() {
            return format!("No files found matching pattern '{}'", pattern);
        }

        // Limit results
        if results.len() > max_results {
            let total = results.len();
            results.truncate(max_results);
            results.push(format!(
                "... and {} more files (truncated)",
                total - max_results
            ));
        }

        results.join("\n")
    })
}

pub fn glob_tool_entry() -> Tool {
    make_tool(
        "glob",
        "Find files by glob pattern. Supports patterns like '**/*.rs', 'src/**/*.toml', '**/Cargo.toml'. Returns sorted results. Use 'max_results' to limit output (default 100).",
        build_string_params_schema(
            &[(
                "pattern",
                "The glob pattern to search for (e.g. '**/*.rs', '**/Cargo.toml')",
            )],
            &[(
                "max_results",
                "Maximum number of results to return (default: 100)",
                "100",
            )],
        ),
        glob_tool,
    )
}
