//! Shared test helpers: mock provider, captured output, context factory.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use tinyharness_lib::provider::AnyProvider;
use tinyharness_lib::provider::mock::{MockProvider, MockProviderHandle};

use tokio::sync::Mutex as TokioMutex;

use crate::commands::registry::CommandContext;

// ── MockProvider re-export ──────────────────────────────────────────────────

/// The mock provider now lives in `tinyharness-lib` (feature `test-util`).
/// Re-exported here so tests can `use crate::test_helpers::MockProvider`.
pub use tinyharness_lib::provider::mock::MockProvider as LibMockProvider;

// ── Captured output ─────────────────────────────────────────────────────────

/// Create an `Output` that captures everything written to it.
/// Returns the output and a shared buffer for inspection.
pub fn captured_output() -> (tinyharness_ui::output::Output, Arc<Mutex<Vec<u8>>>) {
    use std::io::Write;
    use std::sync::Mutex;

    struct CaptureWriter {
        buf: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for CaptureWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.buf.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buf = Arc::new(Mutex::new(Vec::new()));
    let writer = CaptureWriter { buf: buf.clone() };
    let output = tinyharness_ui::output::Output::new(Box::new(writer));
    (output, buf)
}

/// Get captured output as a String.
pub fn captured_string(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8(buf.lock().unwrap().clone()).unwrap_or_default()
}

/// Strip ANSI SGR sequences from a string for content assertions.
pub fn strip_ansi(s: &str) -> String {
    let re = regex::Regex::new(r"\x1b\[[0-9;]*m").unwrap();
    re.replace_all(s, "").to_string()
}

// ── CommandContext factory ──────────────────────────────────────────────────

/// Build a `CommandContext` with a mock provider for testing.
///
/// Returns the context (whose `provider` field is an
/// `Arc<TokioMutex<AnyProvider>>` wrapping the mock) and a shared handle to
/// the raw `MockProvider` for enqueuing responses:
///
/// ```ignore
/// let (mut ctx, mock) = make_context();
/// mock.lock().unwrap().enqueue_text("hello");
/// ```
pub fn make_context() -> (CommandContext, Arc<Mutex<MockProvider>>) {
    let mock = Arc::new(Mutex::new(MockProvider::new()));

    let provider: Arc<TokioMutex<AnyProvider>> = Arc::new(TokioMutex::new(AnyProvider::Mock(
        MockProviderHandle::new(mock.clone()),
    )));

    let workspace_ctx = WorkspaceContext {
        root: std::env::current_dir().unwrap_or_default(),
        project_type: "Test".to_string(),
        project_name: "test-project".to_string(),
        is_git_repo: false,
        build_command: "cargo build".to_string(),
        test_command: "cargo test".to_string(),
        structure: vec![],
        project_md: None,
        additional_project_mds: vec![],
    };

    let paths = isolated_paths("ctx");
    let ctx = CommandContext::new(provider, workspace_ctx, paths);
    (ctx, mock)
}

/// A [`Paths`] rooted in a disposable temp directory, so tests never read or
/// write the real user profile (settings, sessions, prompts, skills).
///
/// The root is unique per call (`tag` + PID + a process-wide counter) to keep
/// parallel tests from interfering, and is intentionally *not* auto-deleted —
/// the returned `Paths` outlives the call, and a missing directory simply makes
/// the loaders fall back to their embedded defaults.
pub fn isolated_paths(tag: &str) -> tinyharness_lib::Paths {
    static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "tinyharness-test-{tag}-{}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let paths = tinyharness_lib::Paths::with_home(&root);
    std::fs::create_dir_all(paths.prompts_dir()).ok();
    paths
}

use tinyharness_lib::context::WorkspaceContext;

/// Create a system + user message pair for testing.
pub fn make_messages(user_text: &str) -> Vec<tinyharness_lib::provider::Message> {
    use tinyharness_lib::provider::{Message, Role};
    vec![
        Message::simple(Role::System, "You are a test assistant."),
        Message::simple(Role::User, user_text),
    ]
}
