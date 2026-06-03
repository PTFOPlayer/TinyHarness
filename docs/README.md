# TinyHarness Documentation

## User Guides

- [Per-Project Settings](per-project-settings.md) — `.tinyharness/config.json` reference
- [Language Detection](language-detection.md) — how project types are auto-detected
- [Project Instructions](project-instructions.md) — configuring `TINYHARNESS.md` discovery

## Files and Directories

TinyHarness stores data in standard XDG paths:

```
~/.config/tinyharness/
├── settings.json           Global settings
├── prompts/                Customizable system prompt .md files
│   ├── header.md           Shared header (agent, planning, research modes)
│   ├── casual.md           Casual mode prompt
│   ├── planning.md         Planning mode prompt
│   ├── agent.md            Agent mode prompt
│   └── research.md         Research mode prompt
└── skills/                 Personal skills
    └── <name>/
        └── SKILL.md

~/.local/share/tinyharness/
├── sessions/               JSONL session files
│   └── <uuid>.jsonl
├── history.txt             Command history (rustyline)
└── backups/                File backups (when /undo is implemented)
    └── <session-id>/

<project>/.tinyharness/
├── config.json             Per-project settings
└── skills/                 Project-local skills
    └── <name>/
        └── SKILL.md
```
