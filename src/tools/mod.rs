pub mod edit;
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

pub struct ToolManager {
    tools: Vec<Tool>,
}

impl ToolManager {
    pub fn new() -> Self {
        ToolManager { tools: vec![] }
    }

    /// Register all built-in tools.
    pub fn register_defaults(&mut self) {
        use edit::edit_tool_entry;
        use glob::glob_tool_entry;
        use grep::grep_tool_entry;
        use ls::ls_tool_entry;
        use question::question_tool_entry;
        use read::read_tool_entry;
        use run::run_tool_entry;
        use switch_mode::switch_mode_tool_entry;
        use web_search::{web_fetch_tool_entry, web_search_tool_entry};
        use write::write_tool_entry;

        self.register_tool(ls_tool_entry());
        self.register_tool(read_tool_entry());
        self.register_tool(write_tool_entry());
        self.register_tool(edit_tool_entry());
        self.register_tool(grep_tool_entry());
        self.register_tool(run_tool_entry());
        self.register_tool(glob_tool_entry());
        self.register_tool(web_search_tool_entry());
        self.register_tool(web_fetch_tool_entry());
        self.register_tool(switch_mode_tool_entry());
        self.register_tool(question_tool_entry());
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
            "web_search",
            "web_fetch",
            "switch_mode",
            "question",
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
            "switch_mode",
            "question",
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
