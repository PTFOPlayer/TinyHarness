//! Sockudo AI Transport Worker.
//!
//! Connects to a Sockudo server via WebSocket (Protocol V2), subscribes to
//! AI channels, listens for `ai-input` events, calls Ollama for inference,
//! and streams responses back to the channel as versioned message mutations
//! (`sockudo:message.create`, `.append`, `.update`) plus `ai-turn-end`.
//!
//! This is the server-side counterpart to `SockudoProvider` in
//! `tinyharness-lib`: the provider publishes `ai-input` and listens for
//! `ai-output`; the worker receives `ai-input` and publishes `ai-output`.

mod auth;
mod ollama;
mod worker;

pub use auth::AuthCredentials;
pub use worker::{SockudoWorker, WorkerConfig};
