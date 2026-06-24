//! Sockudo AI Transport provider.
//!
//! Uses the Sockudo WebSocket server's AI Transport feature to communicate
//! with an LLM backend. The provider:
//!
//! 1. Publishes `ai-input` events via HTTP POST to `/apps/{appId}/events`
//!    (Pusher-style signed auth with HMAC-SHA256).
//! 2. Subscribes to a channel via WebSocket and listens for `ai-output`
//!    events (versioned message mutations: `sockudo:message.create`,
//!    `sockudo:message.append`, `sockudo:message.update`, `sockudo:message.delete`).
//! 3. Converts streamed WebSocket events into `ChatMessageResponse` chunks.
//!
//! See: <https://github.com/sockudo/sockudo> for server documentation.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde::Deserialize;
use sha2::Sha256;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::provider::{
    ChatMessage, ChatMessageResponse, Message, Provider, Role, TokenUsage, ToolCall,
    ToolCallFunction, ToolDefinition,
};

type HmacSha256 = Hmac<Sha256>;

// ── Provider ────────────────────────────────────────────────────────────────

/// Sockudo AI Transport provider.
///
/// Connects to a Sockudo server, publishes `ai-input` events via HTTP,
/// and streams `ai-output` responses via WebSocket.
///
/// The `model` field is shared (`Arc<Mutex<...>>`) so that the background
/// streaming task (which works with a cloned `SockudoProvider`) can update
/// the model name in-place when it's discovered from WebSocket events.
/// This avoids needing a separate channel or response field to propagate
/// the model name back to the caller.
#[derive(Clone)]
pub struct SockudoProvider {
    http_client: Client,
    base_url: String,
    app_id: String,
    app_key: String,
    app_secret: String,
    model: Arc<Mutex<Option<String>>>,
    timeout_secs: u64,
}

impl SockudoProvider {
    /// Create a new Sockudo provider.
    ///
    /// `base_url` is the Sockudo server HTTP root (e.g. `http://127.0.0.1:6001`).
    /// `app_id`, `app_key`, `app_secret` are the Sockudo app credentials.
    pub fn new(base_url: String, app_id: String, app_key: String, app_secret: String) -> Self {
        let http_client = Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(Duration::from_secs(300))
            .build()
            .unwrap_or_else(|_| Client::new());
        SockudoProvider {
            http_client,
            base_url,
            app_id,
            app_key,
            app_secret,
            model: Arc::new(Mutex::new(None)),
            timeout_secs: 120,
        }
    }

    /// Build the WebSocket URL for subscribing to channels.
    /// Format: `ws://host:port/app/{appKey}?protocol=2`
    pub fn ws_url(&self) -> String {
        let ws_base = self
            .base_url
            .trim_end_matches('/')
            .replace("http://", "ws://")
            .replace("https://", "wss://");
        format!("{ws_base}/app/{}?protocol=2", self.app_key)
    }

    /// Build the HTTP events URL: `POST /apps/{appId}/events`
    pub fn events_url(&self) -> String {
        format!(
            "{}/apps/{}/events",
            self.base_url.trim_end_matches('/'),
            self.app_id
        )
    }

    /// Health check: GET `/up/{appId}` — returns 200 if the app is known.
    pub fn health_url(&self) -> String {
        format!("{}/up/{}", self.base_url.trim_end_matches('/'), self.app_id)
    }

    /// Compute the Pusher-style auth signature for an HTTP API request.
    ///
    /// Signature = HMAC-SHA256(secret, "METHOD\npath\nquery_string")
    /// where query_string is sorted alphabetically (excluding auth_signature).
    pub fn sign_request(&self, method: &str, path: &str, body: &str) -> Vec<(String, String)> {
        let body_md5 = format!("{:x}", md5::compute(body.as_bytes()));

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();

        // Build query params (sorted alphabetically, excluding auth_signature)
        let mut params: Vec<(String, String)> = vec![
            ("auth_key".to_string(), self.app_key.clone()),
            ("auth_timestamp".to_string(), timestamp),
            ("auth_version".to_string(), "1.0".to_string()),
            ("body_md5".to_string(), body_md5),
        ];
        params.sort_by(|a, b| a.0.cmp(&b.0));

        // Build the string to sign: "METHOD\npath\nsorted_query_string"
        let qs = params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let string_to_sign = format!("{method}\n{path}\n{qs}");

        let signature = {
            let mut mac =
                HmacSha256::new_from_slice(self.app_secret.as_bytes()).expect("HMAC key invalid");
            mac.update(string_to_sign.as_bytes());
            hex_encode(&mac.finalize().into_bytes())
        };

        params.push(("auth_signature".to_string(), signature));
        params
    }

