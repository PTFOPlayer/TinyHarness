pub mod custom_tool;
pub mod shell;

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub use custom_tool::{CustomToolCategory, CustomToolDefinition};
pub use shell::{ShellCommand, execute_shell_tool};

/// Custom-tool configuration loaded from `custom_tools.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CustomToolConfig {
    #[serde(default)]
    pub custom_tools: Vec<CustomToolDefinition>,
}

/// Custom-tool manager — loads and merges global + project custom tools.
#[derive(Debug, Clone, Default)]
pub struct CustomToolManager {
    config: CustomToolConfig,
}

/// Error type for custom-tool loading.
#[derive(Debug)]
pub enum CustomToolError {
    Io(std::io::Error),
    Parse(serde_json::Error),
}

impl std::fmt::Display for CustomToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CustomToolError::Io(e) => write!(f, "I/O error: {}", e),
            CustomToolError::Parse(e) => write!(f, "parse error: {}", e),
        }
    }
}

impl std::error::Error for CustomToolError {}

impl From<std::io::Error> for CustomToolError {
    fn from(e: std::io::Error) -> Self {
        CustomToolError::Io(e)
    }
}

impl From<serde_json::Error> for CustomToolError {
    fn from(e: serde_json::Error) -> Self {
        CustomToolError::Parse(e)
    }
}

/// Default path for the global custom-tools config file.
fn global_custom_tools_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/tinyharness/custom_tools.json")
}

/// Discover `.tinyharness/custom_tools.json` by walking up from the given directory.
/// Returns `None` if no file is found.
fn discover_project_custom_tools(start_dir: &std::path::Path) -> Option<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join(".tinyharness").join("custom_tools.json");
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

/// Load a single custom-tools config file. Returns `Ok(None)` if the file doesn't exist.
fn load_config_file(path: &std::path::Path) -> Result<Option<CustomToolConfig>, CustomToolError> {
    if !path.is_file() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(path)?;
    let config: CustomToolConfig = serde_json::from_str(&content)?;
    Ok(Some(config))
}

impl CustomToolManager {
    /// Load and merge global + project custom-tools configs.
    ///
    /// Global config is loaded from `~/.config/tinyharness/custom_tools.json`.
    /// Project config is loaded from `.tinyharness/custom_tools.json` (walking up
    /// from CWD). Project tools are **appended** to the global list (both run;
    /// global first, then project).
    pub fn load() -> Self {
        Self::load_from_cwd(&std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }

    /// Load from a specific directory (for testing).
    pub fn load_from_cwd(cwd: &std::path::Path) -> Self {
        let mut combined = CustomToolConfig::default();

        // Global config
        let global_path = global_custom_tools_path();
        match load_config_file(&global_path) {
            Ok(Some(cfg)) => combined = cfg,
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    "Failed to load global custom_tools.json from {}: {e}",
                    global_path.display()
                );
            }
        }

        // Project config (extends global)
        if let Some(project_path) = discover_project_custom_tools(cwd) {
            match load_config_file(&project_path) {
                Ok(Some(cfg)) => {
                    combined.custom_tools.extend(cfg.custom_tools);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        "Failed to load project custom_tools.json from {}: {e}",
                        project_path.display()
                    );
                }
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

        CustomToolManager { config: combined }
    }

    /// Load from an explicit config (for testing).
    pub fn from_config(config: CustomToolConfig) -> Self {
        CustomToolManager { config }
    }

    /// Return the list of custom tool definitions.
    pub fn custom_tools(&self) -> &[CustomToolDefinition] {
        &self.config.custom_tools
    }

    /// Return true if there are no custom tools configured.
    pub fn is_empty(&self) -> bool {
        self.config.custom_tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config_parses() {
        let config: CustomToolConfig = serde_json::from_str("{}").unwrap();
        assert!(config.custom_tools.is_empty());
    }

    #[test]
    fn test_full_config_parses() {
        let json = r#"{
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
        let config: CustomToolConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.custom_tools.len(), 1);
        assert_eq!(config.custom_tools[0].name, "greet");
    }

    #[test]
    fn test_custom_tool_manager_load_from_config() {
        let config = CustomToolConfig {
            custom_tools: vec![],
        };
        let mgr = CustomToolManager::from_config(config);
        assert!(mgr.is_empty());
    }

    #[test]
    fn test_load_config_file_missing_returns_none() {
        let path = std::path::Path::new("/nonexistent/path/custom_tools.json");
        let result = load_config_file(path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_load_config_file_valid() {
        let dir = std::env::temp_dir().join(format!(
            "tinyharness_custom_tools_test_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("custom_tools.json");
        std::fs::write(&path, r#"{"custom_tools": []}"#).unwrap();

        let config = load_config_file(&path).unwrap().unwrap();
        assert!(config.custom_tools.is_empty());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_load_config_file_invalid_json() {
        let dir = std::env::temp_dir().join(format!(
            "tinyharness_custom_tools_test_invalid_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("custom_tools.json");
        std::fs::write(&path, "not json").unwrap();

        let result = load_config_file(&path);
        assert!(result.is_err());

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn test_discover_project_custom_tools_finds_file() {
        let dir = std::env::temp_dir().join(format!(
            "tinyharness_custom_tools_discover_{}",
            std::process::id()
        ));
        let project_dir = dir.join("myproject/.tinyharness");
        std::fs::create_dir_all(&project_dir).unwrap();
        let tools_path = project_dir.join("custom_tools.json");
        std::fs::write(&tools_path, "{}").unwrap();

        let found = discover_project_custom_tools(&dir.join("myproject"));
        assert_eq!(found, Some(tools_path.clone()));

        // Also test from a subdirectory — should walk up and find it
        let subdir = dir.join("myproject/src/deep");
        std::fs::create_dir_all(&subdir).unwrap();
        let found = discover_project_custom_tools(&subdir);
        assert_eq!(found, Some(tools_path.clone()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_discover_project_custom_tools_not_found() {
        let dir = std::env::temp_dir().join(format!(
            "tinyharness_custom_tools_nodiscover_{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let found = discover_project_custom_tools(&dir);
        assert_eq!(found, None);
        let _ = std::fs::remove_dir(&dir);
    }
}
