pub mod auto_compact;
pub mod edit;
pub mod git;
pub mod glob;
pub mod grep;
pub mod ls;
pub mod question;
pub mod read;
pub mod run;
pub mod switch_mode;
pub mod tool;
pub mod web_search;
pub mod write;

use crate::provider::ToolInfo;
use crate::tools::tool::Tool;

/// Register multiple tools at once.
///
/// # Example
/// ```
/// register_tools!(self,
///     crate::tools::ls::ls_tool_entry,
///     crate::tools::read::read_tool_entry,
/// );
/// ```
#[macro_export]
macro_rules! register_tools {
    ($manager:expr, $($entry:path),* $(,)?) => {
        $(
            $manager.register_tool($entry());
        )*
    };
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
        register_tools!(
            self,
            crate::tools::auto_compact::auto_compact_tool_entry,
            crate::tools::ls::ls_tool_entry,
            crate::tools::read::read_tool_entry,
            crate::tools::write::write_tool_entry,
            crate::tools::edit::edit_tool_entry,
            crate::tools::grep::grep_tool_entry,
            crate::tools::run::run_tool_entry,
            crate::tools::glob::glob_tool_entry,
            crate::tools::git::git_status_tool_entry,
            crate::tools::git::git_diff_tool_entry,
            crate::tools::web_search::web_search_tool_entry,
            crate::tools::web_search::web_fetch_tool_entry,
            crate::tools::switch_mode::switch_mode_tool_entry,
            crate::tools::question::question_tool_entry,
        );
    }

    pub fn register_tool(&mut self, tool: Tool) {
        self.tools.push(tool);
    }

    pub fn get_ollama_tools(&self) -> Vec<ToolInfo> {
        self.tools.iter().map(|t| t.tool_info.clone()).collect()
    }

    /// Returns only read-only tools (ls, read, grep, glob) — no write/edit/run.
    /// Also includes switch_mode so planning mode can escalate to agent mode.
    pub fn get_readonly_tools(&self) -> Vec<ToolInfo> {
        let readonly_names = [
            "ls",
            "read",
            "grep",
            "glob",
            "git_status",
            "git_diff",
            "web_search",
            "web_fetch",
            "switch_mode",
            "question",
            "auto_compact",
        ];
        self.tools
            .iter()
            .filter(|t| readonly_names.contains(&t.name()))
            .map(|t| t.tool_info.clone())
            .collect()
    }

    /// Returns research tools (read-only + web search/fetch, no write/edit/run).
    /// Also includes switch_mode so research mode can escalate to agent mode.
    pub fn get_research_tools(&self) -> Vec<ToolInfo> {
        let research_names = [
            "web_search",
            "web_fetch",
            "ls",
            "read",
            "grep",
            "glob",
            "git_status",
            "git_diff",
            "switch_mode",
            "question",
            "auto_compact",
        ];
        self.tools
            .iter()
            .filter(|t| research_names.contains(&t.name()))
            .map(|t| t.tool_info.clone())
            .collect()
    }

    pub async fn execute_tool_call(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> String {
        if let Some(tool) = self.tools.iter().find(|t| t.name() == tool_name) {
            tool::execute_tool_call(tool, arguments).await
        } else {
            format!("Error: Tool '{}' not found", tool_name)
        }
    }
}
