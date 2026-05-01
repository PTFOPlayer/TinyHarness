pub mod agent;
pub mod commands;
pub mod config;
pub mod context;
pub mod mode;
pub mod provider;
pub mod session;
pub mod style;
pub mod tools;
pub mod ui;

use std::{error::Error, sync::Arc};

use crate::{
    agent::run_agent_loop,
    commands::CommandDispatcher,
    config::{ProviderKind, Settings},
    provider::{Provider, llama_cpp::LlamaCppProvider, ollama::OllamaProvider, vllm::VllmProvider},
    session::Session,
    tools::ToolManager,
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
    /// Continue the most recent session in the current directory.
    #[arg(short, long)]
    r#continue: bool,
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
    tool_manager.register_defaults();

    let ollama_tools = tool_manager.get_ollama_tools();

    // Collect workspace context and build the system prompt with the saved mode
    let workspace_ctx = context::WorkspaceContext::collect();
    let initial_mode = settings.preferred_mode;

    // Determine the provider string for session metadata
    let provider_str = match provider_kind {
        ProviderKind::Ollama => "ollama",
        ProviderKind::LlamaCpp => "llama.cpp",
        ProviderKind::Vllm => "vllm",
    };

    // Resolve the current model name
    let current_model = {
        let p = provider.lock().await;
        p.current_model()
    };

    // ── Session persistence ───────────────────────────────────────────────
    let working_dir = std::env::current_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
        .to_string_lossy()
        .to_string();

    let (mut session, mut messages) = if args.r#continue {
        // Try to find and resume the most recent session for this directory
        match Session::find_latest_for_dir(&working_dir) {
            Some(session_id) => match Session::load(&session_id) {
                Ok((sess, loaded_msgs)) => {
                    let meta = sess.meta();
                    eprintln!(
                        "{}Resumed session {}{}{} ({} messages, {})",
                        BOLD, BLUE, &meta.id[..12], RESET,
                        meta.message_count, meta.mode
                    );
                    (sess, loaded_msgs)
                }
                Err(e) => {
                    eprintln!(
                        "{}Warning:{} Failed to resume session: {}. Starting fresh.",
                        BOLD, RESET, e
                    );
                    let sess = Session::new(
                        &working_dir,
                        initial_mode,
                        provider_str,
                        current_model.clone(),
                    );
                    let system_prompt = format!(
                        "{}\n\n---\n{}",
                        initial_mode.system_prompt(),
                        workspace_ctx.format()
                    );
                    let msgs = vec![crate::provider::Message {
                        role: crate::provider::Role::System,
                        content: system_prompt,
                        tool_calls: vec![],
                    }];
                    (sess, msgs)
                }
            },
            None => {
                eprintln!(
                    "{}No previous session found in this directory. Starting fresh.{}",
                    ORANGE, RESET
                );
                let sess = Session::new(
                    &working_dir,
                    initial_mode,
                    provider_str,
                    current_model.clone(),
                );
                let system_prompt = format!(
                    "{}\n\n---\n{}",
                    initial_mode.system_prompt(),
                    workspace_ctx.format()
                );
                let msgs = vec![crate::provider::Message {
                    role: crate::provider::Role::System,
                    content: system_prompt,
                    tool_calls: vec![],
                }];
                (sess, msgs)
            }
        }
    } else {
        // Start a new session
        let sess = Session::new(
            &working_dir,
            initial_mode,
            provider_str,
            current_model.clone(),
        );
        let system_prompt = format!(
            "{}\n\n---\n{}",
            initial_mode.system_prompt(),
            workspace_ctx.format()
        );
        let msgs = vec![crate::provider::Message {
            role: crate::provider::Role::System,
            content: system_prompt,
            tool_calls: vec![],
        }];
        (sess, msgs)
    };

    let mut dispatcher = CommandDispatcher::new(Arc::clone(&provider), workspace_ctx);
    dispatcher.current_mode = initial_mode;
    dispatcher.session_id = Some(session.id().to_string());

    run_agent_loop(
        provider,
        tool_manager,
        ollama_tools,
        &mut messages,
        &mut dispatcher,
        &mut session,
    )
    .await
}
