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

2. Build the project:
   ```bash
   cargo build --release
   ```

### Usage

**Ollama** (default):
```bash
cargo run --release
```
Connects to `http://127.0.0.1:11434` and uses the `gemma4:31b-cloud` model.

**llama.cpp**:
```bash
cargo run --release -- --llama-cpp
```
Connects to `http://127.0.0.1:8080` by default. A health check is performed on startup.

**Custom URL** (works with either provider):
```bash
cargo run --release -- --llama-cpp --url http://localhost:2832
cargo run --release -- --ollama --url http://192.168.1.50:11434
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
├── main.rs              # Entry point, CLI arg parsing, and main interaction loop
├── system_prompt.rs     # System prompt defining AI behavior and tool guidelines
├── provider/
│   ├── mod.rs           # Provider trait, shared types (ToolCall, ChatMessage, etc.)
│   ├── ollama.rs        # Ollama provider implementation
│   └── llama_cpp.rs     # llama.cpp (OpenAI-compatible API) provider
└── tools/
    ├── mod.rs           # ToolManager — registration and execution
    ├── tool.rs          # Tool struct and execute helper
    ├── ls.rs            # `ls` tool — list directory contents
    └── read.rs          # `read` tool — read file contents
```

## Development

### Adding a new tool

1. Create a new file in `src/tools/` (e.g., `grep.rs`).
2. Implement the tool logic and an entry function returning a `Tool`.
3. Register the tool in `main.rs`:
   ```rust
   tool_manager.register_tool(grep_tool_entry());
   ```

### Adding a new provider

1. Create `src/provider/my_provider.rs`.
2. Implement the `Provider` trait using the shared types from `mod.rs`.
3. Register it: `pub mod my_provider;` in `mod.rs`.
4. Wire it up in `main.rs` with a CLI flag.
