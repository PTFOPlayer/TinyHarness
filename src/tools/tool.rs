use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use schemars::Schema;

use crate::provider::ToolInfo;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct Tool {
    pub function: Box<dyn Fn(HashMap<String, String>) -> BoxFuture<'static, String> + Send + Sync>,
    pub tool_info: ToolInfo,
}

impl Tool {
    pub fn name(&self) -> &str {
        &self.tool_info.function.name
    }
}

pub async fn execute_tool_call(tool: &Tool, arguments: &serde_json::Value) -> String {
    let args: HashMap<String, String> = arguments
        .as_object()
        .map(|obj| {
            obj.iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                .collect()
        })
        .unwrap_or_default();

    (tool.function)(args).await
}

/// Extract a required string argument from the tool arguments map.
/// Returns an error message if the key is missing.
pub fn require_arg(args: &HashMap<String, String>, name: &str) -> Result<String, String> {
    args.get(name)
        .cloned()
        .ok_or_else(|| format!("Error: '{}' argument is required", name))
}

/// Extract an optional string argument from the tool arguments map.
/// Returns `None` if the key is missing.
pub fn optional_arg<'a>(args: &'a HashMap<String, String>, name: &str) -> Option<&'a String> {
    args.get(name)
}

/// Build a JSON Schema for a tool that accepts string parameters.
/// `required_params`: list of (name, description) pairs for required parameters.
/// `optional_params`: list of (name, description, default_value) for optional parameters.
pub fn build_string_params_schema(
    required_params: &[(&str, &str)],
    optional_params: &[(&str, &str, &str)],
) -> Schema {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for (name, description) in required_params {
        properties.insert(
            name.to_string(),
            serde_json::json!({
                "type": "string",
                "description": description
            }),
        );
        required.push(name.to_string());
    }

    for (name, description, _default_val) in optional_params {
        properties.insert(
            name.to_string(),
            serde_json::json!({
                "type": "string",
                "description": description
            }),
        );
    }

    let schema_value = serde_json::json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    });

    serde_json::from_value(schema_value)
        .unwrap_or_else(|_| serde_json::from_value(serde_json::json!(true)).unwrap())
}

/// Convenience constructor for creating a `Tool` with a string-parameters schema.
/// Reduces the boilerplate in each `*_tool_entry()` function.
pub fn make_tool(
    name: &str,
    description: &str,
    parameters: Schema,
    function: impl Fn(HashMap<String, String>) -> BoxFuture<'static, String> + Send + Sync + 'static,
) -> Tool {
    Tool {
        function: Box::new(function),
        tool_info: ToolInfo {
            tool_type: crate::provider::ToolType::Function,
            function: crate::provider::ToolFunctionInfo {
                name: name.to_string(),
                description: description.to_string(),
                parameters,
            },
        },
    }
}
