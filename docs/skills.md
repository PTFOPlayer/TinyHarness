# Skills Guide

Skills are pluggable instruction modules that give the AI specialized knowledge about tools, workflows, or conventions. They're discovered from markdown files and can be activated by the user or (optionally) by the AI itself.

## How Skills Work

1. **Discovery** — On startup, TinyHarness scans `~/.config/tinyharness/skills/` and `.tinyharness/skills/` for directories containing `SKILL.md` files.
2. **Injection** — When activated (by user or AI), the skill's content is injected into the system prompt, giving the AI specialized context.
3. **Deactivation** — Skills stay active until explicitly unloaded with `/unload <name>`.

## Skill File Format

Each skill lives in a directory named after the skill, containing a `SKILL.md` file:

```
~/.config/tinyharness/skills/
└── rust-dev/
    └── SKILL.md

.tinyharness/skills/
└── python-lint/
    └── SKILL.md
```

### SKILL.md Structure

```markdown
---
name: rust-dev
description: Rust development best practices and code review guidelines
argument-hint: Rust file or module to review
compatibility: rust
disable-model-invocation: false
license: MIT
metadata:
  version: "1.0"
  author: team-name
user-invocable: true
---

# Rust Development Skill

Always run `cargo fmt` and `cargo clippy -- -D warnings` before
suggesting changes. Prefer `cargo test -- --nocapture` for debugging.

## Code Style

- Use Rust edition 2024
- Prefer `impl Trait` over generics for single-use cases
- Document public APIs with doc comments
```

### Frontmatter Reference

All fields are optional. Sensible defaults apply when fields are omitted.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | Directory name | Unique identifier for the skill. Used with `/use <name>` |
| `description` | string | `""` | Short description shown in `/skills` listings |
| `argument-hint` | string | — | Hint about what argument to pass (e.g. "file path to review") |
| `compatibility` | string | — | Compatibility tag (e.g. "rust", "python", "any") |
| `disable-model-invocation` | boolean | `false` | If `true`, the AI cannot auto-invoke this skill — only manual `/use` works |
| `license` | string | — | SPDX license identifier (e.g. "MIT", "Apache-2.0") |
| `metadata` | map | — | Arbitrary key-value pairs (version, author, etc.) |
| `user-invocable` | boolean | `true` | Whether users can invoke via `/use <name>` |

### Frontmatter Notes

- The `metadata` block uses indented sub-properties (two spaces):
  ```yaml
  metadata:
    version: "1.0"
    author: team
  ```
- Boolean fields accept `true` or anything else (treated as `false`)
- Unknown keys are silently ignored
- Quoting values is optional for simple strings

## Skill Discovery

### Personal Skills (`~/.config/tinyharness/skills/`)

Available to all projects for the current user. Good for personal workflows and preferences.

```bash
# Create a personal skill
mkdir -p ~/.config/tinyharness/skills/my-review-workflow
vim ~/.config/tinyharness/skills/my-review-workflow/SKILL.md
```

### Project Skills (`.tinyharness/skills/`)

Project-specific skills, committed to the repository. Good for team conventions and project-specific tooling.

```bash
# Create a project-local skill
mkdir -p .tinyharness/skills/backend-conventions
vim .tinyharness/skills/backend-conventions/SKILL.md
```

**Precedence**: Project skills override personal skills with the same name. If both `~/.config/tinyharness/skills/review/SKILL.md` and `.tinyharness/skills/review/SKILL.md` exist, the project version wins.

## Invoking Skills

### By the User

| Command | Effect |
|---------|--------|
| `/skills` | List all available skills |
| `/skill <name>` | Show a skill's details and content |
| `/use <name>` | Activate a skill, injecting its instructions |
| `/unload <name>` | Deactivate a previously loaded skill |

### By the AI

The AI can call `invoke_skill` with the skill name. This is allowed unless `disable-model-invocation: true` is set in the skill's frontmatter.

Example in the system prompt (auto-generated from the skill registry):
```
- **review**: Code review and analysis skill (arg: file path or diff to review) [any] _(manual invocation only)_
```

### Active Skills

Multiple skills can be active simultaneously. Each skill's content is injected into the system prompt with a header:

```
## Active Skill: rust-dev

Rust development best practices and code review guidelines

---
Skill instructions:
...
```

Skills are listed in the available tools section as well, so the AI knows what's available even before invocation.

## Content Truncation

Skills longer than 10,000 characters are truncated to prevent overwhelming the AI's context window:

- 70% of the content from the **head** is kept
- 30% from the **tail** is kept
- A truncation notice is inserted in between

```
[...truncated skill 'my-skill': showing first 7000 + last 3000 chars. Use the read tool to view the full file.]
```

Keep skills concise. Use the `read` tool to load detailed reference material when needed.

## Examples

### Minimal Skill

```markdown
# TypeScript Conventions

Always use `const` over `let`. Prefer arrow functions.
```

No frontmatter needed. The skill name defaults to the directory name.

### Model-Restricted Skill

```markdown
---
name: secret-ops
description: Internal deployment procedures
disable-model-invocation: true
user-invocable: false
---

# Secret Operations

These procedures are for human operators only. Never share with automated tools.
```

This skill can never be invoked by the AI (`disable-model-invocation: true`) and can't even be activated by the user with `/use` (`user-invocable: false`). It exists purely as documentation accessible via `/skill secret-ops`.

### Project-Specific Skill (Team Shared)

```markdown
---
name: backend-standards
description: Backend code review and development standards
compatibility: rust
metadata:
  version: "2.1"
  last-reviewed: "2026-01-15"
---

# Backend Standards

## PR Checklist
1. `cargo fmt --all -- --check` passes
2. `cargo clippy --workspace -- -D warnings` clean
3. All tests pass: `cargo test --workspace`
4. New public APIs have doc comments
5. No new dependencies without team discussion

## Architecture
- Keep `tinyharness-lib` free of terminal I/O
- Prefer `Pin<Box<dyn Future>>` over `async-trait`
- Use `Result<T, String>` for user-facing errors
```

Place this in `.tinyharness/skills/backend-standards/SKILL.md`, commit to the repo, and the whole team gets it.

## System Prompt Integration

When skills are active, the system prompt is rebuilt from scratch:

1. Shared header (`header.md`)
2. Mode-specific prompt (`agent.md`, `planning.md`, etc.)
3. Project context (language, build/test commands)
4. Project instructions (TINYHARNESS.md and additional files)
5. **Active skill content** ← injected here
6. Available tools and skill index

Prompts are refreshed on mode switch, skill activation/unload, file pinning changes, and `/refresh`.

## Best Practices

1. **Keep skills focused**: One concern per skill. "Database conventions" and "Deployment procedures" should be separate.
2. **Use metadata**: Track version and last-review dates for team skills.
3. **Set compatibility tags**: Helps the AI decide when to auto-invoke a skill.
4. **Disable model invocation for sensitive content**: Use `disable-model-invocation: true` if the skill contains procedures that need human judgment.
5. **Truncate large skills intentionally**: Write the most important 7,000 characters first. Put reference tables, examples, and edge cases in the tail.
6. **Project skills for teams**: Commit `.tinyharness/skills/` to the repository for shared conventions.
7. **Personal skills for preferences**: Keep personal workflow preferences in `~/.config/tinyharness/skills/`.
