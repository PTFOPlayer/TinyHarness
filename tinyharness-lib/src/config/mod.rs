use std::path::PathBuf;
use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::mode::AgentMode;

/// Identifies which provider backend was used last.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ProviderKind {
    Ollama,
    LlamaCpp,
    Vllm,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderKind::Ollama => f.write_str("ollama"),
            ProviderKind::LlamaCpp => f.write_str("llama.cpp"),
            ProviderKind::Vllm => f.write_str("vllm"),
        }
    }
}

impl FromStr for ProviderKind {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "ollama" => Ok(ProviderKind::Ollama),
            "llama.cpp" | "llamacpp" | "llama_cpp" => Ok(ProviderKind::LlamaCpp),
            "vllm" => Ok(ProviderKind::Vllm),
            _ => Err(format!("Unknown provider '{}'", s)),
        }
    }
}

/// Controls the "think" (reasoning) level for Ollama models that support it
/// (e.g. qwen2.5 variants). Affects how much internal reasoning the model does
/// before responding.
///
/// Corresponds to Ollama's `think` parameter. Only meaningful for Ollama;
/// other providers ignore this setting.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum OllamaThinkType {
    /// Disable thinking entirely (fastest, lowest quality).
    Off,
    /// Minimal reasoning (faster, lower latency).
    Low,
    /// Balanced reasoning (good default).
    Medium,
    /// Maximum reasoning (slowest, highest quality).
    High,
}

impl fmt::Display for OllamaThinkType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OllamaThinkType::Off => f.write_str("off"),
            OllamaThinkType::Low => f.write_str("low"),
            OllamaThinkType::Medium => f.write_str("medium"),
            OllamaThinkType::High => f.write_str("high"),
        }
    }
}

impl FromStr for OllamaThinkType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "off" | "false" | "0" => Ok(OllamaThinkType::Off),
            "low" => Ok(OllamaThinkType::Low),
            "medium" | "mid" => Ok(OllamaThinkType::Medium),
            "high" => Ok(OllamaThinkType::High),
            _ => Err(format!(
                "Unknown think type '{}'. Valid values: off, low, medium, high",
                s
            )),
        }
    }
}

/// Persisted application settings — pure data type with no I/O.
///
/// Use `SettingsStore` to load and save instances of this type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub last_provider: ProviderKind,
    pub last_model: Option<String>,
    pub preferred_mode: AgentMode,
    pub ollama_api_key: Option<String>,
    /// Timeout in seconds for Ollama requests (default: 5)
    pub ollama_timeout_secs: u64,
    /// Maximum number of retries for Ollama requests (default: 3)
    pub ollama_max_retries: u32,
    /// Controls the think/reasoning level for Ollama (default: Medium)
    pub ollama_think_type: OllamaThinkType,
    /// Show the model's thinking/reasoning chain inline during streaming (default: false)
    pub show_thinking: bool,
    /// Context limit for warning calculations only (default: None, uses model default)
    pub context_limit: Option<u32>,
    /// Automatically accept safe read-only commands in the run tool (default: true)
    pub auto_accept_safe_commands: bool,
    /// List of command prefixes considered safe for auto-accept (default: see get_default_safe_commands)
    pub safe_command_prefixes: Option<Vec<String>>,
    /// List of command prefixes that are always denied auto-accept, even if they
    /// match a safe prefix. Takes priority over the safe list. (default: empty)
    pub denied_command_prefixes: Option<Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            last_provider: ProviderKind::Ollama,
            last_model: None,
            preferred_mode: AgentMode::Casual,
            ollama_api_key: None,
            ollama_timeout_secs: 5,
            ollama_max_retries: 3,
            ollama_think_type: OllamaThinkType::Medium,
            show_thinking: false,
            context_limit: None,
            auto_accept_safe_commands: true,
            safe_command_prefixes: None,
            denied_command_prefixes: None,
        }
    }
}

/// Returns the default list of safe command prefixes.
/// These are read-only commands that don't modify the filesystem.
pub fn get_default_safe_commands() -> Vec<String> {
    vec![
        "cd".to_string(),
        "ls".to_string(),
        "grep".to_string(),
        "glob".to_string(),
        "find".to_string(),
        "cat".to_string(),
        "head".to_string(),
        "tail".to_string(),
        "wc".to_string(),
        "pwd".to_string(),
        "echo".to_string(),
        "printf".to_string(),
        "tree".to_string(),
        "file".to_string(),
        "stat".to_string(),
        "du".to_string(),
        "df".to_string(),
        "free".to_string(),
        "uptime".to_string(),
        "whoami".to_string(),
        "hostname".to_string(),
        "uname".to_string(),
        "date".to_string(),
        "cal".to_string(),
        "ps".to_string(),
        "env".to_string(),
        "printenv".to_string(),
        "which".to_string(),
        "whereis".to_string(),
        "type".to_string(),
        "git status".to_string(),
        "git diff".to_string(),
        "git log".to_string(),
        "git show".to_string(),
        "git branch".to_string(),
        "git remote".to_string(),
        "git tag".to_string(),
        "git describe".to_string(),
        "git rev-parse".to_string(),
        "cargo tree".to_string(),
        "cargo metadata".to_string(),
        "cargo doc".to_string(),
        "rustc --version".to_string(),
        "cargo --version".to_string(),
    ]
}

impl Settings {
    /// Returns the list of safe command prefixes, using defaults if not configured.
    pub fn get_safe_commands(&self) -> Vec<String> {
        self.safe_command_prefixes
            .clone()
            .unwrap_or_else(get_default_safe_commands)
    }

    /// Returns the list of always-denied command prefixes.
    /// These take priority over the safe list — if a command matches both
    /// a safe prefix and a denied prefix, it is denied.
    /// Defaults to an empty list (nothing denied).
    pub fn get_denied_commands(&self) -> Vec<String> {
        self.denied_command_prefixes.clone().unwrap_or_default()
    }
}

