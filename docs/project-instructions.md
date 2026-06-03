# Configurable Project Instructions

TinyHarness loads project instruction files (like `TINYHARNESS.md`) to give the AI persistent context about your project. The discovery process is fully configurable.

## Default Behavior

By default, TinyHarness searches for these files in priority order, walking up from the current directory to the filesystem root:

| Priority | File | Purpose |
|----------|------|---------|
| 1 | `TINYHARNESS.md` | TinyHarness-native instruction file |
| 2 | `.tinyharness.md` | Hidden variant |
| 3 | `AGENTS.md` | Industry standard, compatible with other AI tools |
| 4 | `CLAUDE.md` | Claude Code compatibility |

The first file found wins. If `TINYHARNESS.md` exists in your project root, it's used and the search stops — `AGENTS.md` and `CLAUDE.md` are ignored.

## Customizing the File List

You can change which files are searched and in what order.

### Via Environment Variable

```bash
# Replace the default list entirely:
export TINYHARNESS_MD_FILES="CLAUDE.md,TEAM_RULES.md"
tinyharness
```

The env var takes highest priority. When set, it completely replaces the default list.

### Via Global Settings

In `~/.config/tinyharness/settings.json`:

```json
{
  "project_md_files": ["CLAUDE.md", ".cursorrules", "TEAM_RULES.md"]
}
```

This takes effect when the env var is not set.

### Priority Chain

```
TINYHARNESS_MD_FILES env var     (highest)
  → settings.json project_md_files
    → hardcoded defaults          (lowest)
```

## Additional Per-Project Files

Beyond the main instruction file, you can load additional files into the AI's context via `.tinyharness/config.json`:

```json
{
  "project_md_files": ["RULES.md", "DEPLOYMENT.md"]
}
```

These are loaded after the main instruction file and appear as separate sections in the system prompt:

```
---
# Project Instructions (from TINYHARNESS.md)
...main instructions...

---
# Additional Instructions (from RULES.md)
...extra rules...

---
# Additional Instructions (from DEPLOYMENT.md)
...deployment notes...
```

Each file is truncated at 20,000 characters (70% head / 30% tail).

## Generating Instruction Files

Use `/init` to have the AI analyze your project and generate a `TINYHARNESS.md`:

```
[agent]> /init
  Generating project instruction file...
  ✦ Created /path/to/TINYHARNESS.md (45 lines)
```

If one already exists, `/init` updates it — preserving accurate sections, removing outdated ones, and adding what's missing.

## Use Cases

### Team with shared rules

Multiple team members use TinyHarness. The team has `TEAM_RULES.md` with shared conventions.

```bash
# Everyone sets:
export TINYHARNESS_MD_FILES="TEAM_RULES.md,TINYHARNESS.md,AGENTS.md"
```

Now `TEAM_RULES.md` is always loaded first.

### Claude Code compatibility

You already have a `CLAUDE.md` and want TinyHarness to use it as the primary file:

```bash
export TINYHARNESS_MD_FILES="CLAUDE.md"
```

### Suppress AGENTS.md

Your project has a generic `AGENTS.md` for other tools, but you don't want TinyHarness to pick it up:

```json
{
  "project_md_files": ["TINYHARNESS.md", ".tinyharness.md"]
}
```

By omitting `AGENTS.md` from the list, it's never discovered.

### Multiple instruction files by concern

```json
{
  "project_md_files": ["CODING-STYLE.md", "ARCHITECTURE.md", "DEPLOY.md"]
}
```

Each concern in its own file, all loaded into context.
