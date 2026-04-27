pub mod agent;
pub mod commands;
pub mod context;
pub mod mode;
pub mod provider;
pub mod tools;

use std::{error::Error, sync::Arc};

use clap::Parser;
use tokio::sync::Mutex;

use crate::{
    agent::run_agent_loop,
    commands::CommandDispatcher,
    mode::AgentMode,
    provider::{Provider, llama_cpp::LlamaCppProvider, ollama::OllamaProvider},
    tools::{
        ToolManager, edit::edit_tool_entry, glob::glob_tool_entry, grep::grep_tool_entry,
        ls::ls_tool_entry, read::read_tool_entry, run::run_tool_entry, write::write_tool_entry,
    },
};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const BLUE: &str = "\x1b[34m";
const ORANGE: &str = "\x1b[38;5;208m";

#[derive(clap::Parser, Debug)]
struct Args {
    #[arg(short, long)]
    ollama: bool,
    #[arg(short, long)]
    llama_cpp: bool,
    #[arg(short, long, default_value_t = String::new())]
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let default_url = if args.llama_cpp {
        "http://127.0.0.1:8080"
    } else {
        "http://127.0.0.1:11434"
    };

    let url = if args.url.is_empty() {
        default_url.to_string()
    } else {
        args.url
    };

    let provider: Arc<Mutex<dyn Provider + Send + Sync>> = if args.llama_cpp {
        let llama = LlamaCppProvider::new(url);
        if let Err(e) = llama.health_check().await {
            eprintln!("{}Error:{} LlamaCpp health check failed: {}", BOLD, RESET, e);
            std::process::exit(1);
        }
        Arc::new(Mutex::new(llama))
    } else {
        Arc::new(Mutex::new(OllamaProvider::new(url)))
    };

    {
        let mut provider = provider.lock().await;
        provider.select_model(String::from("gemma4:31b-cloud"));
    }

    let mut tool_manager = ToolManager::new();
    tool_manager.register_tool(ls_tool_entry());
    tool_manager.register_tool(read_tool_entry());
    tool_manager.register_tool(write_tool_entry());
    tool_manager.register_tool(edit_tool_entry());
    tool_manager.register_tool(grep_tool_entry());
    tool_manager.register_tool(run_tool_entry());
    tool_manager.register_tool(glob_tool_entry());

    let ollama_tools = tool_manager.get_ollama_tools();

    // Collect workspace context and build the system prompt with it
    let workspace_ctx = context::WorkspaceContext::collect();
    let system_prompt = format!(
        "{}\n\n---\n{}",
        AgentMode::Casual.system_prompt(),
        workspace_ctx.format()
    );

    let mut messages = vec![crate::provider::Message {
        role: crate::provider::Role::System,
        content: system_prompt,
        tool_calls: vec![],
    }];

    let mut dispatcher = CommandDispatcher::new(Arc::clone(&provider), workspace_ctx);

    run_agent_loop(provider, tool_manager, ollama_tools, &mut messages, &mut dispatcher).await
}
