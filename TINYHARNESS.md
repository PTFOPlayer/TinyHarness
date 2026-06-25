# TinyHarness

Lightweight AI assistant framework in Rust with pluggable LLM providers (Ollama, llama.cpp, vLLM, OpenAI-compatible gateways, and experimental Sockudo AI Transport), built-in tool calling, and an experimental terminal UI (TUI).

## Commands

- Build: `cargo build`
- Test: `cargo test --workspace`
- Lint: `cargo clippy --workspace -- -D warnings`
- Format check: `cargo fmt --all -- --check`
- Formatting: `cargo fmt --all`
- Install: `make install` (builds release + copies to `~/.local/bin`)
- Run: `cargo run` (Ollama default) or `cargo run -- --llama-cpp` / `--vllm` / `--openai-compat --url <url> --api-key <key>` / `--sockudo`
- TUI (experimental): `cargo run -- --tui`

## Workspace Structure

Three crates in a Cargo workspace:

- **`tinyharness-lib`** — Core library: providers, tools, sessions, context, skills, tokens. No terminal I/O.
- **`tinyharness-ui`** — UI library: ANSI output, confirmation prompts, diff display, command input, experimental TUI subsystem.
- **`TinyHarness`** — Binary CLI: agent loop, slash commands, tool dispatch, setup.

### Key `tinyharness-lib` modules

- `provider/` — Provider trait, `OllamaProvider` (raw SSE, Gemini signatures), `OpenAiCompatProvider` (unified llama.cpp / vLLM / Bearer-auth hosted gateways — built on shared `OpenAiCompatInner`), `SockudoProvider` (WebSocket, ⚠️ experimental). `ToolCall` carries optional `id`, `Message` carries optional `tool_call_id`.
- `tools/` — 15 tools (ls, read, write, edit, grep, glob, run, web_search, web_fetch, switch_mode, question, auto_compact, invoke_skill, screenshot), registration in `register_defaults()`, mode-based filtering
- `session.rs` — JSONL persistence, auto-save every 5 messages
- `context.rs` — Workspace metadata + instruction file discovery (TINYHARNESS.md → .tinyharness.md → AGENTS.md → CLAUDE.md)
- `skill.rs` — Skill discovery from `~/.config/tinyharness/skills/` and `.tinyharness/skills/`
- `mode.rs` — Agent modes with `.md` system prompts
- `config/mod.rs` — SettingsStore, ProviderKind (ollama/llamacpp/vllm/openai-compat/sockudo), OllamaThinkType

### Binary crate structure

- `src/agent/` — Agent loop, tool execution, safety checks, display, multi-line input, provider setup
- `src/agent/tui_loop.rs` — Background agent loop for TUI mode (communicates with TUI via mpsc channels)
- `src/commands/` — 22+ slash commands (mode, model, sessions, compact, init, context, files, image, skill, settings, help, etc.), `CommandRegistry` and `async_command!` macro

## Code Conventions

- Rust edition 2024
- Core logic (`tinyharness-lib`) must not use terminal I/O, ANSI codes, or rustyline
- Use `serde` + `schemars` for serialization and tool schema generation
- Prefer `Pin<Box<dyn Future>>` over `async-trait` to keep dependency tree small
- Error handling: `Result<T, String>` for user-facing, `Result<T, Box<dyn Error>>` for internal
- Minimize dependencies; avoid adding new crates when existing ones suffice
- `#[macro_export]` macros (`extract_args!`) live at `tinyharness_lib` root, not inside `tools`
- Tool categories: `ReadOnly` (auto-executed), `Destructive` (requires confirmation), `Signal` (handled specially by agent loop)

## Architecture

1. `main.rs` → parse CLI, create provider, health check, auto-select model, collect workspace context, initialize prompts, register tools, load/create session, build command registry, enter `run_agent_loop()`
2. Agent loop: read input (or `--prompt`), dispatch slash commands, send messages to provider, stream response, handle tool calls
3. Signal tools (`switch_mode`, `question`, `auto_compact`, `invoke_skill`) bypass generic tool execution and are handled inline
4. Destructive tools prompt for confirmation (except `run` which cannot be auto-accepted); ReadOnly tools run immediately
5. Tool results are batched into a single `Role::Tool` message, appended to conversation. Each tool result carries `tool_call_id` linking it back to the originating `ToolCall.id` (required by OpenAI-compatible servers).
6. Auto-save session every 5 messages; flush on mode switch, session switch, exit

