pub mod auto_compact;
pub mod edit;
pub mod glob;
pub mod grep;
pub mod invoke_skill;
pub mod ls;
pub mod question;
pub mod read;
pub mod run;
pub mod switch_mode;
pub mod tool;
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
    /// **Must be called before** [`ToolManager::register_defaults`]: the
    /// guard is checked in [`ToolManager::execute_tool_call`] *and* cloned
    /// into the `ls`/`read`/`write`/`edit` handlers, which verify the opened
    /// file descriptor against it (closing the check-then-open race).
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

    /// Validate a tool call's path arguments against the guard and return
    /// arguments with every path **replaced by its resolved absolute form**.
    ///
    /// Substitution is the point: a handler that received the raw string would
    /// resolve a relative path against the *process* cwd, while the guard
    /// resolved it against the *workspace root*. Those coincide only when the
    /// harness happens to be started in the workspace, so checking one and
    /// opening the other leaves the containment purely incidental.
    ///
    /// Returns `Err(violation_message)` when the guard rejects a path, or the
    /// arguments unchanged when no guard is active.
    pub fn guard_arguments(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        match self.path_guard.as_ref() {
            Some(guard) => resolve_tool_args_guarded(guard, tool_name, arguments),
            None => Ok(arguments.clone()),
        }
    }

    /// Confine a `glob` result to the guard's roots, line by line.
    ///
    /// Validating the pattern's static prefix is not sufficient: the `glob`
    /// crate descends into symlinked directories, so `**/*.env` lists
    /// `<root>/link/canary.env` — and thus whatever is really under
    /// `/outside` — even though every argument was in bounds. Filtering the
    /// *results* through the guard is what confines the tool.
    ///
    /// Non-path lines (the "no files found" and truncation notices) resolve
    /// under the workspace root and so survive; only paths a symlink carried
    /// the walker out to are dropped.
    fn filter_glob_output(&self, result: String) -> String {
        let Some(guard) = self.path_guard.as_ref() else {
            return result;
        };
        let kept: Vec<&str> = result
            .lines()
            .filter(|line| guard.resolve_contained(line).is_ok())
            .collect();
        if kept.len() == result.lines().count() {
            return result;
        }
        if kept.is_empty() {
            return "No files found matching pattern (all matches resolved outside the workspace)"
                .to_string();
        }
        kept.join("\n")
    }

    /// Register all built-in tools.
    pub fn register_defaults(&mut self) {
        let sandbox = self.sandbox.clone();
        // The file tools get the guard cloned in so they can verify the
        // *opened* descriptor, not just the argument (see
        // `PathGuard::open_read_contained`). Set the guard before this call.
        let guard = self.path_guard.clone();
        self.register_tool(crate::tools::auto_compact::auto_compact_tool_entry());
        self.register_tool(crate::tools::ls::ls_tool_entry(guard.clone()));
        self.register_tool(crate::tools::read::read_tool_entry(guard.clone()));
        self.register_tool(crate::tools::write::write_tool_entry(guard.clone()));
        self.register_tool(crate::tools::edit::edit_tool_entry(guard));
        self.register_tool(crate::tools::grep::grep_tool_entry());
        self.register_tool(crate::tools::run::run_tool_entry(sandbox));
        self.register_tool(crate::tools::glob::glob_tool_entry());
        self.register_tool(crate::tools::switch_mode::switch_mode_tool_entry());
        self.register_tool(crate::tools::question::question_tool_entry());
        self.register_tool(crate::tools::invoke_skill::invoke_skill_tool_entry());
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
            // Casual mode is pure chat — no tools are advertised.
            AgentMode::Casual => Vec::new(),
            // Planning mode: read-only + signal tools only.
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
        // outside the allowed roots, and hand the handler the *resolved*
        // paths. This happens before the tool handler runs, so it applies
        // regardless of auto-accept mode.
        let arguments = match self.guard_arguments(tool_name, arguments) {
            Ok(args) => args,
            Err(error) => return error,
        };

        let Some(tool) = self.tools.iter().find(|t| t.name == tool_name) else {
            return format!("Error: Tool '{}' not found", tool_name);
        };

        let result = tool::execute_tool_call(tool, &arguments).await;
        if tool_name == "glob" {
            self.filter_glob_output(result)
        } else {
            result
        }
    }
}

