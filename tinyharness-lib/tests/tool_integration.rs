//! Integration tests for tool execution.
//!
//! Tests the built-in tools with controlled inputs and temp directories.

use tempfile::TempDir;
use tinyharness_lib::tools::{ToolAvailability, ToolManager};

fn make_manager() -> ToolManager {
    let mut manager = ToolManager::new();
    manager.register_defaults();
    manager
}

#[tokio::test]
async fn ls_tool_lists_directory() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("file1.txt"), "content").unwrap();
    std::fs::write(tmp.path().join("file2.rs"), "fn main() {}").unwrap();

    let manager = make_manager();
    let result = manager
        .execute_tool_call(
            "ls",
            &serde_json::json!({"path": tmp.path().to_str().unwrap()}),
        )
        .await;
    assert!(result.contains("file1.txt"));
    assert!(result.contains("file2.rs"));
}

#[tokio::test]
async fn read_tool_reads_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("test.txt");
    std::fs::write(&path, "Hello, integration test!").unwrap();

    let manager = make_manager();
    let result = manager
        .execute_tool_call("read", &serde_json::json!({"path": path.to_str().unwrap()}))
        .await;
    assert!(result.contains("Hello, integration test!"));
}

#[tokio::test]
async fn read_tool_nonexistent_returns_error() {
    let manager = make_manager();
    let result = manager
        .execute_tool_call(
            "read",
            &serde_json::json!({"path": "/nonexistent/file.txt"}),
        )
        .await;
    assert!(result.contains("Error") || result.contains("not exist") || result.contains("No such"));
}

#[tokio::test]
async fn write_tool_creates_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("output.txt");

    let manager = make_manager();
    let _result = manager
        .execute_tool_call(
            "write",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "content": "test content"
            }),
        )
        .await;
    assert!(path.exists());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "test content");
}

#[tokio::test]
async fn edit_tool_modifies_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("editable.txt");
    std::fs::write(&path, "old content here").unwrap();

    let manager = make_manager();
    let _result = manager
        .execute_tool_call(
            "edit",
            &serde_json::json!({
                "path": path.to_str().unwrap(),
                "old_str": "old content",
                "new_str": "new content"
            }),
        )
        .await;
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "new content here");
}

#[tokio::test]
async fn grep_tool_searches_pattern() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("search.rs");
    std::fs::write(&path, "fn foo() {}\nfn bar() {}\nfn foobar() {}\n").unwrap();

    let manager = make_manager();
    let result = manager
        .execute_tool_call(
            "grep",
            &serde_json::json!({
                "pattern": "foo",
                "path": tmp.path().to_str().unwrap()
            }),
        )
        .await;
    assert!(result.contains("foo"), "result was: {result}");
}

#[tokio::test]
async fn glob_tool_finds_files() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.rs"), "").unwrap();
    std::fs::write(tmp.path().join("b.rs"), "").unwrap();
    std::fs::write(tmp.path().join("c.txt"), "").unwrap();

    let manager = make_manager();
    let result = manager
        .execute_tool_call(
            "glob",
            &serde_json::json!({
                "pattern": "**/*.rs",
                "path": tmp.path().to_str().unwrap()
            }),
        )
        .await;
    assert!(result.contains("a.rs"));
    assert!(result.contains("b.rs"));
    assert!(!result.contains("c.txt"));
}

#[tokio::test]
async fn unknown_tool_returns_error() {
    let manager = make_manager();
    let result = manager
        .execute_tool_call("nonexistent_tool", &serde_json::json!({}))
        .await;
    assert!(result.contains("Error") || result.contains("not found"));
}

#[tokio::test]
async fn tool_manager_has_all_default_tools() {
    let manager = make_manager();
    let defs = manager.get_all_tool_definitions();
    // Should have 14 tools (auto_compact + 13 others)
    assert!(defs.len() >= 10);
    let names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
    assert!(names.contains(&"ls".to_string()));
    assert!(names.contains(&"read".to_string()));
    assert!(names.contains(&"write".to_string()));
    assert!(names.contains(&"edit".to_string()));
    assert!(names.contains(&"grep".to_string()));
    assert!(names.contains(&"glob".to_string()));
    assert!(names.contains(&"run".to_string()));
}

