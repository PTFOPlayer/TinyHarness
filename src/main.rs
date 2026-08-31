pub mod agent;
pub mod commands;
#[cfg(test)]
pub mod test_helpers;

use std::{
    error::Error,
    io::Write,
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
};

use tinyharness_lib::{
    SecretString,
    config::{ProviderKind, Settings, ensure_prompts_initialized, load_settings, save_settings},
    context::WorkspaceContext,
    custom_tools::CustomToolManager,
    mode::AgentMode,
    provider::{
        Message, Provider, Role, ollama::OllamaProvider,
        openai_compat_provider::OpenAiCompatProvider, sockudo::SockudoProvider,
    },
    session::{Session, SessionStore},
    tools::ToolManager,
};

use crate::agent::setup as agent_setup;
use crate::{agent::run_agent_loop, commands::CommandContext};
use clap::Parser;
use tinyharness_ui::output::Output;
use tinyharness_ui::style::*;
use tokio::sync::Mutex;

#[derive(clap::Parser, Debug)]
#[command(version, about = "tinyharness - ai coding harness")]
struct Args {
    /// Use the Ollama provider (local LLM inference server).
    #[arg(short, long)]
    ollama: bool,

    /// Use the llama.cpp provider (llama-server HTTP API).
    #[arg(short, long)]
    llama_cpp: bool,

    /// Use the vLLM provider (OpenAI-compatible API server).
    #[arg(short, long)]
    vllm: bool,

    /// Use the generic OpenAI-compatible provider for hosted gateways
    /// (OpenRouter, Together, custom proxies, etc.) that require a Bearer
    /// API key. Requires `--api-key` or the `OPENAI_API_KEY` env var, and
    /// `--url` to specify the gateway endpoint.
    #[arg(long)]
    openai_compat: bool,

    /// Use the Sockudo provider (AI Transport via Sockudo WebSocket server).
    #[arg(long)]
    sockudo: bool,

    /// Provider server URL (e.g. http://127.0.0.1:11434 for Ollama).
    /// Overrides the saved setting. Use `-u ""` to reset to the provider default.
    #[arg(short, long, default_value_t = String::new())]
    url: String,

    /// Bearer token for the `--openai-compat` provider. Sent as
    /// `Authorization: Bearer <key>` on every request. Overrides the saved
    /// setting and the `OPENAI_API_KEY` env var. Use `--api-key -` to clear
    /// the saved key, or `--api-key ""` to explicitly disable auth (no
    /// `Authorization` header sent — useful for local OpenAI-compatible
    /// servers behind no auth). Has no effect on Ollama, llama.cpp, vLLM, or
    /// Sockudo.
    #[arg(long)]
    api_key: Option<String>,

    /// Skip the provider health check at startup. Useful when the server
    /// requires a separate scope on `/health`, doesn't expose one, or you
    /// want the agent to start fast and surface errors on the first request.
    #[arg(long)]
    skip_health_check: bool,

    /// Continue the most recent session in the current directory.
    #[arg(short, long)]
    r#continue: bool,

    /// Run interactive provider setup: pick a provider, enter a URL, save to
    /// settings. Exits when done.
    #[arg(long)]
    config: bool,

    /// Start the conversation with this prompt instead of waiting for input.
    /// Use `-p` for short flags. The agent then drops into the normal
    /// interactive loop for follow-up turns.
    #[arg(short = 'p', long = "prompt")]
    prompt: Option<String>,
}

/// Determine the provider kind from CLI flags or saved settings.
fn resolve_provider_kind(args: &Args, settings: &Settings) -> ProviderKind {
    if args.llama_cpp {
        ProviderKind::LlamaCpp
    } else if args.vllm {
        ProviderKind::Vllm
    } else if args.openai_compat {
        ProviderKind::OpenAiCompat
    } else if args.sockudo {
        ProviderKind::Sockudo
    } else if args.ollama {
        ProviderKind::Ollama
    } else {
        settings.last_provider
    }
}

