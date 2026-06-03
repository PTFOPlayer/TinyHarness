# Agent Modes

TinyHarness has four agent modes that control which tools are available and how the AI behaves. Modes provide a graduated trust model — start with restricted access and escalate when you need more power.

## Mode Overview

| Mode | Tools | Purpose | Best For |
|------|-------|---------|----------|
| **casual** | `web_search`, `web_fetch` | Chat with web access | General questions, research without touching your code |
| **planning** | All ReadOnly + all Signal | Analyze, plan, escalate | Codebase exploration, architecture review, planning changes |
| **agent** | All 15 tools | Full development access | Writing code, running commands, full development workflow |
| **research** | All ReadOnly + all Signal | Web research, escalate | Research-heavy tasks with codebase awareness |

## Tool Availability by Mode

### Casual
```
Available: web_search, web_fetch
Not available: ls, read, write, edit, grep, glob, run,
               switch_mode, question, auto_compact, invoke_skill, screenshot
```
The most restricted mode. No filesystem access at all. The AI can search the web and fetch pages, but cannot read or modify files.

### Planning
```
Available: ls, read, grep, glob, web_search, web_fetch,
          switch_mode, question, auto_compact, invoke_skill, screenshot
Not available: write, edit, run
```
Can explore code but not change it. Ideal for architecture discussions, code reviews, and planning before switching to agent mode.

### Agent
```
Available: ls, read, write, edit, grep, glob, run,
          web_search, web_fetch, switch_mode, question,
          auto_compact, invoke_skill, screenshot
Not available: (none)
```
Full access. All tools are available. This is the default for development work.

### Research
```
Available: ls, read, grep, glob, web_search, web_fetch,
          switch_mode, question, auto_compact, invoke_skill, screenshot
Not available: write, edit, run
```
Same toolset as planning, but with a research-focused system prompt. The prompt emphasizes gathering information, cross-referencing sources, and presenting findings.

---

## Mode Switching

### By the User

```
/mode agent             # Show current mode + list all
/mode planning          # Switch to planning
/plan                   # Shortcut alias
/agent                  # Shortcut alias
/research               # Shortcut alias
/casual                 # Shortcut alias
```

Modes also accept aliases:
- `planning` or `plan`
- `agent` or `dev`
- `research` or `researching`
- `casual` (no alias)

### By the AI (Signal Tool)

The AI can request a mode switch by calling `switch_mode`:

```json
{"name": "switch_mode", "arguments": {"mode": "agent"}}
```

This is handled as a signal — the agent loop catches it, switches the mode, rebuilds the system prompt, and continues the conversation in the new mode. The AI typically uses this pattern:

1. Start in `planning` mode
2. Explore the codebase, understand the problem
3. Call `switch_mode` to `agent` when ready to implement
4. Make changes
5. Optionally switch back to `planning` to review

### What Happens on Mode Switch

1. System prompt is rebuilt from disk (header.md + new mode's .md file)
2. Tool definitions are regenerated for the new mode
3. Active skills are re-injected
4. Project context and file pins are refreshed
5. Previous conversation history is preserved
6. Session is saved (flush)

---

## System Prompt Architecture

Prompts are assembled from multiple components in a specific order:

```
1. header.md                    (shared — agent/planning/research)
2. blank line
3. <mode>.md                   (mode-specific instructions)
4. Project context              (language, root, git status, structure)
5. Project instructions         (TINYHARNESS.md + additional files)
6. File pins                    (pinned file contents)
7. Active skills                (skill content)
8. Available tools + skill index
```

### Header (`header.md`)

Used by agent, planning, and research modes. Contains:
- Role definition ("development AI with tools")
- Operating context (project name, language, workspace root)
- File pinning instructions
- Conversation behavior guidelines

### Mode Files

#### `casual.md` (self-contained, no header)
```
You are a helpful AI assistant named TinyHarness. You run in casual mode
with access to web search and web fetch only...
```

#### `planning.md`
```
You are in PLANNING mode. Analyze, research, plan — do NOT make changes.
Use ReadOnly tools to explore, Signal tools to escalate...
```

#### `agent.md`
```
You are in AGENT mode. Full development access — write code, run commands,
make changes. You have access to all tools...
```

#### `research.md`
```
You are in RESEARCH mode. Gather information from the web and codebase.
Cross-reference sources, present findings clearly...
```

### Customizing Prompts

See [Configuration Guide](configuration.md#system-prompts). Key points:
- Edit files in `~/.config/tinyharness/prompts/`
- Existing files are never overwritten
- Use `/refresh` to reload

---

## Mode Use Cases

### Recommended Workflow: Escalation Ladder

```
casual → planning → agent
  ↓
(review question)
  ↓
planning (review changes)
```

1. **Casual**: Ask general questions, research concepts
2. **Planning**: Explore the codebase, understand the architecture, plan changes
3. **Agent**: Implement changes, run tests, iterate
4. **Planning**: Review the diff, verify correctness

### When to Stay in Planning

- Onboarding to a new codebase
- Code reviews
- Architecture discussions
- Debugging with read-only access
- Writing documentation (if using `write` via agent escalation)

### When to Use Casual

- General programming questions ("How does Rust's borrow checker work?")
- Research before touching code ("What's the best library for X?")
- Quick web searches during development

### When to Use Research

- Gathering external API documentation
- Comparing libraries/frameworks
- Finding solutions to error messages from multiple sources
- Pre-implementation research with codebase awareness

---

## Mode Configuration

### Default Mode on Startup

Set via settings:
```json
{
  "preferred_mode": "agent"
}
```

Or per-project:
```json
{
  "preferred_mode": "planning"
}
```

### Forcing a Mode via CLI

There's no `--mode` CLI flag. The saved `preferred_mode` is used on startup. Switch immediately after with `/mode <name>`.

---

## Mode Internals

### `AgentMode` Enum

Located in `tinyharness-lib/src/mode.rs`:

```rust
pub enum AgentMode {
    Casual,
    Planning,
    Agent,
    Research,
}
```

### Tool Filtering

Located in `ToolManager::tools_for_mode()` (`tinyharness-lib/src/tools/mod.rs`):

```rust
fn tools_for_mode(&self, mode: AgentMode) -> Vec<ToolDefinition> {
    match mode {
        AgentMode::Agent => self.get_all_tool_definitions(),
        AgentMode::Casual => /* web_search + web_fetch only */,
        AgentMode::Planning => /* ReadOnly + Signal */,
        AgentMode::Research => /* ReadOnly + Signal */,
    }
}
```

### Prompt Loading

Located in `AgentMode::load_system_prompt()` (`tinyharness-lib/src/mode.rs`):

- Files are read from `~/.config/tinyharness/prompts/`
- Empty or missing files fall back to hardcoded defaults (`include_str!`)
- Casual mode uses only its own file
- Other modes prepend the shared header

---

## Tips

1. **Start sessions in planning mode**: Set `preferred_mode: "planning"` in project config so the AI explores before changing anything.
2. **Use per-project mode**: Different projects have different trust levels:
   ```json
   { "preferred_mode": "agent" }    // your own project
   { "preferred_mode": "planning" } // a codebase you're exploring
   ```
3. **The AI knows to escalate**: The planning and research prompts instruct the AI to call `switch_mode` when it needs write access. You don't need to switch manually.
4. **Tool filtering is automatic**: The tool list in the system prompt changes on mode switch. The AI literally cannot call `write` in planning mode.
