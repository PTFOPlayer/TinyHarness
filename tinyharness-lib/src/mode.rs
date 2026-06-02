use std::fmt;
use std::path::Path;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum AgentMode {
    Casual,
    Planning,
    Agent,
    Research,
}

impl AgentMode {
    /// Returns the filename for this mode's prompt file (e.g. "casual.md").
    pub fn prompts_filename(&self) -> &'static str {
        match self {
            AgentMode::Casual => "casual.md",
            AgentMode::Planning => "planning.md",
            AgentMode::Agent => "agent.md",
            AgentMode::Research => "research.md",
        }
    }

    /// Returns whether this mode uses the shared developer header.
    /// Casual mode is self-contained; all other modes use header + mode section.
    pub fn uses_header(&self) -> bool {
        !matches!(self, AgentMode::Casual)
    }

    /// Load the shared header prompt from a `.md` file in `prompts_dir`.
    /// Falls back to the hardcoded default if the file cannot be read.
    fn load_header(prompts_dir: &Path) -> String {
        let file_path = prompts_dir.join("header.md");
        match std::fs::read_to_string(&file_path) {
            Ok(content) => {
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    include_str!("prompts/header.md").trim().to_string()
                } else {
                    trimmed.to_string()
                }
            }
            Err(_) => include_str!("prompts/header.md").trim().to_string(),
        }
    }

    /// Load the system prompt for this mode from `.md` files in `prompts_dir`.
    ///
    /// For Agent, Planning, and Research modes, the prompt is assembled as:
    ///   header.md + blank line + <mode>.md
    ///
    /// Casual mode uses only its own file (self-contained).
    ///
    /// Falls back to hardcoded defaults if files cannot be read.
    pub fn load_system_prompt(&self, prompts_dir: &Path) -> String {
        let mode_path = prompts_dir.join(self.prompts_filename());
        let mode_content = match std::fs::read_to_string(&mode_path) {
            Ok(content) => {
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    self.default_system_prompt().to_string()
                } else {
                    trimmed.to_string()
                }
            }
            Err(_) => self.default_system_prompt().to_string(),
        };

        if self.uses_header() {
            let header = Self::load_header(prompts_dir);
            format!("{}\n\n{}", header, mode_content)
        } else {
            mode_content
        }
    }

    /// Returns the hardcoded default system prompt (embedded at compile time
    /// via `include_str!`). Used as fallback when no custom prompt file exists
    /// and as the seed for first-launch initialization.
    ///
    /// For Agent, Planning, and Research modes, this returns only the
    /// mode-specific section (the header is handled separately).
    pub fn default_system_prompt(&self) -> &'static str {
        match self {
            AgentMode::Casual => include_str!("prompts/casual.md"),
            AgentMode::Planning => include_str!("prompts/planning.md"),
            AgentMode::Agent => include_str!("prompts/agent.md"),
            AgentMode::Research => include_str!("prompts/research.md"),
        }
    }
}

impl fmt::Display for AgentMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentMode::Casual => f.write_str("casual"),
            AgentMode::Planning => f.write_str("planning"),
            AgentMode::Agent => f.write_str("agent"),
            AgentMode::Research => f.write_str("research"),
        }
    }
}

impl FromStr for AgentMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_lowercase().as_str() {
            "casual" => Ok(AgentMode::Casual),
            "planning" | "plan" => Ok(AgentMode::Planning),
            "agent" | "dev" => Ok(AgentMode::Agent),
            "research" | "researching" => Ok(AgentMode::Research),
            _ => Err(format!(
                "Unknown mode '{}'. Valid modes: casual, planning, agent, research",
                s
            )),
        }
    }
}