    /// Publish an `ai-input` event via HTTP POST to `/apps/{appId}/events`.
    pub async fn publish_ai_input(
        &self,
        channel: &str,
        data: &serde_json::Value,
    ) -> Result<(), String> {
        let events_url = self.events_url();
        // Pusher protocol requires `data` to be a string, not a JSON object.
        // Serialize the data value to a JSON string so Sockudo accepts it.
        let data_str =
            serde_json::to_string(data).map_err(|e| format!("serialize event data: {e}"))?;
        let mut body_json = serde_json::json!({
            "name": "ai-input",
            "channel": channel,
            "data": data_str,
        });

        // Include the model identifier in extras.ai.transport.model if present
        // in the data payload. Sockudo validates this as a non-empty opaque
        // string (AIT-S99) and passes it through to subscribers.
        if let Some(model) = data.get("model").and_then(|m| m.as_str())
            && !model.is_empty()
        {
            body_json["extras"] = serde_json::json!({
                "ai": {
                    "transport": {
                        "model": model
                    }
                }
            });
        }

        let body =
            serde_json::to_string(&body_json).map_err(|e| format!("serialize event body: {e}"))?;

        // Extract path from full URL for signing
        let path = format!("/apps/{}/events", self.app_id);
        let auth_params = self.sign_request("POST", &path, &body);

        // Build query string from auth params
        let qs = auth_params
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");
        let url = format!("{events_url}?{qs}");

        let response = self
            .http_client
            .post(&url)
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| format!("publish ai-input: {e}"))?;

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            return Err(format!("publish ai-input failed: HTTP {status}: {text}"));
        }

        Ok(())
    }

    /// Subscribe to a channel via WebSocket, publish the `ai-input` event
    /// after subscription is confirmed, then stream `ai-output` events
    /// as `ChatMessageResponse` chunks.
    ///
    /// The flow is:
    /// 1. Connect to `ws://host:port/app/{appKey}?protocol=2`
    /// 2. Receive `pusher:connection_established` → extract `socket_id`
    /// 3. Send `pusher:subscribe` for the AI output channel
    /// 4. Wait for `subscription_succeeded` — **then** publish `ai-input`
    ///    via HTTP POST (this prevents a race where the worker emits
    ///    response events before we are listening)
    /// 5. Listen for versioned message events (`sockudo:message.create`,
    ///    `sockudo:message.append`, `sockudo:message.update`) and convert
    ///    them to streaming response chunks
    /// 6. When `sockudo:message.update` with the final content arrives, or
    ///    `ai-turn-end` is received, send the done marker
    async fn subscribe_and_stream(
        &self,
        channel: &str,
        worker_channel: &str,
        ai_input: &serde_json::Value,
        send: mpsc::Sender<ChatMessageResponse>,
    ) -> Result<(), String> {
        let ws_url = self.ws_url();

        // Connect to WebSocket
        let (ws_stream, _response) = tokio_tungstenite::connect_async(&ws_url)
            .await
            .map_err(|e| format!("WebSocket connect to {ws_url} failed: {e}"))?;

        let (mut ws_write, mut ws_read) = ws_stream.split();
        let mut subscribed = false;
        let mut final_tool_calls: Vec<ToolCall> = Vec::new();
        let mut done_sent = false;

        // Send initial ping to keep connection alive
        let _ = ws_write
            .send(WsMessage::Text(
                r#"{"event":"pusher:ping","data":{}}"#.into(),
            ))
            .await;

        let timeout = Duration::from_secs(self.timeout_secs);

        loop {
            let msg = tokio::time::timeout(timeout, ws_read.next()).await;

            match msg {
                Err(_) => {
                    if !done_sent {
                        send_error(&send, "WebSocket stream timed out").await;
                    }
                    return Err("WebSocket stream timed out".to_string());
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => return Err(format!("WebSocket read error: {e}")),
                Ok(Some(Ok(ws_msg))) => {
                    let text = match ws_msg {
                        WsMessage::Text(t) => t.to_string(),
                        WsMessage::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                        WsMessage::Ping(_) => {
                            let _ = ws_write.send(WsMessage::Pong(vec![])).await;
                            continue;
                        }
                        WsMessage::Pong(_) | WsMessage::Close(_) | WsMessage::Frame(_) => continue,
                    };

                    let event: WsEvent = match serde_json::from_str(&text) {
                        Ok(e) => e,
                        Err(_) => continue,
                    };

                    // Extract model from extras.ai.transport.model if present.
                    // This is the model reported by the Sockudo worker/agent
                    // and may differ from the locally-selected model name.
                    // Update the shared model field in-place so that
                    // `current_model()` reflects it immediately.
                    if let Some(model) = event.extras.as_ref().and_then(WsExtras::transport_model) {
                        tracing::debug!("Sockudo event model: {model}");
                        if let Ok(mut m) = self.model.lock() {
                            *m = Some(model);
                        }
                    }

                    // Connection established → subscribe
                    if event.event == "pusher:connection_established"
                        || event.event == "sockudo:connection_established"
                    {
                        let subscribe_msg = serde_json::json!({
                            "event": "pusher:subscribe",
                            "data": { "channel": channel }
                        });
                        let _ = ws_write
                            .send(WsMessage::Text(
                                serde_json::to_string(&subscribe_msg).unwrap_or_default(),
                            ))
                            .await;
                        continue;
                    }

                    if event.event == "pusher_internal:subscription_succeeded"
                        || event.event == "sockudo_internal:subscription_succeeded"
                        || event.event == "sockudo:subscription_succeeded"
                    {
                        subscribed = true;

                        // Now that we are subscribed, publish the ai-input
                        // event. This prevents a race condition where the
                        // worker emits response events before we are listening.
                        if let Err(e) = self.publish_ai_input(worker_channel, ai_input).await {
                            send_error(&send, &format!("Error publishing ai-input: {e}")).await;
                            return Err(format!("publish ai-input: {e}"));
                        }
                        continue;
                    }

                    // Pusher error
                    if event.event == "pusher:error" || event.event == "sockudo:error" {
                        let err_msg =
                            if let Ok(err) = serde_json::from_str::<PusherErrorData>(&event.data) {
                                format!("Sockudo error: {}", err.message)
                            } else {
                                format!("Sockudo error: {}", event.data)
                            };
                        send_error(&send, &err_msg).await;
                        return Err("Sockudo pusher:error received".to_string());
                    }

                    if !subscribed {
                        continue;
                    }

                    // ── AI Transport events ──

                    // Versioned message mutations
                    if let Some(op) = event.event.strip_prefix("sockudo:message.") {
                        match op {
                            "create" | "append" => {
                                if let Ok(vm) =
                                    serde_json::from_str::<VersionedMessage>(&event.data)
                                {
                                    // Extract model from message data if not
                                    // already known from extras.
                                    update_model_if_new(&self.model, &vm.model);

                                    // Send incremental content chunk
                                    let chunk_content = vm.content.unwrap_or_default();
                                    if !chunk_content.is_empty() {
                                        send_chunk(&send, &chunk_content).await;
                                    }

                                    // Capture tool calls (can appear in any chunk)
                                    if let Some(tc_json) = &vm.tool_calls
                                        && let Some(tcs) = parse_tool_calls(tc_json)
                                    {
                                        final_tool_calls = tcs;
                                    }
                                } else {
                                    // Plain-text payload (not JSON). Treat as content chunk.
                                    if !event.data.is_empty() {
                                        send_chunk(&send, &event.data).await;
                                    }
                                }
                                continue;
                            }
                            "update" => {
                                // The update event is the final marker.
                                // It may carry tool_calls in JSON format.
                                // Content is NOT re-sent (already streamed via append).
                                if let Ok(vm) =
                                    serde_json::from_str::<VersionedMessage>(&event.data)
                                {
                                    // Extract model from message data if not
                                    // already known from extras.
                                    update_model_if_new(&self.model, &vm.model);

                                    // Capture tool calls
                                    if let Some(tc_json) = &vm.tool_calls
                                        && let Some(tcs) = parse_tool_calls(tc_json)
                                    {
                                        final_tool_calls = tcs;
                                    }

                                    let usage = vm.usage.as_ref().map(|u| TokenUsage {
                                        prompt_tokens: u.prompt_tokens,
                                        completion_tokens: u.completion_tokens,
                                        total_tokens: u.total_tokens,
                                    });
                                    send_done(&send, &final_tool_calls, usage).await;
                                    done_sent = true;
                                    break;
                                } else {
                                    // Plain-text update — just signal done.
                                    send_done(&send, &final_tool_calls, None).await;
                                    done_sent = true;
                                    break;
                                }
                            }
                            "delete" => {
                                if !done_sent {
                                    send_done(&send, &final_tool_calls, None).await;
                                    done_sent = true;
                                }
                                break;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    if event.event == "ai-turn-end" {
                        if !done_sent {
                            send_done(&send, &final_tool_calls, None).await;
                            done_sent = true;
                        }
                        break;
                    }

                    if event.event == "ai-cancel" {
                        if !done_sent {
                            send_done(&send, &[], None).await;
                            done_sent = true;
                        }
                        break;
                    }
                    // Ignore other events (presence, member_added, etc.)
                }
            }
        }

        // Synthetic done if stream ended without one
        if !done_sent {
            send_done(&send, &final_tool_calls, None).await;
        }

        // Gracefully close the WebSocket
        let _ = ws_write.send(WsMessage::Close(None)).await;

        Ok(())
    }
}

// ── Streaming helper functions ──────────────────────────────────────────────

/// Send an incremental content chunk (done: false).
async fn send_chunk(send: &mpsc::Sender<ChatMessageResponse>, content: &str) {
    let _ = send
        .send(ChatMessageResponse {
            message: ChatMessage {
                content: content.to_string(),
                tool_calls: vec![],
                thinking: None,
            },
            done: false,
            is_error: false,
            usage: None,
        })
        .await;
}

/// Update the shared model field only if it's currently `None` and the
/// new model name is non-empty. Called from the streaming task when a
/// `VersionedMessage` carries a `model` field (fallback when not present
/// in WebSocket extras).
fn update_model_if_new(shared: &Arc<Mutex<Option<String>>>, new_model: &Option<String>) {
    if let Some(m) = new_model
        && !m.is_empty()
        && let Ok(mut current) = shared.lock()
        && current.is_none()
    {
        *current = Some(m.clone());
    }
}

/// Send the final done marker with optional tool calls and usage.
async fn send_done(
    send: &mpsc::Sender<ChatMessageResponse>,
    tool_calls: &[ToolCall],
    usage: Option<TokenUsage>,
) {
    let _ = send
        .send(ChatMessageResponse {
            message: ChatMessage {
                content: String::new(),
                tool_calls: tool_calls.to_vec(),
                thinking: None,
            },
            done: true,
            is_error: false,
            usage,
        })
        .await;
}

/// Send an error response with done: true.
async fn send_error(send: &mpsc::Sender<ChatMessageResponse>, message: &str) {
    let _ = send
        .send(ChatMessageResponse {
            message: ChatMessage {
                content: message.to_string(),
                tool_calls: vec![],
                thinking: None,
            },
            done: true,
            is_error: true,
            usage: None,
        })
        .await;
}

impl Provider for SockudoProvider {
    fn health_check(&self) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> {
        let url = self.health_url();
        let client = self.http_client.clone();
        Box::pin(async move {
            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => Ok(()),
                Ok(resp) => Err(format!(
                    "Sockudo health check failed: HTTP {}: {}",
                    resp.status().as_u16(),
                    resp.text().await.unwrap_or_default()
                )),
                Err(e) => Err(format!("Cannot reach Sockudo at {}: {}", url, e)),
            }
        })
    }

    fn list_models(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send>> {
        let model = self.model.lock().ok().and_then(|m| m.clone());
        Box::pin(async move {
            // Sockudo AI Transport doesn't expose a model list endpoint.
            // Return the currently selected model if set, or an empty vec
            // so that auto-selection can prompt the user on first launch.
            model.into_iter().collect()
        })
    }

    fn select_model(&mut self, name: String) {
        if let Ok(mut m) = self.model.lock() {
            *m = Some(name);
        }
    }

    fn current_model(&self) -> Option<String> {
        self.model.lock().ok().and_then(|m| m.clone())
    }

    fn set_timeout(&mut self, timeout_secs: u64) {
        self.timeout_secs = timeout_secs;
    }

    fn chat(
        &mut self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
    ) -> Pin<Box<dyn Future<Output = Result<mpsc::Receiver<ChatMessageResponse>, String>> + Send>>
    {
        // Sockudo doesn't require a locally-selected model — the worker
        // picks the backend model (or uses its default). We send the model
        // name if we have one, otherwise omit it and let the worker decide.
        // The actual model name is discovered from WebSocket extras during
        // streaming and stored in the shared `model` field.
        let model = self.model.lock().ok().and_then(|m| m.clone());
        let model_for_input = model.as_deref().filter(|s| !s.is_empty());

        let (send, recv) = mpsc::channel::<ChatMessageResponse>(1024);

        let mut ai_input = build_ai_input_opt(model_for_input, &messages, &tools);
        let worker_channel = "ai-output".to_string();
        let response_channel = format!("ai-output-{}", uuid::Uuid::new_v4());
        ai_input["response_channel"] = serde_json::Value::String(response_channel.clone());

        // Clone self for the background task instead of recreating from parts
        let provider = self.clone();

        tokio::spawn(async move {
            // Subscribe to the response channel via WebSocket and publish
            // ai-input only after subscription is confirmed (prevents race
            // condition where worker emits events before we are listening).
            if let Err(e) = provider
                .subscribe_and_stream(&response_channel, &worker_channel, &ai_input, send)
                .await
            {
                tracing::warn!("Sockudo stream ended with error: {e}");
            }
        });

        Box::pin(async move { Ok(recv) })
    }
}

// ── WebSocket event types ───────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct WsEvent {
    event: String,
    data: String,
    #[serde(default)]
    #[allow(dead_code)]
    channel: Option<String>,
    /// V2 extras envelope — carries AI transport metadata.
    #[serde(default)]
    extras: Option<WsExtras>,
}

/// Extras envelope with AI transport headers.
#[derive(Debug, Deserialize, Default)]
struct WsExtras {
    #[serde(default)]
    ai: Option<WsAiExtras>,
}

/// AI extras with transport and codec tiers.
#[derive(Debug, Deserialize, Default)]
struct WsAiExtras {
    #[serde(default)]
    transport: Option<serde_json::Value>,
    #[serde(default)]
    #[allow(dead_code)]
    codec: Option<serde_json::Value>,
}

impl WsExtras {
    /// Extract the `extras.ai.transport.model` field if present.
    fn transport_model(&self) -> Option<String> {
        self.ai
            .as_ref()
            .and_then(|ai| ai.transport.as_ref())
            .and_then(|t| t.get("model"))
            .and_then(|m| m.as_str())
            .map(|s| s.to_string())
    }
}

#[derive(Debug, Deserialize)]
struct PusherErrorData {
    message: String,
    #[serde(default)]
    #[allow(dead_code)]
    code: Option<u32>,
}

// ── Versioned message types ──

/// A versioned realtime message from Sockudo's AI Transport.
///
/// This is the payload of `sockudo:message.create`, `.append`, `.update`
/// events. The `content` field carries the AI output text (streamed in
/// chunks via append operations).
#[derive(Debug, Deserialize)]
struct VersionedMessage {
    /// Message content (text chunk from the AI).
    #[serde(default)]
    content: Option<String>,
    /// Action type for this versioned message.
    #[serde(default)]
    #[allow(dead_code)]
    action: Option<String>,
    /// Monotonically increasing version number.
    #[serde(default)]
    #[allow(dead_code)]
    version: Option<u64>,
    /// Serial number for ordering.
    #[serde(default)]
    #[allow(dead_code)]
    serial: Option<String>,
    /// Whether this is the final message in the stream.
    #[serde(default, rename = "is_final")]
    #[allow(dead_code)]
    is_final: Option<bool>,
    /// Tool calls from the AI (when the model wants to call tools).
    #[serde(default)]
    tool_calls: Option<serde_json::Value>,
    /// Token usage info (present in final message).
    #[serde(default)]
    usage: Option<AiUsage>,
    /// Model name reported by the worker (may appear in the message data
    /// as a fallback when not present in WebSocket extras).
    #[serde(default)]
    model: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
    #[serde(default)]
    total_tokens: u32,
}

// ── AI input payload ────────────────────────────────────────────────────────

/// Build the `ai-input` event data payload from the conversation messages.
///
/// The payload includes the model name, conversation messages, and tool
/// definitions, serialized as JSON. The Sockudo AI Transport server
/// forwards this to the configured LLM backend.
#[cfg(test)]
fn build_ai_input(
    model: &str,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> serde_json::Value {
    build_ai_input_opt(Some(model), messages, tools)
}

/// Same as `build_ai_input` but with an optional model.
/// When `model` is `None`, the `model` field is omitted from the payload,
/// letting the Sockudo worker use its configured default model.
fn build_ai_input_opt(
    model: Option<&str>,
    messages: &[Message],
    tools: &[ToolDefinition],
) -> serde_json::Value {
    let msgs: Vec<serde_json::Value> = messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let mut obj = serde_json::json!({
                "role": role,
                "content": m.content,
            });
            if !m.tool_calls.is_empty() {
                obj["tool_calls"] = serde_json::json!(
                    m.tool_calls
                        .iter()
                        .enumerate()
                        .map(|(i, tc)| {
                            let id = tc
                                .id
                                .clone()
                                .filter(|s| !s.is_empty())
                                .unwrap_or_else(|| format!("call-{}", i));
                            serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": {
                                    "name": tc.function.name,
                                    "arguments": tc.function.arguments,
                                }
                            })
                        })
                        .collect::<Vec<_>>()
                );
            }
            // Tool messages must include tool_call_id matching the assistant's tool call
            if m.role == Role::Tool
                && let Some(tcid) = &m.tool_call_id
                && !tcid.is_empty()
            {
                obj["tool_call_id"] = serde_json::Value::String(tcid.clone());
            }
            obj
        })
        .collect();

    let tool_defs: Vec<serde_json::Value> = tools
        .iter()
        .map(|t| {
            serde_json::json!({
                "type": "function",
                "function": {
                    "name": t.name,
                    "description": t.description,
                    "parameters": serde_json::to_value(&t.parameters).unwrap_or_default(),
                }
            })
        })
        .collect();

    let mut payload = serde_json::json!({
        "messages": msgs,
        "stream": true,
    });
    if let Some(m) = model {
        payload["model"] = serde_json::Value::String(m.to_string());
    }
    if !tool_defs.is_empty() {
        payload["tools"] = serde_json::Value::Array(tool_defs);
    }
    payload
}

