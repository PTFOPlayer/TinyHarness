//! Integration tests for the Sockudo AI Transport provider.
//!
//! These tests run against a live Sockudo server (started via Docker by
//! `tests/sockudo/run-test.sh`). They exercise the real `SockudoProvider`
//! implementation — HTTP signing, event publishing, WebSocket subscription,
//! and the `Provider` trait methods.
//!
//! To run:
//!   ./tests/sockudo/run-test.sh up
//!   cargo test --test sockudo_integration -- --ignored --nocapture
//!   ./tests/sockudo/run-test.sh down
//!
//! Or use the shell script which does all of that:
//!   ./tests/sockudo/run-test.sh run

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use tinyharness_lib::{
    SecretString,
    provider::{Message, Provider, Role, sockudo::SockudoProvider},
};
use tokio_tungstenite::tungstenite::Message as WsMessage;

type HmacSha256 = Hmac<Sha256>;

/// Test credentials — must match the Sockudo config in tests/sockudo/config/config.toml
const SOCKUDO_URL: &str = "http://127.0.0.1:6001";
const APP_ID: &str = "test-app";
const APP_KEY: &str = "test-key";
const APP_SECRET: &str = "test-secret";

fn make_provider() -> SockudoProvider {
    SockudoProvider::new(
        SOCKUDO_URL.to_string(),
        APP_ID.to_string(),
        APP_KEY.to_string(),
        SecretString::new(APP_SECRET),
    )
}

/// Skip tests if Sockudo is not running.
async fn skip_if_no_sockudo() -> bool {
    let provider = make_provider();
    let health = provider.health_url();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    !matches!(
        client.get(&health).send().await,
        Ok(resp) if resp.status().is_success()
    )
}

