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
    provider::{Message, Provider, Role, llama_cpp::LlamaCppProvider, ollama::OllamaProvider, vllm::VllmProvider},
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

/// Return the default URL for a given provider kind.
fn default_url_for_provider(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::LlamaCpp => "http://127.0.0.1:8080",
        ProviderKind::Vllm => "http://127.0.0.1:8000",
        ProviderKind::Ollama => "http://127.0.0.1:11434",
    }
}

/// Determine the provider kind from CLI flags or saved settings.
fn resolve_provider_kind(args: &Args, settings: &Settings) -> ProviderKind {
    let has_cli_flag = args.ollama || args.llama_cpp || args.vllm;
    if has_cli_flag {
        if args.llama_cpp {
            ProviderKind::LlamaCpp
        } else if args.vllm {
            ProviderKind::Vllm
        } else {
            ProviderKind::Ollama
        }
    } else {
        settings.last_provider
    }
}

/// Create the provider backend, run health checks for non-Ollama providers,
/// and return it wrapped in Arc<Mutex>.
async fn create_provider(kind: ProviderKind, url: String) -> Arc<Mutex<dyn Provider + Send + Sync>> {
    match kind {
        ProviderKind::LlamaCpp => {
            let llama = LlamaCppProvider::new(url);
            if let Err(e) = llama.health_check().await {
                eprintln!("{}Error:{} LlamaCpp health check failed: {}", BOLD, RESET, e);
                std::process::exit(1);
            }
            Arc::new(Mutex::new(llama)) as Arc<Mutex<dyn Provider + Send + Sync>>
        }
        ProviderKind::Vllm => {
            let vllm = VllmProvider::new(url);
            if let Err(e) = vllm.health_check().await {
                eprintln!("{}Error:{} vLLM health check failed: {}", BOLD, RESET, e);
                std::process::exit(1);
            }
            Arc::new(Mutex::new(vllm)) as Arc<Mutex<dyn Provider + Send + Sync>>
        }
        ProviderKind::Ollama => {
            Arc::new(Mutex::new(OllamaProvider::new(url))) as Arc<Mutex<dyn Provider + Send + Sync>>
        }
    }
}

/// Auto-select a model on the provider if none is currently set.
/// Tries the saved model first, then falls back to the first available model.
async fn auto_select_model(provider: &mut dyn Provider, saved_model: Option<&String>) {
    if provider.current_model().is_some() {
        return;
    }

    let models = provider.list_models().await;

    if let Some(saved) = saved_model {
        if models.iter().any(|m| m == saved) {
            provider.select_model(saved.clone());
            return;
        }
        // Saved model not available — warn and fall through
        if let Some(first) = models.first() {
            eprintln!(
                "{}Warning:{} Saved model '{}' not available. Picked first: {}{}{}",
                BOLD, RESET, saved, BLUE, first, RESET
            );
            provider.select_model(first.clone());
        } else {
            eprintln!(
                "{}Error:{} No models available. Use /model <name> to set one manually.",
                BOLD, RESET
            );
        }
        return;
    }

    // No saved model — pick first available
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

/// Create a brand-new session with an initial system prompt message.
fn create_initial_session(
    working_dir: &str,
    initial_mode: crate::mode::AgentMode,
    provider_str: &str,
    current_model: Option<String>,
    workspace_ctx: &crate::context::WorkspaceContext,
) -> (Session, Vec<Message>) {
    let sess = Session::new(working_dir, initial_mode, provider_str, current_model);
    let system_prompt = format!(
        "{}\n\n---\n{}",
        initial_mode.system_prompt(),
        workspace_ctx.format()
    );
    let msgs = vec![Message {
        role: Role::System,
        content: system_prompt,
        tool_calls: vec![],
    }];
    (sess, msgs)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    // Load saved settings (will be used as defaults when no CLI flags are given)
    let settings = Settings::load();

    // Determine which provider to use: CLI flags override saved settings
    let provider_kind = resolve_provider_kind(&args, &settings);
    let url = if args.url.is_empty() {
        default_url_for_provider(provider_kind).to_string()
    } else {
        args.url.clone()
    };

    let provider = create_provider(provider_kind, url).await;

    // Auto-select model if none is currently set
    {
        let mut p = provider.lock().await;
        auto_select_model(&mut *p, settings.last_model.as_ref()).await;
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

    let provider_str = provider_kind.to_string();

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
        match Session::find_latest_for_dir(&working_dir) {
            Some(session_id) => match Session::load(&session_id) {
                Ok((sess, loaded_msgs)) => {
                    let meta = sess.meta();
                    let name = meta.name.as_deref().unwrap_or("unnamed");
                    eprintln!(
                        "{}Resumed session {}{}{} — {}{}{} ({} messages, {})",
                        BOLD, BLUE, &meta.id[..12], RESET,
                        BOLD, name, RESET,
                        meta.message_count, meta.mode
                    );
                    (sess, loaded_msgs)
                }
                Err(e) => {
                    eprintln!(
                        "{}Warning:{} Failed to resume session: {}. Starting fresh.",
                        BOLD, RESET, e
                    );
                    create_initial_session(&working_dir, initial_mode, &provider_str, current_model.clone(), &workspace_ctx)
                }
            },
            None => {
                eprintln!(
                    "{}No previous session found in this directory. Starting fresh.{}",
                    ORANGE, RESET
                );
                create_initial_session(&working_dir, initial_mode, &provider_str, current_model.clone(), &workspace_ctx)
            }
        }
    } else {
        create_initial_session(&working_dir, initial_mode, &provider_str, current_model.clone(), &workspace_ctx)
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