/// Parse tool calls from a JSON value (from the versioned message's
/// `tool_calls` field).
fn parse_tool_calls(value: &serde_json::Value) -> Option<Vec<ToolCall>> {
    let arr = value.as_array()?;
    let result: Vec<ToolCall> = arr
        .iter()
        .filter_map(|tc| {
            let func = tc.get("function")?;
            let name = func.get("name")?.as_str()?.to_string();
            let arguments = func
                .get("arguments")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            Some(ToolCall {
                id: None,
                function: ToolCallFunction {
                    name,
                    arguments,
                    thought_signature: None,
                },
            })
        })
        .collect();
    if result.is_empty() {
        None
    } else {
        Some(result)
    }
}

/// Hex-encode a byte slice.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x01, 0x23, 0xab, 0xff]), "0123abff");
        assert_eq!(hex_encode(&[]), "");
        assert_eq!(hex_encode(&[0x00]), "00");
    }

    #[test]
    fn test_build_ai_input_basic() {
        let messages = vec![Message::simple(Role::User, "Hello, world!")];
        let result = build_ai_input("test-model", &messages, &[]);
        assert_eq!(result["model"], "test-model");
        assert_eq!(result["messages"][0]["role"], "user");
        assert_eq!(result["messages"][0]["content"], "Hello, world!");
        assert_eq!(result["stream"], true);
        assert!(result.get("tools").is_none());
    }

    #[test]
    fn test_build_ai_input_with_system_and_assistant() {
        let messages = vec![
            Message::simple(Role::System, "You are helpful."),
            Message::simple(Role::User, "What is 2+2?"),
            Message::simple(Role::Assistant, "4"),
        ];
        let result = build_ai_input("m", &messages, &[]);
        assert_eq!(result["messages"].as_array().unwrap().len(), 3);
        assert_eq!(result["messages"][0]["role"], "system");
        assert_eq!(result["messages"][1]["role"], "user");
        assert_eq!(result["messages"][2]["role"], "assistant");
    }

    #[test]
    fn test_build_ai_input_with_tools() {
        let messages = vec![Message::simple(Role::User, "List files")];
        let params = serde_json::from_value(serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "The directory path to list"
                }
            }
        }))
        .unwrap();
        let tool = ToolDefinition {
            name: "ls".to_string(),
            description: "List directory contents".to_string(),
            parameters: params,
        };
        let result = build_ai_input("m", &messages, &[tool]);
        assert_eq!(result["tools"].as_array().unwrap().len(), 1);
        assert_eq!(result["tools"][0]["type"], "function");
        assert_eq!(result["tools"][0]["function"]["name"], "ls");
    }

    #[test]
    fn test_parse_tool_calls_valid() {
        let json = serde_json::json!([
            {
                "function": {
                    "name": "ls",
                    "arguments": {"path": "/home"}
                }
            },
            {
                "function": {
                    "name": "read",
                    "arguments": {"path": "/etc/passwd"}
                }
            }
        ]);
        let result = parse_tool_calls(&json);
        assert!(result.is_some());
        let calls = result.unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].function.name, "ls");
        assert_eq!(calls[1].function.name, "read");
    }

    #[test]
    fn test_parse_tool_calls_empty_array() {
        let json = serde_json::json!([]);
        let result = parse_tool_calls(&json);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_tool_calls_missing_function() {
        let json = serde_json::json!([{"not_function": {}}]);
        let result = parse_tool_calls(&json);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_tool_calls_not_array() {
        let json = serde_json::json!({"key": "value"});
        let result = parse_tool_calls(&json);
        assert!(result.is_none());
    }

    #[test]
    fn test_ws_url_construction() {
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "app-id".to_string(),
            "app-key".to_string(),
            "app-secret".to_string(),
        );
        assert_eq!(
            provider.ws_url(),
            "ws://127.0.0.1:6001/app/app-key?protocol=2"
        );
    }

    #[test]
    fn test_ws_url_https() {
        let provider = SockudoProvider::new(
            "https://example.com".to_string(),
            "app-id".to_string(),
            "app-key".to_string(),
            "app-secret".to_string(),
        );
        assert_eq!(
            provider.ws_url(),
            "wss://example.com/app/app-key?protocol=2"
        );
    }

    #[test]
    fn test_ws_url_trailing_slash() {
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001/".to_string(),
            "app-id".to_string(),
            "app-key".to_string(),
            "app-secret".to_string(),
        );
        assert_eq!(
            provider.ws_url(),
            "ws://127.0.0.1:6001/app/app-key?protocol=2"
        );
    }

    #[test]
    fn test_events_url() {
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "my-app".to_string(),
            "key".to_string(),
            "secret".to_string(),
        );
        assert_eq!(
            provider.events_url(),
            "http://127.0.0.1:6001/apps/my-app/events"
        );
    }

    #[test]
    fn test_health_url() {
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "my-app".to_string(),
            "key".to_string(),
            "secret".to_string(),
        );
        assert_eq!(provider.health_url(), "http://127.0.0.1:6001/up/my-app");
    }

    #[test]
    fn test_sign_request_structure() {
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "app-id".to_string(),
            "my-key".to_string(),
            "my-secret".to_string(),
        );
        let params = provider.sign_request("POST", "/apps/app-id/events", r#"{"test":true}"#);

        // Must have 5 params: auth_key, auth_timestamp, auth_version, body_md5, auth_signature
        assert_eq!(params.len(), 5);

        // auth_key must match app_key
        let auth_key = params.iter().find(|(k, _)| k == "auth_key").unwrap();
        assert_eq!(auth_key.1, "my-key");

        // auth_version must be 1.0
        let auth_version = params.iter().find(|(k, _)| k == "auth_version").unwrap();
        assert_eq!(auth_version.1, "1.0");

        // auth_signature must be present and non-empty
        let auth_sig = params.iter().find(|(k, _)| k == "auth_signature").unwrap();
        assert!(!auth_sig.1.is_empty());

        // body_md5 must be the MD5 hex of the body
        let body_md5 = params.iter().find(|(k, _)| k == "body_md5").unwrap();
        let expected = format!("{:x}", md5::compute(r#"{"test":true}"#.as_bytes()));
        assert_eq!(body_md5.1, expected);
    }

    #[test]
    fn test_sign_request_sorted_params() {
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "app-id".to_string(),
            "key".to_string(),
            "secret".to_string(),
        );
        let params = provider.sign_request("GET", "/apps/app-id/events", "body");

        // The first 4 params (excluding auth_signature which is last) must be
        // sorted alphabetically by key
        let sorted_keys: Vec<&str> = params[..4].iter().map(|(k, _)| k.as_str()).collect();
        let mut expected = sorted_keys.to_vec();
        expected.sort();
        assert_eq!(sorted_keys, expected);
    }

    #[test]
    fn test_sign_request_deterministic() {
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "app-id".to_string(),
            "key".to_string(),
            "secret".to_string(),
        );
        // Same request → same signature (when timestamp is the same second)
        let p1 = provider.sign_request("POST", "/apps/app-id/events", "body");
        let p2 = provider.sign_request("POST", "/apps/app-id/events", "body");

        let sig1 = p1
            .iter()
            .find(|(k, _)| k == "auth_signature")
            .unwrap()
            .1
            .clone();
        let sig2 = p2
            .iter()
            .find(|(k, _)| k == "auth_signature")
            .unwrap()
            .1
            .clone();

        // Signatures should match if timestamps are in the same second
        let ts1 = p1
            .iter()
            .find(|(k, _)| k == "auth_timestamp")
            .unwrap()
            .1
            .clone();
        let ts2 = p2
            .iter()
            .find(|(k, _)| k == "auth_timestamp")
            .unwrap()
            .1
            .clone();
        if ts1 == ts2 {
            assert_eq!(sig1, sig2);
        }
        // If timestamps differ (edge of second boundary), signatures differ — that's fine
    }

    #[test]
    fn test_sign_request_different_bodies() {
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "app-id".to_string(),
            "key".to_string(),
            "secret".to_string(),
        );
        let p1 = provider.sign_request("POST", "/apps/app-id/events", "body1");
        let p2 = provider.sign_request("POST", "/apps/app-id/events", "body2");

        let sig1 = p1
            .iter()
            .find(|(k, _)| k == "auth_signature")
            .unwrap()
            .1
            .clone();
        let sig2 = p2
            .iter()
            .find(|(k, _)| k == "auth_signature")
            .unwrap()
            .1
            .clone();

        // Different bodies → different signatures (if same timestamp second)
        let ts1 = p1
            .iter()
            .find(|(k, _)| k == "auth_timestamp")
            .unwrap()
            .1
            .clone();
        let ts2 = p2
            .iter()
            .find(|(k, _)| k == "auth_timestamp")
            .unwrap()
            .1
            .clone();
        if ts1 == ts2 {
            assert_ne!(sig1, sig2);
        }
    }

    #[test]
    fn test_sign_request_different_methods() {
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "app-id".to_string(),
            "key".to_string(),
            "secret".to_string(),
        );
        let p1 = provider.sign_request("POST", "/apps/app-id/events", "body");
        let p2 = provider.sign_request("GET", "/apps/app-id/events", "body");

        let sig1 = p1
            .iter()
            .find(|(k, _)| k == "auth_signature")
            .unwrap()
            .1
            .clone();
        let sig2 = p2
            .iter()
            .find(|(k, _)| k == "auth_signature")
            .unwrap()
            .1
            .clone();

        let ts1 = p1
            .iter()
            .find(|(k, _)| k == "auth_timestamp")
            .unwrap()
            .1
            .clone();
        let ts2 = p2
            .iter()
            .find(|(k, _)| k == "auth_timestamp")
            .unwrap()
            .1
            .clone();
        if ts1 == ts2 {
            assert_ne!(sig1, sig2);
        }
    }

    #[test]
    fn test_versioned_message_deserialize() {
        let json = r#"{
            "content": "Hello, world!",
            "action": "message.create",
            "version": 1,
            "serial": "msg-001"
        }"#;
        let vm: VersionedMessage = serde_json::from_str(json).unwrap();
        assert_eq!(vm.content.as_deref(), Some("Hello, world!"));
        assert_eq!(vm.action.as_deref(), Some("message.create"));
        assert_eq!(vm.version, Some(1));
        assert_eq!(vm.serial.as_deref(), Some("msg-001"));
    }

    #[test]
    fn test_versioned_message_minimal() {
        let json = r#"{"content": "chunk"}"#;
        let vm: VersionedMessage = serde_json::from_str(json).unwrap();
        assert_eq!(vm.content.as_deref(), Some("chunk"));
        assert!(vm.action.is_none());
        assert!(vm.version.is_none());
    }

    #[test]
    fn test_versioned_message_with_tool_calls() {
        let json = r#"{
            "content": "",
            "tool_calls": [
                {
                    "function": {
                        "name": "ls",
                        "arguments": {"path": "/"}
                    }
                }
            ],
            "is_final": true
        }"#;
        let vm: VersionedMessage = serde_json::from_str(json).unwrap();
        assert!(vm.is_final == Some(true));
        assert!(vm.tool_calls.is_some());
        let tcs = parse_tool_calls(vm.tool_calls.as_ref().unwrap());
        assert!(tcs.is_some());
        assert_eq!(tcs.unwrap()[0].function.name, "ls");
    }

    #[test]
    fn test_versioned_message_with_usage() {
        let json = r#"{
            "content": "",
            "is_final": true,
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 50,
                "total_tokens": 150
            }
        }"#;
        let vm: VersionedMessage = serde_json::from_str(json).unwrap();
        assert!(vm.usage.is_some());
        let u = vm.usage.unwrap();
        assert_eq!(u.prompt_tokens, 100);
        assert_eq!(u.completion_tokens, 50);
        assert_eq!(u.total_tokens, 150);
    }

    #[test]
    fn test_ws_event_deserialize() {
        let json = r#"{
            "event": "pusher:connection_established",
            "data": "{\"socket_id\":\"12345.67890\",\"activity_timeout\":120}"
        }"#;
        let event: WsEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event, "pusher:connection_established");
        // Verify the data field is a valid JSON string with socket_id
        let data: serde_json::Value = serde_json::from_str(&event.data).unwrap();
        assert_eq!(data["socket_id"], "12345.67890");
    }

    #[test]
    fn test_ws_event_with_channel() {
        let json = r#"{
            "event": "sockudo:message.append",
            "data": "{\"content\":\"hello\"}",
            "channel": "ai-output-abc"
        }"#;
        let event: WsEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event, "sockudo:message.append");
        assert_eq!(event.channel.as_deref(), Some("ai-output-abc"));
    }

    #[test]
    fn test_build_ai_input_with_tool_call_messages() {
        let mut msg = Message::simple(Role::Assistant, "I'll list files.");
        msg.tool_calls = vec![ToolCall {
            id: Some("call-42".to_string()),
            function: ToolCallFunction {
                name: "ls".to_string(),
                arguments: serde_json::json!({"path": "/"}),
                thought_signature: None,
            },
        }];
        let mut tool_msg = Message::simple(Role::Tool, "file1\nfile2");
        tool_msg.tool_call_id = Some("call-42".to_string());
        let messages = vec![Message::simple(Role::User, "List /"), msg, tool_msg];
        let result = build_ai_input("m", &messages, &[]);
        assert_eq!(result["messages"].as_array().unwrap().len(), 3);
        assert_eq!(result["messages"][1]["role"], "assistant");
        assert!(result["messages"][1]["tool_calls"].is_array());
        // Tool calls must include id and type fields for OpenAI-compatible backends
        let tc = &result["messages"][1]["tool_calls"][0];
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["id"], "call-42");
        assert_eq!(tc["function"]["name"], "ls");
        assert_eq!(result["messages"][2]["role"], "tool");
        // Tool messages must include tool_call_id matching the assistant's tool call
        assert_eq!(result["messages"][2]["tool_call_id"], "call-42");
    }

    #[test]
    fn test_provider_select_and_current_model() {
        let mut p = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "app".to_string(),
            "key".to_string(),
            "secret".to_string(),
        );
        assert!(p.current_model().is_none());
        p.select_model("gpt-4".to_string());
        assert_eq!(p.current_model(), Some("gpt-4".to_string()));
    }

    #[test]
    fn test_provider_set_timeout() {
        let mut p = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "app".to_string(),
            "key".to_string(),
            "secret".to_string(),
        );
        assert_eq!(p.timeout_secs, 120);
        p.set_timeout(60);
        assert_eq!(p.timeout_secs, 60);
    }

    #[test]
    fn test_pusher_error_data_deserialize() {
        let json = r#"{"message":"Invalid signature","code":4003}"#;
        let err: PusherErrorData = serde_json::from_str(json).unwrap();
        assert_eq!(err.message, "Invalid signature");
        assert_eq!(err.code, Some(4003));
    }

    #[test]
    fn test_pusher_error_data_no_code() {
        let json = r#"{"message":"Bad request"}"#;
        let err: PusherErrorData = serde_json::from_str(json).unwrap();
        assert_eq!(err.message, "Bad request");
        assert!(err.code.is_none());
    }

    #[test]
    fn test_ws_event_with_extras_transport_model() {
        let json = r#"{
            "event": "sockudo:message.append",
            "data": "{\"content\":\"hello\"}",
            "extras": {
                "ai": {
                    "transport": {
                        "codec-message-id": "msg-1",
                        "stream": "true",
                        "status": "streaming",
                        "model": "qwen2.5:0.5b"
                    }
                }
            }
        }"#;
        let event: WsEvent = serde_json::from_str(json).unwrap();
        assert_eq!(event.event, "sockudo:message.append");
        let extras = event.extras.expect("extras should be present");
        assert_eq!(extras.transport_model(), Some("qwen2.5:0.5b".to_string()));
    }

    #[test]
    fn test_ws_event_without_extras() {
        let json = r#"{
            "event": "pusher:connection_established",
            "data": "{\"socket_id\":\"12345\"}"
        }"#;
        let event: WsEvent = serde_json::from_str(json).unwrap();
        assert!(event.extras.is_none());
    }

    #[test]
    fn test_ws_event_extras_without_model() {
        let json = r#"{
            "event": "sockudo:message.append",
            "data": "{\"content\":\"hi\"}",
            "extras": {
                "ai": {
                    "transport": {
                        "codec-message-id": "msg-2",
                        "status": "streaming"
                    }
                }
            }
        }"#;
        let event: WsEvent = serde_json::from_str(json).unwrap();
        let extras = event.extras.expect("extras should be present");
        assert_eq!(extras.transport_model(), None);
    }

    #[test]
    fn test_build_ai_input_includes_model_for_extras() {
        // build_ai_input puts the model in the data payload; publish_ai_input
        // extracts it and adds extras.ai.transport.model. Verify the data
        // payload has the model field that publish_ai_input looks for.
        let messages = vec![Message::simple(Role::User, "Hello")];
        let result = build_ai_input("my-model", &messages, &[]);
        assert_eq!(result["model"], "my-model");
    }

    #[test]
    fn test_publish_ai_input_body_includes_model_in_extras() {
        // Verify that publish_ai_input includes extras.ai.transport.model
        // by checking the serialized body it would send.
        let provider = SockudoProvider::new(
            "http://127.0.0.1:6001".to_string(),
            "app-id".to_string(),
            "key".to_string(),
            "secret".to_string(),
        );

        let data = serde_json::json!({
            "model": "qwen2.5:0.5b",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true,
        });

        // Reproduce the body construction logic from publish_ai_input
        let data_str = serde_json::to_string(&data).unwrap();
        let mut body_json = serde_json::json!({
            "name": "ai-input",
            "channel": "ai-test",
            "data": data_str,
        });
        if let Some(model) = data.get("model").and_then(|m| m.as_str())
            && !model.is_empty()
        {
            body_json["extras"] = serde_json::json!({
                "ai": {
                    "transport": {
                        "model": model
                    }
                }
            });
        }

        assert_eq!(
            body_json["extras"]["ai"]["transport"]["model"],
            "qwen2.5:0.5b"
        );
        assert_eq!(body_json["name"], "ai-input");
        assert_eq!(body_json["channel"], "ai-test");

        // Verify the data field is a JSON string (Pusher protocol)
        let parsed_data: serde_json::Value =
            serde_json::from_str(body_json["data"].as_str().unwrap()).unwrap();
        assert_eq!(parsed_data["model"], "qwen2.5:0.5b");

        let _ = provider; // suppress unused warning
    }

    #[test]
    fn test_publish_ai_input_body_no_extras_without_model() {
        // When the data payload has no model field, extras should not be added.
        let data = serde_json::json!({
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true,
        });

        let data_str = serde_json::to_string(&data).unwrap();
        let mut body_json = serde_json::json!({
            "name": "ai-input",
            "channel": "ai-test",
            "data": data_str,
        });
        if let Some(model) = data.get("model").and_then(|m| m.as_str())
            && !model.is_empty()
        {
            body_json["extras"] = serde_json::json!({
                "ai": {
                    "transport": {
                        "model": model
                    }
                }
            });
        }

        assert!(body_json.get("extras").is_none());
    }

    #[test]
    fn test_publish_ai_input_body_no_extras_with_empty_model() {
        // When the model is an empty string, extras should not be added
        // (Sockudo rejects empty model values).
        let data = serde_json::json!({
            "model": "",
            "messages": [{"role": "user", "content": "hi"}],
            "stream": true,
        });

        let data_str = serde_json::to_string(&data).unwrap();
        let mut body_json = serde_json::json!({
            "name": "ai-input",
            "channel": "ai-test",
            "data": data_str,
        });
        if let Some(model) = data.get("model").and_then(|m| m.as_str())
            && !model.is_empty()
        {
            body_json["extras"] = serde_json::json!({
                "ai": {
                    "transport": {
                        "model": model
                    }
                }
            });
        }

        assert!(body_json.get("extras").is_none());
    }
}
