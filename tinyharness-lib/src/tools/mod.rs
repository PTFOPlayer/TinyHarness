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

#[derive(Default)]
pub struct ToolManager {
    tools: Vec<Tool>,
}

impl ToolManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register all built-in tools.
    pub fn register_defaults(&mut self) {
        self.register_tool(crate::tools::auto_compact::auto_compact_tool_entry());
        self.register_tool(crate::tools::ls::ls_tool_entry());
        self.register_tool(crate::tools::read::read_tool_entry());
        self.register_tool(crate::tools::write::write_tool_entry());
        self.register_tool(crate::tools::edit::edit_tool_entry());
        self.register_tool(crate::tools::grep::grep_tool_entry());
        self.register_tool(crate::tools::run::run_tool_entry());
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

    /// Returns the tool definitions for all registered tools.
    pub fn get_all_tool_definitions(&self) -> Vec<ToolDefinition> {
        self.tools.iter().map(|t| t.to_definition()).collect()
    }

    /// Returns the tool definitions appropriate for the given agent mode.
    pub fn tools_for_mode(&self, mode: AgentMode) -> Vec<ToolDefinition> {
        match mode {
            AgentMode::Agent => self.get_all_tool_definitions(),
            AgentMode::Casual => self
                .tools
                .iter()
                .filter(|t| t.name == "web_search" || t.name == "web_fetch")
                .map(|t| t.to_definition())
                .collect(),
            AgentMode::Planning => self
                .tools
                .iter()
                .filter(|t| {
                    t.category == ToolCategory::ReadOnly || t.category == ToolCategory::Signal
                })
                .map(|t| t.to_definition())
                .collect(),
            AgentMode::Research => self
                .tools
                .iter()
                .filter(|t| {
                    t.category == ToolCategory::ReadOnly || t.category == ToolCategory::Signal
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
        if let Some(tool) = self.tools.iter().find(|t| t.name == tool_name) {
            tool::execute_tool_call(tool, arguments).await
        } else {
            format!("Error: Tool '{}' not found", tool_name)
        }
    }
}
