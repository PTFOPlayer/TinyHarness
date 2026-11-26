# ADR-0011: Streaming through a bounded `mpsc` channel with a sentinel-terminated protocol

**Status:** Accepted

## Context

A chat response arrives as SSE/WebSocket deltas that must be rendered as they
arrive, interleaved with the tool-call results the loop produces afterwards. The
agent loop must also be able to abandon a stream mid-flight when the user presses
Ctrl+C, and the backend must not borrow anything from the caller while streaming.

Returning `impl Stream` would couple the returned type to the provider impl and to
the borrow of `&mut self`; returning a fully-collected response would lose
streaming entirely.

## Decision

`AnyProvider::chat` returns `Result<mpsc::Receiver<ChatMessageResponse>, String>`.
The provider spawns a background `tokio` task that owns the HTTP/WebSocket
connection, parses deltas, and pushes `ChatMessageResponse` chunks into the
channel. The channel is bounded at `STREAM_CHANNEL_CAPACITY = 64` chunks, which
applies backpressure to the provider's read loop when the UI is slow instead of
buffering without bound.

Termination is explicit: the final chunk carries `done: true`. Errors are also a
terminal chunk — `done: true, is_error: true` with the message in `content` — so
the consumer has exactly one loop (`while let Some(chunk) = recv.recv().await {
… if chunk.done { break } }`) and no second error channel.

Consequences of that shape are handled explicitly:

- dropping the receiver is the cancellation mechanism: every `send().await`
  checks its result and stops reading the source stream when it fails;
- `thinking`/reasoning deltas travel on the same channel in their own field, so
  the UI can render them separately;
- token usage rides on the final chunk (`usage`), avoiding a separate call.

Retry logic lives inside the provider (`chat_with_retries`): an attempt that
fails *before* delivering anything is `Retryable`; once a terminal chunk has
reached the receiver the outcome is `Final`, because retrying would duplicate
output.

## Consequences

- The loop and the backend share no lifetime, so streaming does not constrain
  how the provider is stored (`Arc<Mutex<AnyProvider>>`).
- Every backend must remember to always emit a `done` chunk, including on
  error paths. A backend that returns early without one hangs the consumer —
  `send_error_chunk` exists to make that hard to get wrong.
- Backpressure is invisible from the consumer side; 64 is chosen as "far more
  than a terminal renders per frame".
- Cancellation is graceful but not synchronous — the background task notices on
  its next `send`, so an in-flight HTTP read may complete first.
- Usage is only available on the final chunk, so a cancelled turn reports no
  token count.

## Alternatives considered

- **`impl Stream`.** Couples the returned type to the backend struct and to the
  borrow of `&mut self`, so it could not be returned from a shared
  `Arc<Mutex<AnyProvider>>` without boxing.
- **Unbounded channel.** No backpressure; a slow terminal or a paused UI grows
  memory with a fast model.
- **Callback/`FnMut` sink.** The consumer's UI borrow cannot be sent into a
  spawned task.
