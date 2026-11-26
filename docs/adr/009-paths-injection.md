# ADR-0009: `Paths` as the single ambient-environment boundary in the library

**Status:** Accepted (migrated incrementally; `paths-guard` CI job enforces it)

## Context

Every module used to compute its own location: `~/.config/tinyharness/...`,
`~/.local/share/tinyharness/...`, `std::env::current_dir()`. That made the
library impossible to drive against an alternate root — tests shared the real
home directory, an embedded host application could not isolate profiles, and
portable installs or a second profile were out of the question. It also made
`tinyharness-lib` untestable in parallel without clobbering user state.

## Decision

`tinyharness-lib/src/paths.rs::Paths` is the **only** place that reads `$HOME`,
`$XDG_CONFIG_HOME`, `$XDG_DATA_HOME` or the process CWD. It exposes derived
locations (`settings_file()`, `sessions_dir()`, `prompts_dir()`,
`custom_tools_file()`, `personal_skills_dirs()`, `project_skills_dir()`, …) and
is constructed via `default()`, `with_home()`, `from_env(lookup)`, or
`new(config, data, project)` plus `with_*` builders.

Every subsystem gets an injectable entry point taking `&Paths`:
`SettingsStore::from_paths`, `SessionStore::from_paths`, `load_settings_from`,
`save_settings_to`, `load_merged_settings_from`, `prompts_dir_for`,
`ensure_prompts_initialized_in`, `discover_skills_in`,
`SkillRegistry::discover_in`, `CustomToolManager::load_from`,
`WorkspaceContext::collect_in`, `ImageAttachment::load_from_str_in`,
`AgentMode::load_system_prompt(&dir)`.

The no-arg forms (`load_settings()`, `SessionStore::default_path()`,
`discover_skills()`, …) survive as thin wrappers over `Paths::default()` — for
host applications, not for the binary.

**The binary never uses the wrappers.** In `src/commands/` and `src/agent/`,
settings resolve through the injected instance: `&ctx.paths` where a
`CommandContext` exists (including inside `async_command!`, where `ctx` is moved
into the future), an explicit `paths: &Paths` parameter threaded from the
registration closure otherwise, or `&Paths` from `main.rs` for startup code that
runs before a `CommandContext` exists. Helpers that need one value take that
value instead of the whole `Paths` (`display.rs` takes
`context_limit: Option<u32>`).

This is enforced, not merely documented: the `paths-guard` CI job requires
`grep -rn "load_settings()\|save_settings(\|load_merged_settings()" src/` to be
empty, and `tinyharness-lib/tests/paths_injection.rs` guards the library side.

Resolution rules worth knowing: per the XDG spec, empty or relative
`XDG_*_HOME` values are ignored; when `$HOME` is also unset the fallback is
relative (`.config/tinyharness`), preserving the old
`home.unwrap_or(".")` behaviour so a caller with no home still gets a
self-consistent writable tree instead of a panic. `personal_skills_dirs()`
returns `<config>/skills` *then* the legacy `~/.tinyharness/skills`, in
precedence order.

Documented escape hatches (everything else is a bug): `TINYHARNESS_MD_FILES` in
`config::resolve_project_md_files` (an override knob, not a storage location),
the CWD fallback in `ImageAttachment::load_from_str`, and an ambient
`load_settings()` inside `tools/web_search.rs`.

## Consequences

- Multiple fully isolated profiles can run in one process, and tests use
  `Paths::with_home(tempdir)` instead of touching the real home directory.
- Every new filesystem location needs a `Paths` getter *and* an injectable
  constructor, plus keeping the wrapper in sync. That is the tax.
- The CI grep is textual, so it catches only the three known no-arg names; a new
  ambient read is caught by review and by the injection test, not by the linter.
- `save_settings_to` returns `Result` where `save_settings` swallowed errors, so
  callers keep the old behaviour explicitly with
  `if let Err(e) = … { tracing::warn!(…) }`.

## Alternatives considered

- **A global `OnceLock<Paths>`.** Reintroduces ambient state and makes two
  profiles in one process impossible.
- **Pass individual paths.** Every subsystem would grow 3–5 parameters that all
  derive from one root; `&Paths` carries the same information.
- **`directories`/`etcetera` crate.** A dependency for logic that is ~100 lines
  and needs behaviour it does not offer (relative fallbacks, legacy dirs).
