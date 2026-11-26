# ADR-0005: Modes filter tools by category, and signal tools are handled inline

**Status:** Accepted

## Context

Two problems, one mechanism. First: different tasks need different blast radii.
Analyzing a codebase should not offer `write`; chatting should not offer `run`.
Second: some "tools" are not executed at all — `switch_mode` has to change the
agent's own state, `question` has to block on user input, `auto_compact` has to
rewrite the conversation. Routing those through the generic
`execute_tool_call` path produces a string the caller then has to re-parse.

Both are really "which tools does the model see, and what happens when it calls
one".

## Decision

Every `Tool` carries a `ToolCategory` (`tinyharness-lib/src/tools/tool.rs`):

- `ReadOnly` — no side effects, auto-executed;
- `Destructive` — needs approval;
- `Signal` — the agent loop interprets it instead of executing it.

`ToolManager::tools_for_mode(mode, availability)` decides what is *advertised*:

| Mode | Tools offered |
|---|---|
| `Casual` | none (pure chat) |
| `Planning` | `ReadOnly` + `Signal` |
| `Agent` | all registered tools |

Filtering happens at advertisement time, so a disabled tool never reaches the
provider and the loop does not re-check at execution time. `ToolAvailability`
covers the optional ones (`auto_compact`, `question`), both default-on and both
overridable globally or per project.

Signal tools have a stub handler that returns an error if it is ever actually
executed; the agent loop instead calls `parse_signal_event(name, arguments)` and
gets a typed `SignalEvent` (`SwitchMode`, `Question`, `AutoCompact`,
`InvokeSkill`).

`Research` was folded into `Planning` — it was the same tool set with a
different prompt. `"research"` / `"dev"` / `"plan"` remain parse aliases in
`AgentMode::from_str` for backwards compatibility.

## Consequences

- The security model is "the model cannot call what it cannot see", which is
  simpler to reason about than a runtime permission check.
- `Signal` tools bypass the generic path entirely, so a new signal tool means
  touching `parse_signal_event` *and* the agent loop — the type system does not
  catch a missing arm.
- Mode is a coarse granularity: within `Planning` you cannot grant, say, `grep`
  but not `read`.
- Because filtering is by category, a `ReadOnly` custom tool is offered in
  `Planning` automatically — desirable, but it means the custom-tool author
  picks the mode exposure by picking the category.

## Alternatives considered

- **Runtime permission checks per call.** The tool is advertised but refuses at
  execution; the model wastes turns discovering this.
- **Per-tool allowlists per mode.** More expressive, but a second configuration
  surface for a distinction no user has asked for.
- **Signal tools returning tagged strings parsed by the caller.** Error-prone
  and untestable; the typed `SignalEvent` is what makes
  `decide_tool_confirmation` and the loop unit-testable.
