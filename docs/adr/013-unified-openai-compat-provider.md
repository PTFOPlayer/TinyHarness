# ADR-0013: One OpenAI-compatible provider for llama.cpp, vLLM and hosted gateways

**Status:** Accepted (supersedes separate per-backend provider modules)

## Context

llama.cpp, vLLM, OpenRouter, Together and most self-hosted gateways all speak
`/v1/chat/completions`. They had a provider module each, and the modules were
near-identical: same SSE parser, same request/response types, same retry logic.
The only real differences were (a) whether an `Authorization: Bearer` header is
sent and (b) how `list_models` resolves — llama.cpp serves a single model and has
no useful `/v1/models`.

Meanwhile Ollama *also* exposes an OpenAI-compatible `/v1`, but its native API is
richer (thinking deltas, Gemini `thought_signature` round-tripping), so it keeps
its own provider.

## Decision

`OpenAiCompatProvider` (`tinyharness-lib/src/provider/openai_compat_provider.rs`)
is a thin shell over `OpenAiCompatInner`
(`openai_compat.rs`), which owns the shared HTTP/SSE/retry machinery. The
differences are expressed as builder options rather than as separate types:

- `new(url)` — no auth, queries `/v1/models` (vLLM);
- `with_api_key(url, key)` — Bearer auth (hosted gateways);
- `with_options(url, key, timeout_secs, max_retries)`;
- `with_static_models(vec![…])` — `list_models` returns a fixed list instead of
  querying (llama.cpp).

`ProviderKind` (`Ollama`, `LlamaCpp`, `Vllm`, `OpenAiCompat`, `Sockudo`) survives
as the *identity* of a backend: it keys the per-provider URL and model maps in
`Settings`, drives CLI flags, and picks defaults
(`http://127.0.0.1:8080` for llama.cpp vs `:8000` for vLLM). Several kinds map
onto the same provider type; the kind is configuration, not implementation.

Health check lives in the shared inner: probe `/health`; on HTTP 404 fall back to
`/v1/models` and treat a **non-empty** list as healthy. An empty list or a failing
`/v1/models` is a failure — it proves nothing. Any non-404 `/health` status is
reported directly, so a real 503 is never masked by the fallback.

Serialization normalises provider quirks in one place: tool call `type` is
lowercased, a missing `id` is synthesized as `call_{index}`, and an assistant
message with only tool calls sends `content: null` (MiniMax and other strict
providers reject `""`).

## Consequences

- Three backends and any future OpenAI-compatible gateway cost zero new
  provider code — the cost is a constructor call in `src/agent/setup.rs`.
- A quirk of one gateway must be fixed for all of them. When that stops being
  acceptable, the escape hatch is a dedicated provider type in `AnyProvider`.
- The `ProviderKind` ↔ provider-type mapping is many-to-one, which is why
  "adding a provider" in `CONTRIBUTING.md` still lists five steps.
- `static_models` is carried on the outer provider, so
  `OpenAiCompatProvider::list_models` branches before delegating — the one place
  the abstraction leaks.
- Model-variant suffixes (`:free`, `:floor`, `:batch`) are ignored when
  validating a saved model against the served list
  (`provider/model_variants.rs`), because gateways list base ids only.

## Alternatives considered

- **Keep one provider per backend.** Duplicated SSE parsing and retry logic that
  drifted; three copies of every fix.
- **A `ProviderKind`-keyed `match` inside one giant `chat`.** Same sharing, but
  the auth/model-list differences would be runtime branches instead of
  construction-time options.
- **A third-party OpenAI SDK.** A large dependency for ~600 lines of code, and
  it would not tolerate local servers' protocol deviations.
