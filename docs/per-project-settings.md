# Per-Project Settings

TinyHarness supports per-project configuration via `.tinyharness/config.json`. This lets you customize behavior for a specific project without affecting your global setup.

## How It Works

Settings are layered in this priority order:

```
CLI flags                          (highest)
  → .tinyharness/config.json       (project)
    → ~/.config/tinyharness/settings.json  (global)
      → hardcoded defaults         (lowest)
```

The `.tinyharness/config.json` file is discovered by walking up from the current working directory — the same algorithm used to find `TINYHARNESS.md`. The first match wins (closest to CWD).

## Creating a Project Config

Run the scaffolding command:

```
/project-settings init
```

This generates `.tinyharness/config.json` in your project root with all available fields commented out:

```json
{
  "auto_accept_safe_commands": true
}
```

Uncomment and change fields as needed. Run `/project-settings` to see the effective merged settings.

## Available Fields

### `safe_command_prefixes`

Extends the global safe command list. Useful for adding project-specific commands that should be auto-accepted.

```json
{
  "safe_command_prefixes": ["pytest", "npm run lint", "just build"]
}
```

This is additive — your project commands are merged with the global safe list, not replacing it.

### `denied_command_prefixes`

Always blocks these commands from auto-accept, even if they'd match a safe prefix. This replaces the global deny list entirely for this project.

```json
{
  "denied_command_prefixes": ["git push --force", "rm -rf"]
}
```

### `auto_accept_safe_commands`

Toggle whether safe commands are auto-accepted without confirmation.

```json
{
  "auto_accept_safe_commands": false
}
```

Set to `false` on sensitive projects to require manual approval for every command.

### `context_limit`

Override the context window warning threshold (in tokens). The default is model-dependent. Use this if your project requires a specific context size.

```json
{
  "context_limit": 32768
}
```

Set to `null` to use the model default.

### `project_md_files`

Additional instruction files to load into the AI's context. These are loaded after the main instruction file (TINYHARNESS.md or equivalent).

```json
{
  "project_md_files": ["RULES.md", "DEPLOYMENT.md", ".cursorrules"]
}
```

Each file must exist in the project root. Files are truncated at 20,000 characters.

### `preferred_mode`

Set the default agent mode when starting a session in this project.

```json
{
  "preferred_mode": "agent"
}
```

Valid values: `casual`, `planning`, `agent`, `research`.

## Viewing Merged Settings

```
/project-settings
```

Shows all effective settings with source annotations:

```
╭─ Project Settings (.tinyharness/config.json) ─╮
│ Mode:      agent (project)
│ Auto-Accept: enabled (default)
│ Ctx Limit:  32768 tokens (project)
│ Safe Cmds:  48 configured (default)
│ Denied:     3 denied (project)
│ Extra MD:   RULES.md, DEPLOYMENT.md (project)
╰─────────────────────────────────────────────────╯

Legend: (project) = from .tinyharness/config.json
        (global)  = from ~/.config/tinyharness/settings.json
        (default) = hardcoded default
```

## Common Use Cases

### Strict project with manual approvals

```json
{
  "auto_accept_safe_commands": false,
  "denied_command_prefixes": ["rm", "mv", "chmod", "chown", "sudo"]
}
```

### Python project with custom test commands

```json
{
  "safe_command_prefixes": ["python -m pytest", "pip install -e '.[dev]'", "ruff check"],
  "preferred_mode": "agent"
}
```

### Monorepo with extra instruction files

```json
{
  "project_md_files": ["RULES.md", "frontend-GUIDELINES.md", "backend-CONVENTIONS.md"]
}
```

## Files on Disk

```
my-project/
├── .tinyharness/
│   ├── config.json          # Per-project settings (this file)
│   └── skills/              # Project-local skills
│       └── my-skill/
│           └── SKILL.md
├── TINYHARNESS.md           # Main project instructions
└── src/
```
