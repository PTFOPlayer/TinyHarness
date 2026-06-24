# Configuration Guide

TinyHarness stores all persistent settings in `~/.config/tinyharness/`. This guide covers every configurable option.

## Settings File

**Location**: `~/.config/tinyharness/settings.json`

Settings are loaded at startup. On first launch, defaults are used until the user runs `--config` (interactive setup) or manually edits the file.

### Setting vs. Saving

- **Global settings** are managed via slash commands (`/command`, `/model`, `/apikey`, etc.)
- **Project settings** are managed via `/project-settings` and `.tinyharness/config.json`
- **Manual editing** the JSON file works too — reload with `/refresh`

### Atomic Writes

Settings are saved atomically: written to a `.tmp` file, then renamed. This prevents corruption if the process crashes during a write.

---

## All Settings Fields

```json
{
  "last_provider": "ollama",
  "last_provider_url": "http://127.0.0.1:11434",
  "last_model": "qwen2.5-coder:14b",
  "preferred_mode": "agent",
  "ollama_api_key": null,
  "openai_compat_api_key": null,
  "sockudo_app_id": null,
  "sockudo_app_key": null,
  "sockudo_app_secret": null,
  "skip_health_check": false,
  "ollama_timeout_secs": 5,
  "ollama_max_retries": 3,
  "ollama_think_type": "medium",
  "show_thinking": false,
  "context_limit": null,
  "auto_accept_safe_commands": true,
  "safe_command_prefixes": null,
  "denied_command_prefixes": null,
  "project_md_files": null
}
```

### Provider Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `last_provider` | string | `"ollama"` | Last used provider: `"ollama"`, `"llamacpp"`, `"vllm"`, `"openai-compat"`, or `"sockudo"` |
| `last_provider_url` | string\|null | `null` | Custom base URL for the provider. Set by `--url` flag or `--config` interactive setup. If `null`, uses the provider's default URL |
| `last_model` | string\|null | `null` | Last used model name. Set by `/model <name>`. If `null`, the provider auto-selects the first available model |

**Provider default URLs** (used when `last_provider_url` is `null`):
- Ollama: `http://127.0.0.1:11434`
- llama.cpp: `http://127.0.0.1:8080`
- vLLM: `http://127.0.0.1:8000`
- OpenAI-compat: _(none — `--url` is required)_
- Sockudo: `http://127.0.0.1:6001` (⚠️ highly experimental)

### OpenAI-Compatible Provider Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `openai_compat_api_key` | string\|null | `null` | Bearer token for `--openai-compat` provider. Sent as `Authorization: Bearer <key>`. Set via `--api-key <key>`, `OPENAI_API_KEY` env var, or `--config` interactive setup. Use `--api-key -` to clear the saved key |
| `skip_health_check` | bool | `false` | Skip the provider health check at startup. Useful for gateways without a `/health` endpoint. Set via `--skip-health-check` |

**API key resolution precedence:**
1. `--api-key <key>` CLI flag (highest — also persists to settings)
2. `OPENAI_API_KEY` environment variable
3. `openai_compat_api_key` in settings.json
4. None (provider startup fails with an error)

### Sockudo Provider (Experimental)

> ⚠️ **The Sockudo AI Transport provider is highly experimental.** It is not recommended for production use and may have stability issues, incomplete features, or breaking changes without notice.

