# Contributing to TinyHarness

TinyHarness is a Rust workspace with three crates and a focus on minimal dependencies. This guide covers setup, conventions, and the PR workflow.

## Project Setup

### Prerequisites

- Nix with flakes enabled or Rust latest stable (edition 2024)
- An LLM backend for testing (Ollama recommended, but not required for library tests)

### Getting the Code

```bash
git clone https://github.com/yourusername/TinyHarness.git
cd TinyHarness
```

### First Build

>[!NOTE]
>If you have Nix, run `nix develop`

```bash
cargo build --workspace
cargo test --workspace
```

This compiles all three crates and runs the test suite (~490 tests across all crates).

---

## Workspace Structure

```
TinyHarness/                  Binary crate — CLI, agent loop, slash commands
├── src/
│   ├── main.rs               Entry point, CLI parsing, provider creation
│   ├── agent/                Agent loop, tool execution, safety, display, input
│   └── commands/             22+ slash command modules + registry
│
tinyharness-lib/              Core library — no terminal I/O, no ANSI, no rustyline
├── src/
│   ├── lib.rs                Re-exports all public types
│   ├── provider/             Provider trait + Ollama/llama.cpp/vLLM/OpenAI-compat/Sockudo impls
│   │   ├── ollama.rs             OllamaProvider — raw SSE streaming, retries, Gemini signatures
│   │   ├── llama_cpp.rs          LlamaCppProvider — OpenAI-compatible, no auth (local)
│   │   ├── vllm.rs               VllmProvider — OpenAI-compatible, no auth (local)
│   │   ├── openai_compat.rs      Shared HTTP/SSE logic for OpenAI-compatible backends
│   │   ├── openai_compat_provider.rs OpenAiCompatProvider — Bearer auth wrapper for hosted gateways
│   │   └── sockudo.rs            SockudoProvider — AI Transport via WebSocket (⚠️ experimental)
│   ├── tools/                15 tools + ToolManager with mode filtering
│   ├── config/mod.rs         Settings, project settings, prompt management
│   ├── context.rs            Workspace detection, instruction file discovery
│   ├── session.rs            JSONL persistence, auto-save, atomic writes
│   ├── token.rs              Token estimation, context windows, warnings
│   ├── skill.rs              Skill discovery, registry, frontmatter parsing
│   ├── image.rs              Image attachment handling
│   ├── mode.rs               AgentMode enum, prompt assembly
│   └── prompts/              Hardcoded default system prompts (.md files)
│
tinyharness-ui/               UI library — terminal output abstractions + experimental TUI
├── src/
│   ├── lib.rs                Module declarations
│   ├── output.rs             Structured output writer
│   ├── style.rs              ANSI color constants, spinner frames
│   ├── ui/                   confirm.rs, diff.rs, input.rs, wrap.rs
│   └── tui/                  ⚠️ Experimental TUI subsystem
│       ├── mod.rs             Agent integration types (TuiAgentEvent, TuiUserAction)
│       ├── app.rs             Main TUI application loop
│       ├── backend.rs         Backend trait (StdioBackend + TestBackend)
│       ├── cell.rs            Color/style for screen buffer (raw ANSI, no framework)
│       ├── event.rs           Keyboard/mouse/paste events
│       ├── layout.rs          Constraint-based layout
│       ├── screen.rs          Differential rendering screen buffer with Unicode width support
│       ├── terminal.rs        Raw terminal control, alternate screen
│       ├── widget.rs          Widget trait, Action enum
│       └── widgets/           conversation, input_bar, sidebar, spinner, status_bar, tool_output
│
docs/examples/                Example code (not part of Cargo workspace)
└── sockudo-worker/           ⚠️ Example Sockudo AI Transport worker bridge
    ├── src/
    │   ├── main.rs           Binary entry point (clap CLI, env vars)
    │   ├── lib.rs            Module declarations + re-exports
    │   ├── auth.rs           Pusher-style HMAC-SHA256 signed HTTP requests
    │   ├── ollama.rs         Streaming Ollama chat client (NDJSON over bytes_stream)
    │   └── worker.rs         WebSocket connection, ai-input handling, versioned message publishing
    └── Cargo.toml            Standalone crate (not in workspace)
│
docs/                         User-facing documentation
└── todo/                     Enhancement tracking (local only, not committed)
```

### Crate Rules