/// Create the provider backend, run health checks, and return it wrapped in `Arc<Mutex>`.
#[allow(clippy::too_many_arguments)]
async fn create_provider(
    kind: ProviderKind,
    url: String,
    api_key: Option<SecretString>,
    skip_health_check: bool,
    skip_health_check_source: &str,
    settings: &Settings,
) -> Arc<Mutex<dyn Provider + Send + Sync>> {
    let provider: Arc<Mutex<dyn Provider + Send + Sync>> = match kind {
        ProviderKind::LlamaCpp => Arc::new(Mutex::new(
            OpenAiCompatProvider::new(url).with_static_models(vec!["llama-cpp".to_string()]),
        )),
        ProviderKind::Vllm => Arc::new(Mutex::new(OpenAiCompatProvider::new(url))),
        ProviderKind::OpenAiCompat => match api_key {
            Some(key) if !key.is_empty() => {
                Arc::new(Mutex::new(OpenAiCompatProvider::with_api_key(url, key)))
            }
            // Explicit empty `--api-key ""` opts out of auth entirely.
            Some(_) => Arc::new(Mutex::new(OpenAiCompatProvider::new(url))),
            None => {
                let mut err_out = Output::stderr();
                let _ = writeln!(
                    err_out,
                    "{BOLD}Error:{RESET} --openai-compat requires an API key. \
                     Pass {CYAN}--api-key <KEY>{RESET}, set the {CYAN}OPENAI_API_KEY{RESET} \
                     env var, or configure it via {CYAN}--config{RESET}. \
                     To use the gateway without auth, pass {CYAN}--api-key \"\"{RESET}.",
                );
                std::process::exit(1);
            }
        },
        ProviderKind::Ollama => {
            let provider = OllamaProvider::new(
                url,
                settings.ollama_timeout_secs,
                settings.ollama_max_retries,
                settings.ollama_think_type,
            )
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                std::process::exit(1);
            });
            Arc::new(Mutex::new(provider))
        }
        ProviderKind::Sockudo => {
            let app_id = settings.sockudo_app_id.clone().unwrap_or_default();
            let app_key = settings.sockudo_app_key.clone().unwrap_or_default();
            let app_secret = settings.sockudo_app_secret.clone().unwrap_or_default();
            Arc::new(Mutex::new(SockudoProvider::new(
                url, app_id, app_key, app_secret,
            )))
        }
    };

    // Run health check for all providers (Ollama included)
    // Skipped when the CLI flag or settings flag is set.
    if !skip_health_check {
        let p = provider.lock().await;
        if let Err(e) = p.health_check().await {
            // Non-fatal: warn and let the agent start anyway. Errors will
            // surface on the first request.
            let mut err_out = Output::stderr();
            let _ = writeln!(
                err_out,
                "{ORANGE}Warning:{RESET} {kind} health check failed: {e}",
            );
            let _ = writeln!(
                err_out,
                "Continuing anyway — errors will surface on the first request. \
                 Use {CYAN}--skip-health-check{RESET} to suppress this warning.",
            );
        }
    } else {
        let mut err_out = Output::stderr();
        let _ = writeln!(
            err_out,
            "{BOLD}{kind}:{RESET} Skipping health check ({skip_health_check_source}).",
        );
    }

    provider
}

/// Auto-select a model on the provider if none is currently set.
/// Tries the saved model first, then falls back to the first available model.
async fn auto_select_model(provider: &mut dyn Provider, saved_model: Option<&String>) {
    if provider.current_model().is_some() {
        return;
    }

    // Query the server's model list once. An empty list means either the
    // backend doesn't expose `/v1/models` (e.g. raw llama.cpp) or the call
    // failed; in that case we can't validate anything and must trust the
    // saved name.
    let models = provider.list_models().await;

    if let Some(saved) = saved_model {
        // If the server *did* return a model list, validate against it so
        // we don't silently keep using a stale name carried over from a
        // different provider/endpoint. A mismatch produces a visible
        // warning and we fall back to the first available model.
        if !models.is_empty() && !models.iter().any(|m| m == saved) {
            let mut err_out = Output::stderr();
            if let Some(first) = models.first() {
                let _ = writeln!(
                    err_out,
                    "{BOLD}Warning:{RESET} Saved model {BLUE}{saved}{RESET} is not available on \
                     this server. Switching to {BLUE}{first}{RESET}. Use {CYAN}/model <name>{RESET} \
                     to pick a different one.",
                );
                provider.select_model(first.clone());
                return;
            }
        }

        let mut err_out = Output::stderr();
        let _ = writeln!(
            err_out,
            "{BOLD}Using saved model:{RESET} {BLUE}{saved}{RESET}",
        );
        provider.select_model(saved.clone());
        return;
    }

    // No saved model — pick first available
    if let Some(first) = models.first() {
        let mut err_out = Output::stderr();
        let _ = writeln!(
            err_out,
            "{BOLD}Warning:{RESET} No model selected. Automatically picked first available model: {BLUE}{first}{RESET}",
        );
        provider.select_model(first.clone());
    } else {
        let mut err_out = Output::stderr();
        let _ = writeln!(
            err_out,
            "{BOLD}Error:{RESET} No models available. Use /model <name> to set one manually.",
        );
    }
}

