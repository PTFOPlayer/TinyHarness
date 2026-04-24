use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};

use crate::tools::tool::Tool;
use ollama_rs::generation::tools::ToolInfo;

pub fn read_tool(args: HashMap<String, String>) -> String {
    let path = match args.get("path") {
        Some(p) => p,
        None => return "Error: 'path' argument is required".to_string(),
    };

    // Check if partial reading is requested
    let from = args.get("from").and_then(|f| f.parse::<usize>().ok());
    let to = args.get("to").and_then(|t| t.parse::<usize>().ok());

    match (from, to) {
        (Some(from), Some(to)) => read_partial(path, from, to),
        _ => fs::read_to_string(path).unwrap_or_else(|e| format!("Error reading file: {}", e)),
    }
}

fn read_partial(path: &str, from: usize, to: usize) -> String {
    let file = match File::open(path) {
        Ok(f) => f,
        Err(err) => return format!("Error: {}", err),
    };

    let reader = BufReader::new(file);

    reader
        .lines()
        .skip(from)
        .take(to)
        .fold(String::new(), |mut acc, line| {
            if let Ok(line) = line {
                acc.push_str(&line);
                acc.push('\n');
            }
            acc
        })
}

pub fn read_tool_entry() -> Tool {
    let tool_info = ToolInfo {
        tool_type: ollama_rs::generation::tools::ToolType::Function,
        function: ollama_rs::generation::tools::ToolFunctionInfo {
            name: "read".to_string(),
            description: "Read file content. Returns the entire file or a specific line range if from/to are provided.".to_string(),
            parameters: schemars::schema_for!(serde_json::Map::<String, serde_json::Value>),
        },
    };

    Tool {
        name: "read".to_string(),
        function: read_tool,
        tool_info,
    }
}
