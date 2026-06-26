use std::path::PathBuf;
use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

use crate::mode::AgentMode;

// ── AutoAccept Mode ────────────────────────────────────────────────────────

/// Controls how aggressively tool calls are auto-approved.
///
/// Parsed from string values for JSON config: `"off"`, `"safe"`, `"all"`.
/// Legacy boolean fields (`auto_accept_all`, `auto_accept_safe_commands`)
/// are resolved in `SettingsStore::load()` when `auto_accept_mode` is absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoAcceptMode {
    Off,
    #[default]
    Safe,
    All,
}

impl<'de> Deserialize<'de> for AutoAcceptMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct AutoAcceptModeVisitor;

        impl<'de> serde::de::Visitor<'de> for AutoAcceptModeVisitor {
            type Value = AutoAcceptMode;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter
                    .write_str("a string (\"off\", \"safe\", \"all\") or a boolean (true/false)")
            }

            fn visit_bool<E>(self, v: bool) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Ok(if v {
                    AutoAcceptMode::All
                } else {
                    AutoAcceptMode::Off
                })
            }

            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                AutoAcceptMode::from_str(v).map_err(serde::de::Error::custom)
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                AutoAcceptMode::from_str(&v).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_any(AutoAcceptModeVisitor)
    }
}

impl FromStr for AutoAcceptMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "off" | "false" | "no" | "0" | "disabled" => Ok(AutoAcceptMode::Off),
            "safe" | "on" => Ok(AutoAcceptMode::Safe),
            "all" | "always" | "true" | "yes" | "1" | "enabled" => Ok(AutoAcceptMode::All),
            _ => Err(format!(
                "Unknown auto-accept mode '{}'. Valid values: off, safe, all",
                s
            )),
        }
    }
}

impl fmt::Display for AutoAcceptMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AutoAcceptMode::Off => write!(f, "off"),
            AutoAcceptMode::Safe => write!(f, "safe"),
            AutoAcceptMode::All => write!(f, "all"),
        }
    }
}

// ── Project Settings ────────────────────────────────────────────────────────

/// Per-project override settings discovered from `.tinyharness/config.json`.
///
/// All fields are `Option` — only present fields override the global setting.
/// Discovery walks up from CWD, same algorithm as `discover_project_md`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectSettings {
    /// Override safe command prefixes (extends, doesn't replace)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_command_prefixes: Option<Vec<String>>,
    /// Override denied command prefixes
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied_command_prefixes: Option<Vec<String>>,
    /// Override auto-accept mode for this project
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_accept_mode: Option<AutoAcceptMode>,
    /// Override context_limit
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_limit: Option<u32>,
    /// Additional project-specific MD file names to include in context
    /// (e.g. ["RULES.md", ".cursorrules"]). These are loaded AFTER the main
    /// project instruction file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_md_files: Option<Vec<String>>,
    /// Override the preferred mode for this project
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_mode: Option<AgentMode>,
}

/// Discover and load `.tinyharness/config.json` by walking up from CWD.
///
/// Returns `None` if no config file is found. Returns `Some(Err(...))` if
/// a file is found but cannot be parsed.
pub fn discover_project_settings(
    start_dir: &std::path::Path,
) -> Option<Result<ProjectSettings, SettingsError>> {
    let mut dir = start_dir.to_path_buf();

    loop {
        let candidate = dir.join(".tinyharness").join("config.json");
        if candidate.is_file() {
            let content = match std::fs::read_to_string(&candidate) {
                Ok(c) => c,
                Err(e) => return Some(Err(SettingsError::Io(e))),
            };
            let parsed = serde_json::from_str(&content).map_err(SettingsError::Parse);
            return Some(parsed);
        }

        // Walk up one directory
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

/// Source annotation for merged settings values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingSource {
    /// Value came from the global `~/.config/tinyharness/settings.json`
    Global,
    /// Value came from `.tinyharness/config.json`
    Project,
    /// Value is the hardcoded default (no config found)
    Default,
}

impl std::fmt::Display for SettingSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingSource::Global => f.write_str("global"),
            SettingSource::Project => f.write_str("project"),
            SettingSource::Default => f.write_str("default"),
        }
    }
}

