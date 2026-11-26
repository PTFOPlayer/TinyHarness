# ADR-0008: Two independent containment layers — path guard and process sandbox

**Status:** Accepted

## Context

TinyHarness has two kinds of tool with nothing in common security-wise:

- **In-process tools** (`ls`, `read`, `write`, `edit`, `grep`, `glob`) run as
  ordinary function calls in the harness's own process. They take a *path*, and
  a path is a small, checkable thing.
- **Shell tools** (`run`, custom tools) run an arbitrary command string. There
  is nothing reliable to inspect before it runs.

Trying to confine both with one mechanism means either sandboxing the harness's
own process (which needs its config, its terminal, its provider connection) or
hand-parsing shell commands (which [ADR-0003](003-command-safety-model.md)
already rejects as a security boundary). Prompt-asking is not containment
either: with `/autoaccept all` every prompt is gone.

## Decision

Two layers, both enforced at execution time and neither adding a prompt.

**Layer 1 — `PathGuard` (`tinyharness-lib/src/path_guard.rs`). Always on.**
Every path argument to a file tool must resolve inside the workspace root plus
`allowed_paths`. Details that are load-bearing:

- resolution walks components in `open(2)` order, so a `..` *after* a symlink
  resolves the way the kernel will: `<root>/link/../etc/passwd` with
  `link -> /usr` is `/etc/passwd`, not `<root>/etc/passwd`;
- the guard returns the **resolved** path and `ToolManager::guard_arguments`
  substitutes it into the arguments, because a relative argument means
  "relative to the workspace root" to the guard but "relative to the process
  cwd" to `std::fs`;
- `read`/`write`/`edit`/`ls` re-verify the *opened file descriptor's* real path
  (`/proc/self/fd`, `F_GETPATH` on macOS), closing the check-then-open window;
- `grep` never descends into symlinked directories, and `glob` results are
  filtered through the guard, because a pattern's static prefix passing says
  nothing about where the walker ends up;
- `DEFAULT_BLOCKED_PREFIXES` (`/etc`, `/usr`, `/proc`, …) caps `allowed_paths`,
  and an entry that *contains* a blocked prefix (like `/`) is rejected too;
- it fails closed: a path that cannot be *proven* inside is rejected.

**Layer 2 — `Sandbox` (`tinyharness-lib/src/sandbox.rs`). On by default, Linux
only.** `run` and custom tools execute under `bwrap` (preferred) or `firejail`.
bwrap builds a tmpfs root and binds only what is needed, so an unbound path does
not exist; it also unshares PID/IPC/UTS/cgroup namespaces, clears the
environment, and starts a new session. firejail is the fallback and is weaker —
read-only host root means reads outside the workspace still work, so it is
write/privilege containment, not visibility containment. Resolution lives in
`src/sandbox_setup.rs`: CLI beats settings, `--no-sandbox` forces off, and
because sandboxing is the *default*, a sandbox that was expected but cannot be
built is a **fatal startup error** rather than a silent downgrade.

The network is always available inside the sandbox: a coding agent runs
`git fetch`, `curl` and `cargo add` constantly, and blocking those makes the
sandbox unusable rather than safe.

The sandbox is documented to the model in the system prompt; the path guard
deliberately is not — its errors are self-explanatory, and mentioning it invites
the model to argue about it.

## Consequences

- Each tool goes to the layer that fits it, and a new tool must pick one. A new
  in-process file tool that is *not* added to `guard_arguments` is silently
  unconfined — that is the sharpest edge in this design.
- `ToolManager::set_sandbox` and `set_path_guard` must be called **before**
  `register_defaults`, because handlers are opaque closures that capture the
  sandbox at construction. Ordering is a documented precondition, not a type
  error.
- The guard is a path check, not a sandbox: `write` inside the workspace can
  clobber `.env` or `.git`. Both layers run as the same uid, so the sandbox does
  not defend against the workspace's own contents either.
- Windows/macOS get layer 1 only; the sandbox flag exits with "Linux-only
  feature" and the setting is forced off (`cfg(target_os)` gate in `main.rs`).
- Sandboxed commands see no inherited environment, so env-var-based auth fails
  inside them — a real usability cost, documented in the prompt and README.

## Alternatives considered

- **Sandbox the whole harness.** Needs the terminal, the config dir and the
  provider socket; the surface is larger than what it protects.
- **One mechanism for both.** Shell strings are not path-checkable; file tools
  cannot be kernel-confined without moving out of process.
- **Prompt-based confirmation as containment.** Defeated by auto-accept.