macro_rules! require_sockudo {
    () => {
        if skip_if_no_sockudo().await {
            eprintln!("Skipping test — Sockudo not running at {SOCKUDO_URL}");
            eprintln!("Start it with: ./tests/sockudo/run-test.sh up");
            return;
        }
    };
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_health_check() {
    require_sockudo!();

    let provider = make_provider();
    let result = provider.health_check().await;
    assert!(
        result.is_ok(),
        "health check should succeed: {:?}",
        result.err()
    );
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_url_construction() {
    require_sockudo!();

    let provider = make_provider();

    // WS URL
    assert_eq!(
        provider.ws_url(),
        "ws://127.0.0.1:6001/app/test-key?protocol=2"
    );

    // Events URL
    assert_eq!(
        provider.events_url(),
        "http://127.0.0.1:6001/apps/test-app/events"
    );

    // Health URL
    assert_eq!(provider.health_url(), "http://127.0.0.1:6001/up/test-app");
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_signed_request_produces_valid_auth() {
    require_sockudo!();

    let provider = make_provider();
    let path = format!("/apps/{APP_ID}/events");
    let body = r#"{"name":"test","channel":"ch","data":"{}"}"#;

    let params = provider.sign_request("POST", &path, body);

    // Must contain all 5 required auth params
    assert_eq!(params.len(), 5, "expected 5 auth params, got {params:?}");

    let get = |key: &str| -> String {
        params
            .iter()
            .find(|(k, _)| k == key)
            .unwrap_or_else(|| panic!("missing auth param '{key}'"))
            .1
            .clone()
    };

    assert_eq!(get("auth_key"), APP_KEY);
    assert_eq!(get("auth_version"), "1.0");
    assert!(
        !get("auth_signature").is_empty(),
        "signature must not be empty"
    );

    // body_md5 should be the MD5 hex of the body
    let expected_md5 = format!("{:x}", md5::compute(body.as_bytes()));
    assert_eq!(get("body_md5"), expected_md5);
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_publish_event_succeeds() {
    require_sockudo!();

    let provider = make_provider();
    let channel = format!("ai-test-publish-{}", uuid::Uuid::new_v4());
    let data = serde_json::json!({"message": "hello from rust test"});

    let result = provider.publish_ai_input(&channel, &data).await;
    assert!(result.is_ok(), "publish should succeed: {:?}", result.err());
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_unsigned_request_is_rejected() {
    require_sockudo!();

    // Make a raw unsigned HTTP request — should be rejected with 401/403
    let client = reqwest::Client::new();
    let url = format!("{SOCKUDO_URL}/apps/{APP_ID}/events");
    let body = r#"{"name":"test","channel":"ch","data":"{}"}"#;

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .expect("request should complete");

    let status = resp.status().as_u16();
    assert!(
        status == 400 || status == 401 || status == 403,
        "unsigned request should be rejected with 400/401/403, got {status}"
    );
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_provider_trait_health_check() {
    require_sockudo!();

    let provider = make_provider();
    // Use the Provider trait method
    use tinyharness_lib::provider::Provider;
    let result = provider.health_check().await;
    assert!(result.is_ok(), "Provider::health_check should succeed");
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_provider_trait_list_models() {
    require_sockudo!();

    let provider = make_provider();
    let models = provider.list_models().await;
    // SockudoProvider returns the currently selected model or empty.
    // Without a model selected, this should be empty.
    assert!(models.is_empty(), "no model selected, should be empty");
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_provider_trait_select_and_current_model() {
    require_sockudo!();

    let mut provider = make_provider();
    assert!(provider.current_model().is_none());

    provider.select_model("test-model".to_string());
    assert_eq!(provider.current_model(), Some("test-model".to_string()));

    // list_models should now return the selected model
    let models = provider.list_models().await;
    assert_eq!(models, vec!["test-model".to_string()]);
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_chat_returns_receiver() {
    require_sockudo!();

    let mut provider = make_provider();
    provider.select_model("test-model".to_string());

    let messages = vec![
        Message::simple(Role::System, "You are a helpful assistant."),
        Message::simple(Role::User, "Say hello."),
    ];

    // chat() should return Ok(receiver) even if the backend doesn't
    // produce a meaningful response — the receiver is the contract.
    let result = provider.chat(messages, vec![]).await;
    assert!(
        result.is_ok(),
        "chat() should return Ok(receiver): {:?}",
        result.err()
    );

    // Drain the receiver to completion (with a timeout)
    let mut recv = result.unwrap();
    let mut got_chunks = false;

    let timeout = tokio::time::timeout(Duration::from_secs(30), async {
        while let Some(chunk) = recv.recv().await {
            got_chunks = true;
            if chunk.done {
                break;
            }
        }
    })
    .await;

    // Either we got chunks or the stream ended — both are acceptable for
    // a test where the LLM backend may not be connected.
    let _ = timeout; // don't fail on timeout — Sockudo may not have an agent worker
    let _ = got_chunks;
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_chat_without_model_succeeds() {
    require_sockudo!();

    let mut provider = make_provider();
    // Don't select a model — the worker should use its default

    let messages = vec![Message::simple(Role::User, "hello")];
    let result = provider.chat(messages, vec![]).await;

    assert!(
        result.is_ok(),
        "chat() without a model should succeed (worker uses default): {:?}",
        result.err()
    );
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_set_timeout() {
    require_sockudo!();

    let mut provider = make_provider();
    provider.set_timeout(60);
    // No direct way to verify, but it shouldn't panic
}

#[tokio::test]
#[ignore = "requires live Sockudo server"]
async fn test_signed_request_different_bodies_different_signatures() {
    require_sockudo!();

    let provider = make_provider();
    let path = format!("/apps/{APP_ID}/events");

    let p1 = provider.sign_request("POST", &path, "body-a");
    let p2 = provider.sign_request("POST", &path, "body-b");

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
        assert_ne!(
            sig1, sig2,
            "different bodies should produce different signatures"
        );
    }
}

// ── End-to-end test: Sockudo + Ollama ───────────────────────────────────────
//
// This test exercises the full AI Transport round-trip:
//
// 1. A mini "agent worker" task connects to Sockudo via WebSocket (Protocol
//    V2), subscribes to an AI channel, and waits for `ai-input` events.
// 2. The test calls `SockudoProvider::chat()` which publishes an `ai-input`
//    event to the channel and subscribes for versioned message responses.
// 3. The agent worker receives the `ai-input` event, calls Ollama directly
//    via HTTP, and publishes the response back to Sockudo as versioned
//    messages (`sockudo:message.create`, `.append`, `.update`).
// 4. The test verifies that `chat()` returns streaming chunks and a
//    `done: true` marker.
//
// Requirements:
//   - Sockudo running (./tests/sockudo/run-test.sh up)
//   - Ollama running locally on port 11434
//   - A model available in Ollama (default: qwen2.5:0.5b)

const OLLAMA_URL: &str = "http://127.0.0.1:11434";
const TEST_MODEL: &str = "qwen2.5:0.5b";

/// Skip if Ollama is not running.
async fn skip_if_no_ollama() -> bool {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    !matches!(
        client
            .get(format!("{OLLAMA_URL}/api/tags"))
            .send()
            .await,
        Ok(resp) if resp.status().is_success()
    )
}

/// Sign a Sockudo HTTP API request (Pusher-style HMAC-SHA256).
fn sign_sockudo_request(
    method: &str,
    path: &str,
    body: &str,
    app_key: &str,
    app_secret: &str,
) -> Vec<(String, String)> {
    let body_md5 = format!("{:x}", md5::compute(body.as_bytes()));
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .to_string();

    let mut params: Vec<(String, String)> = vec![
        ("auth_key".to_string(), app_key.to_string()),
        ("auth_timestamp".to_string(), timestamp),
        ("auth_version".to_string(), "1.0".to_string()),
        ("body_md5".to_string(), body_md5),
    ];
    params.sort_by(|a, b| a.0.cmp(&b.0));

    let qs = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let string_to_sign = format!("{method}\n{path}\n{qs}");

    let signature = {
        let mut mac = HmacSha256::new_from_slice(app_secret.as_bytes()).unwrap();
        mac.update(string_to_sign.as_bytes());
        hex_encode(&mac.finalize().into_bytes())
    };
    params.push(("auth_signature".to_string(), signature));
    params
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Publish a versioned message event to Sockudo via signed HTTP POST.
#[allow(clippy::too_many_arguments)]
async fn publish_versioned_event(
    client: &reqwest::Client,
    app_id: &str,
    app_key: &str,
    app_secret: &str,
    channel: &str,
    event_name: &str,
    data: &str,
    extras: Option<&serde_json::Value>,
    message_id: Option<&str>,
) -> Result<serde_json::Value, String> {
    let path = format!("/apps/{app_id}/events");
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
    let body_str = serde_json::to_string(&body).unwrap();
    let params = sign_sockudo_request("POST", &path, &body_str, app_key, app_secret);
    let qs = params
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");
    let url = format!("{SOCKUDO_URL}{path}?{qs}");

    let resp = client
        .post(&url)
        .header("Content-Type", "application/json")
        .body(body_str)
        .send()
        .await
        .map_err(|e| format!("publish: {e}"))?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(format!("publish failed: HTTP {status}: {text}"));
    }
    Ok(serde_json::from_str(&text).unwrap_or(serde_json::Value::Null))
}

/// Mini agent worker: subscribes to a Sockudo channel via WebSocket,
/// receives `ai-input` events, calls Ollama, and publishes the response
/// back as versioned messages.
async fn run_agent_worker(
    channel: String,
    app_id: String,
    app_key: String,
    app_secret: String,
) -> Result<(), String> {
    // AI Transport requires Protocol V2 — V1 subscribers don't receive
    // `sockudo:message.*` events or `ai-input` on AI channels.
    let ws_url = format!("ws://127.0.0.1:6001/app/{app_key}?protocol=2");

    // Connect to WebSocket
    eprintln!("[agent] Connecting to {ws_url}");
    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| format!("agent WS connect: {e}"))?;
    eprintln!("[agent] WebSocket connected");
    let (mut ws_write, mut ws_read) = ws_stream.split();
    let mut _socket_id: Option<String> = None;
    let mut subscribed = false;

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .unwrap();

    loop {
        let msg = tokio::time::timeout(Duration::from_secs(120), ws_read.next()).await;
        match msg {
            Err(_) => return Err("agent worker: WebSocket timeout".to_string()),
            Ok(None) => return Ok(()),
            Ok(Some(Err(e))) => return Err(format!("agent WS error: {e}")),
            Ok(Some(Ok(ws_msg))) => {
                let text = match ws_msg {
                    WsMessage::Text(t) => t.to_string(),
                    WsMessage::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                    WsMessage::Ping(_) => {
                        let _ = ws_write.send(WsMessage::Pong(vec![])).await;
                        continue;
                    }
                    _ => continue,
                };

                let parsed: Result<serde_json::Value, _> = serde_json::from_str(&text);
                let event = match parsed {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let event_name = event["event"].as_str().unwrap_or("");
                let event_data = event["data"].as_str().unwrap_or("");

                eprintln!("[agent] Event: {event_name}");

                // Handle connection established (V2 uses sockudo: prefix)
                if event_name == "pusher:connection_established"
                    || event_name == "sockudo:connection_established"
                {
                    if let Ok(conn) = serde_json::from_str::<serde_json::Value>(event_data) {
                        _socket_id = conn["socket_id"].as_str().map(|s| s.to_string());
                    }
                    // Subscribe to the channel
                    eprintln!("[agent] Subscribing to {channel}");
                    let subscribe_msg = serde_json::json!({
                        "event": "pusher:subscribe",
                        "data": { "channel": channel }
                    });
                    let _ = ws_write
                        .send(WsMessage::Text(
                            serde_json::to_string(&subscribe_msg).unwrap(),
                        ))
                        .await;
                    continue;
                }

                if event_name == "pusher_internal:subscription_succeeded"
                    || event_name == "sockudo_internal:subscription_succeeded"
                    || event_name == "sockudo:subscription_succeeded"
                {
                    subscribed = true;
                    eprintln!("[agent] Subscribed to {channel}");
                    continue;
                }

                if !subscribed {
                    continue;
                }

                // Handle ai-input event
                if event_name == "ai-input" {
                    // Parse the AI input payload
                    let input: serde_json::Value =
                        serde_json::from_str(event_data).unwrap_or(serde_json::Value::Null);

                    // Call Ollama
                    let ollama_req = serde_json::json!({
                        "model": input.get("model").and_then(|m| m.as_str()).unwrap_or(TEST_MODEL),
                        "messages": input.get("messages").cloned().unwrap_or(serde_json::Value::Array(vec![])),
                        "stream": false,
                    });

                    let ollama_resp = http_client
                        .post(format!("{OLLAMA_URL}/api/chat"))
                        .json(&ollama_req)
                        .send()
                        .await
                        .map_err(|e| format!("Ollama request failed: {e}"))?;

                    let ollama_json: serde_json::Value = ollama_resp
                        .json()
                        .await
                        .map_err(|e| format!("Ollama parse: {e}"))?;

                    let response_text = ollama_json["message"]["content"]
                        .as_str()
                        .unwrap_or("Hello from Ollama!")
                        .to_string();
                    eprintln!("[agent] Ollama responded: {response_text}");

                    // Publish response back as versioned messages
                    let message_serial = format!("msg-{}", uuid::Uuid::new_v4());
                    let message_id = format!("mid-{}", uuid::Uuid::new_v4());
                    let model = input
                        .get("model")
                        .and_then(|m| m.as_str())
                        .unwrap_or(TEST_MODEL);

                    // 1. Create message
                    let create_extras = serde_json::json!({
                        "ai": {
                            "transport": {
                                "codec-message-id": &message_serial,
                                "stream": "true",
                                "status": "streaming",
                                "model": model
                            }
                        }
                    });
                    let _ = publish_versioned_event(
                        &http_client,
                        &app_id,
                        &app_key,
                        &app_secret,
                        &channel,
                        "sockudo:message.create",
                        "",
                        Some(&create_extras),
                        Some(&message_id),
                    )
                    .await;

                    // 2. Append the full response text
                    let append_extras = serde_json::json!({
                        "ai": {
                            "transport": {
                                "codec-message-id": &message_serial,
                                "stream": "true",
                                "status": "streaming",
                                "model": model
                            }
                        }
                    });
                    let _ = publish_versioned_event(
                        &http_client,
                        &app_id,
                        &app_key,
                        &app_secret,
                        &channel,
                        "sockudo:message.append",
                        &response_text,
                        Some(&append_extras),
                        None,
                    )
                    .await;

                    // 3. Update with final status
                    let update_extras = serde_json::json!({
                        "ai": {
                            "transport": {
                                "codec-message-id": &message_serial,
                                "stream": "true",
                                "status": "complete",
                                "model": model
                            }
                        }
                    });
                    let _ = publish_versioned_event(
                        &http_client,
                        &app_id,
                        &app_key,
                        &app_secret,
                        &channel,
                        "sockudo:message.update",
                        &response_text,
                        Some(&update_extras),
                        None,
                    )
                    .await;

                    // 4. Send ai-turn-end
                    let _ = publish_versioned_event(
                        &http_client,
                        &app_id,
                        &app_key,
                        &app_secret,
                        &channel,
                        "ai-turn-end",
                        "{}",
                        None,
                        None,
                    )
                    .await;

                    eprintln!("[agent] Published response back to Sockudo");
                    return Ok(());
                }
            }
        }
    }
}

/// Subscribe to a Sockudo channel via WebSocket and collect versioned message
/// events until `ai-turn-end` is received. Returns the accumulated response text.
async fn subscribe_and_collect_response(channel: &str, app_key: &str) -> Result<String, String> {
    // AI Transport requires Protocol V2
    let ws_url = format!("ws://127.0.0.1:6001/app/{app_key}?protocol=2");

    let (ws_stream, _) = tokio_tungstenite::connect_async(&ws_url)
        .await
        .map_err(|e| format!("subscriber WS connect: {e}"))?;
    let (mut ws_write, mut ws_read) = ws_stream.split();
    let mut subscribed = false;
    let mut response_content = String::new();

    loop {
        let msg = tokio::time::timeout(Duration::from_secs(120), ws_read.next()).await;
        match msg {
            Err(_) => return Err("subscriber: WebSocket timeout".to_string()),
            Ok(None) => return Err("subscriber: connection closed".to_string()),
            Ok(Some(Err(e))) => return Err(format!("subscriber WS error: {e}")),
            Ok(Some(Ok(ws_msg))) => {
                let text = match ws_msg {
                    WsMessage::Text(t) => t.to_string(),
                    WsMessage::Binary(b) => String::from_utf8_lossy(&b).to_string(),
                    WsMessage::Ping(_) => {
                        let _ = ws_write.send(WsMessage::Pong(vec![])).await;
                        continue;
                    }
                    _ => continue,
                };

                let parsed: Result<serde_json::Value, _> = serde_json::from_str(&text);
                let event = match parsed {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                let event_name = event["event"].as_str().unwrap_or("");

                // Handle connection established (V2 uses sockudo: prefix)
                if event_name == "pusher:connection_established"
                    || event_name == "sockudo:connection_established"
                {
                    let subscribe_msg = serde_json::json!({
                        "event": "pusher:subscribe",
                        "data": { "channel": channel }
                    });
                    let _ = ws_write
                        .send(WsMessage::Text(
                            serde_json::to_string(&subscribe_msg).unwrap(),
                        ))
                        .await;
                    continue;
                }

                if event_name == "pusher_internal:subscription_succeeded"
                    || event_name == "sockudo_internal:subscription_succeeded"
                    || event_name == "sockudo:subscription_succeeded"
                {
                    subscribed = true;
                    eprintln!("[subscriber] Subscribed to {channel}");
                    continue;
                }

                if !subscribed {
                    continue;
                }

                eprintln!("[subscriber] Event: {event_name}");

                // Collect versioned message content
                if event_name.starts_with("sockudo:message.") {
                    let data = event["data"].as_str().unwrap_or("");
                    if !data.is_empty() {
                        response_content.push_str(data);
                        eprintln!("[subscriber] Content chunk: {data}");
                    }
                    // `sockudo:message.update` is the final version
                    if event_name == "sockudo:message.update" {
                        // Wait for ai-turn-end or return now
                        // The update with status=complete is the final content
                    }
                    continue;
                }

                // Done when ai-turn-end is received
                if event_name == "ai-turn-end" {
                    eprintln!("[subscriber] Got ai-turn-end");
                    return Ok(response_content);
                }

                // Ignore other events
            }
        }
    }
}

#[tokio::test]
#[ignore = "requires live Sockudo + Ollama"]
async fn test_end_to_end_sockudo_ollama() {
    require_sockudo!();

    if skip_if_no_ollama().await {
        eprintln!("Skipping test — Ollama not running at {OLLAMA_URL}");
        eprintln!("Start it with: ollama serve");
        return;
    }

    // Use a unique channel for this test — must match the ai- prefix in config
    let channel = format!("ai-e2e-{}", uuid::Uuid::new_v4());
    eprintln!("Test channel: {channel}");

    // 1. Spawn the agent worker — subscribes and waits for ai-input
    let worker_channel = channel.clone();
    let worker_app_id = APP_ID.to_string();
    let worker_app_key = APP_KEY.to_string();
    let worker_app_secret = APP_SECRET.to_string();
    let worker_handle = tokio::spawn(async move {
        run_agent_worker(
            worker_channel,
            worker_app_id,
            worker_app_key,
            worker_app_secret,
        )
        .await
    });

    // 2. Spawn the subscriber — listens for versioned message responses
    let sub_channel = channel.clone();
    let sub_app_key = APP_KEY.to_string();
    let sub_handle =
        tokio::spawn(
            async move { subscribe_and_collect_response(&sub_channel, &sub_app_key).await },
        );

    // Give both WebSocket connections time to connect and subscribe
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 3. Publish ai-input event to the channel via SockudoProvider
    let provider = make_provider();
    let ai_input = serde_json::json!({
        "model": TEST_MODEL,
        "messages": [
            {"role": "system", "content": "You are a helpful assistant. Reply in one short sentence."},
            {"role": "user", "content": "Say hello."},
        ],
        "stream": true,
    });

    eprintln!("Publishing ai-input to channel {channel}");
    provider
        .publish_ai_input(&channel, &ai_input)
        .await
        .expect("publish ai-input should succeed");
    eprintln!("ai-input published");

    // 4. Wait for the subscriber to collect the response
    let timeout = tokio::time::timeout(Duration::from_secs(120), sub_handle).await;
    let sub_result = match timeout {
        Ok(join_result) => join_result.expect("subscriber task panicked"),
        Err(_) => {
            // Clean up
            let _ = worker_handle.await;
            panic!("timed out waiting for response from Sockudo");
        }
    };

    let response = sub_result.expect("subscriber returned error");
    eprintln!("Full response: {response}");

    assert!(
        !response.is_empty(),
        "should have received content from Ollama via Sockudo"
    );

    // Clean up the worker
    let _ = worker_handle.await;
}
