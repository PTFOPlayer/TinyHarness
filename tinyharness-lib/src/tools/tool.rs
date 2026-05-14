use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use schemars::Schema;

/// Extract required string arguments from the tool arguments map.
///
/// Creates `let` bindings for each named argument, with early return on missing args.
///
/// # Example
/// ```ignore
/// extract_args!(args, path, content);
/// // expands to:
/// // let path = match require_arg(&args, "path") { Ok(v) => v, Err(e) => return e };
/// // let content = match require_arg(&args, "content") { Ok(v) => v, Err(e) => return e };
/// ```
#[macro_export]
macro_rules! extract_args {
    ($args:expr, $($name:ident),* $(,)?) => {
        $(
            let $name = match $crate::tools::tool::require_arg(&$args, stringify!($name)) {
                Ok(v) => v,
                Err(e) => return e,
            };
        )*
    };
}

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Classifies a tool by its side effects, used to determine whether the tool
/// requires user approval before execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCategory {
    /// Safe to execute automatically — reads data without side effects
    /// (ls, read, grep, glob, web_search, web_fetch).
    ReadOnly,
    /// Modifies the filesystem or executes commands — needs user approval
    /// (write, edit, run).
    Destructive,
    /// Returns a signal that the caller must interpret; not meant to be
    /// executed as a normal tool (switch_mode, question, auto_compact).
    Signal,
}

pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Schema,
    pub category: ToolCategory,
    pub handler: Box<dyn Fn(HashMap<String, String>) -> BoxFuture<'static, String> + Send + Sync>,
}

impl Tool {
    /// Convert this tool into a `ToolDefinition` for the provider.
    pub fn to_definition(&self) -> crate::provider::ToolDefinition {
        crate::provider::ToolDefinition {
            name: self.name.clone(),
            description: self.description.clone(),
            parameters: self.parameters.clone(),
        }
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

    (tool.handler)(args).await
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

    serde_json::from_value(schema_value).unwrap()
}

/// Convenience constructor for creating a `Tool` with a string-parameters schema.
pub fn make_tool(
    name: &str,
    description: &str,
    category: ToolCategory,
    parameters: Schema,
    handler: impl Fn(HashMap<String, String>) -> BoxFuture<'static, String> + Send + Sync + 'static,
) -> Tool {
    Tool {
        name: name.to_string(),
        description: description.to_string(),
        parameters,
        category,
        handler: Box::new(handler),
    }
}
