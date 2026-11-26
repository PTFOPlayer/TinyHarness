# ADR-0016: Tool handlers are boxed closures over `HashMap<String, String>`

**Status:** Accepted

## Context

Tools come from two sources: built-ins compiled into the library, and custom
tools declared in JSON whose command and parameters only exist at runtime. Both
must be storable in one homogeneous `Vec` inside `ToolManager`, be dispatched by
name, and be awaitable.

A trait (`trait Tool { async fn run(&self, args: Value) -> String }`) would need
`async-trait` or `dyn`-compatible futures, and each implementation would need its
own struct. The pre-existing tool bodies all take named string arguments.

## Decision

```rust
pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: schemars::Schema,
    pub category: ToolCategory,
    pub handler: Box<dyn Fn(HashMap<String, String>) -> BoxFuture<'static, String> + Send + Sync>,
}
```

- `BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>` — boxing is
  needed here because the trait-object `Fn` must name a single concrete return
  type.
- `arguments` are flattened to `HashMap<String, String>` by
  `tool::execute_tool_call`, where **non-string JSON values are stringified with
  `to_string()`** rather than being dropped, so `{"timeout": 60000}` reaches the
  handler as `"60000"`. Handlers parse what they need.
- `extract_args!(args, path, content)` (exported at the crate root) expands to
  one `require_arg` per name with an early `Err` return, which is what makes
  handlers read like `let path = …; let content = …;`.
- `make_tool(...)` + `build_string_params_schema(required, optional)` are the
  constructors; a tool needing a non-string schema (`question`, with an array
  parameter) builds its `schemars::Schema` from raw JSON instead.
- Built-ins capture what they need at construction: `run`/custom tools capture
  `Option<Sandbox>`, the file tools capture a clone of `PathGuard`.

## Consequences

- One homogeneous vector, no per-tool struct, no dyn-async machinery.
- **Registration order is load-bearing**: `set_sandbox` and `set_path_guard` must
  be called *before* `register_defaults`/`register_custom_tools`, because the
  handlers capture those values when they are built. Setting them afterwards
  leaves already-registered tools unconfined, and nothing in the type system
  prevents it — only doc comments and the tests.
- Losing JSON type information at the boundary means every handler does its own
  parsing (`t.parse::<usize>()`), and a schema/parse mismatch surfaces as a
  runtime error string rather than a compile error.
- `Signal` tools still need a handler (a stub returning an error), because the
  struct has no "handler: Option" case — deliberate, so the type stays simple.
- Adding a tool is mechanical (module + `register_defaults` + category), which is
  why the "Adding a New Tool" checklist in `CONTRIBUTING.md` is short.

## Alternatives considered

- **`trait Tool` with `async fn`.** Not dyn-compatible without `async-trait`, and
  the values must be stored in a `Vec` of heterogeneous impls — the boxed
  closure is the smaller of the two problems. (Providers dropped their trait for
  the opposite reason: there the set of impls is closed, here it is open-ended
  because custom tools are runtime data.)
- **`enum ToolKind`.** Every tool becomes a variant and the `match` grows; custom
  tools cannot be variants at all since they are runtime data.
- **Pass `serde_json::Value` to handlers.** More faithful, but every handler then
  repeats the same `get(..).and_then(as_str)` dance.
