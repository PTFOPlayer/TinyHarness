pub mod auto_compact;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod invoke_skill;
pub mod ls;
pub mod question;
pub mod read;
pub mod run;
pub mod screenshot;
pub mod switch_mode;
pub mod tool;
pub mod web_search;
pub mod write;

use crate::mode::AgentMode;
use crate::provider::ToolDefinition;
use crate::sandbox::Sandbox;
use crate::tools::tool::{Tool, ToolCategory};

/// Events emitted by signal-category tools that the caller must interpret.
/// These tools return a result string, but the caller should parse these
/// into structured events for proper handling (e.g., prompting the user,
/// switching mode, triggering compaction).
#[derive(Debug, Clone)]
pub enum SignalEvent {
    /// The model requests a mode switch.
    SwitchMode { mode: AgentMode },
    /// The model asks the user a question with options.
    Question {
        question: String,
        answers: Vec<String>,
    },
    /// The model requests conversation compaction.
    AutoCompact { focus: String },
    /// The model requests invocation of a skill by name.
    InvokeSkill { skill_name: String },
}

/// Controls which optional tools are advertised to the model.
///
/// Tools disabled here are filtered out of [`ToolManager::tools_for_mode`],
/// so the model never sees them and cannot call them. All flags default to
/// `true` — opt out explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolAvailability {
    /// Whether the model may request conversation compaction (`auto_compact`).
    pub auto_compact: bool,
    /// Whether the model may ask the user multiple-choice questions (`question`).
    pub question: bool,
}

impl Default for ToolAvailability {
    fn default() -> Self {
        Self::all()
    }
}

impl ToolAvailability {
    /// Every optional tool available (the default).
    pub fn all() -> Self {
        ToolAvailability {
            auto_compact: true,
            question: true,
        }
    }

    /// Whether the named tool is available under this configuration.
    /// Tools not listed here are always available.
    pub fn allows(&self, tool_name: &str) -> bool {
        match tool_name {
            "auto_compact" => self.auto_compact,
            "question" => self.question,
            _ => true,
        }
    }
}

#[derive(Default)]
pub struct ToolManager {
    tools: Vec<Tool>,
    /// Confines the built-in file tools to the workspace (see
    /// [`PathGuard`]). Always set by the binary; `None` means no containment,
    /// which is only sensible in tests.
    path_guard: Option<crate::path_guard::PathGuard>,
    /// OS-level sandbox for shell-executing tools. Cloned into each shell
    /// tool's handler at registration time.
    sandbox: Option<Sandbox>,
}

