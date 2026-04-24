# TinyHarness

TinyHarness is a lightweight AI assistant framework written in Rust, designed to provide a flexible way to interact with Large Language Models (LLMs) via providers like Ollama, with built-in support for tool calling.

## Features

- **Provider Abstraction**: Easily switch between different LLM providers.
- **Tool Integration**: A modular system for registering and executing tools (e.g., `ls`, `read`) that the AI can call to interact with the local system.
- **Async Execution**: Built on `tokio` for efficient asynchronous communication with LLM APIs.
- **Interactive CLI**: A clean terminal interface for chatting with the AI.

## Getting Started

### Prerequisites

- [Rust](https://www.rust-lang.org/tools/install) (latest stable version)
- [Ollama](https://ollama.com/) (if using the Ollama provider)

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

Run the assistant:
```bash
cargo run --release
```

By default, it connects to Ollama at `http://127.0.0.1:11434` and uses the `gemma4:31b-cloud` model.

## Project Structure

- `src/main.rs`: Entry point and main interaction loop.
- `src/provider/`: LLM provider implementations (e.g., `ollama.rs`).
- `src/tools/`: Tool definitions and the `ToolManager`.
- `src/system_prompt.rs`: The system prompt that defines the AI's behavior and tool usage guidelines.

## Development

To add a new tool:
1. Create a new file in `src/tools/`.
2. Implement the tool logic and an entry function.
3. Register the tool in `src/main.rs` using `tool_manager.register_tool()`.
