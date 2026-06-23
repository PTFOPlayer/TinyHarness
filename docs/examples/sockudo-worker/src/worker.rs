//! Sockudo AI Transport worker — connects to Sockudo, receives `ai-input`
//! events, calls Ollama, and streams responses back as versioned message
//! mutations.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use crate::auth::{AuthCredentials, sign_request};
use crate::ollama::{OllamaChatMessage, OllamaChatRequest, stream_chat};

/// Configuration for the Sockudo AI Transport worker.
#[derive(Clone)]
pub struct WorkerConfig {
    /// Sockudo server HTTP root (e.g. `http://127.0.0.1:6001`).
    pub sockudo_url: String,
    /// Sockudo app credentials.
    pub creds: AuthCredentials,
    /// Ollama server base URL (e.g. `http://127.0.0.1:11434`).
    pub ollama_url: String,
    /// Default model to use if the `ai-input` event doesn't specify one.
    pub default_model: String,
    /// Channel name to subscribe to for `ai-input` events.
    /// The worker listens on this channel and publishes responses back to it.
    /// Default: `ai-output`
    pub channel: String,
    /// WebSocket read timeout in seconds.
    pub ws_timeout_secs: u64,
    /// Whether to stream Ollama output (true) or batch it (false).
    pub stream: bool,
}

impl WorkerConfig {
    /// Build the WebSocket URL for connecting to Sockudo.
    /// Uses Protocol V2 (`?protocol=2`) which is required for AI Transport.
    pub fn ws_url(&self) -> String {
        let ws_base = self
            .sockudo_url
            .trim_end_matches('/')
            .replace("http://", "ws://")
            .replace("https://", "wss://");
        format!("{ws_base}/app/{}?protocol=2", self.creds.app_key)
    }
}

/// The Sockudo AI Transport worker.
///
/// Connects to Sockudo via WebSocket (Protocol V2), subscribes to AI channels,
/// listens for `ai-input` events, calls Ollama for inference, and publishes
/// responses back to the channel as versioned message mutations.
pub struct SockudoWorker {
    config: WorkerConfig,
    http_client: Client,
}

impl SockudoWorker {
    /// Create a new worker with the given configuration.
    pub fn new(config: WorkerConfig) -> Self {
        let http_client = crate::ollama::default_http_client();
        SockudoWorker {
            config,
            http_client,
        }
    }

    /// Run the worker. This connects to Sockudo via WebSocket, subscribes
    /// to channels matching the configured prefix, and processes `ai-input`
    /// events indefinitely.
    ///
    /// If the WebSocket connection drops, it reconnects with exponential
    /// backoff.
    pub async fn run(&self) -> Result<(), String> {
        let mut backoff_secs: u64 = 1;
        let max_backoff: u64 = 30;

        loop {
            info!("Connecting to Sockudo at {}", self.config.ws_url());

            match self.connect_and_serve().await {
                Ok(()) => {
                    info!("Worker session ended cleanly, reconnecting...");
                    backoff_secs = 1;
                }
                Err(e) => {
                    error!("Worker error: {e}. Reconnecting in {backoff_secs}s...");
                    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(max_backoff);
                }
            }
        }
    }

    /// Connect to Sockudo and serve requests until the connection drops.
    async fn connect_and_serve(&self) -> Result<(), String> {
        let ws_url = self.config.ws_url();
        let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| format!("WebSocket connect to {ws_url} failed: {e}"))?;

        info!("WebSocket connected to Sockudo");

        let (mut ws_write, mut ws_read) = ws_stream.split();
        let mut subscribed = false;

        // Keepalive: send a ping every 30s to keep NAT/firewall state alive.
        // We do NOT treat silence as an error — a truly dead connection will
        // be detected naturally when ws_read.next() returns None or Err (the
        // TCP layer will eventually notice).  This prevents idle reconnect
        // cycles when there are simply no ai-input events to process.
        let mut ping_ticker = tokio::time::interval(Duration::from_secs(30));
        ping_ticker.reset();