impl ToolManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Confine the built-in file tools (`ls`, `read`, `write`, `edit`,
    /// `grep`, `glob`) to the paths the guard allows.
    ///
    /// Unlike the process sandbox this can be set at any time — it is checked
    /// in [`ToolManager::execute_tool_call`], not baked into handlers.
    pub fn set_path_guard(&mut self, guard: crate::path_guard::PathGuard) {
        self.path_guard = Some(guard);
    }

    /// The active path guard, if any.
    pub fn path_guard(&self) -> Option<&crate::path_guard::PathGuard> {
        self.path_guard.as_ref()
    }

    /// Enable the OS-level sandbox for `run` and custom tools.
    ///
    /// **Must be called before** [`register_defaults`] and
    /// [`register_custom_tools`]: tool handlers are opaque closures, so the
    /// sandbox is cloned into them at construction. Setting it afterwards
    /// leaves the already-registered tools unsandboxed.
    ///
    /// [`register_defaults`]: ToolManager::register_defaults
    /// [`register_custom_tools`]: ToolManager::register_custom_tools
    pub fn set_sandbox(&mut self, sandbox: Sandbox) {
        self.sandbox = Some(sandbox);
    }

    /// The active process sandbox, if any.
    pub fn sandbox(&self) -> Option<&Sandbox> {
        self.sandbox.as_ref()
    }

    /// If a path guard is active and this tool call would access a path
    /// outside it, return the violation error message. Returns `None` if the
    /// call is allowed (or no guard is active).
    pub fn path_check(&self, tool_name: &str, arguments: &serde_json::Value) -> Option<String> {
        let guard = self.path_guard.as_ref()?;
        check_tool_args_guarded(guard, tool_name, arguments)
    }

    /// Register all built-in tools.
    pub fn register_defaults(&mut self) {
        let sandbox = self.sandbox.clone();
        self.register_tool(crate::tools::auto_compact::auto_compact_tool_entry());
        self.register_tool(crate::tools::ls::ls_tool_entry());
        self.register_tool(crate::tools::read::read_tool_entry());
        self.register_tool(crate::tools::write::write_tool_entry());
        self.register_tool(crate::tools::edit::edit_tool_entry());
        self.register_tool(crate::tools::grep::grep_tool_entry());
        self.register_tool(crate::tools::run::run_tool_entry(sandbox));
        self.register_tool(crate::tools::glob::glob_tool_entry());
        self.register_tool(crate::tools::glob::glob_tool_entry());
        self.register_tool(crate::tools::web_search::web_search_tool_entry());
        self.register_tool(crate::tools::web_search::web_fetch_tool_entry());
        self.register_tool(crate::tools::switch_mode::switch_mode_tool_entry());
        self.register_tool(crate::tools::question::question_tool_entry());
        self.register_tool(crate::tools::invoke_skill::invoke_skill_tool_entry());
        self.register_tool(crate::tools::screenshot::screenshot_tool_entry());
    }

    pub fn register_tool(&mut self, tool: Tool) {
        self.tools.push(tool);
    }

    /// Register custom tools from [`crate::custom_tools::CustomToolManager`].
    /// Tools whose names collide with built-in tools are silently skipped.
    pub fn register_custom_tools(&mut self, tools: &[crate::custom_tools::CustomToolDefinition]) {
        let existing: std::collections::HashSet<String> =
            self.tools.iter().map(|t| t.name.clone()).collect();
        let sandbox = self.sandbox.clone();
        for def in tools {
            if existing.contains(&def.name) {
                tracing::warn!(
                    "Custom tool '{}' collides with an existing tool — skipping",
                    def.name
                );
                continue;
            }
            match def.build_tool_with_sandbox(sandbox.clone()) {
                Ok(tool) => self.register_tool(tool),
                Err(e) => tracing::warn!("Failed to register custom tool '{}': {}", def.name, e),
            }
        }
    }

    /// Returns the tool definitions for all registered tools.
    pub fn get_all_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.to_definition()).collect()
    }

    /// Returns the tool definitions appropriate for the given agent mode.
    ///
    /// Tools disabled in `availability` (e.g. `auto_compact`, `question`) are
    /// excluded, so the model never sees them.
    pub fn tools_for_mode(
        &self,
        mode: AgentMode,
        availability: ToolAvailability,
    ) -> Vec<ToolDefinition> {
        let is_available = |t: &&Tool| availability.allows(&t.name);
        match mode {
            AgentMode::Agent => self
                .tools
                .iter()
                .filter(is_available)
                .map(|t| t.to_definition())
                .collect(),
            AgentMode::Casual => self
                .tools
                .iter()
                .filter(|t| is_available(t) && (t.name == "web_search" || t.name == "web_fetch"))
                .map(|t| t.to_definition())
                .collect(),
            AgentMode::Planning => self
                .tools
                .iter()
                .filter(|t| {
                    is_available(t)
                        && (t.category == ToolCategory::ReadOnly
                            || t.category == ToolCategory::Signal)
                })
                .map(|t| t.to_definition())
                .collect(),
            AgentMode::Research => self
                .tools
                .iter()
                .filter(|t| {
                    is_available(t)
                        && (t.category == ToolCategory::ReadOnly
                            || t.category == ToolCategory::Signal)
                })
                .map(|t| t.to_definition())
                .collect(),
        }
    }

    /// Returns the category of a tool by name, or `None` if not found.
    pub fn category_of(&self, tool_name: &str) -> Option<ToolCategory> {
        self.tools
            .iter()
            .find(|t| t.name == tool_name)
            .map(|t| t.category)
    }

    /// Returns `true` if the tool requires user approval before execution.
    /// Destructive tools (write, edit, run) and signal tools (switch_mode,
    /// question, auto_compact) require approval.
    pub fn needs_approval(&self, tool_name: &str) -> bool {
        self.category_of(tool_name)
            .map(|c| c == ToolCategory::Destructive || c == ToolCategory::Signal)
            .unwrap_or(false)
    }

    /// Returns `true` if the tool is a signal tool (switch_mode, question, auto_compact).
    /// Signal tools are handled specially by the agent loop rather than executed generically.
    pub fn is_signal_tool(&self, tool_name: &str) -> bool {
        self.category_of(tool_name) == Some(ToolCategory::Signal)
    }

    /// Parse a signal tool's result string into a structured `SignalEvent`.
    ///
    /// Signal tools return plain strings, but the agent loop needs structured
    /// data to dispatch them correctly. This method interprets the tool call
    /// arguments (not the result string) to produce the appropriate event.
    ///
    /// Returns `None` if the tool is not a signal tool or the arguments are invalid.
    pub fn parse_signal_event(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Option<SignalEvent> {
        match tool_name {
            "switch_mode" => {
                let mode_str = arguments.get("mode").and_then(|v| v.as_str()).unwrap_or("");
                mode_str
                    .parse::<AgentMode>()
                    .ok()
                    .map(|mode| SignalEvent::SwitchMode { mode })
            }
            "question" => {
                let question = arguments
                    .get("question")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let answers: Vec<String> = arguments
                    .get("answers")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|item| item.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(SignalEvent::Question { question, answers })
            }
            "auto_compact" => {
                let focus = arguments
                    .get("focus")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Some(SignalEvent::AutoCompact { focus })
            }
            "invoke_skill" => {
                let skill_name = arguments
                    .get("skill_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if skill_name.is_empty() {
                    None
                } else {
                    Some(SignalEvent::InvokeSkill { skill_name })
                }
            }
            _ => None,
        }
    }

    pub async fn execute_tool_call(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> String {
        // Path containment: reject any tool call that would touch a path
        // outside the allowed roots. This happens before the tool handler
        // runs, so it applies regardless of auto-accept mode.
        if let Some(error) = self.path_check(tool_name, arguments) {
            return error;
        }

        if let Some(tool) = self.tools.iter().find(|t| t.name == tool_name) {
            tool::execute_tool_call(tool, arguments).await
        } else {
            format!("Error: Tool '{}' not found", tool_name)
        }
    }
}

/// Enforce path containment for a tool call's arguments.
///
/// Path-bearing tools (`ls`, `read`, `write`, `edit`, `grep`, `glob`) have
/// their path/pattern arguments checked against the guard. The `run` tool's
/// explicit `cwd` is also checked — not because a sandboxed shell could
/// escape (it couldn't; the directory simply would not exist in the mount
/// tree), but so the model gets a clear containment error instead of a
/// confusing `No such file or directory`.
///
/// Returns `Some(error)` to block the call, or `None` to allow it.
fn check_tool_args_guarded(
    guard: &crate::path_guard::PathGuard,
    tool_name: &str,
    arguments: &serde_json::Value,
) -> Option<String> {
    let arg_str = |name: &str| -> Option<String> {
        arguments
            .get(name)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };

    match tool_name {
        // Tools with a required "path" argument
        "ls" | "read" | "write" | "edit" => {
            let path = arg_str("path").unwrap_or_default();
            guard.resolve_contained(&path).err()
        }
        // grep: optional "path" directory argument (defaults to ".")
        "grep" => {
            let path = arg_str("path").unwrap_or_else(|| ".".to_string());
            guard.resolve_contained(&path).err()
        }
        // glob: the static prefix of the pattern must be inside an allowed root
        "glob" => {
            let pattern = arg_str("pattern").unwrap_or_default();
            guard.check_glob_pattern(&pattern).err()
        }
        // run: only the explicit "cwd" can be checked statically
        "run" => match arg_str("cwd") {
            Some(cwd) if !cwd.trim().is_empty() => guard.resolve_contained(&cwd).err(),
            _ => None,
        },
        // All other tools (web_search, web_fetch, signal tools, custom tools
        // without a known path arg) are not subject to path containment.
        _ => None,
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::path_guard::PathGuard;

    /// Manager with a path guard rooted at `root` and no process sandbox —
    /// the shape the binary uses on platforms without bwrap/firejail.
    fn guarded_manager(root: &std::path::Path) -> ToolManager {
        let mut manager = ToolManager::new();
        manager.set_path_guard(PathGuard::new(root).unwrap());
        manager.register_defaults();
        manager
    }

    #[tokio::test]
    async fn guard_blocks_read_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call("read", &serde_json::json!({"path": "/etc/passwd"}))
            .await;
        assert!(result.starts_with("Error: Path '"), "result was: {result}");
        assert!(result.contains("/etc/passwd"), "result was: {result}");
    }

    #[tokio::test]
    async fn guard_blocks_write_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());
        let target = tmp.path().join("../escape.txt");
        let result = manager
            .execute_tool_call(
                "write",
                &serde_json::json!({"path": target.to_str().unwrap(), "content": "x"}),
            )
            .await;
        assert!(result.starts_with("Error: Path '"), "result was: {result}");
        // Verify the file was NOT created
        assert!(!target.exists());
    }

    #[tokio::test]
    async fn guard_allows_write_inside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());
        let target = tmp.path().join("ok.txt");
        let result = manager
            .execute_tool_call(
                "write",
                &serde_json::json!({"path": target.to_str().unwrap(), "content": "hello"}),
            )
            .await;
        assert!(result.starts_with("Wrote"), "result was: {result}");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "hello");
    }

    #[tokio::test]
    async fn guard_blocks_ls_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call("ls", &serde_json::json!({"path": "/"}))
            .await;
        assert!(result.starts_with("Error: Path '"), "result was: {result}");
    }

    #[tokio::test]
    async fn guard_blocks_grep_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call("grep", &serde_json::json!({"pattern": "x", "path": "/tmp"}))
            .await;
        assert!(result.starts_with("Error: Path '"), "result was: {result}");
    }

    #[tokio::test]
    async fn guard_blocks_glob_outside_root() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call("glob", &serde_json::json!({"pattern": "/etc/**/*"}))
            .await;
        assert!(result.starts_with("Error: Path '"), "result was: {result}");
    }

    #[tokio::test]
    async fn guard_allows_relative_glob() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.rs"), "fn main() {}").unwrap();
        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call("glob", &serde_json::json!({"pattern": "**/*.rs"}))
            .await;
        assert!(result.contains("a.rs"), "result was: {result}");
    }

    #[tokio::test]
    async fn guard_blocks_run_with_outside_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call("run", &serde_json::json!({"command": "pwd", "cwd": "/"}))
            .await;
        assert!(result.starts_with("Error: Path '"), "result was: {result}");
    }

    #[tokio::test]
    async fn guard_allows_run_without_cwd() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call("run", &serde_json::json!({"command": "echo hi"}))
            .await;
        assert!(result.contains("hi"), "result was: {result}");
    }

    #[tokio::test]
    async fn guard_does_not_affect_non_path_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());
        // screenshot tool has no path arg — should not be blocked
        let result = manager
            .execute_tool_call("screenshot", &serde_json::json!({"description": "test"}))
            .await;
        assert!(!result.starts_with("Error: Path '"), "result was: {result}");
    }

    #[tokio::test]
    async fn no_guard_allows_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let mut manager = ToolManager::new();
        manager.register_defaults();
        // Without a guard, reading an arbitrary temp dir works.
        let result = manager
            .execute_tool_call(
                "ls",
                &serde_json::json!({"path": tmp.path().to_str().unwrap()}),
            )
            .await;
        assert!(!result.starts_with("Error: Path '"), "result was: {result}");
    }

    #[tokio::test]
    async fn guard_rejects_symlink_escape_through_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "top secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();

        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call(
                "read",
                &serde_json::json!({"path": format!("{}/link/secret.txt", tmp.path().display())}),
            )
            .await;
        assert!(result.starts_with("Error: Path '"), "result was: {result}");
        assert!(!result.contains("top secret"), "leaked: {result}");
    }

    #[tokio::test]
    async fn guard_honours_allowed_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let extra = tempfile::tempdir().unwrap();
        std::fs::write(extra.path().join("note.txt"), "from allowed").unwrap();

        let mut manager = ToolManager::new();
        manager.set_path_guard(
            PathGuard::configured(
                tmp.path(),
                &[extra.path().to_string_lossy().to_string()],
                None,
                &[],
            )
            .unwrap(),
        );
        manager.register_defaults();

        let result = manager
            .execute_tool_call(
                "read",
                &serde_json::json!({"path": extra.path().join("note.txt").to_str().unwrap()}),
            )
            .await;
        assert!(result.contains("from allowed"), "result was: {result}");
    }
}