#[tokio::test]
async fn tools_for_agent_mode_includes_all() {
    let manager = make_manager();
    let defs = manager.tools_for_mode(
        tinyharness_lib::mode::AgentMode::Agent,
        ToolAvailability::all(),
    );
    let names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
    assert!(names.contains(&"ls".to_string()));
    assert!(names.contains(&"write".to_string()));
    assert!(names.contains(&"run".to_string()));
}

#[tokio::test]
async fn tools_for_casual_mode_limited() {
    let manager = make_manager();
    let defs = manager.tools_for_mode(
        tinyharness_lib::mode::AgentMode::Casual,
        ToolAvailability::all(),
    );
    let names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
    // Casual mode only has web_search and web_fetch
    assert!(names.contains(&"web_search".to_string()));
    assert!(names.contains(&"web_fetch".to_string()));
    assert!(!names.contains(&"write".to_string()));
    assert!(!names.contains(&"run".to_string()));
}

#[tokio::test]
async fn tools_for_planning_mode_excludes_destructive() {
    let manager = make_manager();
    let defs = manager.tools_for_mode(
        tinyharness_lib::mode::AgentMode::Planning,
        ToolAvailability::all(),
    );
    let names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
    // Planning mode has read-only + signal tools
    assert!(names.contains(&"ls".to_string()));
    assert!(names.contains(&"read".to_string()));
    assert!(!names.contains(&"write".to_string()));
    assert!(!names.contains(&"run".to_string()));
}

#[tokio::test]
async fn questions_disabled_hides_question_tool_in_every_mode() {
    let manager = make_manager();
    let availability = ToolAvailability {
        question: false,
        ..ToolAvailability::all()
    };
    for mode in [
        tinyharness_lib::mode::AgentMode::Agent,
        tinyharness_lib::mode::AgentMode::Planning,
        tinyharness_lib::mode::AgentMode::Research,
        tinyharness_lib::mode::AgentMode::Casual,
    ] {
        let defs = manager.tools_for_mode(mode, availability);
        let names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
        assert!(
            !names.contains(&"question".to_string()),
            "{mode} still advertises `question` when disabled"
        );
    }
}

#[tokio::test]
async fn questions_enabled_by_default() {
    let manager = make_manager();
    let defs = manager.tools_for_mode(
        tinyharness_lib::mode::AgentMode::Agent,
        ToolAvailability::default(),
    );
    let names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
    assert!(names.contains(&"question".to_string()));
    assert!(names.contains(&"auto_compact".to_string()));
}

#[tokio::test]
async fn questions_disabled_leaves_auto_compact_enabled() {
    let manager = make_manager();
    let availability = ToolAvailability {
        question: false,
        ..ToolAvailability::all()
    };
    let defs = manager.tools_for_mode(tinyharness_lib::mode::AgentMode::Agent, availability);
    let names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
    assert!(!names.contains(&"question".to_string()));
    assert!(names.contains(&"auto_compact".to_string()));
    // Other signal tools are unaffected
    assert!(names.contains(&"switch_mode".to_string()));
    assert!(names.contains(&"invoke_skill".to_string()));
}

#[tokio::test]
async fn both_tools_can_be_disabled_together() {
    let manager = make_manager();
    let availability = ToolAvailability {
        question: false,
        auto_compact: false,
    };
    let defs = manager.tools_for_mode(tinyharness_lib::mode::AgentMode::Agent, availability);
    let names: Vec<String> = defs.iter().map(|d| d.name.clone()).collect();
    assert!(!names.contains(&"question".to_string()));
    assert!(!names.contains(&"auto_compact".to_string()));
    assert!(names.contains(&"ls".to_string()));
}

#[test]
fn tool_availability_allows_known_and_unknown_tools() {
    let off = ToolAvailability {
        question: false,
        auto_compact: false,
    };
    assert!(!off.allows("question"));
    assert!(!off.allows("auto_compact"));
    // Unmanaged tools are always available
    assert!(off.allows("ls"));
    assert!(off.allows("some_custom_tool"));

    let all = ToolAvailability::all();
    assert!(all.allows("question"));
    assert!(all.allows("auto_compact"));
}