/// Effective merged settings — the result of layering global + project settings.
///
/// This is a read-only view. Each field has a source annotation for display.
#[derive(Debug, Clone)]
pub struct MergedSettings {
    /// Merged safe command prefixes (project extends global)
    pub safe_commands: Vec<String>,
    pub safe_commands_source: SettingSource,
    /// Merged denied command prefixes (project overrides global)
    pub denied_commands: Vec<String>,
    pub denied_commands_source: SettingSource,
    pub auto_accept_mode: AutoAcceptMode,
    pub auto_accept_mode_source: SettingSource,
    pub context_limit: Option<u32>,
    pub context_limit_source: SettingSource,
    pub project_md_files: Vec<String>,
    pub project_md_files_source: SettingSource,
    pub preferred_mode: AgentMode,
    pub preferred_mode_source: SettingSource,
}

/// Load and merge global + project settings.
///
/// Layering: project overrides global where specified, otherwise falls back
/// to global. For safe commands, project *extends* the global list rather
/// than replacing it.
pub fn load_merged_settings() -> (Settings, Option<ProjectSettings>, MergedSettings) {
    let global = load_settings();

    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let project = match discover_project_settings(&cwd) {
        Some(Ok(ps)) => Some(ps),
        Some(Err(e)) => {
            tracing::warn!(
                "Failed to parse .tinyharness/config.json: {e}. Ignoring project settings."
            );
            None
        }
        None => None,
    };

    let merged = merge_settings(&global, project.as_ref());
    (global, project, merged)
}

/// Merge global settings with optional project overrides.
fn merge_settings(global: &Settings, project: Option<&ProjectSettings>) -> MergedSettings {
    let global_safe = global.get_safe_commands();
    let global_denied = global.get_denied_commands();

    match project {
        None => MergedSettings {
            safe_commands: global_safe,
            safe_commands_source: SettingSource::Default,
            denied_commands: global_denied,
            denied_commands_source: SettingSource::Default,
            auto_accept_mode: global.auto_accept_mode,
            auto_accept_mode_source: SettingSource::Default,
            context_limit: global.context_limit,
            context_limit_source: SettingSource::Default,
            project_md_files: Vec::new(),
            project_md_files_source: SettingSource::Default,
            preferred_mode: global.preferred_mode,
            preferred_mode_source: SettingSource::Default,
        },
        Some(p) => {
            // Safe commands: project extends global
            let safe_commands = if let Some(ref proj_safe) = p.safe_command_prefixes {
                let mut combined = global_safe.clone();
                for cmd in proj_safe {
                    if !combined.contains(cmd) {
                        combined.push(cmd.clone());
                    }
                }
                combined
            } else {
                global_safe
            };
            let safe_source = if p.safe_command_prefixes.is_some() {
                SettingSource::Project
            } else {
                SettingSource::Default
            };

            // Denied commands: project replaces global
            let (denied_commands, denied_source) =
                if let Some(ref proj_denied) = p.denied_command_prefixes {
                    (proj_denied.clone(), SettingSource::Project)
                } else {
                    (global_denied, SettingSource::Default)
                };

            let (auto_accept_mode, auto_source) = p
                .auto_accept_mode
                .map(|v| (v, SettingSource::Project))
                .unwrap_or((global.auto_accept_mode, SettingSource::Default));

            let (context_limit, ctx_source) = p
                .context_limit
                .map(|v| (Some(v), SettingSource::Project))
                .unwrap_or((global.context_limit, SettingSource::Default));

            let (project_md_files, md_source) = p
                .project_md_files
                .as_ref()
                .map(|files| (files.clone(), SettingSource::Project))
                .unwrap_or((Vec::new(), SettingSource::Default));

            let (preferred_mode, mode_source) = p
                .preferred_mode
                .map(|m| (m, SettingSource::Project))
                .unwrap_or((global.preferred_mode, SettingSource::Default));

            MergedSettings {
                safe_commands,
                safe_commands_source: safe_source,
                denied_commands,
                denied_commands_source: denied_source,
                auto_accept_mode,
                auto_accept_mode_source: auto_source,
                context_limit,
                context_limit_source: ctx_source,
                project_md_files,
                project_md_files_source: md_source,
                preferred_mode,
                preferred_mode_source: mode_source,
            }
        }
    }
}

