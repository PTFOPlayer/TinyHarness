# ADR-0010: Compaction keeps a verbatim tail and chooses single-pass vs cascade by message count

**Status:** Accepted

## Context

A conversation eventually exceeds the context window. Deleting old messages
loses decisions the model will need again; keeping everything fails the request.
The standard answer is to replace the middle of the history with a summary, but
two things make it awkward here:

- the summarisation *itself* uses the same model, so the text being summarised
  must fit the window too — a 2,000-message history cannot be summarised in one
  request;
- the original model output has to be reconstructed into a valid history
  afterwards, including the tool-call pairing invariants
  ([ADR-0012](012-tool-pair-repair.md)).

## Decision

`/compact` and the `auto_compact` signal tool share
`src/commands/compact.rs::execute_compact`:

- the system prompt (index 0) is always kept;
- the last `KEEP_RECENT` (4) messages are always kept verbatim;
- everything between is summarised into one `Role::System`-ish summary message;
- a `focus` argument is appended to the summarisation prompt so a user can say
  "keep the API discussion".

The cut point is then **pulled backwards** by `align_keep_window` until the kept
window does not begin with a `tool` result whose call was summarised away —
otherwise the next request fails with HTTP 400 on strict providers.

Strategy is chosen by the count of intermediate messages, not by tokens:

| Intermediate messages | Strategy |
|---|---|
| ≤ 200 (`SINGLE_PASS_LIMIT`) | one summarisation call |
| > 200 | cascade: split into chunks of ≤ 200 (`messages_per_chunk`), summarise each, then merge the summaries with a second prompt that asks for redundancy removal |

Each message is truncated to `MAX_CHARS_PER_MESSAGE` (2,000) when formatted for
the summary prompt, so one giant tool output cannot eat the whole budget.

Token usage returned by the summarisation calls is reported back so the context
indicator reflects reality after compaction.

## Consequences

- The thresholds are counts, not tokens: a 150-message history of huge messages
  can still overflow the summarisation request. The per-message truncation is the
  only mitigation.
- Compaction costs at least two LLM calls in cascade mode and is not free in
  single-pass mode; `execute_compact` refuses to run on ≤ 6 messages.
- `KEEP_RECENT = 4` is a fixed number of messages, not a token budget, so a turn
  with large tool outputs may lose useful recent context.
- Because the cut is aligned to call/result pairs, the effective keep window can
  be larger than 4 — correct, but the printed stage counts then don't match the
  message counts exactly.
- A failed summarisation leaves the conversation untouched (the function returns
  `Err` before mutating), which is the important failure property.

## Alternatives considered

- **Drop oldest messages.** Cheapest, but loses the decisions the summary exists
  to preserve.
- **Token-budgeted chunking.** More correct, but requires an accurate tokenizer
  per model, which the project does not have (counts come from the provider
  after the fact).
- **Summarise on a different/smaller model.** Needs model routing, which does
  not exist yet.