- **`tinyharness-lib`**: Must not use terminal I/O, ANSI escape codes, or `rustyline`. Uses `tracing` for logging.
- **`tinyharness-ui`**: Terminal UI abstractions — ANSI colors, confirmation prompts, diff display, word wrapping. Includes an experimental TUI subsystem (`tui/` module) built from scratch with raw ANSI escape sequences (no ratatui/crossterm). The TUI is feature-gated behind the `tui` Cargo feature.
- **`src/` (binary)**: Wires everything together. Handles I/O, user interaction, and the agent loop.

---

## Development Workflow

### Building

```bash
cargo build                    # Debug build (all crates)
cargo build --release          # Release build
cargo build -p tinyharness-lib # Build only the library
```

### Testing

```bash
cargo test --workspace                  # All tests
cargo test -p tinyharness-lib           # Library tests only
cargo test -p TinyHarness               # Binary crate tests only
cargo test -p tinyharness-ui            # UI crate tests only
cargo test <test_name>                  # Specific test (searches all crates)
```

### Linting & Formatting

```bash
cargo clippy --workspace -- -D warnings   # Lint all crates (warnings = errors)
cargo fmt --all                            # Auto-format
cargo fmt --all -- --check                 # Check formatting without changing
```

### Verification Checklist

Before submitting a PR, run these in order:

1. `cargo fmt --all` — ensure formatting is clean
2. `cargo clippy --workspace -- -D warnings` — no clippy warnings
3. `cargo test --workspace` — all tests pass
4. `cargo build` — clean debug build succeeds

If you have Nix installed, the same checks are available as a single command:

```bash
nix flake check
```