/// Create a brand-new session with an initial system prompt message.
fn create_initial_session(
    working_dir: &str,
    initial_mode: AgentMode,
    provider_str: &str,
    current_model: Option<String>,
    workspace_ctx: &WorkspaceContext,
    prompts_dir: &std::path::Path,
) -> (Session, Vec<Message>) {
    let sess =
        SessionStore::default_path().create(working_dir, initial_mode, provider_str, current_model);
    let system_prompt = format!(
        "{}\n\n---\n{}",
        initial_mode.load_system_prompt(prompts_dir),
        workspace_ctx.format()
    );
    let msgs = vec![Message {
        role: Role::System,
        content: system_prompt,
        tool_calls: vec![],
        tool_call_id: None,
        images: vec![],
        thinking: None,
    }];
    (sess, msgs)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Initialize tracing: library code uses tracing::warn!/error! instead of
    // direct eprintln!, so diagnostics are routed through the subscriber.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();

    // Install Ctrl+C handler: set an atomic flag that the agent loop checks
    // during streaming generation. This allows interrupting LLM responses
    // without terminating the process.
    let interrupted = Arc::new(AtomicBool::new(false));
    // First Ctrl+C sets a flag that the agent loop checks during streaming.
    // A second Ctrl+C exits the process immediately so the user is never stuck.
    ctrlc::set_handler({
        let interrupted = Arc::clone(&interrupted);
        move || {
            if interrupted.load(Ordering::SeqCst) {
                // Already interrupted — user wants out now
                std::process::exit(130);
            }
            interrupted.store(true, Ordering::SeqCst);
        }
    })?;

    let args = Args::parse();

    // -- Handle --config: interactive provider setup, then exit ───────────────
    if args.config {
        let mut out = Output::stdout();
        let result = agent_setup::interactive_setup(&mut out);
        match result {
            Ok(_) => return Ok(()),
            Err(e) => {
                let mut err_out = Output::stderr();
                let _ = writeln!(err_out, "{BOLD}Error:{RESET} {e}");
                std::process::exit(1);
            }
        }
    }

    // Load saved settings (will be used as defaults when no CLI flags are given)
    let settings = load_settings();

    // Determine which provider to use: CLI flags override saved settings
    let provider_kind = resolve_provider_kind(&args, &settings);

    // Resolve URL: CLI > saved > default. If a provider flag was passed without
    // --url, prompt interactively (requires a TTY) and persist the result.
    let url = if args.url.is_empty() {
        let cli_provider_flag_set =
            args.ollama || args.llama_cpp || args.vllm || args.sockudo || args.openai_compat;
        if cli_provider_flag_set {
            // User explicitly chose a provider without a URL — ask for it
            // interactively so the saved URL stays in sync.
            let default = agent_setup::default_url_for(provider_kind);
            let mut out = Output::stdout();
            let url = match agent_setup::prompt_for_url(&mut out, provider_kind, default) {
                Ok(u) => u,
                Err(e) => {
                    let mut err_out = Output::stderr();
                    let _ = writeln!(err_out, "{BOLD}Error:{RESET} {e}");
                    std::process::exit(1);
                }
            };
            agent_setup::save_provider_settings(provider_kind, &url);
            url
        } else {
            agent_setup::resolve_url(provider_kind, &args.url, &settings)
        }
    } else {
        args.url.clone()
    };

    let api_key = agent_setup::resolve_api_key(args.api_key.as_deref(), &settings);

    // Reload settings to include any API key persisted by resolve_api_key
    let settings = load_settings();

    let skip_hc = args.skip_health_check || settings.skip_health_check;
    let skip_hc_source = if args.skip_health_check {
        "--skip-health-check"
    } else {
        "settings.skip_health_check"
    };
    let provider = create_provider(
        provider_kind,
        url.clone(),
        api_key,
        skip_hc,
        skip_hc_source,
        &settings,
    )
    .await;

    // Auto-select model if none is currently set.
    // Sockudo doesn't use a saved model — the worker selects the backend
    // model, and the actual model name is reported back via WebSocket
    // extras during streaming. Using a saved model from a different
    // provider (e.g. Ollama) would be incorrect.
    {
        let mut p = provider.lock().await;
        if provider_kind == ProviderKind::Sockudo {
            // For Sockudo, the model is determined by the worker. If the
            // user has already selected one via /model, keep it; otherwise
            // the worker's default will be used and the name will be
            // discovered from the first response.
            if p.current_model().is_none() {
                let mut err_out = Output::stderr();
                let _ = writeln!(
                    err_out,
                    "{BOLD}Sockudo:{RESET} No model selected. The worker will use its default model. Use {CYAN}/model <name>{RESET} to set one manually.",
                );
            }
        } else {
            auto_select_model(
                &mut *p,
                settings
                    .get_model_for(provider_kind)
                    .map(|s| s.to_string())
                    .as_ref(),
            )
            .await;
        }
    }

    // Save the provider kind + URL now that we know which one is active.
    // We persist whenever anything was explicitly chosen via CLI (provider
    // flag or --url) so the next run doesn't have to re-prompt. The
    // URL-resolution block above already calls `save_provider_settings` when
    // a provider flag was passed without --url, so we only need to cover
    // the remaining cases here.
    let mut settings = settings;
    let explicit_provider =
        args.ollama || args.llama_cpp || args.vllm || args.sockudo || args.openai_compat;
    if explicit_provider && !args.url.is_empty() {
        // User gave both --ollama/--llama-cpp/--vllm and --url. Persist both.
        if settings.last_provider != provider_kind {
            settings.last_provider = provider_kind;
        }
        settings.set_url_for(provider_kind, url.clone());
        save_settings(&settings);
    } else if !explicit_provider && !args.url.is_empty() {
        // User gave --url only (provider resolved from saved settings).
        settings.set_url_for(provider_kind, url.clone());
        save_settings(&settings);
    }
    // else: no CLI override — leave settings as they are.

    // ── Collect workspace context ──────────────────────────────────────────
    let workspace_ctx = WorkspaceContext::collect();

    // ── Ensure prompt files exist (seeded from hardcoded defaults on first launch)
    let prompts_dir = ensure_prompts_initialized();

    let mut tool_manager = ToolManager::new();
    tool_manager.register_defaults();

    // Load custom tools (shell-command tools configured via custom_tools.json)
    let custom_tool_manager = CustomToolManager::load();
    tool_manager.register_custom_tools(custom_tool_manager.custom_tools());

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
        let store = SessionStore::default_path();
        match store.find_latest_for_dir(&working_dir) {
            Some(session_id) => match store.load(&session_id) {
                Ok((sess, loaded_msgs)) => {
                    let meta = sess.meta();
                    let name = meta.name.as_deref().unwrap_or("unnamed");
                    let mut err_out = Output::stderr();
                    let _ = writeln!(
                        err_out,
                        "{BOLD}Resumed session {BLUE}{}{RESET} — {BOLD}{name}{RESET} ({} messages, {})",
                        &meta.id[..12],
                        meta.message_count,
                        meta.mode,
                    );
                    Some((sess, loaded_msgs))
                }
                Err(e) => {
                    let mut err_out = Output::stderr();
                    let _ = writeln!(
                        err_out,
                        "{BOLD}Warning:{RESET} Failed to resume session: {e}. Starting fresh.",
                    );
                    None
                }
            },
            None => {
                let mut err_out = Output::stderr();
                let _ = writeln!(
                    err_out,
                    "{ORANGE}No previous session found in this directory. Starting fresh.{RESET}",
                );
                None
            }
        }
    } else {
        None
    }
    .unwrap_or_else(|| {
        create_initial_session(
            &working_dir,
            initial_mode,
            &provider_str,
            current_model.clone(),
            &workspace_ctx,
            &prompts_dir,
        )
    });

    let mut ctx = CommandContext::new(Arc::clone(&provider), workspace_ctx, prompts_dir);
    ctx.current_mode = initial_mode;
    ctx.show_thinking = settings.show_thinking;
    ctx.session_id = Some(session.id().to_string());

    // ── CLI mode ──────────────────────────────────────────────────────────
    run_agent_loop(
        provider,
        tool_manager,
        &mut messages,
        &mut ctx,
        &mut session,
        &interrupted,
        args.prompt.as_deref(),
    )
    .await
}