        // Read timeout — if no message arrives within this period, treat as
        // a dead connection and reconnect. Only active between messages
        // (not during active request handling).
        let read_timeout = Duration::from_secs(self.config.ws_timeout_secs);

        loop {
            tokio::select! {
                // Periodic keepalive ping
                _ = ping_ticker.tick() => {
                    if ws_write.send(WsMessage::Ping(vec![])).await.is_err() {
                        return Err("WebSocket ping send failed".to_string());
                    }
                    continue;
                }
                msg = tokio::time::timeout(read_timeout, ws_read.next()) => {
                    let ws_msg = match msg {
                        Err(_) => {
                            return Err(format!(
                                "WebSocket read timeout after {}s",
                                self.config.ws_timeout_secs
                            ));
                        }
                        Ok(None) => {
                            return Err("WebSocket connection closed".to_string());
                        }
                        Ok(Some(Err(e))) => {
                            return Err(format!("WebSocket read error: {e}"));
                        }
                        Ok(Some(Ok(m))) => m,
                    };

                    let text = match ws_msg {
                        WsMessage::Text(t) => t.to_string(),
                        WsMessage::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                        WsMessage::Ping(_) => {
                            let _ = ws_write.send(WsMessage::Pong(vec![])).await;
                            continue;
                        }
                        WsMessage::Pong(_) | WsMessage::Close(_) | WsMessage::Frame(_) => continue,
                    };

                    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&text);
                    let event = match parsed {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    let event_name = event["event"].as_str().unwrap_or("");
                    let event_data = event["data"].as_str().unwrap_or("");

                    debug!("Event: {event_name}");

                    // Handle connection established (V2 uses sockudo: prefix, V1 uses pusher:)
                    if event_name == "pusher:connection_established"
                        || event_name == "sockudo:connection_established"
                    {
                        debug!("Connection established");

                        // Subscribe to the AI channel
                        debug!("Subscribing to channel: {}", self.config.channel);
                        let subscribe_msg = serde_json::json!({
                            "event": "pusher:subscribe",
                            "data": { "channel": &self.config.channel }
                        });
                        let _ = ws_write
                            .send(WsMessage::Text(
                                serde_json::to_string(&subscribe_msg).unwrap_or_default(),
                            ))
                            .await;
                        continue;
                    }

                    if event_name == "pusher_internal:subscription_succeeded"
                        || event_name == "sockudo_internal:subscription_succeeded"
                        || event_name == "sockudo:subscription_succeeded"
                    {
                        subscribed = true;
                        info!("Subscribed to AI channel, ready to receive ai-input events");
                        continue;
                    }

                    if event_name == "pusher:error" || event_name == "sockudo:error" {
                        let err_msg = if let Ok(err) =
                            serde_json::from_str::<serde_json::Value>(event_data)
                        {
                            err["message"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_string()
                        } else {
                            event_data.to_string()
                        };
                        error!("Sockudo error: {err_msg}");
                        continue;
                    }

                    if !subscribed {
                        continue;
                    }

                    // Handle ai-input event
                    if event_name == "ai-input" {
                        // The provider includes a response_channel in the payload
                        // for per-request isolation. Fall back to the event's
                        // channel for backwards compatibility.
                        let input: serde_json::Value =
                            serde_json::from_str(event_data).unwrap_or(serde_json::Value::Null);
                        let channel = input
                            .get("response_channel")
                            .and_then(|c| c.as_str())
                            .or_else(|| event["channel"].as_str())
                            .unwrap_or("ai-output")
                            .to_string();

                        debug!("Received ai-input on channel: {channel}");

                        // Spawn a task to handle this request independently
                        let config = self.config.clone();
                        let http_client = self.http_client.clone();
                        let input_data = event_data.to_string();

                        tokio::spawn(async move {
                            if let Err(e) =
                                handle_ai_input(&config, &http_client, &channel, &input_data).await
                            {
                                error!("Failed to handle ai-input on {channel}: {e}");
                            }
                        });
                    }
                }
            }
        }
    }
}

