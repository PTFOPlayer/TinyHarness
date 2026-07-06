# Plugins: Custom Tools & Hooks

TinyHarness supports a plugin system that lets you extend the agent with custom tools and lifecycle hooks — no Rust code or recompilation required.

## Configuration

Plugin config is loaded from two locations:

1. **Global**: `~/.config/tinyharness/plugins.json`
2. **Project**: `.tinyharness/plugins.json` (extends global — both run, global first)

If neither file exists, no plugins are loaded. The file is optional.

## Custom Tools

Custom tools are shell commands that the LLM can call like built-in tools. You define a name, description, JSON Schema for parameters, and a shell command template.

```jsonc
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

## Hooks

Hooks are shell commands that fire automatically at specific lifecycle points during the agent loop. They can inject text into the conversation or block actions.

```jsonc
{
  "hooks": [
    {
      "name": "log-tools",
      "event": "after_tool_call",
      "command": "echo \"[{tool}] {result}\" >> ~/.local/share/tinyharness/tool-log.txt",
      "timeout_secs": 5
    },
    {
      "name": "inject-git-status",
      "event": "before_llm_call",
      "command": "git status --short 2>/dev/null",
      "timeout_secs": 3,
      "inject_stdout": true
    },
    {
      "name": "block-rm-rf",
      "event": "before_tool_call",
      "command": "echo \"{args}\" | grep -q \"rm -rf\" && echo BLOCKED || true",
      "timeout_secs": 1,
      "block_on_output": "BLOCKED"
    }
  ]
}
```

### Lifecycle Events

| Event | When it fires | Context available |
|-------|-------------|-------------------|
| `before_user_message` | After user input is read, before sending to LLM | `user_input` |
| `after_user_message` | After user message is saved to session | `user_input`, `message_count` |
| `before_llm_call` | Before calling the LLM provider | `message_count` |
| `after_llm_response` | After LLM response streaming completes | `response`, `message_count` |
| `before_tool_call` | Before each tool execution | `tool`, `args` |
| `after_tool_call` | After each tool returns | `tool`, `args`, `result` |
| `on_exit` | When the agent loop exits | `message_count` |

> **⚠️ TUI mode:** In `--tui` mode, only `before_llm_call`, `before_tool_call`,
> and `after_tool_call` hooks fire. The `before_user_message`,
> `after_user_message`, `after_llm_response`, and `on_exit` events are
> CLI-only.

### Hook Features

#### `inject_stdout`

When `true`, the hook's stdout is injected into the conversation:

- `before_user_message`: Prepended to the user message
- `before_llm_call`: Injected as a temporary system message (removed after the call)
- `after_llm_response`: Appended to the response content
- `after_tool_call`: Appended to the tool result

This is useful for injecting dynamic context (e.g., `git status` output before each LLM call).

#### `block_on_output`

When set, if the hook's stdout starts with this string, the action is blocked:

- `before_user_message`: The user message is skipped entirely (not sent to the LLM)
- `before_tool_call`: The tool call is denied with a "blocked by hook" message
- `after_llm_response`: Tool calls from the response are skipped (the response is saved as-is)

Example: block `rm -rf` in the `run` tool:

```json
{
  "name": "block-rm-rf",
  "event": "before_tool_call",
  "command": "echo \"{args}\" | grep -q \"rm -rf\" && echo BLOCKED || true",
  "block_on_output": "BLOCKED"
}
```

### Context Variables

Hook context is available in two ways:

**Inline template substitution** (`{var}`) — values are **shell-escaped** automatically:
```json
"command": "echo \"[{tool}] {result}\" >> log.txt"
```

**Environment variables** (`TH_*`) — raw values, no escaping:
```json
"command": "echo $TH_TOOL_NAME >> log.txt"
```

> **⚠️ Shell escaping:** `{var}` values are wrapped in single quotes
> during substitution to prevent shell injection. Use `$TH_*` env vars
> if you need the shell to interpret the value (e.g. subcommands).
>
> Missing variables (no value in the context) are left as-is — the
> `{placeholder}` appears literally in the rendered command.

| Variable | Env var | Available for events |
|----------|---------|---------------------|
| `{user_input}` | `TH_USER_INPUT` | `before_user_message`, `after_user_message` |
| `{message_count}` | `TH_MESSAGE_COUNT` | Most events |
| `{response}` | `TH_RESPONSE` | `after_llm_response` |
| `{tool}` | `TH_TOOL_NAME` | `before_tool_call`, `after_tool_call` |
| `{args}` | `TH_TOOL_ARGS` | `before_tool_call`, `after_tool_call` |
| `{result}` | `TH_TOOL_RESULT` | `after_tool_call` |
| `{session_id}` | `TH_SESSION_ID` | All events |

### Hook Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `name` | ✅ | — | Hook name (for logging) |
| `event` | ✅ | — | Lifecycle event |
| `command` | ✅ | — | Shell command template |
| `timeout_secs` | ❌ | `30` | Timeout in seconds |
| `cwd` | ❌ | inherited | Working directory |
| `inject_stdout` | ❌ | `false` | Inject stdout into conversation |
| `block_on_output` | ❌ | `null` | Block action if stdout starts with this string |

## Complete Example

```jsonc
// ~/.config/tinyharness/plugins.json
{
  "hooks": [
    {
      "name": "inject-project-status",
      "event": "before_llm_call",
      "command": "echo 'Git status:' && git status --short 2>/dev/null || true",
      "timeout_secs": 3,
      "inject_stdout": true
    },
    {
      "name": "log-all-tools",
      "event": "after_tool_call",
      "command": "date '+%H:%M:%S' | xargs -I{} echo '{} [{tool}]' >> ~/.local/share/tinyharness/tool.log",
      "timeout_secs": 2
    },
    {
      "name": "block-dangerous-commands",
      "event": "before_tool_call",
      "command": "echo '{args}' | grep -qE 'rm -rf|sudo|dd if=' && echo BLOCKED || true",
      "timeout_secs": 1,
      "block_on_output": "BLOCKED"
    }
  ],
  "custom_tools": [
    {
      "name": "make_build",
      "description": "Run make with a target",
      "category": "destructive",
      "parameters": {
        "type": "object",
        "properties": {
          "target": { "type": "string", "description": "Make target to run" }
        },
        "required": ["target"]
      },
      "command": "make {target}",
      "timeout_secs": 300
    },
    {
      "name": "todo_count",
      "description": "Count TODO comments in the codebase",
      "category": "readonly",
      "parameters": {
        "type": "object",
        "properties": {
          "path": { "type": "string", "description": "Directory to search" }
        },
        "required": ["path"]
      },
      "command": "grep -r 'TODO' {path} --include='*.rs' -l | wc -l",
      "timeout_secs": 10
    }
  ]
}
```

## Safety

- Custom tools with `category: "destructive"` always require user confirmation (same as built-in `write`/`edit`/`run` tools)
- Custom tools are subject to mode filtering: `readonly` tools appear in planning/research/agent modes; `destructive` tools only in agent mode. Custom tools are **not** available in casual mode (only `web_search` and `web_fetch` are)
- Hook commands execute via `sh -c` with a configurable timeout
- Hook failures (non-zero exit, timeout) produce warnings but don't crash the agent loop
- Multiple hooks for the same event fire in order (global config first, then project config)
- Custom tool names that collide with built-in tools are silently skipped (a warning is logged)