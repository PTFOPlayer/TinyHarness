# ADR-0004: Skills as `SKILL.md` files, not as code

**Status:** Accepted

## Context

Users want to teach the agent reusable behaviour: "here is how we review Rust in
this repo", "here is the deploy checklist". The obvious extension points are
plugins or custom tools, but both are programs — a shared skill becomes a supply
chain problem, and authoring one requires a Rust toolchain.

Prompt-level customisation solves the actual use case (injecting instructions)
without executing anything, and matches what adjacent tools already do
(`CLAUDE.md`, VS Code agent skills), so existing artefacts are portable.

## Decision

A skill is a directory containing `SKILL.md` (`tinyharness-lib/src/skill.rs`):

- optional YAML-ish frontmatter between `---` markers: `name`, `description`,
  `argument-hint`, `compatibility`, `disable-model-invocation`, `license`,
  `metadata` (flat key/value block), `user-invocable`;
- the body is the instruction text, truncated to `SKILL_MAX_CHARS` (10,000) with
  a 70/30 head/tail split that tells the model to `read` the full file.

Discovery is `<config_dir>/skills/*/SKILL.md`, then the legacy
`~/.tinyharness/skills/*/SKILL.md`, then `<project>/.tinyharness/skills/*/SKILL.md`.
Later sources win on name collision, so a project can override a personal skill.
Skills are surfaced two ways: an always-present index in the system prompt, and
full content injected only when active (via `/skill` or the `invoke_skill`
signal tool). `disable-model-invocation` keeps a skill manual-only.

Frontmatter is parsed by hand rather than with `serde_yaml`: it is flat
`key: value` data plus one indented block, and a YAML dependency is not worth it.

## Consequences

- A skill can change what the model *says*, never what it can *do*. A skill that
  needs a new capability requires a tool — that boundary is deliberate.
- Sharing a skill is sharing text: no code execution, no trust decision beyond
  "do I want these instructions in my context".
- The frontmatter parser is not a YAML parser. Nested structures, anchors and
  multi-line scalars do not work; unknown keys are ignored silently.
- A malformed skill logs a warning and is skipped; it never fails startup.
- Content is truncated by characters, not tokens, so a large skill can still
  cost more context than expected.

## Alternatives considered

- **Rust plugins / WASM.** Real capability, but a build toolchain plus a sandbox
  for user-supplied code — much larger than the problem.
- **Extend custom tools instead.** Custom tools run shell commands; that is the
  wrong primitive for "inject instructions".
- **`serde_yaml`.** A dependency to parse eight flat keys.
