# Changelog

All notable changes to TinyHarness are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.0] - 2026-11-26

This release is mostly about **documentation debt and API surface reduction**.
Two breaking changes to `tinyharness-lib` are the reason for the major bump.

### Added

- **Architecture Decision Records** — 17 ADRs in [`docs/adr/`](docs/adr/README.md)
  plus a template and index, covering the JSONL session format, the provider
  interface, the command safety model, the skill system, mode-based tool
  filtering, per-project settings layering, language detection, the two
  containment layers, `Paths` injection, compaction strategy, streaming, tool-pair
  repair, the unified OpenAI-compatible provider, global-only custom tools,
  system-prompt assembly, tool-handler closures, and the `thought_signature`
  removal.
- `docs/contributing.md` gained an **Architecture Decisions** section describing
  when a change warrants an ADR and how to write one.

### Changed

- **`AnyProvider` is now the provider interface.** The `Provider` trait has been
  removed; all seven methods (`chat`, `health_check`, `list_models`,
  `select_model`, `current_model`, `set_timeout`, `set_retries`) are inherent on
  both `AnyProvider` and each backend. Adding a backend is now an enum variant
  plus a match arm. Different backends deliberately use different shapes: the
  provider set is closed (enum), while tool handlers stay boxed closures because
  custom tools are runtime data. See
  [ADR-0002](docs/adr/002-anyprovider-enum.md) and
  [ADR-0016](docs/adr/016-tool-handler-closures.md).
- Rationale that previously lived only in long code comments now lives in the
  ADRs, with the comments reduced to a local summary plus a reference.
- The startup-card tests no longer hardcode the version, so a release bump cannot
  leave them asserting a version that does not exist.

### Removed

- **`Provider` trait** (breaking). Removed from `tinyharness-lib` along with its
  re-export at the crate root. Consumers holding `Arc<Mutex<dyn Provider>>` or
  implementing the trait for an out-of-tree backend must move to
  `Arc<Mutex<AnyProvider>>`; there is no longer a backend extension point.
- **`ToolCallFunction::thought_signature`** (breaking). The field was documented
  as the Gemini signed-token round-trip required for multi-turn tool calling, but
  it has been dead since the native `OllamaProvider` was replaced: an audit found
  14 construction sites and zero `Some` values, with nothing ever reading it.
  Gemini tool calling is now honestly unsupported rather than silently
  half-supported. See [ADR-0017](docs/adr/017-toolcall-field-removal.md).
- Two tracked docs no longer cite `todo/` files by number — that directory is
  gitignored and its numbering is not stable.

### Fixed

- `SandboxConfig::disabled()` was documented as "No sandbox (the default)" and
  `Settings::sandbox` as "Defaults to disabled", but `Default::default()` returns
  `auto()`. The documentation now matches the behaviour — sandboxing is on by
  default.
- A broken intra-doc link in `SockudoProvider::set_retries` that made
  `cargo doc` warn.

### Breaking changes

If you only use the `tinyharness` binary, nothing changes. If you use
`tinyharness-lib` as a library:

```rust
// 0.5.x
use tinyharness_lib::provider::Provider;
fn chat(provider: &mut impl Provider) { /* … */ }
let sig = call.function.thought_signature;

// 0.6.0
use tinyharness_lib::provider::AnyProvider;
async fn chat(provider: &mut AnyProvider) { /* … */ }
```

## [0.5.6] and earlier

Not retroactively documented — see `git log` for the history up to and including
0.5.6.

[Unreleased]: https://github.com/PTFOPlayer/TinyHarness/compare/0.6.0...HEAD
[0.6.0]: https://github.com/PTFOPlayer/TinyHarness/compare/0.5.6...0.6.0
