pub mod custom_tool;
pub mod hook;
pub mod shell;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use custom_tool::{CustomToolCategory, CustomToolDefinition};
pub use hook::{HookContext, HookDefinition, HookEvent, HookOutcome};
pub use shell::ShellCommand;

/// Plugin configuration loaded from `plugins.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginConfig {
    #[serde(default)]
    pub hooks: Vec<HookDefinition>,
    #[serde(default)]
    pub custom_tools: Vec<CustomToolDefinition>,
}

/// Plugin manager — loads, merges, and dispatches hooks and custom tools.
#[derive(Debug, Clone, Default)]
pub struct PluginManager {
    config: PluginConfig,
}

/// Error type for plugin loading.
#[derive(Debug)]
pub enum PluginError {
    Io(std::io::Error),
    Parse(serde_json::Error),
}

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PluginError::Io(e) => write!(f, "I/O error: {}", e),
            PluginError::Parse(e) => write!(f, "parse error: {}", e),
        }
    }
}

impl std::error::Error for PluginError {}

impl From<std::io::Error> for PluginError {
    fn from(e: std::io::Error) -> Self {
        PluginError::Io(e)
    }
}

impl From<serde_json::Error> for PluginError {
    fn from(e: serde_json::Error) -> Self {
        PluginError::Parse(e)
    }
}

/// Default path for the global plugins config file.
fn global_plugins_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/tinyharness/plugins.json")
}

/// Discover `.tinyharness/plugins.json` by walking up from the given directory.
/// Returns `None` if no file is found.
fn discover_project_plugins(start_dir: &std::path::Path) -> Option<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join(".tinyharness").join("plugins.json");
        if candidate.is_file() {
            return Some(candidate);
        }
        if let Some(parent) = dir.parent() {
            if parent == dir {
                break;
            }
            dir = parent.to_path_buf();
        } else {
            break;
        }
    }
    None
}

/// Load a single plugin config file. Returns `Ok(None)` if the file doesn't exist.
fn load_config_file(path: &std::path::Path) -> Result<Option<PluginConfig>, PluginError> {
    if !path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    let config: PluginConfig = serde_json::from_str(&content)?;
    Ok(Some(config))
}

impl PluginManager {
    /// Load and merge global + project plugin configs.
    ///
    /// Global config is loaded from `~/.config/tinyharness/plugins.json`.
    /// Project config is loaded from `.tinyharness/plugins.json` (walking up
    /// from CWD). Project hooks and tools are **appended** to the global
    /// list (both run; global first, then project).
    pub fn load() -> Self {
        Self::load_from_cwd(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Load from a specific directory (for testing).
    pub fn load_from_cwd(cwd: &std::path::Path) -> Self {
        let mut combined = PluginConfig::default();

        // Global config
        let global_path = global_plugins_path();
        match load_config_file(&global_path) {
            Ok(Some(cfg)) => combined = cfg,
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    "Failed to load global plugins.json from {}: {e}",
                    global_path.display()
                );
            }
        }

        // Project config (extends global)
        if let Some(project_path) = discover_project_plugins(cwd) {
            match load_config_file(&project_path) {
                Ok(Some(cfg)) => {
                    combined.hooks.extend(cfg.hooks);
                    combined.custom_tools.extend(cfg.custom_tools);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        "Failed to load project plugins.json from {}: {e}",
                        project_path.display()
                    );
                }
            }
        }

        // Validate hook names for uniqueness (warn on duplicates)
        let mut seen = std::collections::HashSet::new();
        for hook in &combined.hooks {
            if !seen.insert(&hook.name) {
                tracing::warn!("Duplicate hook name '{}'", hook.name);
            }
        }

        // Validate custom tool names don't collide with built-in tools
        let builtin = [
            "ls",
            "read",
            "write",
            "edit",
            "grep",
            "glob",
            "run",
            "web_search",
            "web_fetch",
            "switch_mode",
            "question",
            "auto_compact",
            "invoke_skill",
            "screenshot",
        ];
        for tool in &combined.custom_tools {
            if builtin.contains(&tool.name.as_str()) {
                tracing::warn!(
                    "Custom tool '{}' has the same name as a built-in tool and will be ignored",
                    tool.name
                );
            }
        }

