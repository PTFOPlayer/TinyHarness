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


## AI Usage & Security Disclosure

TinyHarness provides a framework that grants Large Language Models (LLMs) the ability to interact with your local filesystem through tool calling. While powerful, this capability introduces specific risks:

- **Security Risk**: Granting an AI execution privileges on your host system can be dangerous. It is strongly recommended to run this framework within a **sandboxed environment** (e.g., a Docker container or a dedicated VM) to prevent accidental or malicious modification of critical system files.
- **Non-Deterministic Behavior**: LLMs are prone to "hallucinations" and may generate incorrect or unintended tool arguments. Always review the AI's proposed actions before execution in production environments.
- **User Accountability**: The user assumes full responsibility for all operations performed by the AI via the integrated tools. Ensure you have appropriate backups and permissions configured.
