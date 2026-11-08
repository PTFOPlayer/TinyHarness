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
- Custom tools are subject to mode filtering: `readonly` tools appear in planning/research/agent modes; `destructive` tools only in agent mode. Custom tools are **not** available in casual mode (which has no tools at all)

## Example: Web Search via curl (Ollama API)

The built-in Ollama web tools were removed; if you want web search, define it as a
custom tool that shells out to `curl`. This example calls Ollama's
[`web_search` API](https://docs.ollama.com/capabilities/web-search#examples) and is
illustrative — it is not wired into any test:

```jsonc
// ~/.config/tinyharness/custom_tools.json
{
  "custom_tools": [
    {
      "name": "web_search",
      "description": "Search the web using Ollama's web search API. Returns titles, URLs, and content snippets.",
      "category": "readonly",
      "parameters": {
        "type": "object",
        "properties": {
          "query": { "type": "string", "description": "The search query" },
          "max_results": { "type": "string", "description": "Maximum results to return (default 5, max 10)" }
        },
        "required": ["query"]
      },
      // Mirrors the docs example:
      //   curl https://ollama.com/api/web_search \
      //     --header "Authorization: Bearer $OLLAMA_API_KEY" \
      //     -d '{ "query": "what is ollama?" }'
      // The query goes in via $TH_QUERY (not {query}) so it lands inside the
      // JSON body without extra shell quoting.
      "command": "curl -s https://ollama.com/api/web_search --header \"Authorization: Bearer $OLLAMA_API_KEY\" -d \"{\\\"query\\\": \\\"$TH_QUERY\\\", \\\"max_results\\\": ${TH_MAX_RESULTS:-5}}\"",
      "timeout_secs": 30
    }
  ]
}
```

Notes:

- Custom tools run inside the process sandbox when `--sandbox` is on; the
  environment is cleared there, but tool parameters are re-injected as
  `TH_<NAME>` variables, which is why the template uses `$TH_QUERY` /
  `$TH_MAX_RESULTS`. `$OLLAMA_API_KEY` is **not** re-injected — with the
  sandbox on, inline the key in the template (not recommended) or run without
  `--sandbox` so the variable passes through.
- The network is intentionally left available in the sandbox so `curl` works.
- There is no built-in `/apikey` command anymore; the key lives in your shell
  environment (`export OLLAMA_API_KEY=...`).
- Custom tool names that collide with built-in tools are silently skipped (a warning is logged)
