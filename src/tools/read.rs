use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

use crate::provider::{ToolFunctionInfo, ToolInfo, ToolType};
use crate::tools::tool::{build_string_params_schema, BoxFuture, Tool};

pub fn read_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let path = match args.get("path") {
            Some(p) => p.clone(),
            None => return "Error: 'path' argument is required".to_string(),
        };

        // Check if partial reading is requested
        let from = args.get("from").and_then(|f| f.parse::<usize>().ok());
        let to = args.get("to").and_then(|t| t.parse::<usize>().ok());

        match (from, to) {
            (Some(from), Some(to)) => read_partial(&path, from, to),
            _ => match fs::read_to_string(&path) {
                Ok(content) => {
                    let line_count = content.lines().count();
                    format!(
                        "Read '{}' ({} lines)\n{}",
                        path, line_count, content
                    )
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

    for line in reader.lines().skip(from).take(to) {
        if let Ok(line) = line {
            content.push_str(&line);
            content.push('\n');
            lines_read += 1;
        }
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

pub fn read_tool_entry() -> Tool {
    let tool_info = ToolInfo {
        tool_type: ToolType::Function,
        function: ToolFunctionInfo {
            name: "read".to_string(),
            description: "Read file content. Returns the entire file or a specific line range if from/to are provided.".to_string(),
            parameters: build_string_params_schema(
                &[("path", "The absolute path to the file to read")],
                &[
                    ("from", "Starting line number (0-based, optional)", "0"),
                    ("to", "Number of lines to read (optional, reads entire file if omitted)", ""),
                ],
            ),
        },
    };

    Tool {
        function: Box::new(read_tool),
        tool_info,
    }
}
