# TinyHarness

TinyHarness is a lightweight AI assistant framework written in Rust, designed to provide a flexible way to interact with Large Language Models (LLMs) via pluggable providers, with built-in support for tool calling.

## Features

- **Provider Abstraction**: Swap between Ollama and llama.cpp without changing any application code.
- **Tool Integration**: A modular system for registering and executing tools (e.g., `ls`, `read`) that the AI can call to interact with the local filesystem.
- **Async Streaming**: Built on `tokio` for efficient streaming communication with LLM APIs.
- **Interactive CLI**: A clean terminal interface with color-coded output for chatting with the AI.

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (latest stable edition 2024)
- At least one LLM backend running locally:
  - [Ollama](https://ollama.com/) (default)
  - [llama.cpp](https://github.com/ggml-org/llama.cpp) server

### Installation

1. Clone the repository:
   ```bash
   git clone https://github.com/yourusername/TinyHarness.git
   cd TinyHarness
   ```

2. Install the binary (builds in release mode and copies to `~/.local/bin`):
   ```bash
   make install
   ```

   To uninstall:
   ```bash
   make uninstall
   ```

   > **Note:** Make sure `~/.local/bin` is in your `$PATH`. If it's not, add this to your shell config:
   > ```bash
   > export PATH="$HOME/.local/bin:$PATH"
   > ```

Alternatively, you can use Cargo to install:
```bash
cargo install --path .
```

### Usage

**Ollama** (default):
```bash
tinyharness
```
Connects to `http://127.0.0.1:11434` and uses the `gemma4:31b-cloud` model.

**llama.cpp**:
```bash
tinyharness --llama-cpp
```
Connects to `http://127.0.0.1:8080` by default. A health check is performed on startup.

**Custom URL** (works with either provider):
```bash
tinyharness --llama-cpp --url http://localhost:2832
tinyharness --ollama --url http://192.168.1.50:11434
```

### CLI Arguments

| Flag | Description |
|---|---|
| `-o`, `--ollama` | Use the Ollama provider (default) |
| `-l`, `--llama-cpp` | Use the llama.cpp provider |
| `-u`, `--url` | Custom base URL for the provider |

## Project Structure

```
src/
├── main.rs              # Entry point, CLI arg parsing, session management
├── agent.rs             # Main interaction loop, tool call dispatch, confirmation UI
├── mode.rs              # AgentMode enum (casual/planning/agent/research) with system prompts
├── context.rs           # WorkspaceContext — auto-detected project metadata + TINYHARNESS.md
├── config/mod.rs        # Settings persistence (provider, model, mode, API key)
├── session.rs           # JSONL session persistence with UUIDs
├── style.rs             # ANSI color constants
├── commands/            # Slash command handlers
│   ├── mod.rs           # CommandDispatcher — parse and dispatch /commands
│   ├── apikey.rs        # /apikey — set/show/clear Ollama API key
│   ├── clear.rs         # /clear — clear terminal
│   ├── compact.rs       # /compact — summarize conversation history
│   ├── context.rs       # /context — show workspace context
│   ├── exit.rs          # /exit — quit
│   ├── files.rs         # /add, /drop, /dropall, /files, /refresh — pin files into context
│   ├── help.rs          # /help — show available commands
│   ├── init.rs          # /init — generate or update TINYHARNESS.md
│   ├── models.rs        # /models, /model — list and switch models
│   ├── sessions.rs      # /sessions, /session, /rename — session management
│   └── settings.rs      # /settings — show current configuration
├── provider/
│   ├── mod.rs           # Provider trait, shared types (ToolCall, ChatMessage, etc.)
│   ├── ollama.rs        # Ollama provider implementation
│   ├── llama_cpp.rs     # llama.cpp server provider
│   ├── vllm.rs          # vLLM provider
│   └── openai_compat.rs # Shared OpenAI-compatible API helpers
├── tools/
│   ├── mod.rs           # ToolManager — registration and execution
│   ├── tool.rs          # Tool struct and execute helper
│   ├── ls.rs            # `ls` tool — list directory contents
│   ├── read.rs          # `read` tool — read file content
│   ├── write.rs         # `write` tool — write content to file
│   ├── edit.rs          # `edit` tool — find-and-replace in file
│   ├── grep.rs          # `grep` tool — search regex across files
│   ├── glob.rs          # `glob` tool — find files by pattern
│   ├── run.rs           # `run` tool — execute shell commands
│   ├── web_search.rs    # `web_search` and `web_fetch` tools
│   ├── switch_mode.rs   # `switch_mode` tool — change agent mode
│   └── question.rs      # `question` tool — ask user a multiple-choice question
└── ui/
    ├── mod.rs           # UI module root
    ├── confirm.rs       # Confirmation prompt for sensitive tool calls
    ├── input.rs         # Command helper for readline
    └── diff.rs          # Diff display for file edits
```


## AI Usage & Security Disclosure

TinyHarness provides a framework that grants Large Language Models (LLMs) the ability to interact with your local filesystem through tool calling. While powerful, this capability introduces specific risks:

- **Security Risk**: Granting an AI execution privileges on your host system can be dangerous. It is strongly recommended to run this framework within a **sandboxed environment** (e.g., a Docker container or a dedicated VM) to prevent accidental or malicious modification of critical system files.
- **Non-Deterministic Behavior**: LLMs are prone to "hallucinations" and may generate incorrect or unintended tool arguments. Always review the AI's proposed actions before execution in production environments.
- **User Accountability**: The user assumes full responsibility for all operations performed by the AI via the integrated tools. Ensure you have appropriate backups and permissions configured.

## Project Instructions (TINYHARNESS.md)

TinyHarness automatically discovers and loads project instruction files, similar to how `CLAUDE.md` works in Claude Code and `HERMES.md`/`AGENTS.md` work in Hermes Agent. These files give the AI persistent context about your project — build commands, coding conventions, architecture notes, and gotchas — without repeating them every session.

### How It Works

When TinyHarness starts, it searches for a project instruction file in the current directory and walks up parent directories until it finds one. The file's content is injected into the system prompt so the AI always has your project's context.

### File Discovery Priority

TinyHarness searches for files in this order (first match wins):

| Priority | File | Origin |
|---|---|---|
| 1 | `TINYHARNESS.md` | TinyHarness-native project config |
| 2 | `.tinyharness.md` | Hidden variant (useful for gitignored preferences) |
| 3 | `AGENTS.md` | Industry standard (60K+ repos) |
| 4 | `CLAUDE.md` | Claude Code compatibility |

This priority system means:
- If you use TinyHarness primarily, create a `TINYHARNESS.md`
- If you share a repo with Claude Code or other agents, they'll pick up `AGENTS.md` or `CLAUDE.md` as fallback
- You can commit `TINYHARNESS.md` to version control for team sharing

### Directory Walking

TinyHarness walks from the current working directory up to the filesystem root, checking each directory for instruction files. This means if you run TinyHarness from `foo/bar/`, it checks `foo/bar/`, then `foo/`, then `/`.

### Size Limits

Files exceeding 20,000 characters are automatically truncated (70% head / 20% tail with a truncation marker). This matches Hermes Agent's approach and prevents oversized instruction files from consuming too much context.

### What to Include

A good project instruction file should contain what you'd tell a new teammate on their first morning:

- **Build and test commands** — specific, not vague ("`cargo test`" not "run tests")
- **Code conventions** — rules that differ from defaults
- **Architecture** — key directories, module relationships, how things connect
- **Known gotchas** — things that trip up newcomers
- **Verification steps** — what to run after making changes

Keep it concise (under 200 lines). For detailed reference material, the AI can always use the `read` tool on specific files.

### Generating with /init

You don't have to write the instruction file manually. Run `/init` inside a TinyHarness session and the AI will analyze your project and generate a `TINYHARNESS.md` for you:

```
[agent]> /init
  Generating project instruction file...
  Creating — analyzing project...
  ✦ Created /path/to/TINYHARNESS.md (45 lines)
```

If a project instruction file already exists, `/init` updates it instead — keeping what's still accurate, removing what's outdated, and adding anything missing:

```
[agent]> /init
  Found existing TINYHARNESS.md (320 bytes). Updating...
  Updating — analyzing project...
  ✦ Updated /path/to/TINYHARNESS.md (52 lines)
```

After `/init` completes, TinyHarness automatically refreshes its workspace context so the new instructions take effect immediately.

### Example

```markdown
# MyProject

Rust web service using Axum and SQLx.

## Commands

- Build: `cargo build`
- Test: `cargo test`
- Run: `cargo run`

## Conventions

- Use `thiserror` for error types, never `anyhow` in library crates
- All API handlers go in `src/handlers/`
- Database queries go through `src/db/` — never import `sqlx` directly in handlers

## Gotchas

- The `migrations/` folder must be in sync with `src/db/schema.rs`
- Tests require a running Postgres instance (use `docker compose up -d`)
```
