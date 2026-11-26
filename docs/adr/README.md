# Architecture Decision Records

Architecture Decision Records (ADRs) capture *why* TinyHarness is built the way
it is. They complement the module docs: a doc comment says what a function does,
an ADR says why the function exists in that shape and what it costs.

They are written for the next contributor — the one who is about to "fix" a
deliberate workaround and needs to know what breaks if they do.

## Format

Every ADR uses [template.md](template.md):

- **Status** — `Proposed`, `Accepted`, `Deprecated`, or `Superseded by ADR-XXXX`
- **Context** — the forces at play, without presupposing the answer
- **Decision** — what we do, in the active voice, naming the module that carries it
- **Consequences** — what gets easier, what gets harder, what is knowingly accepted
- **Alternatives considered** — optional, but cheap and often the most useful section

ADRs are immutable once accepted. To change a decision, write a new ADR with a
new number and mark the old one `Superseded by ADR-XXXX` — do not edit history.

## Index

| # | Title | Status |
|---|-------|--------|
| [001](001-jsonl-session-format.md) | JSONL as the session storage format | Accepted |
| [002](002-anyprovider-enum.md) | A single `AnyProvider` enum as the provider interface (no `Provider` trait) | Accepted |
| [003](003-command-safety-model.md) | Command safety by prefix allow/deny lists, not by parsing the shell | Accepted |
| [004](004-skill-system.md) | Skills as `SKILL.md` files, not as code | Accepted |
| [005](005-mode-based-tool-filtering.md) | Modes filter tools by category, and signal tools are handled inline | Accepted |
| [006](006-per-project-settings-layering.md) | Per-project settings layer over global settings, with an asymmetric merge | Accepted |
| [007](007-language-detection.md) | Project type detected from marker files, not from file extensions | Accepted |
| [008](008-two-layer-containment.md) | Two independent containment layers — path guard and process sandbox | Accepted |
| [009](009-paths-injection.md) | `Paths` as the single ambient-environment boundary in the library | Accepted |
| [010](010-context-compaction-strategy.md) | Compaction keeps a verbatim tail and chooses single-pass vs cascade by count | Accepted |
| [011](011-streaming-via-mpsc.md) | Streaming through a bounded `mpsc` channel with a sentinel-terminated protocol | Accepted |
| [012](012-tool-pair-repair.md) | Repair assistant ↔ tool-call pairing at request time, not at each cause | Accepted |
| [013](013-unified-openai-compat-provider.md) | One OpenAI-compatible provider for llama.cpp, vLLM and hosted gateways | Accepted |
| [014](014-custom-tools-global-only.md) | Custom tools load from the global config only | Accepted |
| [015](015-system-prompt-assembly.md) | System prompts are seeded files, assembled as header + mode | Accepted |
| [016](016-tool-handler-closures.md) | Tool handlers are boxed closures over `HashMap<String, String>` | Accepted |
| [017](017-toolcall-field-removal.md) | Remove the dead `thought_signature` tool-call field | Accepted |

## Reading order

New to the codebase? These four explain the shape of everything else:

1. [009](009-paths-injection.md) — every filesystem location comes from `Paths`
2. [002](002-anyprovider-enum.md) — how a backend is implemented and shared
3. [016](016-tool-handler-closures.md) — how a tool is implemented and registered
4. [008](008-two-layer-containment.md) — what an approved tool call can actually reach

Then, for whatever you are touching: [003](003-command-safety-model.md) and
[006](006-per-project-settings-layering.md) for safety/settings,
[005](005-mode-based-tool-filtering.md) and [015](015-system-prompt-assembly.md)
for modes and prompts, [010](010-context-compaction-strategy.md) and
[012](012-tool-pair-repair.md) for long conversations,
[001](001-jsonl-session-format.md) for persistence,
[004](004-skill-system.md) and [014](014-custom-tools-global-only.md) for
extensibility, [007](007-language-detection.md) and
[013](013-unified-openai-compat-provider.md) for startup.

## Cross-cutting rules

A few rules recur across ADRs, and are worth remembering as rules rather than as
per-decision details:

- **Nothing in `tinyharness-lib` reads the ambient environment** except `Paths`
  ([009](009-paths-injection.md)).
- **A repository-controlled file may not grant execution or widen access.** That
  is why `sandbox`, `allowed_paths` and custom tools are global-only
  ([006](006-per-project-settings-layering.md), [014](014-custom-tools-global-only.md)).
- **Fail closed on paths** ([008](008-two-layer-containment.md)); **fail loud on
  a missing sandbox** (same); **fail soft on bad user input** — a malformed
  skill, project config or prompt file logs and degrades
  ([004](004-skill-system.md), [006](006-per-project-settings-layering.md),
  [015](015-system-prompt-assembly.md)).
- **Decide once where the shape is known.** The tool-pair repair lives where the
  request is assembled ([012](012-tool-pair-repair.md)); the path guard runs
  where the handler is about to open a file.

## Writing a new ADR

Copy `template.md` to `NNNN-short-slug.md` (next free number), fill it in, add
the row to the index above, and mention it in the PR description. See
[CONTRIBUTING.md](../contributing.md#architecture-decisions) for when a change
warrants one.
