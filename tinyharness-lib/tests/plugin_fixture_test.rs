/// Integration test: load the test fixtures plugins.json through PluginManager
/// and verify all hooks and custom tools are parsed and usable.
use std::path::PathBuf;

use tinyharness_lib::tools::ToolManager;

fn plugins_json_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("..");
    path.push("tests");
    path.push("plugins");
    path.push("plugins.json");
    path
}

#[test]
fn load_test_fixture_config() {
    let path = plugins_json_path();
    let content = std::fs::read_to_string(&path).expect("plugins.json should exist");

    // Parse the fixture as a PluginConfig
    let config: tinyharness_lib::plugin::PluginConfig =
        serde_json::from_str(&content).expect("plugins.json should parse");

    assert_eq!(config.hooks.len(), 7, "expected 7 hooks in fixture");
    assert_eq!(
        config.custom_tools.len(),
        5,
        "expected 5 custom tools in fixture"
    );

    // Verify hook names
    let hook_names: Vec<&str> = config.hooks.iter().map(|h| h.name.as_str()).collect();
    assert!(hook_names.contains(&"before-msg-echo"));
    assert!(hook_names.contains(&"after-msg-echo"));
    assert!(hook_names.contains(&"before-llm-inject-timestamp"));
    assert!(hook_names.contains(&"after-llm-log"));
    assert!(hook_names.contains(&"before-tool-log"));
    assert!(hook_names.contains(&"after-tool-log"));
    assert!(hook_names.contains(&"on-exit-goodbye"));

    // Verify tool names
    let tool_names: Vec<&str> = config
        .custom_tools
        .iter()
        .map(|t| t.name.as_str())
        .collect();
    assert!(tool_names.contains(&"line_count"));
    assert!(tool_names.contains(&"word_count"));
    assert!(tool_names.contains(&"make_target"));
    assert!(tool_names.contains(&"git_branch"));
    assert!(tool_names.contains(&"cargo_test_filtered"));
}

#[test]
fn fixture_tools_register_in_tool_manager() {
    let path = plugins_json_path();
    let content = std::fs::read_to_string(&path).expect("plugins.json should exist");
    let config: tinyharness_lib::plugin::PluginConfig =
        serde_json::from_str(&content).expect("plugins.json should parse");

    let mut tm = ToolManager::new();
    tm.register_defaults();
    tm.register_custom_tools(&config.custom_tools);

    // All custom tools should be registered
    let all_defs = tm.get_all_tool_definitions();
    let all_names: Vec<&str> = all_defs.iter().map(|d| d.name.as_str()).collect();
    assert!(all_names.contains(&"line_count"));
    assert!(all_names.contains(&"word_count"));
    assert!(all_names.contains(&"make_target"));
    assert!(all_names.contains(&"git_branch"));
    assert!(all_names.contains(&"cargo_test_filtered"));

    // Readonly tools should not need approval
    assert!(!tm.needs_approval("line_count"));
    assert!(!tm.needs_approval("word_count"));
    assert!(!tm.needs_approval("git_branch"));

    // Destructive tools should need approval
    assert!(tm.needs_approval("make_target"));
    assert!(tm.needs_approval("cargo_test_filtered"));
}

#[tokio::test]
async fn fixture_line_count_tool_executes() {
    let path = plugins_json_path();
    let content = std::fs::read_to_string(&path).expect("plugins.json should exist");
    let config: tinyharness_lib::plugin::PluginConfig =
        serde_json::from_str(&content).expect("plugins.json should parse");

    let line_count_def = config
        .custom_tools
        .iter()
        .find(|t| t.name == "line_count")
        .expect("line_count tool should exist");

    let tool = line_count_def
        .build_tool()
        .expect("build_tool should succeed");

    // Create a test file with 5 lines
    let test_file = std::env::temp_dir().join("tinyharness_plugin_line_count_test.txt");
    std::fs::write(&test_file, "a\nb\nc\nd\ne\n").unwrap();

    let args = serde_json::json!({"path": test_file.to_string_lossy()});
    let result = tinyharness_lib::tools::tool::execute_tool_call(&tool, &args).await;

    // wc -l should return 5
    assert!(
        !result.starts_with("Error:"),
        "tool should succeed: {result}"
    );
    assert!(
        result.contains('5'),
        "result should contain line count 5: {result}"
    );

    let _ = std::fs::remove_file(&test_file);
}

#[tokio::test]
async fn fixture_before_llm_hook_injects_stdout() {
    let path = plugins_json_path();
    let content = std::fs::read_to_string(&path).expect("plugins.json should exist");
    let config: tinyharness_lib::plugin::PluginConfig =
        serde_json::from_str(&content).expect("plugins.json should parse");

    let manager = tinyharness_lib::plugin::PluginManager::from_config(config);

    let ctx = tinyharness_lib::plugin::HookContext::default();
    let outcome = manager
        .run_hooks(tinyharness_lib::plugin::HookEvent::BeforeLlmCall, &ctx)
        .await;

    // The before-llm-inject-timestamp hook has inject_stdout: true
    assert!(
        outcome.injected_text.is_some(),
        "should have injected text from before_llm_call hook"
    );
    let injected = outcome.injected_text.unwrap();
    assert!(
        injected.contains("current time"),
        "injected text should contain timestamp info: {injected}"
    );
}

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn fixture_all_hook_events_fire() {
    let path = plugins_json_path();
    let content = std::fs::read_to_string(&path).expect("plugins.json should exist");
    let config: tinyharness_lib::plugin::PluginConfig =
        serde_json::from_str(&content).expect("plugins.json should parse");

    let manager = tinyharness_lib::plugin::PluginManager::from_config(config);

    // Test each hook event fires without error
    let events = [
        tinyharness_lib::plugin::HookEvent::BeforeUserMessage,
        tinyharness_lib::plugin::HookEvent::AfterUserMessage,
        tinyharness_lib::plugin::HookEvent::BeforeLlmCall,
        tinyharness_lib::plugin::HookEvent::AfterLlmResponse,
        tinyharness_lib::plugin::HookEvent::BeforeToolCall,
        tinyharness_lib::plugin::HookEvent::AfterToolCall,
        tinyharness_lib::plugin::HookEvent::OnExit,
    ];

    for event in &events {
        let ctx = tinyharness_lib::plugin::HookContext::default();
        let outcome = manager.run_hooks(*event, &ctx).await;
        // Some hooks write to files and might fail if the directory doesn't exist,
        // but that should just be a warning, not a crash
        // The before_llm_call hook should produce injected text
        if *event == tinyharness_lib::plugin::HookEvent::BeforeLlmCall {
            assert!(
                outcome.injected_text.is_some(),
                "before_llm_call should inject"
            );
        }
    }
}