        PluginManager { config: combined }
    }

    /// Load from an explicit config (for testing).
    pub fn from_config(config: PluginConfig) -> Self {
        PluginManager { config }
    }

    /// Return the list of custom tool definitions.
    pub fn custom_tools(&self) -> &[CustomToolDefinition] {
        &self.config.custom_tools
    }

    /// Return the list of hook definitions.
    pub fn hooks(&self) -> &[HookDefinition] {
        &self.config.hooks
    }

    /// Run all hooks matching the given event.
    ///
    /// Hooks fire in order (global first, then project). Each hook's command
    /// is executed with the hook context variables substituted.
    ///
    /// Returns a [`HookOutcome`] aggregating results from all hooks:
    /// - `injected_text`: concatenated stdout from hooks with `inject_stdout`
    /// - `blocked`: true if any hook returned the `block_on_output` string
    /// - `warnings`: non-fatal errors from hook execution
    pub async fn run_hooks(&self, event: HookEvent, ctx: &HookContext) -> HookOutcome {
        let mut outcome = HookOutcome::default();

        for hook in &self.config.hooks {
            if hook.event != event {
                continue;
            }

            let vars = ctx.to_vars();
            match hook.command.execute(&vars).await {
                Ok(stdout) => {
                    let stdout_trim = stdout.trim().to_string();
                    if hook.inject_stdout && !stdout_trim.is_empty() {
                        if let Some(ref mut text) = outcome.injected_text {
                            text.push('\n');
                            text.push_str(&stdout_trim);
                        } else {
                            outcome.injected_text = Some(stdout_trim.clone());
                        }
                    }
                    if let Some(ref block_str) = hook.block_on_output
                        && stdout.starts_with(block_str.as_str())
                    {
                        outcome.blocked = true;
                        outcome.block_reason = Some(stdout_trim);
                    }
                }
                Err(e) => {
                    outcome
                        .warnings
                        .push(format!("Hook '{}' failed: {}", hook.name, e));
                }
            }
        }

        outcome
    }

    /// Return true if there are no hooks or custom tools configured.
    pub fn is_empty(&self) -> bool {
        self.config.hooks.is_empty() && self.config.custom_tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config_parses() {
        let config: PluginConfig = serde_json::from_str("{}").unwrap();
        assert!(config.hooks.is_empty());
        assert!(config.custom_tools.is_empty());
    }

    #[test]
    fn test_full_config_parses() {
        let json = r#"{
            "hooks": [
                {
                    "name": "log",
                    "event": "after_tool_call",
                    "command": "echo '{tool}' >> /tmp/log.txt"
                }
            ],
            "custom_tools": [
                {
                    "name": "greet",
                    "description": "Say hi",
                    "category": "readonly",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string", "description": "Who to greet" }
                        },
                        "required": ["name"]
                    },
                    "command": "echo hello {name}"
                }
            ]
        }"#;
        let config: PluginConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.hooks.len(), 1);
        assert_eq!(config.custom_tools.len(), 1);
        assert_eq!(config.hooks[0].name, "log");
        assert_eq!(config.custom_tools[0].name, "greet");
    }

    #[test]
    fn test_plugin_manager_load_from_config() {
        let config = PluginConfig {
            hooks: vec![],
            custom_tools: vec![],
        };
        let mgr = PluginManager::from_config(config);
        assert!(mgr.is_empty());
    }

    #[test]
    fn test_load_config_file_missing_returns_none() {
        let path = std::path::Path::new("/nonexistent/path/plugins.json");
        let result = load_config_file(path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_load_config_file_valid() {
        let dir =
            std::env::temp_dir().join(format!("tinyharness_plugin_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plugins.json");
        std::fs::write(&path, r#"{"hooks": [], "custom_tools": []}"#).unwrap();

        let config = load_config_file(&path).unwrap().unwrap();
        assert!(config.hooks.is_empty());
        assert!(config.custom_tools.is_empty());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_load_config_file_invalid_json() {
        let dir = std::env::temp_dir().join(format!(
            "tinyharness_plugin_test_invalid_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plugins.json");
        std::fs::write(&path, "not json").unwrap();

        let result = load_config_file(&path);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_discover_project_plugins_finds_file() {
        let dir = std::env::temp_dir().join(format!(
            "tinyharness_plugin_discover_{}",
            std::process::id()
        ));
        let project_dir = dir.join("myproject/.tinyharness");
        std::fs::create_dir_all(&project_dir).unwrap();
        let plugins_path = project_dir.join("plugins.json");
        std::fs::write(&plugins_path, "{}").unwrap();

        let found = discover_project_plugins(&dir.join("myproject"));
        assert_eq!(found, Some(plugins_path.clone()));

        // Also test from a subdirectory — should walk up and find it
        let subdir = dir.join("myproject/src/deep");
        std::fs::create_dir_all(&subdir).unwrap();
        let found = discover_project_plugins(&subdir);
        assert_eq!(found, Some(plugins_path.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_project_plugins_not_found() {
        let dir = std::env::temp_dir().join(format!(
            "tinyharness_plugin_nodiscover_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let found = discover_project_plugins(&dir);
        assert_eq!(found, None);
        let _ = std::fs::remove_dir(&dir);
    }
}
