//! Sockudo AI Transport Worker — binary entry point.
//!
//! Connects to a Sockudo server, listens for `ai-input` events on AI
//! channels, calls Ollama for inference, and streams responses back as
//! versioned message mutations.

use clap::Parser;
use sockudo_worker::{AuthCredentials, SockudoWorker, WorkerConfig};
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(
    version,
    about = "Sockudo AI Transport worker — bridges ai-input to Ollama"
)]
struct Args {
    /// Sockudo server URL (e.g. http://127.0.0.1:6001)
    #[arg(long, env = "SOCKUDO_URL", default_value = "http://127.0.0.1:6001")]
    sockudo_url: String,

    /// Sockudo app ID
    #[arg(long, env = "SOCKUDO_APP_ID", default_value = "test-app")]
    app_id: String,

    /// Sockudo app key
    #[arg(long, env = "SOCKUDO_APP_KEY", default_value = "test-key")]
    app_key: String,

    /// Sockudo app secret
    #[arg(long, env = "SOCKUDO_APP_SECRET", default_value = "test-secret")]
    app_secret: String,

    /// Ollama server URL (e.g. http://127.0.0.1:11434)
    #[arg(long, env = "OLLAMA_URL", default_value = "http://127.0.0.1:11434")]
    ollama_url: String,

    /// Default model to use if ai-input doesn't specify one
    #[arg(long, env = "SOCKUDO_WORKER_MODEL", default_value = "qwen2.5:0.5b")]
    model: String,

    /// Channel name to subscribe to for ai-input events
    #[arg(long, env = "SOCKUDO_CHANNEL", default_value = "ai-output")]
    channel: String,

    /// WebSocket read timeout in seconds
    #[arg(long, env = "SOCKUDO_WS_TIMEOUT", default_value_t = 120)]
    ws_timeout: u64,

    /// Disable streaming (batch mode — wait for full Ollama response before publishing)
    #[arg(long, env = "SOCKUDO_WORKER_NO_STREAM")]
    no_stream: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();

    let creds = AuthCredentials::new(&args.app_id, &args.app_key, &args.app_secret);

    let config = WorkerConfig {
        sockudo_url: args.sockudo_url.clone(),
        creds,
        ollama_url: args.ollama_url.clone(),
        default_model: args.model.clone(),
        channel: args.channel.clone(),
        ws_timeout_secs: args.ws_timeout,
        stream: !args.no_stream,
    };

    tracing::info!("Starting Sockudo AI Transport worker");
    tracing::info!("  Sockudo:   {}", config.sockudo_url);
    tracing::info!("  Ollama:    {}", config.ollama_url);
    tracing::info!("  Model:     {}", config.default_model);
    tracing::info!("  Channel:   {}", config.channel);
    tracing::info!("  Streaming: {}", config.stream);

    let worker = SockudoWorker::new(config);
    worker.run().await?;

    Ok(())
}
