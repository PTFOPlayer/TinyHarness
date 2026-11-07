# TinyHarness

Lightweight AI assistant framework in Rust with pluggable LLM providers (Ollama, llama.cpp, vLLM, OpenAI-compatible gateways, and experimental Sockudo AI Transport) and built-in tool calling.

## Commands

- Build: `cargo build`
- Test: `cargo test --workspace`
- Lint: `cargo clippy --workspace -- -D warnings`
- Format check: `cargo fmt --all -- --check`
- Formatting: `cargo fmt --all`
- Install: `make install` (builds release + copies to `~/.local/bin`)
- Run: `cargo run` (Ollama default) or `cargo run -- --llama-cpp` / `--vllm` / `--openai-compat --url <url> --api-key <key>` / `--sockudo`

## Workspace Structure

Three crates in a Cargo workspace:

- **`tinyharness-lib`** — Core library: providers, tools, sessions, context, skills, tokens. No terminal I/O.
- **`tinyharness-ui`** — UI library: ANSI output, confirmation prompts, diff display, command input.
- **`TinyHarness`** — Binary CLI: agent loop, slash commands, tool dispatch, setup.

### Key `tinyharness-lib` modules

- `provider/` — Provider trait, `OllamaProvider` (raw SSE, Gemini signatures), `OpenAiCompatProvider` (unified llama.cpp / vLLM / Bearer-auth hosted gateways — built on shared `OpenAiCompatInner`), `SockudoProvider` (WebSocket, ⚠️ experimental). `ToolCall` carries optional `id`, `Message` carries optional `tool_call_id`.
- `tools/` — 15 tools (ls, read, write, edit, grep, glob, run, web_search, web_fetch, switch_mode, question, auto_compact, invoke_skill, screenshot), registration in `register_defaults()`, mode-based filtering, `ToolAvailability` (opt-out flags for `auto_compact`/`question`, both default on), `register_custom_tools()` for custom tools
- `custom_tools/` — Custom-tool system: `CustomToolManager` (loads the global `custom_tools.json` only — project-level files are not discovered), `CustomToolDefinition`/`CustomToolCategory`, `ShellCommand` (template substitution, shell escaping, timeout, `TH_*` env vars)
- `paths.rs` — `Paths`: the **only** place that reads `$HOME`/`$XDG_*`/CWD. Config/data/project roots + every derived file (settings, prompts, sessions, custom tools, skills). Built via `default()`, `with_home()`, `from_env()`, or `new()` + `with_*` builders
- `session.rs` — JSONL persistence, auto-save every 5 messages
- `context.rs` — Workspace metadata + instruction file discovery (TINYHARNESS.md → .tinyharness.md → AGENTS.md → CLAUDE.md)
- `skill.rs` — Skill discovery from `<config_dir>/skills/`, legacy `~/.tinyharness/skills/`, and `.tinyharness/skills/` (project wins)
- `mode.rs` — Agent modes with `.md` system prompts
- `secret.rs` — `SecretString` wrapper for API key redaction (custom `Debug` impl, serde support)
- `path_guard.rs` — `PathGuard`: canonicalized-root path containment (symlink resolution in `open(2)` component order, so `..` after a symlink cannot escape) for the built-in file tools; the guard returns the resolved path, which `ToolManager` substitutes into the tool arguments so handlers never re-resolve a relative path against the process cwd; `ls`/`read`/`write`/`edit` additionally verify the open file descriptor's real path (`/proc/self/fd`, `F_GETPATH` on macOS) so a symlink swapped in after the check still fails closed; `allowed_paths` widens it (global-only, never from a project config), `DEFAULT_BLOCKED_PREFIXES` caps how far (an entry containing a blocked prefix, like `/`, is rejected too). Always on, no flag
- `sandbox.rs` — `Sandbox`/`SandboxBackend`/`SandboxOptions`: OS-level process sandbox for shell tools. Auto-detects bwrap (preferred: tmpfs root + explicit binds, so unbound paths don't exist) or firejail (read-only host root: writes blocked, reads still work). Builds argv, exposes `probe()` for a startup self-check
- `config/mod.rs` — SettingsStore, ProviderKind (ollama/llamacpp/vllm/openai-compat/sockudo), OllamaThinkType, AutoAcceptMode (off/safe/all), `MergedSettings::tool_availability()`

**Standalone library use**: every subsystem has an injectable entry point taking `&Paths` (`SettingsStore::from_paths`, `SessionStore::from_paths`, `load_settings_from`, `save_settings_to`, `load_merged_settings_from`, `prompts_dir_for`, `ensure_prompts_initialized_in`, `discover_skills_in`, `SkillRegistry::discover_in`, `CustomToolManager::load_from`, `WorkspaceContext::collect_in`, `ImageAttachment::load_from_str_in`, `AgentMode::load_system_prompt(&dir)`). The no-arg forms (`load_settings()`, `SessionStore::default_path()`, `discover_skills()`, `WorkspaceContext::collect()`, …) are thin wrappers over `Paths::default()` and must never be called from code that already holds a `Paths`. New filesystem locations go in `paths.rs`; do not read `std::env` directly in `tinyharness-lib` (documented exceptions: `TINYHARNESS_MD_FILES` in `resolve_project_md_files`, the CWD fallback in `ImageAttachment::load_from_str`, and the still-ambient `load_settings()` call inside `tools/web_search.rs`). See `docs/contributing.md` → "Using tinyharness-lib Standalone".

**Binary handlers never call the no-arg wrappers.** All of `src/commands/` and `src/agent/` resolve settings through the injected instance: `&ctx.paths` where a `CommandContext` exists (including inside `async_command!`, where `ctx` is moved into the future), an explicit `paths: &Paths` parameter passed from the registration closure in `src/commands/mod.rs` otherwise, or `&Paths` threaded from `main.rs` for startup code (`src/agent/setup.rs`) that runs before a `CommandContext` exists. Helpers needing one value take that value instead of the whole `Paths` (e.g. `display.rs` takes `context_limit: Option<u32>`). `save_settings_to` returns `Result` — wrap it in `if let Err(e) = … { tracing::warn!("Failed to save settings: {e}"); }` to preserve the old log-and-continue behaviour. Enforced by the `paths-guard` CI job (`grep -rn "load_settings()\|save_settings(\|load_merged_settings()" src/` must be empty).

### Binary crate structure

- `src/agent/` — Agent loop (`mod.rs`), tool execution (`tools.rs`), safety checks (`safety.rs`), display (`display.rs`), multi-line input (`input.rs`), provider setup (`setup.rs`), confirmation prompts (`confirm.rs`), signal tool handling (`signal.rs`), tool result formatting (`tool_result.rs`), command result types (`command_result.rs`)
- `src/commands/` — 24+ slash commands (mode, model, sessions, compact, init, context, files, image, skill, settings, help, debug, project-settings, autocompact, etc.), `CommandRegistry` and `async_command!` macro

## Code Conventions

- Rust edition 2024
- Core logic (`tinyharness-lib`) must not use terminal I/O, ANSI codes, or rustyline
- Use `serde` + `schemars` for serialization and tool schema generation
- Provider async methods use RPITIT (`-> impl Future<...> + Send`) in the `Provider` trait — no `async-trait`, no `Pin<Box<dyn Future>>`. The trait is not dyn-compatible; use the `AnyProvider` enum (concrete wrapper over Ollama / OpenAI-compat / Sockudo) for shared `Arc<Mutex<AnyProvider>>` state. The test mock (`MockProvider`, feature `test-util`) is an `AnyProvider::Mock` variant.
- Error handling: `Result<T, String>` for user-facing, `Result<T, Box<dyn Error>>` for internal
- Minimize dependencies; avoid adding new crates when existing ones suffice
- `#[macro_export]` macros (`extract_args!`) live at `tinyharness_lib` root, not inside `tools`
- Tool categories: `ReadOnly` (auto-executed), `Destructive` (requires confirmation), `Signal` (handled specially by agent loop)

## Architecture

1. `main.rs` → parse CLI, create provider, health check, auto-select model, collect workspace context, initialize prompts, register tools (+ custom tools from `CustomToolManager::load()`), load/create session, build command registry, enter `run_agent_loop()`
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
- `tinyharness-lib` has good coverage (~186 tests + 13 ignored Sockudo integration tests); `tinyharness-ui` covers output formatting, wrapping, diffs, and confirmation prompts (~45 tests); binary crate has ~186 tests (see `todo/01-testing-gaps.md`)
- Use `tempfile` for test isolation; tool tests must not touch the real filesystem
- Run specific test: `cargo test <test_name>`
- Run per crate: `cargo test -p tinyharness-lib`, `cargo test -p TinyHarness`, `cargo test -p tinyharness-ui`

## Important Rules & Gotchas

- **Provider startup**: All providers run a health check (Ollama calls `list_local_models`). OpenAI-compatible providers probe `/health` and fall back to `/v1/models` on HTTP 404 — a non-empty model list counts as healthy (empty list or a failing `/v1/models` still fails). A failed health check is a non-fatal warning — the agent starts anyway and errors surface on the first request. Use `--skip-health-check` to skip the check entirely. If saved model is unavailable, auto-select picks the first available with a warning. OpenRouter-style variant suffixes (`:free`, `:floor`, `:batch`) are ignored when validating a model against the server's list (`tinyharness-lib/src/provider/model_variants.rs`). `--openai-compat` requires `--api-key` (or `OPENAI_API_KEY` env var) and `--url`.
- **Ollama specifics**: Own raw SSE parser (not ollama-rs streaming) to handle native and OpenAI-compatible formats; captures Gemini `thought_signature` from tool responses and re-injects them; fixes serialization quirks (lowercases tool type, injects `name` in tool results, synthesizes `tool_call_id` when missing for OpenAI-compatible servers).
- **System prompts**: Assembled from `header.md` + `<mode>.md` for Agent/Planning/Research; Casual is self-contained. Prompts are refreshed on mode switch, file pinning changes, skill activation, and `/refresh`.
- **Command safety** (`src/agent/safety.rs`): Prefix matching with word boundaries, deny list priority, strips redirections before matching; rejects `;`, `&`, `|`, `$()`, backticks, newlines. Redirections like `2>&1` are auto-accepted if base command is safe.
- **Confirmation**: `decide_tool_confirmation` (`src/agent/confirm.rs`) is pure logic and knows nothing about sandboxing — containment is enforced at execution time, so it never adds a prompt. Auto-accept has three modes: `off` (always prompt), `safe` (auto-approve safe `run` commands per the prefix lists, prompt otherwise), `all` (approve every destructive tool). The per-turn `a` key approves for the rest of the turn except unsafe `run` commands.
- **Containment** (two independent layers): the **path guard** (`tinyharness-lib/src/path_guard.rs`) confines ls/read/write/edit/grep/glob to the workspace root, checked by `ToolManager::execute_tool_call` *before* the handler runs so it beats auto-accept; `grep` never follows symlinks and `glob` results are filtered through the guard; `run`'s explicit `cwd` is checked too. It is always on and widened by the `allowed_paths` setting. The **process sandbox** (`tinyharness-lib/src/sandbox.rs`) wraps `run` and custom tools in bwrap/firejail, **on by default** (`sandbox.backend` defaults to `auto`); resolution lives in `src/sandbox_setup.rs` (CLI beats settings, `--no-sandbox`/`--sandbox=off` forces off, a missing backend is a fatal startup error rather than a silent downgrade, and the setting is global-only — `ProjectSettings` has no `sandbox` field so a cloned repo cannot downgrade it). Tool handlers are opaque closures, so `ToolManager::set_sandbox` must be called *before* `register_defaults`/`register_custom_tools`. Sandbox state lives on `CommandContext.sandbox: Option<SandboxStatus>`; the system prompt gets a "Sandboxed commands" notice via `build_system_prompt`, and the path guard is deliberately not mentioned there. On Windows/macOS the flag exits early with "Linux-only feature" and the setting is forced off (`cfg(target_os)` gate in `src/main.rs`).
- **Compaction**: `/compact` uses single-pass for ≤200 intermediate messages, cascading (chunk+merge) for larger sessions.
- **Optional tools**: `auto_compact` and `question` can each be hidden from the model via `ToolAvailability` (settings `auto_compact_enabled` / `questions_enabled`, both default `true`; global via `/autocompact` & `/questions`, or per-project via `.tinyharness/config.json`). Filtering happens in `ToolManager::tools_for_mode`, so disabled tools never reach the provider — the agent loop does not re-check at execution time.
- **Context warnings**: Load warnings at 70%/90% thresholds based on last known token count (estimation).
- **Session files**: JSONL (metadata line first, then message lines); malformed lines silently skipped on load; stored in `<data_dir>/sessions/` (`$XDG_DATA_HOME/tinyharness/sessions/`, else `~/.local/share/tinyharness/sessions/`).
- **Web tools**: `web_search` and `web_fetch` use `https://ollama.com/api/web_search` and require an Ollama API key set via `/apikey`.
- **Ctrl+C**: Interrupts current LLM generation; second Ctrl+C exits immediately.
- **Configuration**: Stored as JSON in `<config_dir>/settings.json` (`$XDG_CONFIG_HOME/tinyharness/`, else `~/.config/tinyharness/`). Changed via CLI flags (`--url`, `--api-key`), slash commands (`/model`, `/apikey`, `/timeout`, `/retries`, `/contextlimit`, `/autoaccept`, `/settings`, …), or by editing the file. There is no interactive setup wizard; Sockudo credentials are settings-file-only. Persistent prompts are seeded from embedded defaults into `<config_dir>/prompts/`. All locations resolve through `paths::Paths`.
- **Image attachments**: Base64 data URIs, used by multimodal models; set via `/image`.
- **Notebook reads**: `read` renders `.ipynb` files with binary outputs (`image/*`, `application/pdf`) replaced by placeholders and text values over 4,000 chars truncated — a single cell's base64 figure is otherwise hundreds of KB and can exceed the provider's input-length limit. `from`/`to` index the rendered text, not the file on disk. Invalid JSON falls back to per-line truncation.
- **`async_command!` macro**: Registers commands that need `provider.lock().await`.
- **`CommandResult` variants**: `SwitchSession`, `RenameSession`, `Init`, `SkillUse`, `SkillUnload` carry data back to the agent loop.
- **`CommandContext`** holds shared mutable state: provider, mode, file context, session ID, skill registry, active skills, pending images, thinking toggle, compaction token usage, workspace context, `paths` (all filesystem locations; `prompts_dir()` derives from it), output writer, exit flag.
- **`extract_args!` macro** exported at `tinyharness_lib` root, not in `tools`.
- **Custom tools**: Configured in `~/.config/tinyharness/custom_tools.json` (global) + `.tinyharness/custom_tools.json` (project, extends global), managed by `CustomToolManager`. Custom tools are shell commands with `{param}` substitution (shell-escaped) and `TH_*` env vars. Custom tool names colliding with built-ins are skipped.

## Verification Steps

After making changes, run in order:
1. `cargo fmt --all`
2. `cargo clippy --workspace -- -D warnings`
3. `cargo test --workspace`
4. `cargo build`