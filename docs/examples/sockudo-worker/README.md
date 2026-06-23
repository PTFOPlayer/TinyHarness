# Sockudo AI Transport Worker (Example)

> ⚠️ **This is an example project**, not part of the TinyHarness Cargo workspace.
> It demonstrates how to build a worker bridge for the Sockudo AI Transport
> provider. The Sockudo provider itself is highly experimental.

A Rust worker that bridges Sockudo AI Transport to Ollama. It connects to a
Sockudo server via WebSocket (Protocol V2), listens for `ai-input` events on
a channel, calls Ollama for inference, and streams responses back as versioned
message mutations (`sockudo:message.create`, `.append`, `.update`) plus
`ai-turn-end`.

This is the server-side counterpart to the `SockudoProvider` in
`tinyharness-lib`:

- **SockudoProvider** (client): publishes `ai-input`, subscribes for `ai-output`
- **SockudoWorker** (server): receives `ai-input`, calls Ollama, publishes `ai-output`

## Architecture

```
┌──────────────┐     ai-input      ┌─────────┐     /api/chat     ┌─────────┐
│  SockudoProv │ ───────────────▶  │ Sockudo │  ──────────────▶  │ Ollama  │
│  (client)    │                   │ Server  │                   │         │
│              │  ◀──────────────  │         │  ◀──────────────  │         │
│              │  ai-output stream │         │  streamed chunks   └─────────┘
└──────────────┘                   └─────────┘                        ▲
                                       ▲                              │
                                       │ WebSocket (V2)               │ HTTP
                                       │ subscribes to ai-output      │
                                       │                              │
                                  ┌────────────┐                     │
                                  │ SockudoWkr │ ────────────────────┘
                                  │ (example)  │
                                  └────────────┘
```

## Quick Start

```bash
# 1. Start Sockudo (with AI Transport enabled)
#    From the TinyHarness repo root:
./tests/sockudo/run-test.sh up

# 2. Start Ollama
ollama serve

# 3. Pull a model
ollama pull qwen2.5:0.5b

# 4. Build and run the worker (from this directory)
cd docs/examples/sockudo-worker
cargo run

# 5. In another terminal, run tinyharness with the Sockudo provider
#    From the TinyHarness repo root:
cargo run -- --sockudo --url http://127.0.0.1:6001
```

## Configuration

All options can be set via CLI flags or environment variables:

| Flag | Env Var | Default | Description |
|------|---------|---------|-------------|
| `--sockudo-url` | `SOCKUDO_URL` | `http://127.0.0.1:6001` | Sockudo server URL |
| `--app-id` | `SOCKUDO_APP_ID` | `test-app` | Sockudo app ID |
| `--app-key` | `SOCKUDO_APP_KEY` | `test-key` | Sockudo app key |
| `--app-secret` | `SOCKUDO_APP_SECRET` | `test-secret` | Sockudo app secret |
| `--ollama-url` | `OLLAMA_URL` | `http://127.0.0.1:11434` | Ollama server URL |
| `--model` | `SOCKUDO_WORKER_MODEL` | `qwen2.5:0.5b` | Default Ollama model |
| `--channel` | `SOCKUDO_CHANNEL` | `ai-output` | Channel to subscribe on |
| `--ws-timeout` | `SOCKUDO_WS_TIMEOUT` | `120` | WebSocket timeout (seconds) |
| `--no-stream` | `SOCKUDO_WORKER_NO_STREAM` | (unset) | Disable streaming mode |

## How It Works

1. **Connect**: The worker connects to Sockudo via WebSocket using Protocol V2
   (`?protocol=2`), which is required for AI Transport.

2. **Subscribe**: It subscribes to the configured channel (default: `ai-output`).

3. **Receive**: When a client publishes an `ai-input` event to the channel,
   the worker receives it via WebSocket.

4. **Infer**: The worker calls Ollama's `/api/chat` endpoint with the messages
   from the `ai-input` payload. In streaming mode, it reads the NDJSON stream
   and yields content deltas as they arrive.

5. **Publish Back**: The worker publishes the response back to the same channel
   as versioned message mutations:
   - `sockudo:message.create` — empty message, streaming status
   - `sockudo:message.append` — each token chunk (streaming mode)
   - `sockudo:message.update` — final full content, complete status
   - `ai-turn-end` — signals the turn is complete

6. **Reconnect**: If the WebSocket connection drops, the worker reconnects
   with exponential backoff (1s → 2s → 4s → ... → 30s max).

## Sockudo Config Requirements

The Sockudo server must have:

- `[ai_transport] enabled = true`
- `[versioned_messages] enabled = true`
- `[history] enabled = true`
- Channel prefix matching the worker's channel (default: `ai-`)
- App credentials matching `--app-id`, `--app-key`, `--app-secret`

See `../../../tests/sockudo/config/config.toml` for a working configuration.