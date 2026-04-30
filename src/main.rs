pub mod agent;
pub mod commands;
pub mod config;
pub mod context;
pub mod mode;
pub mod provider;
pub mod style;
pub mod tools;

use std::{error::Error, sync::Arc};

use crate::{
    agent::run_agent_loop,
    commands::CommandDispatcher,
    config::{ProviderKind, Settings},
    provider::{Provider, llama_cpp::LlamaCppProvider, ollama::OllamaProvider, vllm::VllmProvider},
    tools::{
        ToolManager, edit::edit_tool_entry, glob::glob_tool_entry, grep::grep_tool_entry,
        ls::ls_tool_entry, read::read_tool_entry, run::run_tool_entry,
        web_search::{web_fetch_tool_entry, web_search_tool_entry},
        write::write_tool_entry,
    },
};
use clap::Parser;
use style::*;
use tokio::sync::Mutex;

#[derive(clap::Parser, Debug)]
struct Args {
    #[arg(short, long)]
    ollama: bool,
    #[arg(short, long)]
    llama_cpp: bool,
    #[arg(short, long)]
    vllm: bool,
    #[arg(short, long, default_value_t = String::new())]
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    // Load saved settings (will be used as defaults when no CLI flags are given)
    let settings = Settings::load();

    // Determine which provider to use: CLI flags override saved settings
    let has_cli_flag = args.ollama || args.llama_cpp || args.vllm;
    let provider_kind = if has_cli_flag {
        if args.llama_cpp {
            ProviderKind::LlamaCpp
        } else if args.vllm {
            ProviderKind::Vllm
        } else {
            ProviderKind::Ollama
        }
    } else {
        settings.last_provider
    };

    let default_url = match provider_kind {
        ProviderKind::LlamaCpp => "http://127.0.0.1:8080",
        ProviderKind::Vllm => "http://127.0.0.1:8000",
        ProviderKind::Ollama => "http://127.0.0.1:11434",
    };

    let url = if args.url.is_empty() {
        default_url.to_string()
    } else {
        args.url
    };

    let provider: Arc<Mutex<dyn Provider + Send + Sync>> = match provider_kind {
        ProviderKind::LlamaCpp => {
            let llama = LlamaCppProvider::new(url);
            if let Err(e) = llama.health_check().await {
                eprintln!(
                    "{}Error:{} LlamaCpp health check failed: {}",
                    BOLD, RESET, e
                );
                std::process::exit(1);
            }
            Arc::new(Mutex::new(llama))
        }
        ProviderKind::Vllm => {
            let vllm = VllmProvider::new(url);
            if let Err(e) = vllm.health_check().await {
                eprintln!("{}Error:{} vLLM health check failed: {}", BOLD, RESET, e);
                std::process::exit(1);
            }
            Arc::new(Mutex::new(vllm))
        }
        ProviderKind::Ollama => {
            Arc::new(Mutex::new(OllamaProvider::new(url)))
        }
    };

    // Apply saved model if no CLI flag changed the provider (model is provider-specific)
    {
        let mut provider = provider.lock().await;
        if provider.current_model().is_none() {
            // Try saved model first
            if let Some(ref saved_model) = settings.last_model {
                let models = provider.list_models().await;
                if models.iter().any(|m| m == saved_model) {
                    provider.select_model(saved_model.clone());
                } else {
                    // Saved model not available — pick first available
                    if let Some(first) = models.first() {
                        eprintln!(
                            "{}Warning:{} Saved model '{}' not available. Picked first: {}{}{}",
                            BOLD, RESET, saved_model, BLUE, first, RESET
                        );
                        provider.select_model(first.clone());
                    } else {
                        eprintln!(
                            "{}Error:{} No models available. Use /model <name> to set one manually.",
                            BOLD, RESET
                        );
                    }
                }
            } else {
                let models = provider.list_models().await;
                if let Some(first) = models.first() {
                    eprintln!(
                        "{}Warning:{} No model selected. Automatically picked first available model: {}{}{}",
                        BOLD, RESET, BLUE, first, RESET
                    );
                    provider.select_model(first.clone());
                } else {
                    eprintln!(
                        "{}Error:{} No models available. Use /model <name> to set one manually.",
                        BOLD, RESET
                    );
                }
            }
        }
    }

    // Save the provider kind now that we know which one is active
    let mut settings = settings;
    if settings.last_provider != provider_kind {
        settings.last_provider = provider_kind;
        settings.save();
    }

    let mut tool_manager = ToolManager::new();
    tool_manager.register_tool(ls_tool_entry());
    tool_manager.register_tool(read_tool_entry());
    tool_manager.register_tool(write_tool_entry());
    tool_manager.register_tool(edit_tool_entry());
    tool_manager.register_tool(grep_tool_entry());
    tool_manager.register_tool(run_tool_entry());
    tool_manager.register_tool(glob_tool_entry());
    tool_manager.register_tool(web_search_tool_entry());
    tool_manager.register_tool(web_fetch_tool_entry());

    let ollama_tools = tool_manager.get_ollama_tools();

    // Collect workspace context and build the system prompt with the saved mode
    let workspace_ctx = context::WorkspaceContext::collect();
    let initial_mode = settings.preferred_mode;
    let system_prompt = format!(
        "{}\n\n---\n{}",
        initial_mode.system_prompt(),
        workspace_ctx.format()
    );

    let mut messages = vec![crate::provider::Message {
        role: crate::provider::Role::System,
        content: system_prompt,
        tool_calls: vec![],
    }];

    let mut dispatcher = CommandDispatcher::new(Arc::clone(&provider), workspace_ctx);
    dispatcher.current_mode = initial_mode;

    run_agent_loop(
        provider,
        tool_manager,
        ollama_tools,
        &mut messages,
        &mut dispatcher,
    )
    .await
}
