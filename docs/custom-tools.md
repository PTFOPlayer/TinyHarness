# Custom Tools Guide

Custom tools let you extend TinyHarness with shell-command-based tools the LLM can call — no Rust code or recompilation required.

## Configuration

Custom-tool config is loaded from two locations:

1. **Global**: `~/.config/tinyharness/custom_tools.json`
2. **Project**: `.tinyharness/custom_tools.json` (extends global — both run, global first)

If neither file exists, no custom tools are loaded. The file is optional.

## Defining a Custom Tool

A custom tool is a shell command with a name, description, JSON Schema for parameters, and a command template:

```jsonc
// ~/.config/tinyharness/custom_tools.json
{
  "custom_tools": [
    {
      "name": "docker_run",
      "description": "Run a command in a Docker container",
      "category": "destructive",
      "parameters": {
        "type": "object",
        "properties": {
          "image": { "type": "string", "description": "Docker image to use" },
          "command": { "type": "string", "description": "Shell command to run" }
        },
        "required": ["image", "command"]
      },
      "command": "docker run --rm {image} sh -c \"$TH_COMMAND\"",
      "timeout_secs": 120
    },
    {
      "name": "rustfmt_check",
      "description": "Check if Rust files are formatted",
      "category": "readonly",
      "parameters": {
        "type": "object",
        "properties": {
          "path": { "type": "string", "description": "File or directory to check" }
        },
        "required": ["path"]
      },
      "command": "rustfmt --check {path}",
      "timeout_secs": 30
    }
  ]
}
```

### Tool Categories

| Category | Behavior |
|----------|----------|
| `"readonly"` | Auto-executed without confirmation (like `ls`, `read`) |
| `"destructive"` | Requires user confirmation before each call (like `write`, `run`) |

### Parameter Substitution

Tool parameters are substituted into the command template using `{param_name}` placeholders:

```
"command": "docker run --rm {image} sh -c \"$TH_COMMAND\""
```

When the LLM calls the tool with `{"image": "ubuntu:latest", "command": "ls"}`, the shell command becomes:

```
docker run --rm 'ubuntu:latest' sh -c "$TH_COMMAND"
```

> **⚠️ Shell escaping:** `{var}` values are automatically shell-escaped
> (wrapped in single quotes) before substitution. This prevents shell
> injection from untrusted content (e.g. LLM responses, user input).
> If you need the *raw* value without escaping (e.g. to let the shell
> interpret it), use the `$TH_*` environment variable instead.
>
> For multi-word commands like `sh -c`, use `"$TH_COMMAND"` (in double
> quotes) rather than `{command}` to avoid double-quoting issues.

Parameters are also available as environment variables in two forms:
- `TH_<NAME>` — uppercased, with `-` and spaces replaced by `_` (e.g. `$TH_IMAGE`, `$TH_COMMAND`)
- Raw key name — the parameter name as-is (e.g. `$image`, `$command`)

### Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | ✅ | — | Tool name (must not collide with built-in tools) |
| `description` | ✅ | — | Description shown to the LLM |
| `category` | ✅ | — | `"readonly"` or `"destructive"` |
| `parameters` | ✅ | — | JSON Schema for the tool's parameters |
| `command` | ✅ | — | Shell command template |
| `timeout_secs` | ❌ | `30` | Timeout in seconds |
| `cwd` | ❌ | inherited | Working directory for the command |

## Safety

- Custom tools with `category: "destructive"` always require user confirmation (same as built-in `write`/`edit`/`run` tools)
- Custom tools are subject to mode filtering: `readonly` tools appear in planning/research/agent modes; `destructive` tools only in agent mode. Custom tools are **not** available in casual mode (only `web_search` and `web_fetch` are)
- Custom tool names that collide with built-in tools are silently skipped (a warning is logged)
