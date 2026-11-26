# ADR-0012: Repair assistant ↔ tool-call pairing at request time, not at each cause

**Status:** Accepted

## Context

Strict OpenAI-compatible servers (Azure/OpenAI behind OpenRouter, `vLLM` with
`--enable-auto-tool-choice`) reject a request with HTTP 400
`"No tool output found for function call <id>"` if the history contains either
an assistant `tool_calls` entry with no matching `tool` result, or a `tool`
result whose call was never declared. Some also reject a `tool` result with an
empty or missing `tool_call_id`.

Real histories grow these holes in many independent ways:

- the user presses Ctrl+C between the assistant message and its results;
- a signal tool (`switch_mode`, `question`, `invoke_skill`) answers with a `tool`
  message and then the loop changes course;
- `/compact` cuts the keep window between a call and its result;
- a session resumed with `-c` was killed mid-turn;
- a provider returns calls with no ids at all (local models, plain Ollama).

Fixing each cause is five fixes in five modules, each of which must re-derive the
same invariant — and any new cause reopens the bug.

## Decision

`sanitize_tool_pairs` (`tinyharness-lib/src/provider/tool_pairs.rs`) repairs the
invariant at the one place that sees the exact shape of the outgoing request.
Three repairs, in order:

1. **Unusable ids** — a call with a missing, empty, or *duplicate* id gets a
   synthesized unique one (`call_fixed_N`), written back onto the call so the
   assistant message and the result agree.
2. **Unanswered calls** — a placeholder `tool` message is inserted directly
   after the assistant message that declared the call. The placeholder text says
   explicitly that the turn was aborted and tells the model to treat the call as
   never having run.
3. **Orphan / duplicate results** — dropped. A `tool` message with *no* id is
   not an orphan: it is bound positionally to the oldest still-unanswered
   declared call, which keeps id-less local-server histories working.

Ordering is otherwise preserved, so an already-correct history comes back
identical with `ToolPairFixes { stubbed: 0, dropped: 0 }` — the transform is
idempotent, and that is pinned by a test.

`/compact` additionally aligns its cut point to call/result pairs
(`align_keep_window`) so the summary boundary itself does not create holes; the
sanitizer is the backstop, not the only defence.

## Consequences

- One function to get right, one set of tests, and new hole-causes are fixed for
  free.
- The sanitizer *invents* content (a placeholder result). It is deliberately the
  only place that does, and the wording is chosen so the model does not read an
  empty placeholder as a successful empty output.
- Positional binding is a heuristic: an id-less history where results arrive
  out of order would bind the wrong result to the wrong call. In practice
  providers that omit ids also emit results in order.
- The repair costs one clone of the message vector per request.
- Silent repair means a persistent bug (a cause that keeps producing holes) shows
  up as placeholder noise in the transcript rather than as a loud failure.

## Alternatives considered

- **Validate and error out.** Turning an internal history inconsistency into a
  user-visible request failure for a recoverable condition.
- **Fix every cause individually.** Reopens the bug with every new cause, and
  cannot help sessions already saved in a broken state.
- **Repair on load.** Misses holes created *after* load (Ctrl+C, signal tools),
  so it would still need a request-time pass.
