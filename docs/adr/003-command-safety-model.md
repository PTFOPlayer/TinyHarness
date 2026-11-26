# ADR-0003: Command safety by prefix allow/deny lists, not by parsing the shell

**Status:** Accepted

## Context

The model can ask to run arbitrary shell commands. Most of what it wants to run
is benign (`ls`, `git status`, `cargo tree`), and prompting for every one of
those makes the agent unusable — users start blindly approving, which is worse
than not prompting at all. The tempting fix is to parse the command and decide
whether it is read-only. But a shell is not parseable this way in general:
aliases, `eval`, `env`, nested quoting, and every program that happens to have
a destructive flag all defeat it.

The real decision is therefore: what class of mistake are we defending against?
Not a hostile model — a model that is *usually* trying to help and occasionally
writes `rm -rf` where it meant `rm -rf ./target`.

## Decision

`src/agent/safety.rs::is_safe_command` decides only whether a command may be
**auto-accepted**. Declining never blocks anything: it just means the user is
prompted.

1. **Deny list first.** A command whose start matches a denied prefix (with a
   word boundary: end of string, space, or `=`) is never auto-accepted, even if
   a safe prefix also matches.
2. **Reject shell metacharacters outright**: newlines, `;`, a lone `&`, `|`,
   `$(`, backticks, `>` and `<`. These are the constructs that let one command
   hide another, so a command containing them is never auto-accepted at all.
3. **Strip safe descriptor redirections before those checks** — `2>&1`,
   `N>/dev/null` — because they only move output between streams and would
   otherwise be flagged as `&` or `>`.
4. **`&&` / `||` split recursively**, and every part must itself be safe. `cd x
   && git status` is safe; `cd x && rm -rf /` is not. This is checked before the
   `|` rejection because `||` contains `|`.
5. **Prefix match with a word boundary** against the safe list, so `cd` does not
   match `cdx`.

The list comes from `config::get_default_safe_commands()` plus
`settings.safe_command_prefixes`, extended (never replaced) by
`ProjectSettings::safe_command_prefixes`; denied lists replace the global value.

The decision itself is pure (`src/agent/confirm.rs::decide_tool_confirmation`)
and knows nothing about sandboxing — containment is enforced separately, at
execution time (see [ADR-0008](008-two-layer-containment.md)), so a sandboxed
command follows the same policy as an unsandboxed one.

## Consequences

- Not bulletproof, and not trying to be. It does not model the shell, so
  `find . -delete` passes if `find` is on the safe list. The mitigating layer is
  the process sandbox, plus the fact that a wrong decision costs a prompt.
- Denies-by-default is inverted deliberately: an unknown command prompts. The
  failure mode of a missing safe entry is friction, not damage.
- The property tests (`proptest_*` in `safety.rs`) pin the metacharacter
  rejections, so loosening a check breaks a test rather than silently widening
  auto-accept.
- `run` can never be covered by the per-turn `a` key: the key approves
  everything *except* unsafe `run` commands, which still prompt.

## Alternatives considered

- **Parse with `shell-words`/`shlex` and classify the program.** Fixes nothing
  the prefix list gets wrong (the dangerous part is arguments, not the program
  name) and adds a dependency.
- **Allow everything and rely on the sandbox.** The sandbox is Linux-only and
  does not prevent damage *inside* the workspace, which is where a bad `rm`
  usually lands.
- **Prompt for everything.** Correct but unusable; users habituate to approving.
