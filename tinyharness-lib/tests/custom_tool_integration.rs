use tinyharness_lib::custom_tools::{
    CustomToolCategory, CustomToolConfig, CustomToolDefinition, CustomToolManager, ShellCommand,
};
use tinyharness_lib::tools::ToolManager;

/// Integration test: a custom tool defined in config executes a shell command
/// and returns the result when called through the ToolManager.
#[tokio::test]
async fn custom_tool_end_to_end() {
    let def = CustomToolDefinition {
        name: "greet".to_string(),
        description: "Greet someone".to_string(),
        category: CustomToolCategory::ReadOnly,
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Who to greet" }
            },
            "required": ["name"]
        }),
        command: ShellCommand {
            command: "echo Hello, {name}!".to_string(),
            timeout_secs: 5,
            cwd: None,
        },
    };

    let tool = def.build_tool().expect("build_tool should succeed");
    let args = serde_json::json!({"name": "World"});
    let result = tinyharness_lib::tools::tool::execute_tool_call(&tool, &args).await;
    // On Windows, cmd /C doesn't strip single quotes the way sh does,
    // so the shell-escaped value appears literally in the output.
    let expected = if cfg!(target_os = "windows") {
        "Hello, 'World'!"
    } else {
        "Hello, World!"
    };
    assert_eq!(result, expected);
}

/// Integration test: custom tools register in ToolManager and appear in tool definitions.
#[tokio::test]
async fn custom_tool_registers_in_tool_manager() {
    let def = CustomToolDefinition {
        name: "my_tool".to_string(),
        description: "A custom tool".to_string(),
        category: CustomToolCategory::ReadOnly,
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "arg": { "type": "string" }
            },
            "required": ["arg"]
        }),
        command: ShellCommand {
            command: "echo {arg}".to_string(),
            timeout_secs: 5,
            cwd: None,
        },
    };

    let mut tm = ToolManager::new();
    tm.register_defaults();
    tm.register_custom_tools(&[def]);

    // The custom tool should appear in tool definitions
    let defs = tm.get_all_tool_definitions();
    assert!(defs.iter().any(|d| d.name == "my_tool"));

    // The custom tool should be categorized as ReadOnly
    assert_eq!(
        tm.category_of("my_tool"),
        Some(tinyharness_lib::tools::tool::ToolCategory::ReadOnly)
    );
}

/// Integration test: custom tool with destructive category requires approval.
#[tokio::test]
async fn custom_tool_destructive_needs_approval() {
    let def = CustomToolDefinition {
        name: "dangerous".to_string(),
        description: "A destructive custom tool".to_string(),
        category: CustomToolCategory::Destructive,
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        command: ShellCommand {
            command: "echo boom".to_string(),
            timeout_secs: 5,
            cwd: None,
        },
    };

    let mut tm = ToolManager::new();
    tm.register_defaults();
    tm.register_custom_tools(&[def]);

    assert!(tm.needs_approval("dangerous"));
}

/// Integration test: custom tool name collision with built-in tool is skipped.
#[tokio::test]
async fn custom_tool_collision_skipped() {
    let def = CustomToolDefinition {
        name: "ls".to_string(), // collides with built-in
        description: "Custom ls".to_string(),
        category: CustomToolCategory::ReadOnly,
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
        command: ShellCommand {
            command: "echo custom".to_string(),
            timeout_secs: 5,
            cwd: None,
        },
    };

    let mut tm = ToolManager::new();
    tm.register_defaults();
    tm.register_custom_tools(&[def]);

    // The built-in ls should still be there, not the custom one
    let defs = tm.get_all_tool_definitions();
    let ls_defs: Vec<_> = defs.iter().filter(|d| d.name == "ls").collect();
    assert_eq!(ls_defs.len(), 1); // only the built-in, not a duplicate
}

/// Integration test: CustomToolManager loads and exposes its config.
#[test]
fn custom_tool_manager_from_config() {
    let config = CustomToolConfig {
        custom_tools: vec![CustomToolDefinition {
            name: "x".to_string(),
            description: "X".to_string(),
            category: CustomToolCategory::ReadOnly,
            parameters: serde_json::json!({ "type": "object", "properties": {}, "required": [] }),
            command: ShellCommand {
                command: "echo x".to_string(),
                timeout_secs: 5,
                cwd: None,
            },
        }],
    };
    let mgr = CustomToolManager::from_config(config);
    assert_eq!(mgr.custom_tools().len(), 1);
    assert!(!mgr.is_empty());
}
