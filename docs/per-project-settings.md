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
  "auto_accept_mode": "safe"
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

### `auto_accept_mode`

Override the global auto-accept mode. Values: `"off"`, `"safe"` (read-only commands auto-accepted), or `"all"` (all destructive tools except `run`).

```json
{
  "auto_accept_mode": "off"
}
```

Set to `"off"` on sensitive projects to require manual approval for every command.

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

### `questions_enabled`

Controls whether the model may stop the agent to ask the user a multiple-choice
question. Defaults to `true`; set to `false` to hide the `question` tool
entirely (useful for unattended runs, CI, or `--prompt` one-shot mode).

```json
{
  "questions_enabled": false
}
```

### `auto_compact_enabled`

Controls whether the model may request conversation compaction. Defaults to `true`.

```json
{
  "auto_compact_enabled": false
}
```

### `sandbox` and `allowed_paths` — not project settings

Neither the process sandbox **nor** the path-guard allow list can be configured
per project. `ProjectSettings` has no `sandbox` or `allowed_paths` field, and
those blocks in `.tinyharness/config.json` are ignored (unknown keys are
dropped by serde).

This is deliberate: a project config travels inside the repository, so anyone
who sends you a clone controls it. The sandbox block could pin a weaker
backend or set `"backend": "off"`; worse, an `allowed_paths` entry widens the
**in-process file tools**, which no OS sandbox confines — the file tools' guard
is their only protection, and it grants read *and* write, so a cloned repo
adding `"allowed_paths": ["~"]` could let the model overwrite `~/.bashrc` or
`~/.ssh/authorized_keys`. Both are therefore global-only: `sandbox.backend` and
`allowed_paths` in `~/.config/tinyharness/settings.json`, plus `--sandbox=…` /
`--no-sandbox` on the command line. The sandbox defaults to `auto`, and a
missing or broken backend is a **startup error** rather than a silent
downgrade; pass `--no-sandbox` to opt out deliberately.

Global `allowed_paths` entries may be absolute, `~`-prefixed, or relative to
the workspace root. Entries overlapping a system prefix (`/etc`, `/usr`,
`/proc`, `/dev`, `/boot`, `/sys`, `/run`, `/var/log`, `/root`) are rejected in
*both* directions — `/etc/ssl` is pointless, and `/` or `/var` would contain a
blocked prefix, so they are dropped too. The workspace root itself is exempt
from that block list. Under bwrap the allowed paths are also bound read-only
into the process sandbox, so `run` sees the same view as `read`.

Neither key is written by `/project-settings init`: both are machine-local, and
committing them would leak local layout.

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
│ Auto-Compact: on (default)
│ Questions:  off (project)
╰─────────────────────────────────────────────────╯

Legend: (project) = from .tinyharness/config.json
        (global)  = from ~/.config/tinyharness/settings.json
        (default) = hardcoded default
```

## Common Use Cases

### Strict project with manual approvals

```json
{
  "auto_accept_mode": "off",
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
