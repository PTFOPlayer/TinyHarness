//! Ollama streaming chat client.
//!
//! Calls the Ollama `/api/chat` endpoint with `stream: true` and yields
//! content chunks as they arrive. This is a lightweight implementation that
//! avoids depending on `ollama-rs` to keep the worker crate self-contained.

use std::time::Duration;

use futures_util::StreamExt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::warn;

/// A streamed chunk from Ollama.
#[derive(Debug, Clone, Deserialize)]
pub struct OllamaChunk {
    #[serde(default)]
    pub message: OllamaChunkMessage,
    #[serde(default)]
    pub done: bool,
    // Native Ollama usage fields (final chunk)
    #[serde(default)]
    #[allow(dead_code)]
    pub prompt_eval_count: Option<u64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub eval_count: Option<u64>,
    // OpenAI-compatible nested usage (cloud proxies)
    #[serde(default)]
    #[allow(dead_code)]
    pub usage: Option<OllamaUsage>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct OllamaChunkMessage {
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct OllamaUsage {
    #[serde(default)]
    pub prompt_tokens: u32,
    #[serde(default)]
    pub completion_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
}

/// Request body for Ollama `/api/chat`.
#[derive(Debug, Serialize)]
pub struct OllamaChatRequest {
    pub model: String,
    pub messages: Vec<OllamaChatMessage>,
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct OllamaChatMessage {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<serde_json::Value>,
}

/// Result of a streaming Ollama chat call.
pub struct StreamResult {
    /// Receiver for content chunks (text deltas).
    pub chunks: mpsc::Receiver<String>,
    /// Full response info (available after stream completes).
    pub full_text: tokio::sync::oneshot::Receiver<OllamaResponse>,
}

/// Full response from Ollama, available after streaming completes.
#[derive(Debug, Clone, Default)]
pub struct OllamaResponse {
    /// Accumulated text content.
    pub text: String,
    /// Tool calls from the final chunk(s) (if any).
    pub tool_calls: Vec<serde_json::Value>,
}

/// Stream a chat completion from Ollama.
///
/// Spawns a background task that reads the NDJSON stream and sends
/// content deltas through the returned channel. The full accumulated text
/// is sent through the oneshot receiver when the stream completes.
pub async fn stream_chat(
    client: &Client,
    base_url: &str,
    request: &OllamaChatRequest,
) -> Result<StreamResult, String> {
    let url = format!("{}/api/chat", base_url.trim_end_matches('/'));

    let response = client
        .post(&url)
        .json(request)
        .send()
        .await
        .map_err(|e| format!("Ollama request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Ollama HTTP {status}: {text}"));
    }

    let (chunk_tx, chunk_rx) = mpsc::channel::<String>(256);
    let (full_tx, full_rx) = tokio::sync::oneshot::channel::<OllamaResponse>();

    // Use bytes_stream() for true streaming — the response body is read
    // incrementally as chunks arrive over the network, not buffered entirely.
    let mut stream = response.bytes_stream();
    let mut buffer = String::new();

    tokio::spawn(async move {
        let mut accumulated = String::new();
        let mut accumulated_tool_calls: Vec<serde_json::Value> = Vec::new();

        while let Some(chunk_result) = stream.next().await {
            let chunk = match chunk_result {
                Ok(c) => c,
                Err(e) => {
                    warn!("Error reading Ollama stream: {e}");
                    break;
                }
            };

            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // Process complete lines (NDJSON: one JSON object per line)
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                if let Ok(ollama_chunk) = serde_json::from_str::<OllamaChunk>(&line) {
                    if !ollama_chunk.message.content.is_empty() {
                        accumulated.push_str(&ollama_chunk.message.content);
                        if chunk_tx.send(ollama_chunk.message.content).await.is_err() {
                            // receiver dropped — stop streaming
                            let _ = full_tx.send(OllamaResponse {
                                text: accumulated,
                                tool_calls: accumulated_tool_calls,
                            });
                            return;
                        }
                    }
                    // Capture tool calls (Ollama sends them in the final chunk(s))
                    if !ollama_chunk.message.tool_calls.is_empty() {
                        accumulated_tool_calls
                            .extend(ollama_chunk.message.tool_calls.iter().cloned());
                    }
                    if ollama_chunk.done {
                        break;
                    }
                }
            }
        }

        // Process any remaining data in buffer
        let remaining = buffer.trim();
        if !remaining.is_empty()
            && let Ok(ollama_chunk) = serde_json::from_str::<OllamaChunk>(remaining)
        {
            if !ollama_chunk.message.content.is_empty() {
                accumulated.push_str(&ollama_chunk.message.content);
                let _ = chunk_tx.send(ollama_chunk.message.content).await;
            }
            if !ollama_chunk.message.tool_calls.is_empty() {
                accumulated_tool_calls.extend(ollama_chunk.message.tool_calls.iter().cloned());
            }
        }

        let _ = full_tx.send(OllamaResponse {
            text: accumulated,
            tool_calls: accumulated_tool_calls,
        });
    });

    Ok(StreamResult {
        chunks: chunk_rx,
        full_text: full_rx,
    })
}

/// Call Ollama non-streaming and return the full response (text + tool calls).
pub async fn chat(
    client: &Client,
    base_url: &str,
    request: &OllamaChatRequest,
) -> Result<OllamaResponse, String> {
    let url = format!("{}/api/chat", base_url.trim_end_matches('/'));

    let mut req = serde_json::to_value(request).map_err(|e| format!("serialize: {e}"))?;
    req["stream"] = serde_json::Value::Bool(false);

    let response = client
        .post(&url)
        .json(&req)
        .send()
        .await
        .map_err(|e| format!("Ollama request failed: {e}"))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("Ollama HTTP {status}: {text}"));
    }

    let json: serde_json::Value = response.json().await.map_err(|e| format!("parse: {e}"))?;

    let text = json["message"]["content"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let tool_calls = json["message"]["tool_calls"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    Ok(OllamaResponse { text, tool_calls })
}

/// Build a default HTTP client with reasonable timeouts.
pub fn default_http_client() -> Client {
    Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(300))
        .build()
        .unwrap_or_else(|_| Client::new())
}
