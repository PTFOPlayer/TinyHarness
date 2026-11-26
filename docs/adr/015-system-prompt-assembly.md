# ADR-0015: System prompts are seeded files, assembled as header + mode

**Status:** Accepted

## Context

The system prompt must be user-editable (people tune agent behaviour) but must
also work on a fresh install with no configuration, and must not silently break
when a user edits a file. It also changes at runtime: switching mode, pinning a
file, activating a skill, and `/refresh` all rebuild it.

A single monolithic prompt file fails on all counts: nothing to switch, and every
customisation is a fork of the whole thing.

## Decision

Prompts live in `<config_dir>/prompts/` (`tinyharness-lib/src/mode.rs`,
`config::ensure_prompts_initialized_in`):

- `header.md` — shared developer preamble;
- `casual.md`, `planning.md`, `agent.md` — one per mode.

On first launch the directory is created and seeded from
`include_str!("prompts/*.md")`. **Existing files are never overwritten**, so
customisation survives upgrades. An empty or unreadable file falls back to the
embedded default for that mode, so a broken edit degrades rather than breaks.

Assembly is `header.md` + blank line + `<mode>.md`, except `Casual`, which is
self-contained (`AgentMode::uses_header()` is false for it) — chat mode needs no
developer preamble.

`CommandContext::build_system_prompt()` appends, in order: workspace context
([ADR-0007](007-language-detection.md)), pinned file contents, the skill index,
and active skill instructions. When a process sandbox is active it also appends
a "Sandboxed commands" section describing what does not exist inside it — the
path guard is deliberately *not* described there, because its errors explain
themselves and a prompt section invites the model to argue with it.

The prompt is rebuilt on every mode switch, file pin / unpin, skill activation,
and `/refresh` — never cached across those events.

## Consequences

- Users can customise a section without forking the whole prompt, and the header
  changes propagate to all three modes on upgrade for anyone who never edited it.
- Seeding is add-only: a new mode added later *does* get its seed file written,
  but a changed default in an existing file never reaches users who already have
  it. That is the accepted price of "never overwrite".
- Editing `header.md` to be empty restores the embedded default silently, which
  can look like the edit was ignored.
- Assembly order is implicit in `build_system_prompt`'s sequence; a new section
  has to be inserted there and in `docs/modes.md` by hand.
- `AgentMode::from_str` accepts aliases (`plan`, `dev`, `research`), so
  `research` maps to `planning` — a user who sets `research` gets planning
  prompts and planning tools, with no warning.

## Alternatives considered

- **One prompt file per install.** No mode switching without duplicating text.
- **Prompt in `settings.json` as a string.** Unreadable in a JSON blob, and
  multi-line escaping makes hand-editing miserable.
- **Templating engine (`handlebars`, `minijinja`).** A dependency for string
  concatenation; the one place that needs tempo is prompt assembly order, not
  substitution.
- **Generate the prompt once at startup and cache it.** Would silently serve a
  stale prompt after a mode switch — the exact bug the rebuild triggers avoid.
