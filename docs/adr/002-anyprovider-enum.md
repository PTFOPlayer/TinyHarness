# ADR-0002: A single `AnyProvider` enum as the provider interface (no `Provider` trait)

**Status:** Accepted (supersedes the earlier RPITIT `Provider` trait design, which
in turn superseded a `Pin<Box<dyn Future>>` trait)

## Context

TinyHarness talks to several LLM backends and shares the runtime-selected one
through `Arc<Mutex<…>>` so that `/model`, `--vllm` and `--sockudo` can swap it
mid-session. Historically that was a `Provider` trait with one impl per backend.

Two things then changed underneath it:

- **The backends collapsed.** Ollama, llama.cpp, vLLM and hosted gateways all
  speak `/v1/chat/completions`, so they became one `OpenAiCompatProvider` over a
  shared `OpenAiCompatInner` ([ADR-0013](013-unified-openai-compat-provider.md)).
  The trait's main justification — "many backends, one interface" — was spent.
- **Nothing was generic over it.** There is no `P: Provider` anywhere in the
  workspace. Every call site already goes through `provider.lock().await…`, i.e.
  through the enum. The trait added a second dispatch layer on top of the `match`
  that has to exist anyway.

What remained was: an enum with a forwarding `impl Provider for AnyProvider`, and
three concrete types implementing a 7-method trait so that one enum could call
them. Three `impl` blocks and a trait definition, to express "there are three
kinds of backend".

## Decision

There is no `Provider` trait. `AnyProvider`
(`tinyharness-lib/src/provider/mod.rs`) **is** the provider interface: a concrete
enum with inherent async methods.

```rust
pub enum AnyProvider {
    OpenAiCompat(OpenAiCompatProvider),
    Sockudo(SockudoProvider),
    #[cfg(feature = "test-util")]
    Mock(MockProviderHandle),
}

impl AnyProvider {
    pub async fn health_check(&self) -> Result<(), String> { … }
    pub async fn list_models(&self) -> Vec<String> { … }
    pub fn select_model(&mut self, name: String) { … }
    pub fn current_model(&self) -> Option<String> { … }
    pub async fn chat(&mut self, messages: Vec<Message>, tools: Vec<ToolDefinition>)
        -> Result<tokio::sync::mpsc::Receiver<ChatMessageResponse>, String> { … }
    pub fn set_timeout(&mut self, timeout_secs: u64) { … }
    pub fn set_retries(&mut self, max_retries: u32) { … }
}
```

Each backend also exposes the same seven methods as **inherent** methods, so the
enum arms are plain forwarding calls. Shared state is `Arc<Mutex<AnyProvider>>`.

The chat contract is unchanged ([ADR-0011](011-streaming-via-mpsc.md)): a
receiver of chunks, terminated by `done: true`, with token usage on the final
chunk. `set_timeout`/`set_retries` are per-backend inherent methods; Sockudo's
`set_retries` is an explicit documented no-op because its transport has no retry
logic, rather than an inheriting default.

## Consequences

- **Adding a backend is an enum variant plus seven match arms.** That is the
  registry, and it is now visible in one place instead of spread across a trait
  definition and three impl blocks.
- **The library no longer offers a backend extension point.** An out-of-tree
  crate cannot add a provider without patching `AnyProvider` (or forking). This
  is the accepted cost: the crate is a CLI plus its library, and the trait was
  documented as a seam nobody used.
- `AnyProvider` can now be made `dyn`-free end to end; there is no
  `Arc<Mutex<dyn Provider>>` question to answer.
- The forwarding bodies are duplicated between `AnyProvider` and each backend's
  inherent impl. That duplication is the price of not having a trait, and it is
  small (7 short methods × 3 backends).
- Generic code that wanted `fn f<P: Provider>(p: P)` cannot be written. Nothing
  in the workspace wanted it; a future need would reintroduce a trait, which is a
  new ADR.
- Renaming or adding a method is a mechanical edit across four files with no
  compiler-enforced "you forgot the mock" — a `#[cfg(feature = "test-util")]` arm
  missing from one match is caught by the test build.

## Alternatives considered

- **Keep the RPITIT trait** (the previous decision). Correct, dyn-incompatible by
  design, and it worked — but it existed to serve a multi-backend abstraction
  that [ADR-0013](013-unified-openai-compat-provider.md) had already removed, and
  nothing used it generically.
- **Keep the trait, drop the enum**, using `Arc<Mutex<dyn Provider>>`. Needs
  `async-trait` or hand-boxed futures for every method, and adds a dependency to
  reach the same dispatch.
- **`enum_dispatch`.** A macro dependency for the forwarding code; the
  hand-written version is ~70 lines and has no version coupling.
- **Generic `P: Provider` threaded through the agent loop.** Would infect
  `CommandContext` and the command registry with a type parameter, and `/model`
  switches the backend at runtime, so the type would have to be an enum anyway.
