# ADR-0006: Per-project settings layer over global settings, with an asymmetric merge

**Status:** Accepted

## Context

Some settings genuinely belong to a repository: which commands are safe for
*this* codebase, how much context to reserve, which extra instruction files to
load. Committing them means a team shares them. But a project config travels
with the repository — anything it can set, `git clone && tinyharness` lets a
stranger set on your machine. So a project file must be able to make the tool
*more useful*, and must not be able to make it *less safe*.

## Decision

Two files: global `<config_dir>/settings.json` (`Settings`, all fields concrete)
and project `<project>/.tinyharness/config.json` (`ProjectSettings`, every field
`Option`, "absent means inherit"). The project file is found by walking up from
the project directory, the same way instruction files are discovered.

`merge_settings()` produces a read-only `MergedSettings` in which every field
carries a `SettingSource` (`Global` / `Project` / `Default`) so `/settings` can
show *where* a value came from.

The merge is deliberately **not** uniform:

| Field | Project behaviour | Why |
|---|---|---|
| `safe_command_prefixes` | **extends** global | Adding repo-specific read-only commands is the common case; replacing would silently drop the global list |
| `denied_command_prefixes` | **replaces** global | A project may want stricter denials than global |
| `auto_accept_mode`, `context_limit`, `preferred_mode`, `project_md_files`, `auto_compact_enabled`, `questions_enabled` | overrides | Ordinary preferences |
| `sandbox`, `allowed_paths` | **not present at all** | See below |

`ProjectSettings` has no `sandbox` and no `allowed_paths` field, by design.
Both are grants: the sandbox decides what an approved shell command can reach,
and `allowed_paths` widens the in-process file tools (read *and write*, see
[ADR-0008](008-two-layer-containment.md)). A cloned repository must not be able
to turn the sandbox off or open `$HOME` to `write`. Serde ignores unknown keys,
so a stray `"sandbox": {...}` block in a project file has no effect, and two
tests (`project_config_cannot_override_the_sandbox` in `src/sandbox_setup.rs`,
`project_settings_cannot_set_allowed_paths` in `tinyharness-lib/src/config/mod.rs`)
pin that.

## Consequences

- The asymmetry is a real cognitive cost: contributors must remember that safe
  *extends* while denied *replaces*. The table above and the field doc comments
  are the only defence.
- New settings have to make a conscious choice. The default (adding an `Option`
  field to `ProjectSettings`) is to make it project-overridable, which is the
  wrong default for anything that grants access.
- A parse error in a project file is logged and ignored, not fatal — a broken
  committed config should not brick the tool.
- `SettingSource` makes `/settings` answers honest, but only for fields that
  implement the merge; `sandbox_source` is always `Global`.

## Alternatives considered

- **One file.** No sharing of team settings.
- **Uniform "project overrides global".** Simple, but lets a repo disable the
  sandbox — the whole reason for the asymmetry.
- **Allow-list of project-settable keys stored in a separate schema.** Same
  outcome as absent fields, with more machinery.
