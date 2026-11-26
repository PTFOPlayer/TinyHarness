# ADR-0007: Project type detected from marker files, not from file extensions

**Status:** Accepted

## Context

The model needs to be told how to build and test the workspace. Guessing from
the extension of whichever file it happens to read is unreliable and arrives too
late; the answer must be in the system prompt before the first turn. What we
need is a cheap, deterministic guess made once at startup from the directory
listing.

## Decision

`tinyharness-lib/src/context.rs` holds an ordered `LANGUAGE_SIGNATURES` table:
each entry is (label, marker files, build command, test command). Detection
checks for marker files in the project root and takes **every** match, joining
multiple labels with `" + "` — that is the monorepo case, e.g. `"Rust + Node.js"`.
Order in the table is the display/priority order.

A few labels need globs rather than fixed names (`.NET` via `*.csproj`/`*.sln`,
`Haskell (Cabal)` via `*.cabal`), so those are a secondary pass after the fixed
markers.

Fallbacks, in order: `Makefile`/`Justfile` yield
`"Unknown (has Makefile)"` and `make`/`make test`; otherwise `"Unknown"` with no
commands (the fields are omitted from the prompt entirely rather than left
blank).

The project *name* is read from the detected language's manifest
(`Cargo.toml`/`package.json`/`pyproject.toml`/`go.mod`) with a directory-name
fallback. The signature for that is line-wise substring extraction, not a real
TOML/JSON parse for the TOML cases — `serde_json` is used for `package.json`
because it is already a dependency.

## Consequences

- Deterministic and instant — no build system is invoked, no file is scanned.
- Cannot detect a language with no marker file (plain C with only `.c` files, a
  hand-rolled build script). That is an accepted blind spot; the `TINYHARNESS.md`
  escape hatch covers it.
- The commands are guesses about convention, not about this repo. A workspace
  whose tests really run under `cargo nextest` still gets `cargo test` unless
  `TINYHARNESS.md` overrides it.
- Adding a language is a table entry, which is why the table has 17+ rows and is
  cheap to extend.
- Monorepo detection is "root markers only" — a nested `frontend/package.json`
  is not found unless the root also has a JS marker.

## Alternatives considered

- **Extension frequency counting.** More expensive, still a guess, and wrong for
  any polyglot repo with vendored code.
- **Running the build system's introspection** (`cargo metadata`, `npm pkg get`).
  Correct, but spawns processes at startup and needs each toolchain installed.
- **Single-label detection.** Loses the monorepo case, which is common.
