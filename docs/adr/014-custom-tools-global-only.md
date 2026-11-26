# ADR-0014: Custom tools load from the global config only

**Status:** Accepted (project-level `custom_tools.json` is not discovered)

## Context

Custom tools let a user declare shell-command tools in JSON instead of writing
Rust: a name, description, parameter schema, category, and a command template
with `{param}` substitution and `TH_*` environment variables. They are genuinely
useful for wiring in a project-specific script.

The obvious symmetry is to mirror skills and per-project settings: a global
`~/.config/tinyharness/custom_tools.json` extended by
`<project>/.tinyharness/custom_tools.json`. That symmetry is a trap. A custom
tool is an arbitrary shell command, and a project config **travels with the
repository**. Honouring a project-level file means `git clone && tinyharness`
executes a command the person who cloned never read — contained by the process
sandbox only where that sandbox exists (Linux) and only as far as it goes.

Skills do not have this problem: a skill is text injected into the prompt, with
no ability to run anything ([ADR-0004](004-skill-system.md)).

## Decision

`CustomToolManager` (`tinyharness-lib/src/custom_tools/mod.rs`) loads **only**
the global file:

- `CustomToolManager::load()` / `load_from(&Paths)` reads
  `paths.custom_tools_file()` — `<config_dir>/custom_tools.json`;
- a `.tinyharness/custom_tools.json` in a project is never read. A test
  (`project_custom_tools_json_is_not_loaded`) writes one containing
  `rm -rf /` and asserts the manager comes back empty.

Name collisions with built-in tools are logged and skipped, both at load time
and again in `ToolManager::register_custom_tools`.

The same reasoning is why `allowed_paths` and `sandbox` have no
`ProjectSettings` field ([ADR-0006](006-per-project-settings-layering.md)) — the
rule is "a repository-controlled file may not grant execution".

## Consequences

- A team cannot commit a shared custom tool. The workarounds are: document the
  JSON snippet in `TINYHARNESS.md`, or make it a real tool in the workspace.
- One code path, one file to document and to debug — no merge semantics for
  custom tools.
- The sandbox remains the only containment for an approved custom tool, and it
  is Linux-only.
- If project-level custom tools are ever wanted, they must arrive with an
  explicit opt-in that is itself global-only — not as a default-on merge.

## Alternatives considered

- **Mirror the settings layering, project extends global.** Convenient, and the
  exact shape of the attack this ADR exists to prevent.
- **Project tools require confirmation every call.** Still executes a command a
  cloned repo chose; and users auto-accept.
- **Allow project tools but force them to `ReadOnly` category.** The category
  only affects auto-accept, not what the command does.
