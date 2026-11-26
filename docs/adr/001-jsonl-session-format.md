# ADR-0001: JSONL as the session storage format

**Status:** Accepted

## Context

A session has to survive the process dying, be resumable with `-c`, and be
appended to on every turn. Conversation histories grow monotonically and are
written far more often than they are read, so the write path dominates the
design: a format that rewrites the whole file per message makes every turn
quadratic in history length. The files also need to be inspectable by a human
with `less` and recoverable after a partial write.

## Decision

Sessions are JSON Lines files (`tinyharness-lib/src/session.rs`):

- line 1 is `SessionEntry::Meta(SessionMeta)` — id, working dir, timestamps,
  mode, provider, model, name, `message_count`, last token usage, and cumulative
  counters (`total_tool_calls`, `total_tokens_used`);
- every following line is `SessionEntry::Message(Message)`.

`SessionEntry` is a `#[serde(tag = "type")]` enum, so new entry kinds can be
added without breaking existing files. Appending one message is one `writeln!`.
Metadata is *not* rewritten per message: it is flushed every
`AUTO_SAVE_INTERVAL` (5) appended messages, plus explicitly on mode switch,
session switch and exit. A flush rewrites the whole file through a
`<file>.jsonl.tmp` sibling that is then `rename`d over the original.

Malformed lines are skipped on load rather than failing the session.

## Consequences

- Appending is O(1) in the history size — the common case is cheap.
- A metadata flush is O(n) in the history size (read all, write all, rename).
  This is why it is batched to every 5 messages instead of every message.
- `rename` gives atomicity against process death, but not durability: there is
  no `fsync`, so a power cut can still lose the tail.
- No random access: loading a session reads the whole file, and there is no
  index. Acceptable at the sizes a context window bounds us to anyway.
- A crash mid-turn leaves an assistant message with `tool_calls` and no
  `tool` results. That hole is not repaired here — see
  [ADR-0012](012-tool-pair-repair.md).
- Truncation and quoting are the JSON encoder's problem, so message content
  never needs escaping logic of our own.

## Alternatives considered

- **One JSON document rewritten per turn.** Simple, but O(n) per message and
  one bad byte loses the entire session.
- **SQLite.** Gives indexes, durability and atomic updates, at the cost of a
  heavy dependency in a project that deliberately keeps its tree small.
- **A custom line format.** Reinventing escaping for no gain; JSONL is already
  line-oriented, streaming-friendly and inspectable.