/// Resolve a tool call's path arguments against the guard.
///
/// Path-bearing tools (`ls`, `read`, `write`, `edit`, `grep`, `glob`) have
/// their path/pattern arguments resolved, and the resolved absolute form is
/// **substituted into the arguments** the handler receives. The `run` tool's
/// explicit `cwd` is resolved too — not because a sandboxed shell could
/// escape (it couldn't; the directory simply would not exist in the mount
/// tree), but so the model gets a clear containment error instead of a
/// confusing `No such file or directory`.
///
/// Returns `Err(violation)` if any path is outside the allowed roots, or the
/// arguments with paths rewritten to absolute form.
fn resolve_tool_args_guarded(
    guard: &crate::path_guard::PathGuard,
    tool_name: &str,
    arguments: &serde_json::Value,
) -> Result<serde_json::Value, String> {
    let arg_str = |name: &str| -> Option<String> {
        arguments
            .get(name)
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    let mut out = arguments.clone();
    let Some(map) = out.as_object_mut() else {
        return Ok(out);
    };

    let mut resolve_into = |key: &str, value: String| -> Result<(), String> {
        let resolved = guard.resolve_contained(&value)?;
        map.insert(
            key.to_string(),
            serde_json::Value::String(resolved.to_string_lossy().into_owned()),
        );
        Ok(())
    };

    match tool_name {
        // Tools with a required "path" argument.
        "ls" | "read" | "write" | "edit" => {
            let path = arg_str("path").unwrap_or_default();
            resolve_into("path", path)?;
        }
        // grep: optional "path" directory argument (defaults to the workspace root).
        "grep" => {
            let path = arg_str("path").unwrap_or_else(|| ".".to_string());
            resolve_into("path", path)?;
        }
        // glob: only the pattern's static prefix can be checked, so it is the
        // prefix that gets resolved and spliced back in — the wildcards are
        // not paths. Results are filtered separately (see
        // [`ToolManager::filter_glob_output`]), because the glob walker
        // follows symlinks.
        "glob" => {
            let pattern = arg_str("pattern").unwrap_or_default();
            let base = crate::path_guard::glob_static_prefix(&pattern);
            let resolved = guard.resolve_contained(base)?;
            let resolved_str = resolved.to_string_lossy().into_owned();
            // Re-attach the wildcard tail. A pattern whose prefix is "." was
            // anchored at the working directory, so the whole pattern hangs
            // off the resolved root.
            let tail = match pattern.strip_prefix(base) {
                Some(rest) if base != "." => rest.to_string(),
                _ => pattern.clone(),
            };
            let rewritten = if tail.starts_with('/') {
                format!("{resolved_str}{tail}")
            } else {
                format!("{resolved_str}/{tail}")
            };
            map.insert("pattern".to_string(), serde_json::Value::String(rewritten));
        }
        // run: only the explicit "cwd" can be checked statically.
        "run" => {
            if let Some(cwd) = arg_str("cwd").filter(|s| !s.trim().is_empty()) {
                resolve_into("cwd", cwd)?;
            }
        }
        // All other tools (signal tools, custom tools without a known path
        // arg) are not subject to path containment.
        _ => {}
    }

    Ok(out)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::path::Path;

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
        // switch_mode tool has no path arg — should not be blocked
        let result = manager
            .execute_tool_call("switch_mode", &serde_json::json!({"mode": "agent"}))
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

    /// A symlink plus `..` must not reach outside the root: the guard used to
    /// normalize `..` lexically (collapsing `<root>/link/../etc/passwd` to
    /// `<root>/etc/passwd`, in bounds) while the kernel resolved the symlink
    /// first and opened `/etc/passwd`.
    #[tokio::test]
    async fn guard_blocks_symlink_dotdot_read() {
        let tmp = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(Path::new("/usr"), tmp.path().join("link")).unwrap();
        let manager = guarded_manager(tmp.path());

        let raw = format!("{}/link/../etc/passwd", tmp.path().display());
        let result = manager
            .execute_tool_call("read", &serde_json::json!({"path": raw}))
            .await;
        assert!(result.starts_with("Error: Path '"), "result was: {result}");
        assert!(!result.contains("nologin"), "leaked /etc/passwd: {result}");
    }

    /// Same shape, but for a write: previously the payload landed outside the
    /// workspace while the guard believed it had authorized `<root>/x`.
    #[tokio::test]
    async fn guard_blocks_symlink_dotdot_write() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let sub = outside.path().join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        std::os::unix::fs::symlink(&sub, tmp.path().join("link")).unwrap();

        let manager = guarded_manager(tmp.path());
        let raw = format!("{}/link/../dropped.txt", tmp.path().display());
        let _ = manager
            .execute_tool_call("write", &serde_json::json!({"path": raw, "content": "PWN"}))
            .await;

        assert!(
            !outside.path().join("dropped.txt").exists(),
            "wrote outside the workspace via symlink+.."
        );
    }

    /// The guard resolves a relative path against the workspace root, so the
    /// handler must receive that resolved path — not the raw string, which
    /// `std::fs` would resolve against the *process* cwd. Those coincide only
    /// when the harness is started inside the workspace, so a relative
    /// `write("victim.txt")` could otherwise clobber a file next to the cwd
    /// while the guard believed it had authorized `<root>/victim.txt`.
    #[tokio::test]
    async fn guard_substitutes_resolved_path_for_relative_args() {
        let tmp = tempfile::tempdir().unwrap();
        let manager = guarded_manager(tmp.path());

        // The resolved argument is absolute and rooted at the workspace, which
        // is what makes the handler's `fs::write` land in a predictable place.
        let args = manager
            .guard_arguments(
                "write",
                &serde_json::json!({"path": "victim.txt", "content": "x"}),
            )
            .unwrap();
        let path = args.get("path").and_then(|v| v.as_str()).unwrap();
        assert!(
            Path::new(path).is_absolute(),
            "handler still got a relative path: {path}"
        );
        assert_eq!(
            Path::new(path),
            tmp.path().canonicalize().unwrap().join("victim.txt")
        );

        // End-to-end: the file lands in the workspace, wherever the test binary
        // happens to have been started from.
        let result = manager
            .execute_tool_call(
                "write",
                &serde_json::json!({"path": "victim.txt", "content": "OVERWRITTEN"}),
            )
            .await;
        assert!(result.starts_with("Wrote"), "result was: {result}");
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("victim.txt")).unwrap(),
            "OVERWRITTEN"
        );
    }

    /// `grep` walks a directory tree, so checking only its root argument is
    /// not containment: the walker used to descend into symlinked directories
    /// and copy outside content straight into the model's context.
    #[tokio::test]
    async fn grep_does_not_follow_symlinked_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("leak.txt"), "CANARY-CONTENT\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        std::fs::write(tmp.path().join("local.txt"), "CANARY-CONTENT\n").unwrap();

        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call(
                "grep",
                &serde_json::json!({"pattern": "CANARY-CONTENT", "path": tmp.path()}),
            )
            .await;
        assert!(
            result.contains("local.txt"),
            "missed the in-tree file: {result}"
        );
        assert!(
            !result.contains("link/leak.txt"),
            "grep followed a symlink outside the workspace: {result}"
        );
    }

    /// `glob` only has a *pattern* to check, and the pattern's static prefix
    /// passing the guard says nothing about where the walker ends up. Results
    /// reached through a symlink must be filtered out.
    #[tokio::test]
    async fn glob_results_are_confined_to_the_workspace() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("canary.env"), "A=1\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        std::fs::write(tmp.path().join("local.env"), "B=2\n").unwrap();

        let manager = guarded_manager(tmp.path());
        let result = manager
            .execute_tool_call("glob", &serde_json::json!({"pattern": "**/*.env"}))
            .await;
        assert!(
            result.contains("local.env"),
            "missed the in-tree file: {result}"
        );
        assert!(
            !result.contains("canary.env"),
            "glob listed a file outside the workspace through a symlink: {result}"
        );
    }

    /// `glob` must keep working for ordinary in-tree patterns, including after
    /// the pattern is rewritten to an absolute one by the guard.
    #[tokio::test]
    async fn glob_still_finds_in_tree_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src/a.rs"), "// a").unwrap();
        std::fs::write(tmp.path().join("src/b.rs"), "// b").unwrap();

        let manager = guarded_manager(tmp.path());
        for pattern in ["**/*.rs", "src/**/*.rs", "./src/*.rs"] {
            let result = manager
                .execute_tool_call("glob", &serde_json::json!({"pattern": pattern}))
                .await;
            assert!(
                result.contains("a.rs") && result.contains("b.rs"),
                "pattern {pattern:?} returned: {result}"
            );
        }
    }

    /// `run`'s `cwd` is resolved the same way, so a sandboxed command starts in
    /// the directory the guard authorized rather than a raw relative string.
    #[tokio::test]
    async fn run_cwd_is_resolved_against_the_workspace_root() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
        let manager = guarded_manager(tmp.path());

        let args = manager
            .guard_arguments("run", &serde_json::json!({"command": "pwd", "cwd": "."}))
            .unwrap();
        assert_eq!(
            args.get("cwd").and_then(|v| v.as_str()).unwrap(),
            tmp.path().canonicalize().unwrap().to_string_lossy()
        );

        let sub = manager
            .guard_arguments("run", &serde_json::json!({"command": "pwd", "cwd": "sub"}))
            .unwrap();
        assert_eq!(
            sub.get("cwd").and_then(|v| v.as_str()).unwrap(),
            tmp.path()
                .canonicalize()
                .unwrap()
                .join("sub")
                .to_string_lossy()
        );

        // An escaping cwd is still rejected before the handler sees it.
        let err = manager
            .guard_arguments(
                "run",
                &serde_json::json!({"command": "pwd", "cwd": "../escape"}),
            )
            .unwrap_err();
        assert!(err.starts_with("Error: Path '"), "{err}");
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