The Sockudo provider uses a [Sockudo](https://github.com/sockudo/sockudo) WebSocket server's AI Transport feature to communicate with an LLM backend. Unlike the other providers which talk directly to an LLM server, Sockudo requires:

1. A running **Sockudo server** with AI Transport enabled and versioned messages configured.
2. A **worker bridge** process — a separate binary (example in `docs/examples/sockudo-worker/`) that connects to Sockudo, receives `ai-input` events, calls Ollama for inference, and streams responses back as versioned message mutations.

**Sockudo-specific settings:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `sockudo_app_id` | string\|null | `null` | Sockudo app ID. Set via `--config` interactive setup |
| `sockudo_app_key` | string\|null | `null` | Sockudo app key (used for WebSocket auth) |
| `sockudo_app_secret` | string\|null | `null` | Sockudo app secret (used for HMAC-SHA256 request signing) |

These credentials must match the Sockudo server's app configuration. For testing, see `tests/sockudo/config/config.toml` and `tests/sockudo/run-test.sh`.

**How it works:**
1. The provider publishes an `ai-input` event via signed HTTP POST to the Sockudo server.
2. The worker bridge receives the event, calls Ollama, and publishes the response back as `sockudo:message.create` → `.append` → `.update` events.
3. The provider subscribes to the response channel via WebSocket and converts streamed events into `ChatMessageResponse` chunks.

**Limitations:**
- No model list endpoint — `list_models` returns only the selected model or an empty vec
- Tool calls are passed through but tool support depends on the worker and Ollama model capabilities
- The provider does not expose retries or think levels (those are Ollama-specific)
- Requires a running worker bridge process alongside the Sockudo server

### Mode Setting

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `preferred_mode` | string | `"casual"` | Default mode on startup: `"casual"`, `"planning"`, `"agent"`, or `"research"` |

Can be overridden per-project via `.tinyharness/config.json` → `preferred_mode`.

### Ollama-Specific Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `ollama_api_key` | string\|null | `null` | API key for Ollama cloud features (`web_search`, `web_fetch`). Set via `/apikey <key>`. Leave `null` for local-only use |
| `ollama_timeout_secs` | u64 | `5` | HTTP request timeout in seconds. Increase for slow models or large payloads. Set via `/timeout <seconds>` |
| `ollama_max_retries` | u32 | `3` | Maximum retries on transient errors (network failures, 5xx responses). Set via `/retries <count>` |
| `ollama_think_type` | string | `"medium"` | Reasoning level for models that support it (qwen2.5 variants): `"off"`, `"low"`, `"medium"`, `"high"`. Set via `/think <level>` |

### Display Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `show_thinking` | bool | `false` | Whether to render the model's reasoning chain inline during streaming. Toggle with `/showthink` |

### Context Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `context_limit` | u32\|null | `null` | Custom context warning threshold in tokens. If `null`, uses the model's default (8K–256K depending on model). Warnings fire at 70% and 90%. Set via `/contextlimit <tokens>` |

### Command Safety Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `auto_accept_safe_commands` | bool | `true` | Whether safe commands auto-execute without confirmation. Toggle with `/autoaccept` |
| `safe_command_prefixes` | vec\|null | `null` | Custom safe command prefixes. If `null`, uses the hardcoded default list (43 commands). Set via `/command add/rm/reset` |
| `denied_command_prefixes` | vec\|null | `null` | Always-denied prefixes. Takes priority over safe list. Set via `/command deny/undeny/resetdeny` |

The deny list takes priority. If a command matches both a safe prefix and a denied prefix, it is denied. This lets you block specific dangerous commands (e.g. deny `git push` but keep `git status` safe).

### Project Instruction File Settings

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `project_md_files` | vec\|null | `null` | Custom discovery order for project instruction files. If `null`, uses the hardcoded order: `TINYHARNESS.md` → `.tinyharness.md` → `AGENTS.md` → `CLAUDE.md` |

Override priority:
1. `TINYHARNESS_MD_FILES` env var (highest)
2. `project_md_files` in settings
3. Hardcoded defaults (lowest)

---

## Per-Project Settings

**Location**: `.tinyharness/config.json` (discovered walking up from CWD)

Overrides global settings for a specific project. See [Per-Project Settings](per-project-settings.md) for full details.

### Supported Fields

```json
{
  "safe_command_prefixes": ["python -m pytest", "npm run lint"],
  "denied_command_prefixes": ["git push --force"],
  "auto_accept_safe_commands": false,
  "context_limit": 32768,
  "project_md_files": ["RULES.md", ".cursorrules"],
  "preferred_mode": "agent"
}
```

### Layering

```
~/.config/tinyharness/settings.json    (global)
  → .tinyharness/config.json           (project override)
    → CLI flags                        (highest priority)
```

- `safe_command_prefixes`: **Extends** (not replaces) the global list
- `denied_command_prefixes`: **Replaces** the global list
- All other fields: **Override** if present, fall back to global if absent

### Viewing Merged Settings

```
/project-settings
```

Shows every setting with its source annotation:

```
safe_command_prefixes    (project):
  python -m pytest
  ...
auto_accept_safe_commands (project): false
context_limit             (project): 32768
last_provider             (global):  ollama
ollama_timeout_secs       (default): 5
```

### Creating a Project Config

```
/project-settings init
```

Generates a `.tinyharness/config.json` with commented defaults from your current global settings.

---

## System Prompts

**Location**: `~/.config/tinyharness/prompts/`

On first launch, TinyHarness seeds this directory with default `.md` prompt files. Existing files are **never overwritten** — you can safely customize them.

```
~/.config/tinyharness/prompts/
├── header.md         Shared header for agent/planning/research
├── casual.md         Self-contained casual mode prompt
├── planning.md       Planning mode (ReadOnly + Signal tools)
├── agent.md          Agent mode (all 15 tools)
└── research.md       Research mode (ReadOnly + Signal tools)
```

### Prompt Assembly

For Agent, Planning, and Research modes:
```
header.md + blank line + <mode>.md
```

For Casual mode:
```
casual.md (self-contained)
```

Prompts are rebuilt (re-read from disk) on:
- Mode switch
- Skill activation/unload
- File pinning changes (`/add`, `/drop`)
- `/refresh` command

### Customizing Prompts

1. Edit the `.md` files in `~/.config/tinyharness/prompts/`
2. Run `/refresh` or switch modes to apply

To restore a default, delete the file and restart TinyHarness — it will re-seed on next launch.

---

## XDG Paths Reference

```
~/.config/tinyharness/
├── settings.json           Global settings
├── prompts/                Customizable system prompt .md files
│   ├── header.md
│   ├── casual.md
│   ├── planning.md
│   ├── agent.md
│   └── research.md
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

---

## CLI Flags

All CLI flags override settings:

| Flag | Setting Override |
|------|-----------------|
| `-o`, `--ollama` | `last_provider = "ollama"` |
| `-l`, `--llama-cpp` | `last_provider = "llamacpp"` |
| `-v`, `--vllm` | `last_provider = "vllm"` |
| `--openai-compat` | `last_provider = "openai-compat"` (requires `--api-key` and `--url`) |
| `--sockudo` | `last_provider = "sockudo"` (⚠️ experimental) |
| `-u`, `--url <url>` | `last_provider_url = <url>` |
| `--api-key <key>` | `openai_compat_api_key = <key>` (only affects `--openai-compat`; use `-` to clear) |
| `--skip-health-check` | Skips provider health check at startup |
| `-c`, `--continue` | Loads most recent session (doesn't modify settings) |
| `--config` | Runs interactive setup, saves, exits |
| `-p`, `--prompt <text>` | Sends initial prompt then enters interactive mode |
| `--tui` | Launch the experimental terminal UI (split-pane TUI) |

---

## Environment Variables

| Variable | Effect |
|----------|--------|
| `TINYHARNESS_MD_FILES` | Comma-separated list of instruction file names, overrides `project_md_files` in settings |
| `OPENAI_API_KEY` | Bearer token for the `--openai-compat` provider (used when `--api-key` is not passed) |
| `HOME` | Used to resolve `~/.config/tinyharness/` and `~/.local/share/tinyharness/` |
