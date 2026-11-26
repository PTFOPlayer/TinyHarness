# ADR-0017: Remove the dead `thought_signature` tool-call field

**Status:** Accepted

## Context

`ToolCallFunction` carried an optional `thought_signature: Option<String>`:

```rust
/// Gemini `thought_signature` required for multi-turn tool calling.
/// Gemini returns this on tool calls and requires it back in subsequent
/// turns. Ollama Cloud doesn't preserve it, so we capture and re-inject it.
pub thought_signature: Option<String>,
```

This was a real requirement of the Ollama provider: Gemini returns a signed
token on each tool call and rejects the next turn if it is not echoed back
unchanged, and Ollama's transport dropped it, so the provider captured and
re-injected it.

That provider no longer exists — Ollama, llama.cpp, vLLM and hosted gateways all
collapsed into `OpenAiCompatProvider`
([ADR-0013](013-unified-openai-compat-provider.md)), and the OpenAI-compatible
wire format has no such field. An audit found **14 construction sites and zero
`Some` values**: every one built the field as `thought_signature: None`, and
nothing ever read it. The field was dead weight on a public type, advertised as a
feature in `TINYHARNESS.md` and in ADR-0013.

A dead optional field is worse than clutter here. It invites three wrong
assumptions: that Gemini tool calling works (it does not), that a future
provider should populate it rather than re-deriving the requirement, and that
serialization handles it (`skip_serializing_if` means its absence is
undetectable on the wire).

## Decision

Delete the field. `ToolCallFunction` is now:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    pub arguments: serde_json::Value,
}
```

All 14 initializers are dropped along with it, and the two documents that
claimed the capability (`TINYHARNESS.md`'s "Ollama specifics" rule and
ADR-0013's context paragraph) are corrected rather than deleted — ADR-0013 now
records that its stated reason for keeping a native Ollama provider has itself
lapsed, so the reader is not left with a justification for code that is gone.

## Consequences

- **Breaking change for `tinyharness-lib` consumers** who constructed
  `ToolCallFunction` with the field — they now get a compile error, which is the
  intent. Removing a public field is a semver-breaking change, which is why it
  lands in the `0.6.0` release rather than in a patch.
- Gemini-style signed tool calls are **not supported** and are not silently
  half-supported. Anyone who needs them must add capture and re-injection to a
  provider that speaks the protocol, which is a new ADR.
- Saved sessions written by earlier versions may contain
  `"thought_signature": null`. `serde` ignores unknown fields by default, so they
  still load; the field is simply dropped on the next save.
- The remaining `ChatMessage::thinking` field is unrelated and stays: it carries
  streamed reasoning deltas and is genuinely populated
  ([ADR-0011](011-streaming-via-mpsc.md)).

## Alternatives considered

- **Keep it as a reserved field.** A permanently-`None` public field is a lie
  that costs a compile error nobody receives and a doc claim that is false.
- **`#[deprecated]` first, remove later.** Meaningful for a field that works.
  This one has never been set by any code path, so there is no behaviour to
  preserve and no deprecation window to honour.
- **Leave it and wire up Gemini tool calling.** That is a feature, not this
  cleanup, and it would need a provider that speaks Gemini's protocol — the
  native Ollama provider this field was written for is already gone.
