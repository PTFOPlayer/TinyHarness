use std::{
    fmt,
    fs,
    io::Write,
    path::PathBuf,
    str::FromStr,
};

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

/// Persisted application settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub last_provider: ProviderKind,
    pub last_model: Option<String>,
    pub preferred_mode: AgentMode,
    pub ollama_api_key: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            last_provider: ProviderKind::Ollama,
            last_model: None,
            preferred_mode: AgentMode::Casual,
            ollama_api_key: None,
        }
    }
}

fn settings_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/tinyharness")
}

fn settings_path() -> PathBuf {
    settings_dir().join("settings.json")
}

impl Settings {
    /// Load settings from disk. Returns defaults if the file doesn't exist or is corrupt.
    pub fn load() -> Self {
        let path = settings_path();
        if !path.exists() {
            return Settings::default();
        }
        match fs::read_to_string(&path) {
            Ok(content) => {
                serde_json::from_str(&content).unwrap_or_else(|e| {
                    eprintln!("Warning: Failed to parse settings file: {}. Using defaults.", e);
                    Settings::default()
                })
            }
            Err(e) => {
                eprintln!("Warning: Failed to read settings file: {}. Using defaults.", e);
                Settings::default()
            }
        }
    }

    /// Save settings to disk atomically (write to temp file, then rename).
    pub fn save(&self) {
        let dir = settings_dir();
        if let Err(e) = fs::create_dir_all(&dir) {
            eprintln!("Warning: Failed to create settings directory: {}", e);
            return;
        }

        let json = match serde_json::to_string_pretty(self) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("Warning: Failed to serialize settings: {}", e);
                return;
            }
        };

        let tmp_path = dir.join("settings.json.tmp");
        let final_path = settings_path();

        // Write to temp file
        match fs::File::create(&tmp_path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(json.as_bytes()) {
                    eprintln!("Warning: Failed to write settings: {}", e);
                    let _ = fs::remove_file(&tmp_path);
                    return;
                }
                if let Err(e) = file.flush() {
                    eprintln!("Warning: Failed to flush settings: {}", e);
                    let _ = fs::remove_file(&tmp_path);
                    return;
                }
            }
            Err(e) => {
                eprintln!("Warning: Failed to create settings file: {}", e);
                return;
            }
        }

        // Atomic rename
        if let Err(e) = fs::rename(&tmp_path, &final_path) {
            eprintln!("Warning: Failed to rename settings file: {}", e);
            let _ = fs::remove_file(&tmp_path);
        }
    }
}