/// Generate a starter `.tinyharness/config.json` file from current settings
/// overrides that make sense for a project (safe commands, denied commands,
/// auto-accept, context limit).
pub fn generate_project_config_template(settings: &Settings) -> ProjectSettings {
    ProjectSettings {
        safe_command_prefixes: settings.safe_command_prefixes.clone(),
        denied_command_prefixes: settings.denied_command_prefixes.clone(),
        auto_accept_mode: Some(settings.auto_accept_mode),
        context_limit: settings.context_limit,
        project_md_files: None, // user must fill this in
        preferred_mode: Some(settings.preferred_mode),
    }
}

/// Identifies which provider backend was used last.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum ProviderKind {
    #[default]
    Ollama,
    LlamaCpp,
    Vllm,
    /// Generic OpenAI-compatible provider for hosted gateways (OpenRouter,
    /// Together, etc.) that require a Bearer API key. Unlike LlamaCpp/Vllm
    /// which target local unauthenticated servers, this provider always
    /// sends `Authorization: Bearer <key>` and requires `--api-key` or
    /// the `OPENAI_API_KEY` env var.
    OpenAiCompat,
    Sockudo,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProviderKind::Ollama => f.write_str("ollama"),
            ProviderKind::LlamaCpp => f.write_str("llama.cpp"),
            ProviderKind::Vllm => f.write_str("vllm"),
            ProviderKind::OpenAiCompat => f.write_str("openai-compat"),
            ProviderKind::Sockudo => f.write_str("sockudo"),
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
            "openai-compat" | "openaicompat" | "openai_compat" => Ok(ProviderKind::OpenAiCompat),
            "sockudo" => Ok(ProviderKind::Sockudo),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum OllamaThinkType {
    /// Disable thinking entirely (fastest, lowest quality).
    Off,
    /// Minimal reasoning (faster, lower latency).
    Low,
    /// Balanced reasoning (good default).
    #[default]
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
#[serde(default)] // fall back to field defaults if any field is missing
pub struct Settings {
    #[serde(default)]
    pub last_provider: ProviderKind,
    /// Last URL used for the active provider. Set automatically by `--config`
    /// or by passing `--url`. Persisted so subsequent runs don't re-prompt.
    /// (default: None)
    pub last_provider_url: Option<String>,
    #[serde(default)]
    pub last_model: Option<String>,
    #[serde(default)]
    pub preferred_mode: AgentMode,
    #[serde(default)]
    pub ollama_api_key: Option<String>,
    /// API key sent as `Authorization: Bearer <key>` by the OpenAI-compatible
    /// provider (`--openai-compat`). Set via `--api-key` or `OPENAI_API_KEY`
    /// env var. Not used by Ollama, llama.cpp, vLLM, or Sockudo.
    pub openai_compat_api_key: Option<String>,
    /// Sockudo app ID for the AI Transport provider.
    pub sockudo_app_id: Option<String>,
    /// Sockudo app key (used as auth_key in signed API requests and WebSocket URL).
    pub sockudo_app_key: Option<String>,
    /// Sockudo app secret (used to sign API requests via HMAC-SHA256).
    pub sockudo_app_secret: Option<String>,
    /// Timeout in seconds for Ollama requests (default: 5)
    #[serde(default)]
    pub ollama_timeout_secs: u64,
    /// Maximum number of retries for Ollama requests (default: 3)
    #[serde(default)]
    pub ollama_max_retries: u32,
    /// Controls the think/reasoning level for Ollama (default: Medium)
    #[serde(default)]
    pub ollama_think_type: OllamaThinkType,
    /// Show the model's thinking/reasoning chain inline during streaming (default: false)
    #[serde(default)]
    pub show_thinking: bool,
    /// Context limit for warning calculations only (default: None, uses model default)
    pub context_limit: Option<u32>,
    /// Control how aggressively tool calls are auto-approved:
    /// - `off`   – never auto-accept (prompt for everything)
    /// - `safe`  – auto-accept read-only commands (default)
    /// - `all`   – auto-accept every tool call (dangerous)
    ///
    /// Parsed from `/autoaccept` command. Use `safe` for everyday use.
    ///
    /// Legacy boolean configs are also accepted via field resolution
    /// in `SettingsStore::load()`.
    #[serde(default)]
    pub auto_accept_mode: AutoAcceptMode,
    /// Skip the provider health check at startup (default: false).
    /// Useful for the `--openai-compat` provider when the gateway requires a
    /// separate scope on `/health`, or for any server without a `/health`
    /// endpoint. When true, the agent proceeds straight to model selection
    /// and reports any connection error on the first real request instead.
    pub skip_health_check: bool,
    /// List of command prefixes considered safe for auto-accept (default: see get_default_safe_commands)
    pub safe_command_prefixes: Option<Vec<String>>,
    /// List of command prefixes that are always denied auto-accept, even if they
    /// match a safe prefix. Takes priority over the safe list. (default: empty)
    pub denied_command_prefixes: Option<Vec<String>>,
    /// Override the project instruction file discovery list.
    /// When set, replaces the hardcoded default list (TINYHARNESS.md, AGENTS.md, etc.).
    /// Use `TINYHARNESS_MD_FILES` env var for the highest priority override.
    /// (default: None → use hardcoded defaults)
    pub project_md_files: Option<Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            last_provider: ProviderKind::Ollama,
            last_provider_url: None,
            last_model: None,
            preferred_mode: AgentMode::Casual,
            ollama_api_key: None,
            openai_compat_api_key: None,
            sockudo_app_id: None,
            sockudo_app_key: None,
            sockudo_app_secret: None,
            ollama_timeout_secs: 5,
            ollama_max_retries: 3,
            ollama_think_type: OllamaThinkType::Medium,
            show_thinking: false,
            context_limit: None,
            auto_accept_mode: AutoAcceptMode::Safe,
            skip_health_check: false,
            safe_command_prefixes: None,
            denied_command_prefixes: None,
            project_md_files: None,
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
        let mut settings: Settings = serde_json::from_str(&content)?;

        // Legacy field resolution: old configs used `auto_accept_safe_commands: bool`
        // or `auto_accept_all: bool` instead of `auto_accept_mode`.
        // Only resolve if `auto_accept_mode` was NOT present in the JSON —
        // the current value is just the default (Safe).
        if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&content)
            && let Some(obj) = raw.as_object()
            && !obj.contains_key("auto_accept_mode")
        {
            // auto_accept_all: true → All, false → Off
            if let Some(val) = obj.get("auto_accept_all") {
                if val.as_bool() == Some(true) {
                    settings.auto_accept_mode = AutoAcceptMode::All;
                } else if val.as_bool() == Some(false) {
                    settings.auto_accept_mode = AutoAcceptMode::Off;
                }
            }
            // auto_accept_safe_commands: false → Off (was "don't auto-accept safe cmds")
            // auto_accept_safe_commands: true → Safe (already default, no change)
            else if let Some(val) = obj.get("auto_accept_safe_commands")
                && val.as_bool() == Some(false)
            {
                settings.auto_accept_mode = AutoAcceptMode::Off;
            }
        }

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

/// Resolve the effective list of project instruction file names to discover.
///
/// Priority (highest first):
/// 1. `TINYHARNESS_MD_FILES` env var (comma-separated)
/// 2. `settings.project_md_files` from global settings
/// 3. Hardcoded default: TINYHARNESS.md, .tinyharness.md, AGENTS.md, CLAUDE.md
pub fn resolve_project_md_files(settings: Option<&Settings>) -> Vec<String> {
    // 1. Env var takes highest priority
    if let Ok(env) = std::env::var("TINYHARNESS_MD_FILES") {
        let files: Vec<String> = env
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !files.is_empty() {
            return files;
        }
    }

    // 2. Settings override
    if let Some(s) = settings
        && let Some(ref configured) = s.project_md_files
        && !configured.is_empty()
    {
        return configured.clone();
    }

    // 3. Hardcoded defaults
    crate::context::DEFAULT_PROJECT_MD_FILE_NAMES
        .iter()
        .map(|s| s.to_string())
        .collect()
}

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

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// User settings from a future build (with `auto_accept_mode`) must load
    /// against the current `Settings` struct without parse errors.
    #[test]
    fn load_settings_missing_fields_uses_defaults() {
        let json = r#"{
            "last_provider": "OpenAiCompat",
            "last_provider_url": "http://localhost:8080/v1",
            "last_model": "MiniMax-M3",
            "preferred_mode": "Agent",
            "ollama_api_key": null,
            "openai_compat_api_key": "sk-test",
            "ollama_timeout_secs": 5,
            "ollama_max_retries": 3,
            "ollama_think_type": "High",
            "show_thinking": true,
            "context_limit": 256000,
            "auto_accept_mode": "all",
            "skip_health_check": true
        }"#;

        let settings: Settings =
            serde_json::from_str(json).expect("missing fields should use defaults");
        assert_eq!(settings.last_provider, ProviderKind::OpenAiCompat);
        assert_eq!(settings.preferred_mode, AgentMode::Agent);
        // Missing fields default gracefully:
        assert!(settings.skip_health_check);
        assert_eq!(settings.ollama_think_type, OllamaThinkType::High);
    }

    /// An empty settings object should load with all defaults.
    #[test]
    fn load_settings_empty_object_uses_all_defaults() {
        let settings: Settings =
            serde_json::from_str("{}").expect("empty object should use defaults");
        assert_eq!(settings.last_provider, ProviderKind::Ollama);
        assert_eq!(settings.preferred_mode, AgentMode::Casual);
        assert!(!settings.skip_health_check);
    }

    #[test]
    fn auto_accept_mode_from_string() {
        let settings: Settings = serde_json::from_str(r#"{"auto_accept_mode": "all"}"#).unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::All);

        let settings: Settings = serde_json::from_str(r#"{"auto_accept_mode": "off"}"#).unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::Off);

        let settings: Settings = serde_json::from_str(r#"{"auto_accept_mode": "safe"}"#).unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::Safe);
    }

    #[test]
    fn auto_accept_mode_from_legacy_bool() {
        let settings: Settings = serde_json::from_str(r#"{"auto_accept_mode": true}"#).unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::All);

        let settings: Settings = serde_json::from_str(r#"{"auto_accept_mode": false}"#).unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::Off);
    }
    #[test]
    fn auto_accept_mode_from_legacy_all_field() {
        // auto_accept_all: true → All via load() resolution
        let store = SettingsStore::new(temp_settings_path());
        let json = r#"{"auto_accept_all": true}"#;
        std::fs::write(store.path(), json).unwrap();
        let settings = store.load().unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::All);
        let _ = std::fs::remove_file(store.path());

        // auto_accept_all: false → Off
        let store = SettingsStore::new(temp_settings_path());
        let json = r#"{"auto_accept_all": false}"#;
        std::fs::write(store.path(), json).unwrap();
        let settings = store.load().unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::Off);
        let _ = std::fs::remove_file(store.path());
    }
    #[test]
    fn auto_accept_mode_from_legacy_safe_commands_field() {
        // auto_accept_safe_commands: false → Off via load() resolution
        let store = SettingsStore::new(temp_settings_path());
        let json = r#"{"auto_accept_safe_commands": false}"#;
        std::fs::write(store.path(), json).unwrap();
        let settings = store.load().unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::Off);
        let _ = std::fs::remove_file(store.path());

        // auto_accept_safe_commands: true → Safe (default, no change)
        let store = SettingsStore::new(temp_settings_path());
        let json = r#"{"auto_accept_safe_commands": true}"#;
        std::fs::write(store.path(), json).unwrap();
        let settings = store.load().unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::Safe);
        let _ = std::fs::remove_file(store.path());
    }

    #[test]
    fn auto_accept_mode_explicit_overrides_legacy() {
        // Explicit auto_accept_mode string wins over alias
        let settings: Settings =
            serde_json::from_str(r#"{"auto_accept_mode": "off", "auto_accept_all": true}"#)
                .unwrap();
        assert_eq!(settings.auto_accept_mode, AutoAcceptMode::Off);
    }

    fn temp_settings_path() -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "tinyharness_test_{}_{:?}.json",
            std::process::id(),
            std::thread::current().id()
        ));
        dir
    }
}