/// Build the AI transport extras JSON for versioned message events.
fn transport_extras(message_serial: &str, status: &str, model: &str) -> serde_json::Value {
    serde_json::json!({
        "ai": {
            "transport": {
                "codec-message-id": message_serial,
                "stream": "true",
                "status": status,
                "model": model
            }
        }
    })
}

/// Handle a single `ai-input` event: parse the payload, call Ollama, and
/// publish the response back as versioned message mutations.
async fn handle_ai_input(
    config: &WorkerConfig,
    http_client: &Client,
    channel: &str,
    input_data: &str,
) -> Result<(), String> {
    // Parse the AI input payload
    let input: serde_json::Value =
        serde_json::from_str(input_data).unwrap_or(serde_json::Value::Null);

    let model = input
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or(&config.default_model)
        .to_string();

    let messages = input
        .get("messages")
        .and_then(|m| m.as_array())
        .cloned()
        .unwrap_or_default();

    // Extract tool definitions from the input payload (if any)
    let tools: Vec<serde_json::Value> = input
        .get("tools")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();

    // Convert messages to Ollama format
    let ollama_messages: Vec<OllamaChatMessage> = messages
        .iter()
        .map(|m| {
            let role = m["role"].as_str().unwrap_or("user").to_string();
            let content = m["content"].as_str().unwrap_or("").to_string();
            let tool_calls = m
                .get("tool_calls")
                .and_then(|t| t.as_array())
                .cloned()
                .unwrap_or_default();
            OllamaChatMessage {
                role,
                content,
                tool_calls,
            }
        })
        .collect();

    if ollama_messages.is_empty() {
        return Err("ai-input has no messages".to_string());
    }

    let request = OllamaChatRequest {
        model: model.clone(),
        messages: ollama_messages,
        stream: config.stream,
        tools,
    };

    // Generate a message serial for this response
    let message_serial = format!("msg-{}", uuid::Uuid::new_v4());
    let message_id = format!("mid-{}", uuid::Uuid::new_v4());

    // 1. Publish sockudo:message.create (empty data, streaming status)
    let create_extras = transport_extras(&message_serial, "streaming", &model);
    publish_event_with(
        http_client,
        config,
        channel,
        "sockudo:message.create",
        "",
        Some(&create_extras),
        Some(&message_id),
    )
    .await?;

    // 2. Call Ollama and stream response back
    if config.stream {
        // Stream mode: append chunks as they arrive
        let stream_result = stream_chat(http_client, &config.ollama_url, &request).await?;

        let mut chunks = stream_result.chunks;
        let full_response_rx = stream_result.full_text;

        while let Some(chunk) = chunks.recv().await {
            let append_extras = transport_extras(&message_serial, "streaming", &model);
            let _ = publish_event_with(
                http_client,
                config,
                channel,
                "sockudo:message.append",
                &chunk,
                Some(&append_extras),
                None,
            )
            .await;
        }

        // Get the full response (text + tool calls)
        let response = full_response_rx.await.unwrap_or_default();

        // 3. Publish sockudo:message.update with final content + tool_calls
        let update_extras = transport_extras(&message_serial, "complete", &model);
        let update_data = build_update_data(&response.text, &response.tool_calls);
        publish_event_with(
            http_client,
            config,
            channel,
            "sockudo:message.update",
            &update_data,
            Some(&update_extras),
            None,
        )
        .await?;
    } else {
        // Non-streaming mode: call Ollama, get full response, publish as append + update
        let response = crate::ollama::chat(http_client, &config.ollama_url, &request).await?;

        // Append the full response text
        let append_extras = transport_extras(&message_serial, "streaming", &model);
        publish_event_with(
            http_client,
            config,
            channel,
            "sockudo:message.append",
            &response.text,
            Some(&append_extras),
            None,
        )
        .await?;

        // Update with final status + tool_calls
        let update_extras = transport_extras(&message_serial, "complete", &model);
        let update_data = build_update_data(&response.text, &response.tool_calls);
        publish_event_with(
            http_client,
            config,
            channel,
            "sockudo:message.update",
            &update_data,
            Some(&update_extras),
            None,
        )
        .await?;
    }

    // 4. Send ai-turn-end
    publish_event_with(
        http_client,
        config,
        channel,
        "ai-turn-end",
        "{}",
        None,
        None,
    )
    .await?;

    info!("Completed ai-input on {channel} (model: {model})");
    Ok(())
}

