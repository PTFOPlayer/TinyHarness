use tinyharness_lib::plugin::{
    CustomToolCategory, CustomToolDefinition, HookContext, HookDefinition, HookEvent, PluginConfig,
    PluginManager, ShellCommand,
};

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
    assert_eq!(result, "Hello, World!");
}

/// Integration test: a hook fires and its stdout is collected via inject_stdout.
#[tokio::test]
async fn hook_inject_stdout() {
    let hook = HookDefinition {
        name: "test-inject".to_string(),
        event: HookEvent::BeforeLlmCall,
        command: ShellCommand {
            command: "echo 'injected context'".to_string(),
            timeout_secs: 5,
            cwd: None,
        },
        inject_stdout: true,
        block_on_output: None,
    };

    let config = PluginConfig {
        hooks: vec![hook],
        custom_tools: vec![],
    };
    let mgr = PluginManager::from_config(config);

    let ctx = HookContext::default();
    let outcome = mgr.run_hooks(HookEvent::BeforeLlmCall, &ctx).await;

    assert!(outcome.warnings.is_empty());
    assert!(!outcome.blocked);
    assert_eq!(outcome.injected_text.as_deref(), Some("injected context"));
}

/// Integration test: a hook with block_on_output blocks the action.
#[tokio::test]
async fn hook_blocks_action() {
    let hook = HookDefinition {
        name: "block-rm".to_string(),
        event: HookEvent::BeforeToolCall,
        command: ShellCommand {
            command: "echo BLOCKED: rm is not allowed".to_string(),
            timeout_secs: 5,
            cwd: None,
        },
        inject_stdout: false,
        block_on_output: Some("BLOCKED".to_string()),
    };

    let config = PluginConfig {
        hooks: vec![hook],
        custom_tools: vec![],
    };
    let mgr = PluginManager::from_config(config);

    let ctx = HookContext {
        tool_name: Some("run".to_string()),
        ..Default::default()
    };
    let outcome = mgr.run_hooks(HookEvent::BeforeToolCall, &ctx).await;

    assert!(outcome.blocked);
    assert!(
        outcome
            .block_reason
            .as_deref()
            .unwrap()
            .contains("rm is not allowed")
    );
}

/// Integration test: hooks only fire for their registered event.
#[tokio::test]
async fn hook_only_fires_for_matching_event() {
    let hook = HookDefinition {
        name: "after-tool-only".to_string(),
        event: HookEvent::AfterToolCall,
        command: ShellCommand {
            command: "echo 'tool done'".to_string(),
            timeout_secs: 5,
            cwd: None,
        },
        inject_stdout: true,
        block_on_output: None,
    };

    let config = PluginConfig {
        hooks: vec![hook],
        custom_tools: vec![],
    };
    let mgr = PluginManager::from_config(config);

    // Should not fire for a different event
    let ctx = HookContext::default();
    let outcome = mgr.run_hooks(HookEvent::BeforeLlmCall, &ctx).await;
    assert!(outcome.injected_text.is_none());

    // Should fire for the matching event
    let outcome = mgr.run_hooks(HookEvent::AfterToolCall, &ctx).await;
    assert_eq!(outcome.injected_text.as_deref(), Some("tool done"));
}

/// Integration test: multiple hooks for the same event fire in order.
#[tokio::test]
async fn multiple_hooks_same_event_in_order() {
    let hooks = vec![
        HookDefinition {
            name: "first".to_string(),
            event: HookEvent::OnExit,
            command: ShellCommand {
                command: "echo first".to_string(),
                timeout_secs: 5,
                cwd: None,
            },
            inject_stdout: true,
            block_on_output: None,
        },
        HookDefinition {
            name: "second".to_string(),
            event: HookEvent::OnExit,
            command: ShellCommand {
                command: "echo second".to_string(),
                timeout_secs: 5,
                cwd: None,
            },
            inject_stdout: true,
            block_on_output: None,
        },
    ];

    let config = PluginConfig {
        hooks,
        custom_tools: vec![],
    };
    let mgr = PluginManager::from_config(config);

    let ctx = HookContext::default();
    let outcome = mgr.run_hooks(HookEvent::OnExit, &ctx).await;

    let injected = outcome.injected_text.expect("should have injected text");
    assert!(injected.contains("first"));
    assert!(injected.contains("second"));
    // First hook output should come before second
    assert!(injected.find("first").unwrap() < injected.find("second").unwrap());
}

/// Integration test: hook failure doesn't crash other hooks.
#[tokio::test]
async fn hook_failure_does_not_crash_others() {
    let hooks = vec![
        HookDefinition {
            name: "failing".to_string(),
            event: HookEvent::AfterToolCall,
            command: ShellCommand {
                command: "exit 1".to_string(),
                timeout_secs: 5,
                cwd: None,
            },
            inject_stdout: false,
            block_on_output: None,
        },
        HookDefinition {
            name: "succeeding".to_string(),
            event: HookEvent::AfterToolCall,
            command: ShellCommand {
                command: "echo ok".to_string(),
                timeout_secs: 5,
                cwd: None,
            },
            inject_stdout: true,
            block_on_output: None,
        },
    ];

    let config = PluginConfig {
        hooks,
        custom_tools: vec![],
    };
    let mgr = PluginManager::from_config(config);

    let ctx = HookContext::default();
    let outcome = mgr.run_hooks(HookEvent::AfterToolCall, &ctx).await;

    // The failing hook should produce a warning
    assert_eq!(outcome.warnings.len(), 1);
    assert!(outcome.warnings[0].contains("failing"));

    // The succeeding hook should still produce output
    assert_eq!(outcome.injected_text.as_deref(), Some("ok"));
}

/// Integration test: custom tools register in ToolManager and appear in tool definitions.
#[tokio::test]
async fn custom_tool_registers_in_tool_manager() {
    use tinyharness_lib::tools::ToolManager;

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
    use tinyharness_lib::tools::ToolManager;

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
    use tinyharness_lib::tools::ToolManager;

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

/// Integration test: hook context variables are available as TH_* env vars.
#[tokio::test]
async fn hook_context_env_vars_available() {
    let hook = HookDefinition {
        name: "env-test".to_string(),
        event: HookEvent::AfterToolCall,
        command: ShellCommand {
            command: (if cfg!(target_os = "windows") {
                "echo %TH_TOOL%"
            } else {
                "echo $TH_TOOL"
            })
            .to_string(),
            timeout_secs: 5,
            cwd: None,
        },
        inject_stdout: true,
        block_on_output: None,
    };

    let config = PluginConfig {
        hooks: vec![hook],
        custom_tools: vec![],
    };
    let mgr = PluginManager::from_config(config);

    let ctx = HookContext {
        tool_name: Some("my_tool".to_string()),
        ..Default::default()
    };
    let outcome = mgr.run_hooks(HookEvent::AfterToolCall, &ctx).await;

    assert_eq!(outcome.injected_text.as_deref(), Some("my_tool"));
}
