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

    /// Load the system prompt for this mode from a `.md` file in `prompts_dir`.
    /// Falls back to the hardcoded default if the file cannot be read.
    pub fn load_system_prompt(&self, prompts_dir: &Path) -> String {
        let file_path = prompts_dir.join(self.prompts_filename());
        match std::fs::read_to_string(&file_path) {
            Ok(content) => {
                let trimmed = content.trim();
                if trimmed.is_empty() {
                    self.default_system_prompt().to_string()
                } else {
                    trimmed.to_string()
                }
            }
            Err(_) => self.default_system_prompt().to_string(),
        }
    }

    /// Returns the hardcoded default system prompt (embedded at compile time
    /// via `include_str!`). Used as fallback when no custom prompt file exists
    /// and as the seed for first-launch initialization.
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
