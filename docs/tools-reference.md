# Tools Reference

TinyHarness provides 15 tools across three categories. Each tool has a JSON Schema that the AI uses to construct valid calls.

## Tool Categories

| Category | Behavior | Tools |
|----------|----------|-------|
| **ReadOnly** | Execute immediately, no confirmation needed | `ls`, `read`, `grep`, `glob`, `web_search`, `web_fetch` |
| **Destructive** | Require user confirmation before execution | `write`, `edit`, `run` |
| **Signal** | Handled specially by the agent loop (not generic execution) | `switch_mode`, `question`, `auto_compact`, `invoke_skill`, `screenshot` |

### Auto-Execution Rules

- **ReadOnly tools** run immediately in all modes
- **Destructive tools** prompt for confirmation (Yes/No/Auto-accept for all)
  - `run` can **never** be auto-accepted in auto-accept mode (only `write`/`edit` can)
  - Safe commands within `run` may still auto-accept if `/autoaccept` is on and the command passes safety checks
- **Signal tools** are intercepted by the agent loop before reaching generic tool execution

### Mode Filtering

| Mode | Available Tools |
|------|----------------|
| **casual** | `web_search`, `web_fetch` |
| **planning** | All ReadOnly + all Signal tools (no destructive) |
| **agent** | All 15 tools |
| **research** | All ReadOnly + all Signal tools (same as planning, different prompt) |

---

## File System Tools

### `ls` — List Directory

**Category**: ReadOnly | **Auto-executes**: Yes

Lists the contents of a single directory. Returns newline-separated file and directory names. Does not recurse — use `glob` for recursive searches.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `path` | Yes | The directory path to list |

### `read` — Read File

**Category**: ReadOnly | **Auto-executes**: Yes

Reads file content with optional line ranges. For image files (png, jpg, webp, gif, bmp), returns a description and the image data is automatically loaded for the model to view.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `path` | Yes | The absolute path to the file |
| `from` | No | Starting line number (0-based, inclusive) |
| `to` | No | Number of lines to read (only valid when `from` is set) |

### `write` — Write File

**Category**: Destructive | **Auto-executes**: Only in auto-accept mode

Writes content to a file. Creates the file if it doesn't exist, overwrites if it does. Creates parent directories automatically.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `path` | Yes | The absolute path to write |
| `content` | Yes | The text content to write |

Confirmation prompt shows the path and content preview. Use for new files or complete rewrites. For targeted edits, prefer `edit`.

### `edit` — Edit File

**Category**: Destructive | **Auto-executes**: Only in auto-accept mode

Edits a file by finding an exact string and replacing it with new text. The `old_str` must appear exactly once in the file. Use for targeted changes.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `path` | Yes | The absolute path to the file |
| `old_str` | Yes | The exact string to find (must appear exactly once) |
| `new_str` | Yes | The replacement string |

If `old_str` appears multiple times or not at all, the edit fails with an error.

### `grep` — Search with Regex

**Category**: ReadOnly | **Auto-executes**: Yes

Searches for a regex pattern across files. Returns matching lines with file paths and line numbers. Skips hidden directories, `node_modules`, `target`, and binary files.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `pattern` | Yes | The regex pattern to search for |
| `path` | No | The directory to search (defaults to project root) |
| `include` | No | Filter by file extension (e.g. `.rs` for Rust) |

### `glob` — Find Files by Pattern

**Category**: ReadOnly | **Auto-executes**: Yes

Finds files by glob pattern. Returns sorted results. Use this instead of `find` or recursive `ls`.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `pattern` | Yes | The glob pattern (e.g. `**/*.rs`, `**/Cargo.toml`) |
| `max_results` | No | Maximum results to return (default: 100) |

### `run` — Execute Shell Command

**Category**: Destructive | **Auto-executes**: **Never**

