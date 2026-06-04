# Contributing

TinyHarness is a Rust workspace with three crates. See the full guide at [docs/contributing.md](docs/contributing.md).

## Quick Start

```bash
git clone https://github.com/yourusername/TinyHarness.git
cd TinyHarness
cargo build --workspace
cargo test --workspace
```

## Verification Checklist

Before submitting a PR:

```bash
cargo fmt --all
cargo clippy --workspace -- -D warnings
cargo test --workspace
cargo build
```

## Docs

User-facing docs are in `docs/`. Developer docs (this file) are in `docs/contributing.md`. Enhancement tracking is in `todo/` (local only, not committed).

## License

MIT — see [LICENSE](LICENSE).
