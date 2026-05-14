use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

use crate::extract_args;
use crate::tools::tool::{BoxFuture, ToolCategory, build_string_params_schema, make_tool};

pub fn read_tool_entry() -> crate::tools::tool::Tool {
    make_tool(
        "read",
        "Read file content. Returns the entire file or a specific line range if from/to are provided.",
        ToolCategory::ReadOnly,
        build_string_params_schema(
            &[("path", "The absolute path to the file to read")],
            &[
                ("from", "Starting line number (0-based, inclusive)", "0"),
                (
                    "to",
                    "Number of lines to read (if from is set, reads this many lines)",
                    "",
                ),
            ],
        ),
        |args| Box::pin(read_tool(args)),
    )
}

pub fn read_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        extract_args!(args, path);

        // Check if partial reading is requested
        let from = args.get("from").and_then(|f| f.parse::<usize>().ok());
        let to = args.get("to").and_then(|t| t.parse::<usize>().ok());

        match (from, to) {
            (Some(from), Some(to)) => read_partial(&path, from, to),
            _ => match fs::read_to_string(&path) {
                Ok(content) => {
                    let line_count = content.lines().count();
                    format!("Read '{}' ({} lines)\n{}", path, line_count, content)
                }
                Err(e) => format!("Error reading file: {}", e),
            },
        }
    })
}

fn read_partial(path: &str, from: usize, to: usize) -> String {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(err) => return format!("Error: {}", err),
    };

    let reader = BufReader::new(file);

    let mut content = String::new();
    let mut lines_read = 0usize;

    for line in reader.lines().skip(from).take(to).flatten() {
        content.push_str(&line);
        content.push('\n');
        lines_read += 1;
    }

    if content.is_empty() {
        format!("Error: No lines to read in '{}' at offset {}", path, from)
    } else {
        format!(
            "Read '{}' ({} lines, starting at line {})\n{}",
            path, lines_read, from, content
        )
    }
}
