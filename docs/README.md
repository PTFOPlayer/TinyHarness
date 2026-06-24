# TinyHarness Documentation

## User Guides

- [Skills Guide](skills.md) — creating and using SKILL.md skill modules
- [Tools Reference](tools-reference.md) — tool categories, parameters, and behavior
- [Configuration Guide](configuration.md) — all settings, XDG paths, prompt customization
- [Safety & Security](safety.md) — command safety model, best practices
- [Agent Modes](modes.md) — mode behavior, prompt architecture, escalation
- [Per-Project Settings](per-project-settings.md) — `.tinyharness/config.json` reference
- [Language Detection](language-detection.md) — how project types are auto-detected
- [Project Instructions](project-instructions.md) — configuring `TINYHARNESS.md` discovery

## Developer Docs

- [Contributing](contributing.md) — project setup, code conventions, PR process

## Provider Guides

| Provider | Status | Notes |
|----------|--------|-------|
| Ollama | Stable (default) | Raw SSE streaming, retries, think levels, API key for web tools |
| llama.cpp | Stable | OpenAI-compatible, shared HTTP/SSE logic, no auth (local servers) |
| vLLM | Stable | OpenAI-compatible, shared HTTP/SSE logic, no auth (local servers) |
| OpenAI-compat | Stable | Generic OpenAI-compatible gateways (OpenRouter, Together, etc.) with Bearer auth. Requires `--api-key` and `--url` |
| Sockudo | ⚠️ Highly experimental | AI Transport via WebSocket, requires a worker bridge (see `docs/examples/sockudo-worker/`) |

## Quick References

### By Role

| Role | Start Here |
|------|------------|
| New user | [Configuration Guide](configuration.md) → [Agent Modes](modes.md) |
| Setting up a project | [Project Instructions](project-instructions.md) → [Per-Project Settings](per-project-settings.md) |
| Writing skills | [Skills Guide](skills.md) |
| Security-conscious user | [Safety & Security](safety.md) |
| Contributor | [Contributing](contributing.md) |

### By Question

| Question | Doc |
|----------|-----|
| "How do I add project-specific commands?" | [Per-Project Settings](per-project-settings.md) |
| "How do I customize prompts?" | [Configuration Guide](configuration.md#system-prompts) |
| "What's the difference between planning and agent?" | [Agent Modes](modes.md) |
| "Can the AI auto-execute any command?" | [Safety & Security](safety.md#safe-commands) |
| "How do I write a skill?" | [Skills Guide](skills.md#skill-file-format) |
| "What tools are available?" | [Tools Reference](tools-reference.md) |
| "How do I override TINYHARNESS.md discovery?" | [Project Instructions](project-instructions.md#customizing-the-file-list) |
| "What languages are auto-detected?" | [Language Detection](language-detection.md) |
| "Is Sockudo production-ready?" | No — see [Configuration Guide](configuration.md#sockudo-provider-experimental) |
| "How do I contribute?" | [Contributing](contributing.md) |

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
