# Sockudo AI Transport — Integration Tests

This directory contains a Docker-based test harness for the Sockudo AI Transport provider.
Tests are written in **Rust** and exercise the real `SockudoProvider` implementation.

## What's here

```
tests/sockudo/
├── docker-compose.yml           Sockudo container (in-memory drivers, no Redis)
├── config/
│   └── config.toml              Sockudo config with AI Transport enabled
├── run-test.sh                  Test runner script
└── README.md                    This file

tinyharness-lib/tests/
└── sockudo_integration.rs       Rust integration tests (uses SockudoProvider directly)
```

## Prerequisites

- **Ollama running locally** on port 11434 (`ollama serve`)
- **Docker + Docker Compose** for the Sockudo container

## Quick start

```bash
# Full cycle: ensure Ollama → pull model → start Sockudo → run tests → tear down
./tests/sockudo/run-test.sh run

# Or step by step:
./tests/sockudo/run-test.sh up       # Start Sockudo container only
./tests/sockudo/run-test.sh pull     # Pull test model into local Ollama
./tests/sockudo/run-test.sh test     # Run Rust integration tests
./tests/sockudo/run-test.sh down     # Stop and remove Sockudo container
```

## What the Rust tests exercise

The integration tests in `tinyharness-lib/tests/sockudo_integration.rs` use the
`SockudoProvider` struct directly (not via Python or shell):

| # | Test | What it checks |
|---|------|---------------|
| 1 | `test_health_check` | `Provider::health_check()` returns Ok against live Sockudo |
| 2 | `test_url_construction` | `ws_url()`, `events_url()`, `health_url()` produce correct URLs |
| 3 | `test_signed_request_produces_valid_auth` | `sign_request()` generates all 5 auth params with correct values |
| 4 | `test_publish_event_succeeds` | `publish_ai_input()` publishes an event via signed HTTP POST |
| 5 | `test_unsigned_request_is_rejected` | Raw unsigned HTTP request is rejected with 401/403 |
| 6 | `test_provider_trait_health_check` | `Provider` trait `health_check()` works |
| 7 | `test_provider_trait_list_models` | `Provider` trait `list_models()` returns empty when no model set |
| 8 | `test_provider_trait_select_and_current_model` | `select_model` + `current_model` round-trip |
| 9 | `test_chat_returns_receiver` | `Provider::chat()` returns Ok(receiver) |
| 10 | `test_chat_without_model_fails` | `chat()` returns Err when no model selected |
| 11 | `test_set_timeout` | `set_timeout()` doesn't panic |
| 12 | `test_signed_request_different_bodies_different_signatures` | Different bodies → different HMAC signatures |

Tests are marked `#[ignore]` so they don't run during normal `cargo test`. The
shell script runs them with `--ignored` flag when a live Sockudo is available.

## Docker stack

Only Sockudo runs in Docker. Ollama is expected on the host.

- **Sockudo** (`sockudo/sockudo:latest`) — WebSocket server with AI Transport enabled.
  Uses in-memory drivers (no Redis/MySQL needed). Reaches the host's Ollama via
  `host.docker.internal:11434`. Exposed on port 6001.

## Configuration

The Sockudo config (`config/config.toml`) sets:
- `[ai_transport] enabled = true` — turns on the AI Transport feature
- `[versioned_messages] enabled = true` — enables `sockudo:message.*` mutations
- App credentials: `test-app` / `test-key` / `test-secret`

## Environment overrides

```bash
SOCKUDO_TEST_MODEL=qwen2.5:0.5b   # Model to pull into Ollama (default: qwen2.5:0.5b)
```