// ── Errors ──────────────────────────────────────────────────────────────────

/// Error type for settings I/O operations.
#[derive(Debug)]
pub enum SettingsError {
    Io(std::io::Error),
    Parse(serde_json::Error),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsError::Io(e) => write!(f, "I/O error: {}", e),
            SettingsError::Parse(e) => write!(f, "parse error: {}", e),
        }
    }
}

impl std::error::Error for SettingsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            SettingsError::Io(e) => Some(e),
            SettingsError::Parse(e) => Some(e),
        }
    }
}

impl From<std::io::Error> for SettingsError {
    fn from(e: std::io::Error) -> Self {
        SettingsError::Io(e)
    }
}

impl From<serde_json::Error> for SettingsError {
    fn from(e: serde_json::Error) -> Self {
        SettingsError::Parse(e)
    }
}

// ── Settings store ─────────────────────────────────────────────────────────

/// Handles persistence of [`Settings`] to disk.
///
/// By default, uses `~/.config/tinyharness/settings.json`. This can be
/// overridden with [`SettingsStore::new`] for testing or alternative paths.
pub struct SettingsStore {
    path: std::path::PathBuf,
}

impl SettingsStore {
    /// Create a store that reads/writes from the default path
    /// (`~/.config/tinyharness/settings.json`).
    pub fn default_path() -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        let path = std::path::PathBuf::from(home).join(".config/tinyharness/settings.json");
        SettingsStore { path }
    }

    /// Create a store that reads/writes from a custom path.
    pub fn new(path: std::path::PathBuf) -> Self {
        SettingsStore { path }
    }

    /// Load settings from disk.
    ///
    /// Returns `Ok(Settings)` with defaults if the file doesn't exist,
    /// or an error if the file exists but cannot be parsed.
    pub fn load(&self) -> Result<Settings, SettingsError> {
        if !self.path.exists() {
            return Ok(Settings::default());
        }
        let content = std::fs::read_to_string(&self.path)?;
        let settings = serde_json::from_str(&content)?;
        Ok(settings)
    }

    /// Load settings from disk, returning defaults on any error.
    ///
    /// This matches the original behaviour and is suitable for application
    /// startup where you don't want to fail on corrupt settings files.
    pub fn load_or_default(&self) -> Settings {
        self.load().unwrap_or_else(|e| {
            tracing::warn!("Failed to load settings: {e}. Using defaults.");
            Settings::default()
        })
    }

    /// Save settings to disk atomically (write to temp file, then rename).
    pub fn save(&self, settings: &Settings) -> Result<(), SettingsError> {
        let dir = self.path.parent().ok_or_else(|| {
            SettingsError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "settings path has no parent directory",
            ))
        })?;
        std::fs::create_dir_all(dir)?;

        let json = serde_json::to_string_pretty(settings)?;
        let tmp_path = dir.join("settings.json.tmp");

        {
            let mut file = std::fs::File::create(&tmp_path)?;
            std::io::Write::write_all(&mut file, json.as_bytes())?;
            std::io::Write::flush(&mut file)?;
        }

        std::fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }

    /// Returns the path where settings are stored.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

// ── Free convenience functions ─────────────────────────────────────────────

/// Load settings from the default path, returning defaults on any error.
///
/// This is a convenience wrapper around [`SettingsStore::default_path().load_or_default()`].
pub fn load_settings() -> Settings {
    SettingsStore::default_path().load_or_default()
}

/// Save settings to the default path atomically.
///
/// This is a convenience wrapper around [`SettingsStore::default_path().save()`].
/// On error, prints a warning to stderr (matching the original behaviour).
pub fn save_settings(settings: &Settings) {
    let store = SettingsStore::default_path();
    if let Err(e) = store.save(settings) {
        tracing::warn!("Failed to save settings: {e}");
    }
}

// ── Prompt file management ──────────────────────────────────────────────────

/// Returns the directory where per-mode prompt `.md` files are stored.
///
/// Default: `~/.config/tinyharness/prompts/`
pub fn prompts_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("tinyharness")
        .join("prompts")
}

/// Ensure the prompts directory exists and is seeded with `.md` files
/// containing the hardcoded defaults for each mode, plus the shared header.
///
/// On first launch, this creates `~/.config/tinyharness/prompts/` and writes
/// `header.md`, `casual.md`, `planning.md`, `agent.md`, and `research.md`.
/// Existing files are **never** overwritten — users can safely customize them.
///
/// Returns the prompts directory path.
pub fn ensure_prompts_initialized() -> PathBuf {
    let dir = prompts_dir();
    if !dir.exists() {
        std::fs::create_dir_all(&dir).ok();
    }

    // Write shared header (new in 0.2 — always writes if missing)
    {
        let header_path = dir.join("header.md");
        if !header_path.exists() {
            let header_text = include_str!("../prompts/header.md");
            if let Err(e) = std::fs::write(&header_path, header_text.trim()) {
                tracing::warn!(
                    "Failed to write default header to {}: {e}",
                    header_path.display(),
                );
            }
        }
    }

    let modes = [
        AgentMode::Casual,
        AgentMode::Planning,
        AgentMode::Agent,
        AgentMode::Research,
    ];

    for mode in &modes {
        let file_path = dir.join(mode.prompts_filename());
        if !file_path.exists() {
            let default_text = mode.default_system_prompt();
            // Write the default, trimming leading/trailing whitespace/newlines
            if let Err(e) = std::fs::write(&file_path, default_text.trim()) {
                tracing::warn!(
                    "Failed to write default prompt to {}: {e}",
                    file_path.display(),
                );
            }
        }
    }

    dir
}