## Agent Modes

| Mode     | Tools | Purpose |
|----------|-------|---------|
| casual   | web_search, web_fetch | Chat with web access |
| planning | ReadOnly + Signal tools | Analyze, plan, escalate to agent |
| agent    | All 15 tools | Full development access |
| research | Same as planning (research-focused prompt) | Web research, then escalate |

## Testing

- `cargo test --workspace` runs all tests
- `tinyharness-lib` has good coverage (~89 tests); `tinyharness-ui` has extensive coverage (~325 tests, including TUI rendering, Unicode width, scroll/clipping, and overflow tests); binary crate has limited coverage (see `todo/01-testing-gaps.md`)
- Use `tempfile` for test isolation; tool tests must not touch the real filesystem
- Run specific test: `cargo test <test_name>`
- Run per crate: `cargo test -p tinyharness-lib`, `cargo test -p TinyHarness`, `cargo test -p tinyharness-ui`

## Important Rules & Gotchas

- **Provider startup**: All providers run a health check (Ollama calls `list_local_models`). If saved model is unavailable, auto-select picks the first available with a warning. Use `--skip-health-check` to bypass (useful for gateways without `/health`). `--openai-compat` requires `--api-key` (or `OPENAI_API_KEY` env var) and `--url`.
- **Ollama specifics**: Own raw SSE parser (not ollama-rs streaming) to handle native and OpenAI-compatible formats; captures Gemini `thought_signature` from tool responses and re-injects them; fixes serialization quirks (lowercases tool type, injects `name` in tool results, synthesizes `tool_call_id` when missing for OpenAI-compatible servers).
- **System prompts**: Assembled from `header.md` + `<mode>.md` for Agent/Planning/Research; Casual is self-contained. Prompts are refreshed on mode switch, file pinning changes, skill activation, and `/refresh`.
- **Command safety** (`src/agent/safety.rs`): Prefix matching with word boundaries, deny list priority, strips redirections before matching; rejects `;`, `&`, `|`, `$()`, backticks, newlines. Redirections like `2>&1` are auto-accepted if base command is safe.
- **Confirmation**: `run` tool cannot be auto-accepted even with 'a' (auto-accept mode); only `write` and `edit` can.
- **Compaction**: `/compact` uses single-pass for ≤200 intermediate messages, cascading (chunk+merge) for larger sessions.
- **Context warnings**: Load warnings at 70%/90% thresholds based on last known token count (estimation).
- **Session files**: JSONL (metadata line first, then message lines); malformed lines silently skipped on load; stored in `~/.local/share/tinyharness/sessions/`.
- **Web tools**: `web_search` and `web_fetch` use `https://ollama.com/api/web_search` and require an Ollama API key set via `/apikey`.
- **Ctrl+C**: Interrupts current LLM generation; second Ctrl+C exits immediately.
- **Configuration**: Set via `--config` (interactive setup), stored as JSON in `~/.config/tinyharness/settings.json`. Persistent prompts are seeded from embedded defaults into `~/.config/tinyharness/prompts/`.
- **Image attachments**: Base64 data URIs, used by multimodal models; set via `/image`.
- **`async_command!` macro**: Registers commands that need `provider.lock().await`.
- **`CommandResult` variants**: `SwitchSession`, `RenameSession`, `Init`, `SkillUse`, `SkillUnload` carry data back to the agent loop.
- **`CommandContext`** holds shared mutable state: provider, mode, file context, session ID, skill registry, active skills, pending images, thinking toggle, compaction token usage.
- **`extract_args!` macro** exported at `tinyharness_lib` root, not in `tools`.

## Verification Steps

After making changes, run in order:
1. `cargo fmt --all`
2. `cargo clippy --workspace -- -D warnings`
3. `cargo test --workspace`
4. `cargo build`