Executes a shell command and returns its output (stdout, stderr, exit code, duration). Output is truncated at 5,000 chars for stdout and 2,000 for stderr. Default timeout is 30 seconds.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `command` | Yes | The shell command to execute |
| `timeout` | No | Timeout in milliseconds (default: 30000) |
| `cwd` | No | Working directory (default: project root) |

Commands are checked against the safe/denied prefix lists. Shell metacharacters (`;`, `&`, `|`, `$()`, backticks, newlines) are rejected. Safe descriptor redirections (`2>&1`, `2>/dev/null`) are stripped before matching.

---

## Web Tools

### `web_search` — Search the Web

**Category**: ReadOnly | **Auto-executes**: Yes

Searches the web using Ollama's cloud API. Requires an Ollama API key set via `/apikey`. Returns titles, URLs, and content snippets.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `query` | Yes | The search query string |
| `max_results` | No | Maximum results (default: 5, max: 10) |

### `web_fetch` — Fetch Web Page

**Category**: ReadOnly | **Auto-executes**: Yes

Fetches a specific web page by URL. Returns the page title, main content, and links found on the page.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `url` | Yes | The URL to fetch |

---

## Signal Tools

Signal tools are **not executed generically**. The agent loop intercepts them and handles them inline.

### `switch_mode` — Switch Agent Mode

Requests a mode switch. Modes control which tools are available.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `mode` | Yes | Target mode: `casual`, `planning`, `agent`, or `research` |

The agent loop immediately refreshes the system prompt and toolset. Conversation history is preserved.

### `question` — Ask the User

Asks the user a question with predefined answer options.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `question` | Yes | The question to ask |
| `answers` | Yes | Array of answer options (at least one) |

The agent loop presents a numbered list. User selects by number or text. The answer becomes the tool result.

### `auto_compact` — Compact Conversation

Requests conversation compaction to free context space.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `focus` | No | Topics, decisions, or details to preserve in the summary |

For up to 200 intermediate messages: single-pass compaction. For larger sessions: cascading multi-stage (chunk, then per-stage summaries, then merged final summary).

### `invoke_skill` — Activate a Skill

Activates a skill by name, injecting its instructions into the system prompt.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `skill_name` | Yes | The exact skill name from the available skills list |

The skill stays active until unloaded with `/unload <name>`. Skills with `disable-model-invocation: true` are marked as "manual invocation only".

### `screenshot` — Request Screenshot

Requests the user to take a screenshot and attach it.

| Parameter | Required | Description |
|-----------|----------|-------------|
| `description` | Yes | What you want the user to capture |

The agent loop prompts the user to attach an image with `/image` and reply. User can skip if unable.

---

## Tool Schema Format

Tools use JSON Schema to describe their parameters. The schema is sent to the provider, enabling the model to construct valid tool calls.

All parameters are passed as strings to keep parsing simple. For non-string types (booleans, numbers, arrays), the string is coerced by the tool handler.

### Tool Call Format

The model emits tool calls in XML block format with `tool_calls` and `invoke` elements. Multiple tools can be called in a single block. Results are batched into a single response message. Each tool call includes an `id` (OpenAI tool call ID) that is echoed back in the matching `Role::Tool` result message via `tool_call_id`, as required by OpenAI-compatible servers. Signal tools also propagate `tool_call_id` through the agent loop.

---

## Adding a Custom Tool

While the binary crate registers tools via `ToolManager::register_defaults()`, the `tinyharness-lib` API allows registering additional tools programmatically:

```rust
use tinyharness_lib::tools::tool::{make_tool, build_string_params_schema, ToolCategory, require_arg};

let tool = make_tool(
    "echo",
    "Echo a message back",
    ToolCategory::ReadOnly,
    build_string_params_schema(&[("text", "The text to echo")], &[]),
    |args| Box::pin(async move {
        let text = require_arg(&args, "text").unwrap();
        format!("You said: {}", text)
    }),
);

manager.register_tool(tool);
```

Custom tools are uncommon — most users extend functionality via skills rather than writing Rust code.