This runs `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace` (release), and a release build — i.e. steps 1, 2, 3, and a release variant of step 4. See the [Nix installation section](../../README.md#installation-nix) in the README for setup.

---

## Code Conventions

### Rust Edition

All crates use Rust **edition 2024**. Check `Cargo.toml` files if you're unsure.

### Error Handling

- **User-facing errors**: `Result<T, String>` — the binary crate displays these directly
- **Internal errors**: `Result<T, Box<dyn Error>>` — for library code where error types vary
- **I/O errors**: Propagate with `?` or wrap in domain-specific error enums

### Async Patterns

Prefer `Pin<Box<dyn Future>>` over `async-trait`:

```rust
// ✅ Do this
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
handler: Box<dyn Fn(HashMap<String, String>) -> BoxFuture<'static, String> + Send + Sync>,

// ❌ Not this
#[async_trait]
pub trait AsyncHandler {
    async fn handle(&self, args: HashMap<String, String>) -> String;
}
```

This avoids pulling in the `async-trait` crate and keeps the dependency tree small.

### Serialization

Use `serde` + `schemars` for serialization and JSON Schema generation:
- `serde::Serialize` / `serde::Deserialize` for data types
- `schemars::JsonSchema` (or manual `Schema` construction) for tool parameter schemas

### Dependency Policy

- **Minimize dependencies** — avoid adding new crates when existing ones suffice
- **Feature-gate optional functionality** — e.g., a hypothetical `server` feature for HTTP API mode
- **Audit existing usage** — `chrono` usage could be replaced with `std::time` (see `todo/16-dependency-slimming.md`)

### Macros

`#[macro_export]` macros (`extract_args!`) live at the `tinyharness_lib` crate root, not inside a module. They're re-exported via `pub use`.

### Tests

- Use `tempfile` for test isolation — tool tests must not touch the real filesystem
- Test modules go inline: `#[cfg(test)] mod tests { ... }`
- `tinyharness-lib` has good coverage (~89 tests); `tinyharness-ui` has extensive coverage (~325 tests, including TUI rendering, Unicode width, scroll/clipping, and overflow tests); binary crate has limited coverage (see `todo/01-testing-gaps.md`)

### Tool Categories

When adding a new tool, assign it to one of three categories:
- `ReadOnly` — auto-executed, no side effects
- `Destructive` — requires confirmation
- `Signal` — handled specially by the agent loop

See [Tools Reference](tools-reference.md) for the full list and behavior.

---

## CI Pipeline

GitHub Actions runs on every push and PR to `master`:

| Job | Platform | Command | Purpose |
|-----|----------|---------|---------|
| Format | Linux | `cargo fmt --all -- --check` | Ensures consistent formatting |
| Clippy | Linux | `cargo clippy --workspace -- -D warnings` | Catches common mistakes |
| Test | Linux | `cargo test --workspace` | Runs full test suite |
| Build | Linux | `cargo build --workspace` | Confirms compilation |
| Build | Windows | `cargo build --workspace` | Confirms Windows compilation |
| Test | Windows | `cargo test --workspace` | Runs full test suite on Windows |

Uses `dtolnay/rust-toolchain@stable` for Rust and `Swatinem/rust-cache@v2` for caching.

---

## Pull Request Process

1. **Create a feature branch**: `feat/short-description` or `fix/short-description`
2. **Make changes**: Follow code conventions above
3. **Run verification checklist**: fmt → clippy → test → build
4. **Update docs**: If adding/changing user-facing features, update relevant docs in `docs/`
5. **Update todos**: If your PR completes a tracked enhancement, update the status in `todo/<number>-*.md` and `todo/todo.md`
6. **Write a clear PR description**: What, why, and any breaking changes
7. **PR to `master`**: CI runs automatically

### Commit Style

```
feat: language detection for 17+ languages

Adds detection for Zig, Deno, Bun, Swift, Ruby, Elixir,
Haskell, Kotlin, .NET, Dart/Flutter, Nix. Monorepo detection
joins multiple types with "+". Falls back to Makefile/Justfile.

- context.rs: expanded detect_project_type with 17+ signatures
- monorepo detection joins types (e.g. "Rust + Node.js")
- Makefile/Justfile fallback for unknown types
```

Keep the summary line under 72 characters. Use imperative mood ("Add" not "Added").

---

## Common Tasks

### Adding a New Slash Command

1. Create `src/commands/<name>.rs`
2. Implement the handler function
3. Register in `src/commands/mod.rs` → `build_registry()`
4. If async (needs provider access), use the `async_command!` macro
5. Add help text to `/help` output

Example:
```rust
use crate::commands::registry::CommandResult;

pub fn handle_my_command(ctx: &mut CommandContext, _args: &[&str]) -> CommandResult {
    // Command logic here
    CommandResult::Ok
}
```

### Adding a New Tool

1. Create `tinyharness-lib/src/tools/<name>.rs`
2. Implement using `make_tool()` and `build_string_params_schema()`
3. Add `pub mod <name>;` to `tinyharness-lib/src/tools/mod.rs`
4. Register in `ToolManager::register_defaults()`
5. Assign a `ToolCategory` (ReadOnly, Destructive, or Signal)
6. If Destructive, wire into confirmation flow (`src/agent/tools.rs`)
7. If Signal, add to `parse_signal_event()` and agent loop handling

### Adding a New Provider

1. Create `tinyharness-lib/src/provider/<name>.rs`
2. Implement the `Provider` trait
3. Add to `ProviderKind` enum in `config/mod.rs`
4. Add CLI flag in `main.rs`
5. Add provider creation in `src/agent/setup.rs`
6. If the provider requires credentials (like Sockudo), add fields to `Settings` and handle them in setup
7. If the provider needs a separate bridge/worker process (like Sockudo), create a standalone example crate (see `docs/examples/sockudo-worker/` for reference)

### Modifying Settings

1. Add the field to `Settings` struct in `tinyharness-lib/src/config/mod.rs`
2. Add a default value in `Default::default()` (or derive `#[serde(default)]`)
3. Consider per-project override support in `ProjectSettings`
4. Add a slash command to modify it (optional, for user-facing settings)

---

## Where to Get Help

- **Code questions**: Look at existing patterns — most modules follow consistent idioms
- **Architecture**: Read `TINYHARNESS.md` (the project's own instructions) and the module overview above
- **OpenAI-compat provider**: See [Configuration Guide](configuration.md#openai-compatible-provider-settings) for API key setup
- **Sockudo provider**: ⚠️ Highly experimental — see [Configuration Guide](configuration.md#sockudo-provider-experimental) for setup and limitations
- **Planned work**: Check `todo/todo.md` and `todo/<number>-*.md` for tracked enhancements
- **Tool docs**: See [Tools Reference](tools-reference.md) for tool schemas and behavior
- **Configuration**: See [Configuration Guide](configuration.md) for settings and paths

---

## License

MIT — see `LICENSE` at the repository root. All contributions are under the same license.
