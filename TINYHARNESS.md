# TinyHarness

Lightweight AI assistant framework in Rust with pluggable LLM providers (Ollama, llama.cpp, vLLM) and built-in tool calling.

## Commands

- Build: `cargo build`
- Test: `cargo test`
- Lint: `cargo clippy -- -D warnings`
- Format check: `cargo fmt --check`
- Formating: `cargo fmt --all`
- Install: `make install` (builds release + copies to `~/.local/bin`)
- Run: `cargo run` (Ollama default) or `cargo run -- --llama-cpp` / `--vllm`

## Code Conventions

- Rust edition 2024
- Tools registered in `src/tools/mod.rs` via `ToolManager::register_defaults()`
- Tool definitions live in `src/tools/<name>.rs` — each exposes a `*_tool_entry()` function returning a `Tool`
- Providers implement the `Provider` trait in `src/provider/mod.rs`
- Settings persisted as JSON in `~/.config/tinyharness/settings.json`
- Sessions stored as JSONL in `~/.local/share/tinyharness/sessions/`
- Use `serde` + `schemars` for serialization and tool schema generation
- Error handling: `Result<T, String>` for user-facing errors, `Result<T, Box<dyn Error>>` for internal

## Architecture

```
src/
├── main.rs              Entry point, CLI parsing, provider creation
├── agent.rs             Main interaction loop, tool call dispatch, confirmation UI
├── mode.rs              AgentMode enum (casual/planning/agent/research) with system prompts
├── context.rs           WorkspaceContext — auto-detected project metadata + TINYHARNESS.md loading
├── config/mod.rs        Settings persistence (provider, model, mode, API key)
├── session.rs           JSONL session persistence with UUIDs
├── style.rs             ANSI color constants
├── commands/            Slash command handlers (/help, /mode, /compact, etc.)
├── provider/            Provider trait + implementations (ollama, llama_cpp, vllm, openai_compat)
├── tools/               Tool implementations (ls, read, write, edit, grep, run, glob, web_search, etc.)
└── ui/                  Terminal UI helpers (confirmation prompts, input, diffs)
```

Key flow: `main.rs` → `create_provider()` → `run_agent_loop()` (in `agent.rs`) → streams responses from provider → dispatches tool calls → confirms with user for sensitive tools (write/edit/run/switch_mode) → appends results.

## Agent Modes

| Mode | Tools | Purpose |
|------|-------|---------|
| casual | None | Pure chat, no filesystem access |
| planning | read-only (ls, read, grep, glob, web_search) + switch_mode, question | Analyze & plan, then escalate to agent |
| agent | All tools | Full development access |
| research | read-only + web_search, web_fetch + switch_mode, question | Web research, then escalate |

## Testing

- Framework: built-in `#[test]` + `cargo test`
- Use `tempfile` crate in dev-dependencies for test isolation — tool tests must not write to real filesystem
- Run specific test: `cargo test <test_name>`

## Important Rules

- Never modify `src/style.rs` ANSI codes without checking terminal compatibility
- `switch_mode` and `question` tools are handled specially in `agent.rs` — they bypass the generic tool execution path
- Confirmation for `run` tool cannot be auto-accepted even with 'a' (auto-accept) — only write/edit can
- System prompt is refreshed after mode switches, file pinning (/add, /drop), and /refresh
- Session auto-saves every 5 messages

## Known Gotchas

- Ollama provider does not do a health check on startup (unlike llama.cpp and vLLM)
- If the saved model is unavailable, auto-select picks the first available model with a warning
- `rustyline` history stored in `~/.local/share/tinyharness/history.txt`
- Web search requires an Ollama API key set via `/apikey`

## Verification Steps

After making changes, run:
1. `cargo fmt --all` — ensure formatting is clean
2. `cargo clippy -- -D warnings` — no clippy warnings
3. `cargo test` — all tests pass
4. `cargo build` — clean release build succeeds