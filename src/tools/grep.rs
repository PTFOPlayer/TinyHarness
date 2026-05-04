use std::collections::HashMap;
use std::fs;
use std::path::Path;

use regex::Regex;

use crate::tools::tool::{BoxFuture, Tool, build_string_params_schema, make_tool, require_arg};

pub fn grep_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let pattern = match require_arg(&args, "pattern") {
            Ok(p) => p,
            Err(e) => return e,
        };

        let path = match args.get("path") {
            Some(p) => p.clone(),
            None => ".".to_string(),
        };

        let include_pattern = args.get("include").map(|s| s.as_str());

        let regex = match Regex::new(&pattern) {
            Ok(r) => r,
            Err(e) => return format!("Error: Invalid regex pattern '{}': {}", pattern, e),
        };

        let root = Path::new(&path);
        if !root.exists() {
            return format!("Error: Path '{}' does not exist", path);
        }
        if !root.is_dir() {
            return format!("Error: '{}' is not a directory", path);
        }

        let mut results: Vec<String> = Vec::new();
        let mut total_matches = 0;

        if let Err(e) = walk_dir(
            root,
            &regex,
            include_pattern,
            &mut results,
            &mut total_matches,
        ) {
            return format!("Error: {}", e);
        }

        if results.is_empty() {
            return format!("No matches found for pattern '{}'", pattern);
        }

        // Limit output to avoid huge responses
        const MAX_LINES: usize = 200;
        let mut output = String::new();

        for (line_count, line) in results.iter().enumerate() {
            if line_count >= MAX_LINES {
                output.push_str(&format!(
                    "... and {} more matches (truncated)\n",
                    total_matches - line_count
                ));
                break;
            }
            output.push_str(line);
            output.push('\n');
        }

        output.trim_end().to_string()
    })
}

fn walk_dir(
    dir: &Path,
    regex: &Regex,
    include_pattern: Option<&str>,
    results: &mut Vec<String>,
    total_matches: &mut usize,
) -> Result<(), String> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            return Err(format!(
                "Failed to read directory '{}': {}",
                dir.display(),
                e
            ));
        }
    };

    for entry in entries.flatten() {
        let path = entry.path();

        if path.is_dir() {
            // Skip hidden directories and common non-code directories
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && (name.starts_with('.') || name == "node_modules" || name == "target")
            {
                continue;
            }
            walk_dir(&path, regex, include_pattern, results, total_matches)?;
        } else if path.is_file() {
            // Apply include filter if specified
            if let Some(inc) = include_pattern {
                if !path.to_string_lossy().contains(inc) {
                    continue;
                }
            }

            // Skip binary-looking files
            if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                match ext {
                    "png" | "jpg" | "jpeg" | "gif" | "bmp" | "ico" | "svg" | "woff" | "woff2"
                    | "ttf" | "eot" | "otf" | "pdf" | "zip" | "tar" | "gz" | "bz2" | "xz"
                    | "exe" | "dll" | "so" | "dylib" | "wasm" | "o" | "pyc" | "class" => continue,
                    _ => {}
                }
            }

            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue, // Skip binary files
            };

            let rel_path = path.to_string_lossy();
            for (line_num, line) in content.lines().enumerate() {
                if regex.is_match(line) {
                    results.push(format!("{}:{}:{}", rel_path, line_num + 1, line));
                    *total_matches += 1;
                }
            }
        }
    }

    Ok(())
}

pub fn grep_tool_entry() -> Tool {
    make_tool(
        "grep",
        "Search for a regex pattern across files in a directory. Returns matching lines with file paths and line numbers. Use 'include' to filter by file extension (e.g. '.rs' for Rust files). Skips hidden directories, node_modules, target, and binary files.",
        build_string_params_schema(
            &[("pattern", "The regex pattern to search for")],
            &[
                (
                    "path",
                    "The directory to search in (defaults to current directory)",
                    ".",
                ),
                (
                    "include",
                    "Only search files whose path contains this string (e.g. '.rs' for Rust files)",
                    "",
                ),
            ],
        ),
        grep_tool,
    )
}
