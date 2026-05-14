use std::collections::HashMap;

use crate::tools::tool::{BoxFuture, ToolCategory, build_string_params_schema, make_tool};

pub fn auto_compact_tool_entry() -> crate::tools::tool::Tool {
    make_tool(
        "auto_compact",
        "Compact the conversation history by summarizing older messages. Use this when the conversation is getting long and you need to free up context space. The recent messages will be preserved, while older messages will be summarized into a concise summary.",
        ToolCategory::Signal,
        build_string_params_schema(
            &[],
            &[(
                "focus",
                "Specific topics, decisions, or details to preserve in the summary",
                "",
            )],
        ),
        |args| Box::pin(auto_compact_tool(args)),
    )
}

pub fn auto_compact_tool(args: HashMap<String, String>) -> BoxFuture<'static, String> {
    Box::pin(async move {
        let focus = args.get("focus").cloned().unwrap_or_default();

        // Signal to the agent that compaction is requested
        // The actual compaction will be handled specially in agent.rs
        // (similar to how switch_mode and question tools are handled)
        if focus.is_empty() {
            "AUTO_COMPACT_REQUESTED: The model has requested to compact the conversation history. No specific focus provided.".to_string()
        } else {
            format!(
                "AUTO_COMPACT_REQUESTED: The model has requested to compact the conversation history with focus on: {}",
                focus
            )
        }
    })
}
