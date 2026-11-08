# TinyHarness

Lightweight AI assistant framework in Rust with pluggable LLM providers (Ollama, llama.cpp, vLLM), built-in tool calling, agent skills, and customizable system prompts.

## Features

- **Pluggable Providers**: Ollama, llama.cpp, vLLM, any OpenAI-compatible API gateway (OpenRouter, Together, etc.) with Bearer auth, and ⚠️ Sockudo AI Transport as a highly experimental backend requiring a running Sockudo server and a worker bridge (see `docs/examples/sockudo-worker/`). Ollama supports retries with backoff and configurable timeouts.
- **Tool System**: 11 built-in tools (`ls`, `read`, `write`, `edit`, `grep`, `glob`, `run`, `auto_compact`, `invoke_skill`, `switch_mode`, `question`). The `read` tool also loads images for multimodal models. Ollama talks through its OpenAI-compatible `/v1` API, so there is no Ollama-specific provider code.
- **Custom Tools**: Extend TinyHarness with shell-command-based tools the LLM can call — defined in `custom_tools.json` (`~/.config/tinyharness/custom_tools.json` global only; project-level files are not loaded, since a cloned repo must not register shell commands on your machine), no Rust code or recompilation required. Supports `{param}` template substitution, `TH_*` env vars, `readonly`/`destructive` categories, and timeouts.
- **Agent Modes**: Three modes — `casual` (pure chat, no tools), `planning` (read-only + signals, includes research), and `agent` (full access) — to control what the AI can do. Modes are backed by customizable `.md` prompt files.
- **Skills**: Pluggable SKILL.md modules discovered from `~/.config/tinyharness/skills/` and `.tinyharness/skills/`. Invokable by the AI via `invoke_skill` or by the user via `/use <name>`. Supports YAML frontmatter with name, description, compatibility, licensing, and model-invocation controls.
- **Context Management**: Token estimation with per-model context window sizes (8K–256K), load warnings at 70%/90% thresholds, and cascading conversation compaction via `/compact`.
- **Session Persistence**: JSONL-based sessions with UUIDs, saved in `~/.local/share/tinyharness/sessions/`. Supports session listing, switching by prefix, renaming, deletion, and auto-save every 5 messages.
- **Async Streaming**: Built on `tokio` for efficient streaming with all providers. Ctrl+C interrupts generation gracefully.
- **Interactive CLI**: Color-coded terminal interface with 30+ slash commands for session management, configuration, file pinning, image attachment, audit logging, debug diagnostics, and tool control.
- **Customizable Prompts**: System prompts are seeded from hardcoded defaults on first launch to `~/.config/tinyharness/prompts/` and can be freely edited.
- **Command Safety**: Smart auto-accept for safe shell commands with prefix matching, deny lists, redirection stripping, and audit logging.
- **Thinking Display**: Optionally render the model's reasoning chain inline during streaming (toggle via `/showthink`).
- **Image Attachments**: Multimodal support — attach images with `/image` and the `read` tool automatically loads image files for visual models.
- **Project Instructions**: Auto-discovers `TINYHARNESS.md`, `.tinyharness.md`, `AGENTS.md`, or `CLAUDE.md` walking up from the current directory. Configurable via `TINYHARNESS_MD_FILES` env var or settings. Additional project-specific files (e.g. `RULES.md`, `.cursorrules`) can be loaded from `.tinyharness/config.json`.
- **Per-Project Settings**: Layer `.tinyharness/config.json` over global settings. Override safe/denied commands, auto-accept behavior, context limits, preferred mode, and more per project. View merged settings with `/project-settings`.
- **Smart Language Detection**: Auto-detects 17+ languages and build tools (Rust, Zig, Deno, Bun, Swift, Ruby, Elixir, Haskell, Kotlin, .NET, Dart/Flutter, Nix, Node.js, Python, Go, Java, C/C++). Detects monorepos (e.g. "Rust + Node.js").

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (latest stable, edition 2024)
- At least one LLM backend:
  - [Ollama](https://ollama.com/) (default, local)
  - [llama.cpp](https://github.com/ggml-org/llama.cpp) server (local, no auth)
  - [vLLM](https://github.com/vllm-project/vllm) (local, no auth)
  - Any OpenAI-compatible API gateway (OpenRouter, Together, custom proxies, etc.) — requires an API key and explicit URL
  - [Sockudo](https://github.com/sockudo/sockudo) (⚠️ highly experimental — requires a worker bridge, see `docs/examples/sockudo-worker/`)

### Installation

```bash
git clone https://github.com/yourusername/TinyHarness.git
cd TinyHarness
make install
```

This builds in release mode and copies the binary to `~/.local/bin`. Make sure `~/.local/bin` is in your `$PATH`:

```bash
export PATH="$HOME/.local/bin:$PATH"
```

To uninstall:

```bash
make uninstall
```

Alternatively, install via Cargo:

```bash
cargo install --path .
```

### Installation (Nix)

A Nix flake is provided for reproducible builds and dev environments.

**Run once without installing:**

```bash
nix run github:PTFOPlayer/TinyHarness
```

**Build the package:**

```bash
cd TinyHarness
nix build
./result/bin/tinyharness
```

**Enter a development shell** (with Rust toolchain, rustfmt, and clippy pre-configured):

```bash
nix develop
```

### Usage

**Ollama** (default):
```bash
tinyharness
```
Connects to `http://127.0.0.1:11434`. Supports configurable timeout and retries.

**llama.cpp**:
```bash
tinyharness --llama-cpp
```
Connects to `http://127.0.0.1:8080` by default.

**vLLM**:
```bash
tinyharness --vllm
```
Connects to `http://127.0.0.1:8000` by default.

**OpenAI-compatible gateway** (OpenRouter, Together, custom proxies, etc.):
```bash
tinyharness --openai-compat --url https://openrouter.ai/api/v1 --api-key <YOUR_KEY>
```
Requires `--url` (no default URL) and an API key. You can also set the `OPENAI_API_KEY` environment variable instead of `--api-key`. Bearer auth is sent on every request. The startup health check probes `/health` and, when the server returns 404 (no such endpoint), falls back to `/v1/models` — a non-empty model list is treated as healthy. A failed health check only warns — use `--skip-health-check` if the gateway exposes neither endpoint.

**Sockudo** (⚠️ highly experimental):
```bash
tinyharness --sockudo
```
Connects to `http://127.0.0.1:6001` by default. Requires a running Sockudo server with AI Transport enabled and a worker bridge process (see `docs/examples/sockudo-worker/`). This backend is not recommended for production use.

A health check runs on startup to verify the provider is reachable. If the saved model is unavailable, the first available model is auto-selected with a warning.

**Custom URL** (works with any provider):
```bash
tinyharness --llama-cpp --url http://localhost:2832
tinyharness --ollama --url http://192.168.1.50:11434
tinyharness --vllm --url http://gpu-server:8000
tinyharness --openai-compat --url https://api.example.com/v1 --api-key sk-xxx
```

**Continue last session**:
```bash
tinyharness --continue
```
Resumes the most recent session in the current working directory.

**Non-interactive prompt**:
```bash
tinyharness -p "What does this project do?"
tinyharness --prompt "Explain the architecture"
```
Sends an initial prompt and then drops into the interactive loop for follow-up turns.

### CLI Arguments

| Flag | Description |
|---|---|
| `-o`, `--ollama` | Use the Ollama provider (default) |
| `-l`, `--llama-cpp` | Use the llama.cpp provider |
| `-v`, `--vllm` | Use the vLLM provider |
| `--openai-compat` | Use a generic OpenAI-compatible gateway (requires `--api-key` and `--url`) |
| `--sockudo` | Use the Sockudo AI Transport provider (⚠️ highly experimental) |
| `-u`, `--url <url>` | Custom base URL for the provider |
| `--api-key <key>` | Bearer token for `--openai-compat` (use `-` to clear saved key) |
| `--skip-health-check` | Skip provider health check at startup (a failure is otherwise just a warning) |
| `-c`, `--continue` | Continue the most recent session in the current directory |
| `-p`, `--prompt <text>` | Start with this message, then drop into interactive mode |
| `--sandbox[=<backend>]` | Pin the OS sandbox backend (`auto`, `bwrap`, `firejail`); on by default (Linux only) |
| `--no-sandbox` | Run shell commands **unsandboxed** — the only way to opt out (Linux only) |

## Agent Modes

| Mode | Tools Available | Purpose |
|------|----------------|---------|
| **casual** | (none) | Pure chat, no filesystem access |
| **planning** | All read-only tools + signal tools | Analyze, research & plan, then escalate to agent |
| **agent** | All tools | Full development access — code and commands |

Switch modes with `/mode <name>`, use shortcut aliases (`/plan`, `/agent`, `/research`, `/casual` — `/research` maps to planning), or let the AI request escalation via `switch_mode`.

## Tools

| Tool | Category | Description |
|------|----------|-------------|
| `ls` | ReadOnly | List directory contents |
| `read` | ReadOnly | Read file content (loads images for multimodal models; renders `.ipynb` with base64 outputs stripped) |
| `grep` | ReadOnly | Search with regex across files |
| `glob` | ReadOnly | Find files by glob pattern |
| `write` | Destructive | Write content to a file |
| `edit` | Destructive | Edit a file by find-and-replace |
| `run` | Destructive | Execute shell commands with timeout & working directory |
| `switch_mode` | Signal | Request a mode switch |
| `question` | Signal | Ask the user a question with options |
| `auto_compact` | Signal | Request conversation compaction |
| `invoke_skill` | Signal | Activate a skill by name |

## Slash Commands

### Session Management
| Command | Description |
|---------|-------------|
| `/sessions` | List all saved sessions (most recent first) |
| `/session <id>` | Switch to an existing session by ID prefix |
| `/session delete <id\|name>` | Delete a session with confirmation |
| `/rename <name>` | Rename the current session |

### Mode & Model
| Command | Description |
|---------|-------------|
| `/mode [casual\|planning\|agent]` | Show or switch agent mode |
| `/plan`, `/agent`, `/research`, `/casual` | Quick mode switch aliases (`/research` → planning) |
| `/model [name]` | List available models or switch to one |

### Context & Files
| Command                | Description                                                 |
| ---------------------- | ----------------------------------------------------------- |
| `/add <path>`          | Pin a file into context                                     |
| `/drop <path>`         | Remove a pinned file from context                           |
| `/files`               | List all pinned files                                       |
| `/dropall`             | Remove all pinned files                                     |
| `/refresh`             | Re-read pinned files from disk                              |
| `/context`             | Show auto-detected project context                          |
| `/init`                | Generate or update `TINYHARNESS.md`                         |
| `/project-settings [init]` | Show or initialize per-project settings                  |

### Conversation
| Command | Description |
|---------|-------------|
| `/compact [focus]` | Summarize conversation history (with cascading for long sessions) |
| `/image [<path>\|clear\|drop <n>]` | Attach an image to the next message |

### Skills
| Command | Description |
|---------|-------------|
| `/skills` | List all available skills |
| `/skill <name>` | Show a skill's details and content |
| `/use <name>` | Activate a skill, injecting its instructions |
| `/unload <name>` | Deactivate a previously loaded skill |

### Configuration
| Command | Description |
|---------|-------------|
| `/settings [all]` | Show current configuration |
| `/command [list\|add\|rm\|deny\|undeny\|reset\|resetdeny]` | Manage auto-accepted and denied commands |
| `/contextlimit [tokens]` | Show or set the context warning threshold |
| `/autoaccept [all\|safe\|off]` | Show or set auto-accept mode (`off`, `safe` = read-only, `all` = all tools except `run`) |
| `/autocompact [on\|off]` | Toggle the auto_compact tool (when off, the model cannot request compaction) |
| `/questions [on\|off]` | Toggle the question tool (when off, the model cannot ask the user questions) |
| `/showthink [on\|off]` | Toggle display of the model's thinking/reasoning chain |
| `/timeout <seconds>` | Show or set request timeout in seconds (0 resets to per-provider default) |
| `/retries <count>` | Show or set max request retries for the active provider |
| `/audit [last\|session\|clear]` | View command execution audit log |

### General
| Command | Description |
|---------|-------------|
| `/help` | Show available commands |
| `/clear` | Clear terminal screen |
| `/debug [path]` | Dump full conversation context to a log file (for diagnostics) |
| `/exit` or `/quit` | Exit TinyHarness |

### Command Management

The `/command` system controls which shell commands are auto-accepted:

```
/command list          # Show safe and denied commands
/command add <cmd>     # Add a command to auto-accept list
/command rm <cmd>      # Remove from auto-accept list
/command deny <cmd>    # Add to deny list (always requires confirmation)
/command undeny <cmd>  # Remove from deny list
/command reset         # Reset safe commands to defaults
/command resetdeny     # Clear the deny list
```

Safe commands include `cd`, `ls`, `grep`, `cat`, `git status`, `git diff`, `git log`, `cargo tree`, and ~40 more. Shell redirections (`2>&1`, `2>/dev/null`) are stripped before matching. The deny list takes priority — if a command matches both lists, it is denied. In `safe` mode a `run` command auto-executes only if it matches the safe list; `all` mode approves every command, and the per-turn `a` key still prompts for unsafe ones. To bound what an approved command can reach, see the process sandbox below.

### Session Compaction

`/compact` summarizes older messages to free context space. Sessions with over 200 intermediate messages use cascading multi-stage compaction (chunking → per-stage summaries → merged final summary).

```
/compact focus on build errors and fixes
Cascading compaction: 580 intermediate messages → 3 stages (200 messages/stage)
  Stage 1/3: Compacting messages 1–200...
  Stage 2/3: Compacting messages 201–400...
  Stage 3/3: Compacting messages 401–580...
  Merging 3 summaries into final summary...
Compacted: 600 messages → 6 messages
```

On session load, TinyHarness warns if the conversation exceeds 70% or 90% of the context window, using the last known provider token count from session metadata.

## Skills

Skills are pluggable instruction modules that give the AI specialized knowledge. Each skill lives in a directory with a `SKILL.md` file.

**Discovery paths:**
- `~/.config/tinyharness/skills/<name>/SKILL.md` (personal — per-user)
- `.tinyharness/skills/<name>/SKILL.md` (project-local — per-repo)

Project skills take precedence over personal skills with the same name.

**SKILL.md format (YAML frontmatter):**
```markdown
---
name: rust-dev
description: Rust development best practices and code review guidelines
argument-hint: Rust file or module to review
compatibility: rust
disable-model-invocation: false
license: MIT
metadata:
  version: "1.0"
user-invocable: true
---

# Rust Development Skill

Always run `cargo fmt` and `cargo clippy` before suggesting changes...
```

All frontmatter fields are optional with sensible defaults. Skills over 10,000 characters are truncated (70% head / 30% tail). Use the `read` tool to view full content.

**Invoking skills:**
- AI: calls `invoke_skill` with the skill name (unless `disable-model-invocation: true`)
- User: `/use <name>` or `/skill use <name>`
- Deactivate: `/unload <name>`

Both personal and project-local skill directories are scanned at startup and accessible via `/skills`.

## Project Structure

TinyHarness is a Cargo workspace with three crates:

### Core library (`tinyharness-lib/`)

Frontend-agnostic — no terminal I/O, no ANSI codes, no rustyline.

```
tinyharness-lib/src/
├── lib.rs               Re-exports all public types
├── provider/             Provider trait + implementations
│   ├── mod.rs            Provider trait, AnyProvider enum, Message types, ToolDefinition, ToolCall
│   ├── sockudo.rs        SockudoProvider — AI Transport via WebSocket (⚠️ highly experimental)
│   ├── openai_compat.rs  Shared HTTP/SSE logic for OpenAI-compatible backends
│   ├── openai_compat_provider.rs OpenAiCompatProvider — unified Ollama / llama.cpp / vLLM / gateway provider
│   ├── model_variants.rs Model variant suffix stripping for model selection
│   ├── tool_pairs.rs     Tool call/response pairing for multi-turn conversations
│   └── mock.rs           MockProvider for testing (test-util feature)
├── config/mod.rs         Settings persistence (provider, model, mode, API key, safe/denied commands)
├── mode.rs               AgentMode enum (casual/planning/agent) with customizable .md prompts
├── context.rs            WorkspaceContext — auto-detected project metadata + instruction file discovery
├── session.rs            JSONL session persistence with UUIDs, auto-save, atomic writes
├── token.rs              Token estimation, context window sizes (8K–256K), usage warnings
├── skill.rs              Skill discovery, registry, frontmatter parsing, indexing
├── secret.rs             SecretString wrapper for API key redaction (custom Debug, serde support)
├── image.rs              Image attachment handling (base64 encoding, dimension detection)
├── path_guard.rs         Always-on path confinement for in-process file tools
├── paths.rs              Filesystem path resolution (config, data, prompts, sessions)
└── sandbox.rs            OS-level sandbox (bubblewrap/firejail) for shell commands
├── custom_tools/         Custom-tools system — shell-command tools via custom_tools.json
│   ├── mod.rs            CustomToolManager — loads the global custom_tools.json only
│   ├── custom_tool.rs    CustomToolDefinition, CustomToolCategory, build_tool()
│   └── shell.rs          ShellCommand — template substitution, timeout, env vars
├── prompts/              Hardcoded default system prompts (header.md, casual.md, planning.md, agent.md)
└── tools/                11 tool implementations
    ├── mod.rs            ToolManager with mode-based filtering, signal event parsing
    ├── tool.rs           Tool struct, ToolCategory enum, schema builders, extract_args! macro
    ├── ls.rs             Directory listing
    ├── read.rs           File reading with image detection for multimodal models
    ├── write.rs          File writing
    ├── edit.rs           Find-and-replace file editing
    ├── grep.rs           Regex search across files
    ├── glob.rs           File glob pattern matching
    ├── run.rs            Shell command execution with timeout
    ├── switch_mode.rs    Mode switch signal
    ├── question.rs       User question signal
    ├── auto_compact.rs   Compaction signal
    └── invoke_skill.rs   Skill activation signal
```

### UI library (`tinyharness-ui/`)

Terminal UI abstractions — reusable output formatting, diff display, and confirmation prompts.

```
tinyharness-ui/src/
├── lib.rs               Module declarations
├── output.rs            Structured output writer (stdout/stderr abstraction)
├── style.rs             ANSI color constants (BOLD, CYAN, the tool-frame palette, SPINNER_FRAMES)
└── ui/
    ├── mod.rs            Module declarations
    ├── confirm.rs        Tool call confirmation prompts (Yes/No/Auto-accept)
    ├── diff.rs           Unified diff display
    ├── frame.rs          Shared layout primitives for the tool-frame family
    ├── input.rs          CommandHelper for rustyline tab-completion
    ├── markdown.rs       Incremental markdown renderer for streamed assistant output
    ├── tool_result.rs    Tool execution cards (status band + capped body)
    └── wrap.rs           Word-wrapped output with ANSI-aware line filling
```

Tool I/O shares one visual language: a filled **title band** names each call
(`● run · cargo test --workspace · 2.3s`), and an outlined **gutter** carries
its body, closing with an elbow so every block reads as one unit.

```
  ● read · tinyharness-ui/src/ui/frame.rs · 42ms
    │ (180 lines)
    │ use std::io::{self, Write};
    ╰ +176 more lines

  ✕ run · cargo clippy --workspace · 1.5s
    ╰ error[E0308]: mismatched types

  ▲ Allow? · run — execute a shell command
    │ $ rm -rf ./target
  ▲ [y]es  [n]o  [a]uto-accept the rest of this turn ›
```

### Binary crate (`src/`)

CLI application — argument parsing, agent loop, slash commands, tool dispatch, safety checking.

### Sockudo worker (`docs/examples/sockudo-worker/`)

⚠️ **Highly experimental** — example worker binary that bridges Sockudo AI Transport to Ollama. Connects to a Sockudo server via WebSocket, receives `ai-input` events, calls Ollama for inference, and streams responses back as versioned message mutations. Not part of the Cargo workspace and not required for normal TinyHarness usage with Ollama, llama.cpp, or vLLM. Build it standalone with `cargo build` from within the `docs/examples/sockudo-worker/` directory.

```
src/
├── main.rs               Entry point, CLI parsing (clap), provider creation, session init
├── agent/
│   ├── mod.rs            Main interaction loop, streaming response display, spinner, thinking chain
│   ├── tools.rs          Tool call dispatch, confirmation, generic execution, signal handlers
│   ├── safety.rs         Shell command safety checker (prefix + deny list + redirection stripping)
│   ├── setup.rs          Provider/URL resolution, URL prompting
│   ├── display.rs        Context status formatting, args summaries, listing result summaries
│   ├── input.rs          Multi-line input reading with continuation prompt
│   ├── confirm.rs        Confirmation prompt rendering for tool calls
│   ├── signal.rs         Signal tool handling (switch_mode, question, auto_compact, invoke_skill)
│   ├── tool_result.rs    Tool result formatting and batching
│   └── command_result.rs CommandResult enum definition
└── commands/
    ├── mod.rs            CommandRegistry, CommandDispatcher, build_registry() — 30+ commands
    ├── registry.rs       CommandContext, CommandResult, AsyncCommand trait, async_command! macro
    ├── audit.rs          /audit — command execution audit log
    ├── clear.rs          /clear — terminal clear
    ├── command.rs        /command — safe/denied command management
    ├── compact.rs        /compact — cascading conversation summarization
    ├── config_settings.rs /contextlimit, /autoaccept, /autocompact, /showthink, /timeout, /retries
    ├── context.rs        /context — workspace context display
    ├── debug.rs          /debug — full conversation context dump to log file
    ├── exit.rs           /exit — graceful shutdown
    ├── files.rs          /add, /drop, /files, /dropall, /refresh — file pinning
    ├── help.rs           /help — command listing
    ├── image.rs          /image — image attachment management
    ├── init.rs           /init — TINYHARNESS.md generation
    ├── mode.rs           /mode — mode switching
    ├── models.rs         /model — model listing and selection
    ├── project_settings.rs /project-settings — per-project settings display and init
    ├── sessions.rs       /sessions, /session — session listing, switching, deletion
    ├── settings.rs       /settings — configuration display
    └── skill.rs          /skills, /skill, /use, /unload — skill management
```

## Development

### Build & Test

```bash
cargo build                    # Debug build
cargo build --release          # Release build
cargo test --workspace         # Run all tests
cargo test -p tinyharness-lib  # Library tests only
cargo test -p TinyHarness      # Binary tests only
cargo test <test_name>         # Run a specific test
cargo clippy --workspace -- -D warnings   # Lint
cargo fmt --all -- --check     # Format check
cargo fmt --all                # Auto-format
```

If you're using the Nix flake, prefer:

```bash
nix build                      # Build the package
nix flake check                # Run all checks (build + fmt + clippy --deny warnings + test)
nix develop                    # Enter dev shell with cargo/rustfmt/clippy
```

### Verification Steps

After making changes, run:

1. `cargo fmt --all` — ensure formatting is clean
2. `cargo clippy --workspace -- -D warnings` — no clippy warnings
3. `cargo test --workspace` — all tests pass
4. `cargo build` — clean debug build succeeds

## AI Usage & Security

TinyHarness grants LLMs the ability to interact with your filesystem through tool calling. This introduces specific risks:

- **Sandboxing**: OS-level confinement for shell commands, **on by default** on Linux, plus an always-on path guard for the file tools. Opt out with `--no-sandbox`. For full isolation from an untrusted model, still prefer a container or VM.
- **Non-determinism**: LLMs may hallucinate or produce incorrect tool arguments. Always review proposed actions.
- **Accountability**: You assume full responsibility for all operations performed by the AI. Ensure you have backups.

The `run` tool follows the configured auto-accept policy: `/autoaccept safe` prompts for anything outside the safe-command list, `/autoaccept all` approves without asking, and the per-turn `a` key approves for the rest of the turn except for unsafe `run` commands. The sandbox — on by default — bounds what an approved command can reach.

### Containment: path guard (always on)

The built-in file tools — `ls`, `read`, `write`, `edit`, `grep`, `glob` — run in-process, so they are confined by a **path guard** rather than an OS sandbox. This is unconditional: there is no flag to disable it.

- **Confined to the workspace root**: any path outside it is rejected with an error before the tool runs, so the check applies **even when auto-accept is enabled**.
- **Path resolution is robust**: `..` traversal, absolute paths, and symlink escapes are all detected. Paths are resolved component by component in the order `open(2)` uses, so a `..` *after* a symlink is applied to the link's target rather than to the textual path — the case a purely lexical collapse gets wrong (`<root>/link/../etc/passwd` with `link -> /usr` opens `/etc/passwd`, and is rejected).
- **Widen it deliberately**: set `allowed_paths` in the **global** `settings.json` (a project `.tinyharness/config.json` cannot add entries) to grant extra roots. Entries may be absolute, `~`-prefixed, or relative to the workspace. Note the file tools run in-process, so an allowed root gets read **and** write access from `read`/`write`/`edit`. Entries overlapping a system prefix (`/etc`, `/usr`, `/proc`, `/dev`, `/boot`, `/sys`, `/run`, `/var/log`, `/root`) are rejected in both directions — `/etc/ssl` and `/` alike — and the workspace root itself is exempt from that block list.

```json
{
  "allowed_paths": ["../shared-schemas", "~/.local/share/mydata"]
}
```

### Process sandbox (on by default, Linux only)

Shell commands (`run` and custom tools) can optionally run inside a real OS sandbox:

```bash
# Sandboxing is already on — no flag needed. Pin a backend only if you must:
tinyharness --sandbox=bwrap        # pin bubblewrap
tinyharness --sandbox=firejail     # pin firejail
tinyharness --no-sandbox           # opt out for this session (also --sandbox=off)
```

The same choice persists as `sandbox` in `settings.json`:

```json
{
  "sandbox": {
    "backend": "auto",
    "writable_paths": ["/tmp/build-cache"]
  }
}
```

bubblewrap is preferred and is what you should expect to be used. It builds a private mount tree from a `tmpfs` root and binds only the workspace (read-write), `/usr` and `/etc` (read-only), and your `allowed_paths` — so **files outside those roots do not exist inside the sandbox**, rather than merely being unreadable. It also unshares the PID/IPC/UTS/cgroup namespaces, clears the environment (no API keys reach a sandboxed command), and starts a new session. `/etc` is bound read-only along with whatever `/etc/resolv.conf` symlinks to, so DNS keeps working on systemd-resolved hosts.

firejail takes the opposite approach: it mounts the **existing** host filesystem read-only and re-binds the workspace writable. Writes outside the workspace are blocked and privileges are dropped, but **reads outside it still succeed** — it is write/privilege containment, not visibility containment. It also normally needs a setuid helper.

Behaviour notes:

- **Auto-accept is unchanged by the sandbox.** A sandboxed command follows the same confirmation policy as an unsandboxed one; the sandbox bounds its reach instead of adding a prompt.
- **The network is always available.** There is deliberately no network setting: an agent runs `git fetch`, `curl`, and `cargo add` constantly, so blocking the network would make the sandbox unusable rather than safe. Network isolation is out of scope — the containment is filesystem and environment.
- **`allowed_paths` are bound read-only**, so `run` and `read` see the same extra trees. Use `sandbox.writable_paths` when a sandboxed command must write outside the workspace; those are reachable only by sandboxed commands, not by the file tools.
- **The Rust toolchain stays usable**: `~/.cargo/bin` and `~/.rustup` are bound read-only and `$CARGO_HOME` points at a writable cache under the data dir. Because that cache starts empty, a sandboxed build that must *download* crates fetches them into it rather than reusing your populated `~/.cargo/registry`.
- **A missing backend is a startup error, not a silent downgrade.** Since sandboxing is the default, continuing without one would mean unsandboxed shell commands under a user who believes they are contained. Install bubblewrap, or pass `--no-sandbox` to opt out deliberately.
- On Windows and macOS there is no process sandbox: `--sandbox` is rejected at startup (`--sandbox is a Linux-only feature`) and `sandbox.backend` is ignored. `--no-sandbox` is accepted and is the norm there.

Neither backend is a defence against a hostile *user*: sandboxed commands run as your uid, so anything inside the workspace (`.env`, `.git`, committed credentials) remains in scope, and there are no resource limits. For untrusted models, prefer a container or VM.

## Project Instructions (TINYHARNESS.md)

TinyHarness automatically discovers project instruction files, similar to `CLAUDE.md` in Claude Code and `AGENTS.md` in other agents. These files give the AI persistent context about your project.

### Discovery

Searches from the current directory up to the filesystem root (first match wins). The discovery order is configurable via settings or an environment variable.

**Default priority order:**

| Priority | File | Notes |
|---|---|---|
| 1 | `TINYHARNESS.md` | TinyHarness-native |
| 2 | `.tinyharness.md` | Hidden variant |
| 3 | `AGENTS.md` | Industry standard (60K+ repos) |
| 4 | `CLAUDE.md` | Claude Code compatibility |

**Customizing the discovery list:**

```bash
# Env var override (highest priority):
export TINYHARNESS_MD_FILES="CLAUDE.md,TEAM_RULES.md"
tinyharness
```

Or in `~/.config/tinyharness/settings.json`:
```json
{
  "project_md_files": ["CLAUDE.md", ".cursorrules", "TEAM_RULES.md"]
}
```

**Additional per-project files** can be loaded via `.tinyharness/config.json`:
```json
{
  "project_md_files": ["RULES.md", "DEPLOYMENT.md"]
}
```
These are appended after the main instruction file in the AI's context.

Files over 20,000 characters are truncated (70% head / 30% tail with a marker).

### Generating with `/init`

Run `/init` in a TinyHarness session and the AI will analyze your project and generate a `TINYHARNESS.md`:

```
[agent]> /init
  Generating project instruction file...
  ✦ Created /path/to/TINYHARNESS.md (45 lines)
```

If one already exists, `/init` updates it — keeping accurate parts, removing outdated ones, and adding what's missing.

### What to Include

A good instruction file should contain what you'd tell a new teammate:

- **Build and test commands** — specific ones, not vague
- **Code conventions** — rules that differ from defaults
- **Architecture** — key directories and module relationships
- **Known gotchas** — things that trip up newcomers
- **Verification steps** — what to run after making changes

Keep it concise (under 200 lines). For detailed reference, the AI can use `read` on specific files.

## Per-Project Settings

TinyHarness supports per-project configuration via `.tinyharness/config.json`, discovered by walking up from the current working directory (same algorithm as instruction file discovery). Settings are layered:

```
~/.config/tinyharness/settings.json          (global)
  → .tinyharness/config.json                  (project override)
    → CLI flags                               (highest priority)
```

### Supported fields

```json
{
  "safe_command_prefixes": ["python -m pytest", "npm run lint"],
  "denied_command_prefixes": ["git push --force"],
  "auto_accept_mode": "off",
  "context_limit": 32768,
  "project_md_files": ["RULES.md", ".cursorrules"],
  "preferred_mode": "agent"
}
```

- **`safe_command_prefixes`**: Extends (not replaces) the global safe list
- **`denied_command_prefixes`**: Replaces the global deny list entirely
- **`auto_accept_mode`**: Overrides global auto-accept mode (`"off"`, `"safe"`, or `"all"`)
- **`context_limit`**: Overrides context warning threshold
- **`project_md_files`**: Additional instruction files loaded after the main one
- **`preferred_mode`**: Default agent mode for this project

### Managing project settings

```
/project-settings             # Show merged settings with source annotations
/project-settings init        # Generate a commented .tinyharness/config.json template
```

Sources are displayed as `(project)`, `(global)`, or `(default)`, making it clear where each value originates.

## Language Detection

TinyHarness auto-detects your project's language and build tool from marker files in the workspace root. Detection supports 17+ languages and can identify multiple languages in monorepos (e.g. "Rust + Node.js").

### Supported languages

| Language | Detection files |
|----------|----------------|
| Rust | `Cargo.toml` |
| Zig | `build.zig`, `build.zig.zon` |
| Deno | `deno.json`, `deno.jsonc` |
| Bun | `bun.lockb`, `bun.lock` |
| Swift | `Package.swift` |
| Ruby | `Gemfile` |
| Elixir | `mix.exs` |
| Haskell | `stack.yaml`, `*.cabal` |
| Kotlin | `build.gradle.kts`, `settings.gradle.kts` |
| .NET | `*.csproj`, `*.sln` |
| Dart/Flutter | `pubspec.yaml` |
| Nix | `flake.nix`, `default.nix` |
| Node.js | `package.json` |
| Python | `pyproject.toml`, `setup.py`, `setup.cfg`, `requirements.txt` |
| Go | `go.mod` |
| Java (Gradle) | `build.gradle` |
| Java (Maven) | `pom.xml` |
| C/C++ (CMake) | `CMakeLists.txt` |

Unknown project types fall back to `Makefile` or `Justfile` hints when available. Detected commands are injected into the system prompt so the AI knows how to build and test your project without being told.
