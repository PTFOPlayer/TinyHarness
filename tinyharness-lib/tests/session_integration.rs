//! Integration tests for session persistence.
//!
//! Tests the full lifecycle: create, append messages, save, load, verify.

use tempfile::TempDir;
use tinyharness_lib::mode::AgentMode;
use tinyharness_lib::provider::{Message, Role};
use tinyharness_lib::session::SessionStore;

#[test]
fn session_create_and_load_round_trip() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.ensure_dir().unwrap();

    // Create a session
    let mut session = store.create(
        "/tmp/project",
        AgentMode::Agent,
        "ollama",
        Some("test-model".to_string()),
    );
    let session_id = session.id().to_string();

    // Append some messages
    let user_msg = Message::simple(Role::User, "Hello, world!");
    let assistant_msg = Message::simple(Role::Assistant, "Hi there!");
    session.append_message(&user_msg);
    session.append_message(&assistant_msg);
    session.flush();

    // Load it back
    let (loaded_session, messages) = store.load(&session_id).unwrap();

    assert_eq!(loaded_session.id(), session_id);
    assert_eq!(loaded_session.meta().working_dir, "/tmp/project");
    assert_eq!(loaded_session.meta().mode, AgentMode::Agent);
    assert_eq!(loaded_session.meta().provider, "ollama");
    assert_eq!(loaded_session.meta().model, Some("test-model".to_string()));
    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(messages[0].content, "Hello, world!");
    assert_eq!(messages[1].role, Role::Assistant);
    assert_eq!(messages[1].content, "Hi there!");
}

#[test]
fn session_load_nonexistent_returns_error() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let result = store.load("nonexistent-id");
    assert!(result.is_err());
}

#[test]
fn session_list_all_returns_sorted_by_updated() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.ensure_dir().unwrap();

    let _s1 = store.create("/tmp/a", AgentMode::Agent, "ollama", None);
    let mut s2 = store.create("/tmp/b", AgentMode::Casual, "ollama", None);

    // Modify s2 so it has a later updated_at
    std::thread::sleep(std::time::Duration::from_secs(1));
    s2.flush();

    let sessions = store.list_all();
    assert_eq!(sessions.len(), 2);
    // Most recently updated should be first
    assert!(sessions[0].updated_at >= sessions[1].updated_at);
}

#[test]
fn session_find_by_prefix() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.ensure_dir().unwrap();

    let session = store.create("/tmp/project", AgentMode::Agent, "ollama", None);
    let full_id = session.id().to_string();
    let prefix = &full_id[..8];

    let found = store.find_by_prefix(prefix).unwrap();
    assert_eq!(found, full_id);
}

#[test]
fn session_find_by_prefix_no_match() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.ensure_dir().unwrap();

    let result = store.find_by_prefix("nonexistent");
    assert!(result.is_err());
}

#[test]
fn session_find_latest_for_dir() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.ensure_dir().unwrap();

    let mut s1 = store.create("/tmp/myproject", AgentMode::Agent, "ollama", None);
    let s1_id = s1.id().to_string();
    s1.flush();

    std::thread::sleep(std::time::Duration::from_secs(1));

    let mut s2 = store.create("/tmp/myproject", AgentMode::Casual, "ollama", None);
    let s2_id = s2.id().to_string();
    s2.flush();

    let latest = store.find_latest_for_dir("/tmp/myproject");
    assert_eq!(latest, Some(s2_id));
    assert_ne!(latest, Some(s1_id));
}

#[test]
fn session_malformed_lines_skipped() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.ensure_dir().unwrap();

    let session = store.create("/tmp/project", AgentMode::Agent, "ollama", None);
    let session_id = session.id().to_string();

    // Append a valid message
    let msg = Message::simple(Role::User, "valid message");
    let mut session = session;
    session.append_message(&msg);
    session.flush();

    // Append a malformed line to the JSONL file
    let path = tmp.path().join(format!("{}.jsonl", session_id));
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    writeln!(file, "this is not valid json").unwrap();
    drop(file);

    // Load should succeed, skipping the malformed line
    let (_loaded, messages) = store.load(&session_id).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "valid message");
}

#[test]
fn session_delete_removes_file() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    store.ensure_dir().unwrap();

    let session = store.create("/tmp/project", AgentMode::Agent, "ollama", None);
    let session_id = session.id().to_string();
    let mut session = session;
    session.flush();

    let path = tmp.path().join(format!("{}.jsonl", session_id));
    assert!(path.exists());

    store.delete(&session_id).unwrap();
    assert!(!path.exists());
}

#[test]
fn session_auto_name_from_working_dir() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let session = store.create("/tmp/my-cool-project", AgentMode::Agent, "ollama", None);
    assert_eq!(session.meta().name, Some("my-cool-project".to_string()));
}

#[test]
fn session_message_count_tracks_appends() {
    let tmp = TempDir::new().unwrap();
    let store = SessionStore::new(tmp.path().to_path_buf());
    let mut session = store.create("/tmp/project", AgentMode::Agent, "ollama", None);

    assert_eq!(session.meta().message_count, 0);

    session.append_message(&Message::simple(Role::User, "msg 1"));
    session.append_message(&Message::simple(Role::Assistant, "msg 2"));
    session.flush();

    let (loaded, _) = store.load(session.id()).unwrap();
    assert_eq!(loaded.meta().message_count, 2);
}
