use std::collections::HashMap;

use schemars::Schema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tools::tool::{BoxFuture, Tool, ToolCategory, make_tool};

use super::shell::{ShellCommand, execute_shell_tool};

/// Category string for custom tools, as used in the config file.
///
/// `"readonly"` maps to `ToolCategory::ReadOnly` (auto-executed, no confirmation).
/// `"destructive"` maps to `ToolCategory::Destructive` (requires confirmation).
/// `"signal"` is not allowed for custom tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CustomToolCategory {
    ReadOnly,
    Destructive,
}

impl CustomToolCategory {
    pub fn to_tool_category(self) -> ToolCategory {
        match self {
            CustomToolCategory::ReadOnly => ToolCategory::ReadOnly,
            CustomToolCategory::Destructive => ToolCategory::Destructive,
        }
    }
}

/// Definition of a custom tool from the plugin config file.
///
/// Custom tools are registered alongside built-in tools and can be called
/// by the LLM. They execute shell commands with parameter substitution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomToolDefinition {
    /// Tool name (must not collide with built-in tool names).
    pub name: String,
    /// Description shown to the LLM.
    pub description: String,
    /// `"readonly"` or `"destructive"`. Readonly tools are auto-executed;
    /// destructive tools require user confirmation.
    pub category: CustomToolCategory,
    /// JSON Schema for the tool's parameters. This is sent to the LLM as-is.
    pub parameters: Value,
    /// Shell command template with `{param}` placeholders.
    #[serde(flatten)]
    pub command: ShellCommand,
}

impl CustomToolDefinition {
    /// Convert this definition into a `Tool` that can be registered in `ToolManager`.
    pub fn build_tool(&self) -> Result<Tool, String> {
        let parameters: Schema = serde_json::from_value(self.parameters.clone()).map_err(|e| {
            format!(
                "Failed to parse parameters schema for '{}': {}",
                self.name, e
            )
        })?;

        let command = self.command.clone();
        let category = self.category.to_tool_category();
        let description = self.description.clone();

        Ok(make_tool(
            &self.name,
            &description,
            category,
            parameters,
            move |args: HashMap<String, String>| -> BoxFuture<'static, String> {
                let cmd = command.clone();
                Box::pin(async move { execute_shell_tool(&cmd, &args).await })
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_custom_tool_category_serde() {
        let json = r#""readonly""#;
        let cat: CustomToolCategory = serde_json::from_str(json).unwrap();
        assert_eq!(cat, CustomToolCategory::ReadOnly);

        let json = r#""destructive""#;
        let cat: CustomToolCategory = serde_json::from_str(json).unwrap();
        assert_eq!(cat, CustomToolCategory::Destructive);
    }

    #[test]
    fn test_custom_tool_definition_parses() {
        let json = r#"{
            "name": "docker_run",
            "description": "Run a command in a Docker container",
            "category": "destructive",
            "parameters": {
                "type": "object",
                "properties": {
                    "image": { "type": "string", "description": "Docker image" },
                    "command": { "type": "string", "description": "Shell command" }
                },
                "required": ["image", "command"]
            },
            "command": "docker run --rm {image} sh -c '{command}'",
            "timeout_secs": 120
        }"#;
        let tool: CustomToolDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "docker_run");
        assert_eq!(tool.category, CustomToolCategory::Destructive);
        assert_eq!(
            tool.command.command,
            "docker run --rm {image} sh -c '{command}'"
        );
        assert_eq!(tool.command.timeout_secs, 120);
    }

    #[test]
    fn test_custom_tool_definition_default_timeout() {
        let json = r#"{
            "name": "greet",
            "description": "Say hi",
            "category": "readonly",
            "parameters": {
                "type": "object",
                "properties": {
                    "name": { "type": "string" }
                },
                "required": ["name"]
            },
            "command": "echo hello {name}"
        }"#;
        let tool: CustomToolDefinition = serde_json::from_str(json).unwrap();
        assert_eq!(tool.command.timeout_secs, 30);
    }

    #[test]
    fn test_build_tool_success() {
        let def = CustomToolDefinition {
            name: "greet".to_string(),
            description: "Say hello".to_string(),
            category: CustomToolCategory::ReadOnly,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "name": { "type": "string", "description": "Who to greet" }
                },
                "required": ["name"]
            }),
            command: ShellCommand {
                command: "echo hello {name}".to_string(),
                timeout_secs: 5,
                cwd: None,
            },
        };
        let tool = def.build_tool().unwrap();
        assert_eq!(tool.name, "greet");
        assert_eq!(tool.description, "Say hello");
        assert_eq!(tool.category, ToolCategory::ReadOnly);
    }

    #[test]
    fn test_build_tool_invalid_schema() {
        let def = CustomToolDefinition {
            name: "bad".to_string(),
            description: "Bad tool".to_string(),
            category: CustomToolCategory::ReadOnly,
            parameters: serde_json::json!("not an object"),
            command: ShellCommand {
                command: "echo hi".to_string(),
                timeout_secs: 5,
                cwd: None,
            },
        };
        let result = def.build_tool();
        assert!(result.is_err());
        assert!(
            result
                .err()
                .unwrap()
                .contains("Failed to parse parameters schema")
        );
    }

    #[tokio::test]
    async fn test_built_tool_executes() {
        let def = CustomToolDefinition {
            name: "echo_tool".to_string(),
            description: "Echo a value".to_string(),
            category: CustomToolCategory::ReadOnly,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "value": { "type": "string", "description": "Value to echo" }
                },
                "required": ["value"]
            }),
            command: ShellCommand {
                command: "echo {value}".to_string(),
                timeout_secs: 5,
                cwd: None,
            },
        };
        let tool = def.build_tool().unwrap();
        let args = serde_json::json!({"value": "test123"});
        let result = crate::tools::tool::execute_tool_call(&tool, &args).await;
        assert_eq!(result, "test123");
    }

    #[tokio::test]
    async fn test_built_tool_error_on_failure() {
        let def = CustomToolDefinition {
            name: "fail_tool".to_string(),
            description: "Always fails".to_string(),
            category: CustomToolCategory::Destructive,
            parameters: serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            }),
            command: ShellCommand {
                command: "exit 1".to_string(),
                timeout_secs: 5,
                cwd: None,
            },
        };
        let tool = def.build_tool().unwrap();
        let args = serde_json::json!({});
        let result = crate::tools::tool::execute_tool_call(&tool, &args).await;
        assert!(result.starts_with("Error:"));
    }
}
