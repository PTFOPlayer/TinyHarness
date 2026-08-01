/// Integration test: load the test fixtures custom_tools.json through
/// CustomToolManager and verify all custom tools are parsed and usable.
use std::path::PathBuf;

use tinyharness_lib::custom_tools::CustomToolConfig;
use tinyharness_lib::tools::ToolManager;

fn custom_tools_json_path() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("..");
    path.push("tests");
    path.push("custom_tools");
    path.push("custom_tools.json");
    path
}

#[test]
fn load_test_fixture_config() {
    let path = custom_tools_json_path();
    let content = std::fs::read_to_string(&path).expect("custom_tools.json should exist");

    // Parse the fixture as a CustomToolConfig
    let config: CustomToolConfig =
        serde_json::from_str(&content).expect("custom_tools.json should parse");

    assert_eq!(
        config.custom_tools.len(),
        5,
        "expected 5 custom tools in fixture"
    );

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
    let path = custom_tools_json_path();
    let content = std::fs::read_to_string(&path).expect("custom_tools.json should exist");
    let config: CustomToolConfig =
        serde_json::from_str(&content).expect("custom_tools.json should parse");

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

#[cfg(not(target_os = "windows"))]
#[tokio::test]
async fn fixture_line_count_tool_executes() {
    let path = custom_tools_json_path();
    let content = std::fs::read_to_string(&path).expect("custom_tools.json should exist");
    let config: CustomToolConfig =
        serde_json::from_str(&content).expect("custom_tools.json should parse");

    let line_count_def = config
        .custom_tools
        .iter()
        .find(|t| t.name == "line_count")
        .expect("line_count tool should exist");

    let tool = line_count_def
        .build_tool()
        .expect("build_tool should succeed");

    // Create a test file with 5 lines
    let test_file = std::env::temp_dir().join("tinyharness_custom_tool_line_count_test.txt");
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