/// Build the JSON data payload for a `sockudo:message.update` event.
///
/// When tool calls are present, includes them in a JSON object alongside
/// the text content. When there are no tool calls, returns the plain text
/// content (backwards-compatible with existing clients).
fn build_update_data(text: &str, tool_calls: &[serde_json::Value]) -> String {
    if tool_calls.is_empty() {
        return text.to_string();
    }
    serde_json::json!({
        "content": text,
        "tool_calls": tool_calls,
        "is_final": true,
    })
    .to_string()
}

/// Publish a versioned message event to Sockudo via signed HTTP POST.
async fn publish_event_with(
    http_client: &Client,
    config: &WorkerConfig,
    channel: &str,
    event_name: &str,
    data: &str,
    extras: Option<&serde_json::Value>,
    message_id: Option<&str>,
) -> Result<(), String> {
    let path = format!("/apps/{}/events", config.creds.app_id);

    let mut body = serde_json::json!({
        "name": event_name,
        "channel": channel,
        "data": data,
    });
    if let Some(extras) = extras {
        body["extras"] = extras.clone();
    }
    if let Some(mid) = message_id {
        body["message_id"] = serde_json::Value::String(mid.to_string());
    }
    let body_str =
        serde_json::to_string(&body).map_err(|e| format!("serialize event body: {e}"))?;

    let qs = sign_request(&config.creds, "POST", &path, &body_str);
    let url = format!(
        "{}{}?{}",
        config.sockudo_url.trim_end_matches('/'),
        path,
        qs
    );

    let response = http_client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body_str)
        .send()
        .await
        .map_err(|e| format!("publish event: {e}"))?;

    if !response.status().is_success() {
        let status = response.status().as_u16();
        let text = response.text().await.unwrap_or_default();
        warn!("Publish {event_name} to {channel} failed: HTTP {status}: {text}");
        return Err(format!(
            "publish {event_name} failed: HTTP {status}: {text}"
        ));
    }

    debug!("Published {event_name} to {channel}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ws_url_construction() {
        let creds = AuthCredentials::new("app", "key", "secret");
        let config = WorkerConfig {
            sockudo_url: "http://127.0.0.1:6001".to_string(),
            creds,
            ollama_url: "http://127.0.0.1:11434".to_string(),
            default_model: "qwen2.5:0.5b".to_string(),
            channel: "ai-output".to_string(),
            ws_timeout_secs: 120,
            stream: true,
        };
        assert_eq!(config.ws_url(), "ws://127.0.0.1:6001/app/key?protocol=2");
    }

    #[test]
    fn test_ws_url_https() {
        let creds = AuthCredentials::new("app", "key", "secret");
        let config = WorkerConfig {
            sockudo_url: "https://example.com".to_string(),
            creds,
            ollama_url: "http://127.0.0.1:11434".to_string(),
            default_model: "qwen2.5:0.5b".to_string(),
            channel: "ai-output".to_string(),
            ws_timeout_secs: 120,
            stream: true,
        };
        assert_eq!(config.ws_url(), "wss://example.com/app/key?protocol=2");
    }

    #[test]
    fn test_transport_extras_streaming() {
        let extras = transport_extras("msg-123", "streaming", "test-model");
        assert_eq!(extras["ai"]["transport"]["codec-message-id"], "msg-123");
        assert_eq!(extras["ai"]["transport"]["stream"], "true");
        assert_eq!(extras["ai"]["transport"]["status"], "streaming");
        assert_eq!(extras["ai"]["transport"]["model"], "test-model");
    }

    #[test]
    fn test_transport_extras_complete() {
        let extras = transport_extras("msg-abc", "complete", "test-model");
        assert_eq!(extras["ai"]["transport"]["codec-message-id"], "msg-abc");
        assert_eq!(extras["ai"]["transport"]["status"], "complete");
        assert_eq!(extras["ai"]["transport"]["model"], "test-model");
    }
}
