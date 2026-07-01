# Safety & Security

TinyHarness grants an LLM the ability to execute shell commands, write files, and modify your project. This document explains how the safety system works and how to configure it for your threat model.

## Command Safety Model

The `run` tool executes arbitrary shell commands. Every command goes through a safety checker (`is_safe_command`) before auto-accept is allowed.

### What Gets Checked

1. **Deny list** — checked first, takes highest priority
2. **Shell metacharacter rejection** — blocks command injection patterns
3. **Safe descriptor redirections** — strips harmless patterns before further checks
4. **Chain splitting** — `&&` and `||` chains are recursively checked
5. **Safe prefix matching** — word-boundary-aware prefix comparison

### Safety Check Flow

```
Input: "cd /project && cargo test 2>&1"
  ↓
1. Check deny list: not denied ✓
2. Check newlines: none ✓
3. Check semicolons: none ✓
4. Strip safe redirections: "cd /project && cargo test "
5. Check lone `&` (background): none ✓
6. Split on && / ||: ["cd /project", "cargo test "]
7. For each part, check pipes/$()/backticks: none ✓
8. For each part, check >/<: none ✓
9. For each part, prefix-match safe list:
   - "cd" matches ✓
   - "cargo test" matches ✓
10. Result: SAFE ✓
```

---

## Safe Commands

The default safe list contains 43 prefixes covering common read-only CLI utilities:

| Category | Commands |
|----------|----------|
| **Navigation** | `cd`, `ls`, `pwd`, `tree`, `find` |
| **File inspection** | `cat`, `head`, `tail`, `wc`, `file`, `stat` |
| **Search** | `grep`, `glob` |
| **System info** | `du`, `df`, `free`, `uptime`, `whoami`, `hostname`, `uname`, `date`, `cal` |
| **Process info** | `ps` |
| **Environment** | `env`, `printenv`, `which`, `whereis`, `type` |
| **Output** | `echo`, `printf` |
| **Git (read-only)** | `git status`, `git diff`, `git log`, `git show`, `git branch`, `git remote`, `git tag`, `git describe`, `git rev-parse` |
| **Cargo (read-only)** | `cargo tree`, `cargo metadata`, `cargo doc`, `rustc --version`, `cargo --version` |

### What's NOT on the Safe List

Anything that creates, modifies, or executes:

- `git commit`, `git push`, `git add`, `git checkout`
- `cargo build`, `cargo test`, `cargo run`
- `make`, `cmake`, `ninja`
- `npm`, `pip`, `curl`, `wget`
- `rm`, `mv`, `cp`, `chmod`, `chown`

These always require explicit confirmation.

### Customizing the Safe List

```
/command add "python -m pytest"     # Add a custom safe prefix
/command rm "git tag"               # Remove a prefix (make it require confirmation)
/command reset                      # Restore defaults
```

Project-specific additions (in `.tinyharness/config.json`):
```json
{
  "safe_command_prefixes": ["python -m pytest", "npm run lint"]
}
```

---

## Deny List

Commands that should **always** require confirmation, even if they match a safe prefix.

```json
{
  "denied_command_prefixes": ["git push", "git push --force", "rm"]
}
```

The deny list takes **priority over the safe list**. If a command matches both, it's blocked:

```
/command deny "git push"
# Now "git push" always requires confirmation
# But "git status" is still auto-accepted
```

### Block All Cargo Commands

```
/command deny "cargo"
# Blocks: cargo build, cargo test, cargo tree, cargo metadata, ...
```

This overrides any matching safe prefixes like `cargo tree`.

### Useful Deny Patterns

| Pattern | Blocks |
|---------|--------|
| `git push` | `git push`, `git push origin main`, `git push --force` |
| `cargo` | All cargo subcommands including read-only ones |
| `rm` | `rm`, `rm -rf`, `rmdir` |
| `curl` | `curl`, `wget` |
| `git` | All git subcommands (if you want full manual control) |

---

## Shell Metacharacter Rejection

The following patterns are **always rejected** and cannot be made safe:

| Pattern | Reason |
|---------|--------|
| `\n` (newline) | Could hide a second command on a new line |
| `;` | Command separator: `ls; rm -rf /` |
| Single `&` | Background operator: `sleep 1 & rm -rf /` |
| `\|` (pipe) | Could pipe to a dangerous command |
| `$()` | Command substitution: `echo $(rm -rf /)` |
| `` ` `` (backticks) | Alternative command substitution |
| `>` (redirection) | File writing: `echo hi > /etc/hosts` |
| `<` (redirection) | File input: `cat < /etc/shadow` |

### Safe Exceptions

Safe descriptor redirections are stripped **before** metacharacter checks:

| Pattern | What it does | Why it's safe |
|---------|-------------|---------------|
| `2>&1` | Redirect stderr to stdout | No file access, just merges output streams |
| `2>/dev/null` | Discard stderr | Only writes to `/dev/null` (bit bucket) |
| `1>&2` | Redirect stdout to stderr | No file access |

This enables auto-accepted commands like `cargo test 2>&1` (if `cargo test` is on the safe list).

### Chain Splitting

`&&` and `||` are handled by recursively checking each part:

```
"cd /path && ls && git status"  →  safe (all 3 parts are safe)
"cd /path && rm -rf /"          →  NOT safe (rm is not safe)
"ls || cargo build"             →  NOT safe (cargo build is not safe)
```

Mixed chains work:
```
"cd /path && ls || pwd"  →  safe
"cd /path && ls || rm -rf /"  →  NOT safe
```

---

## Confirmation Behavior

### Destructive Tools

`write`, `edit`, and `run` always show a confirmation prompt:

```
  Write to /path/to/file.rs (4194 bytes)

Confirm? (Y)es / (N)o / (A)uto-accept future
```

### Auto-Accept Mode

Toggle with `/autoaccept off`, `/autoaccept safe`, or `/autoaccept all`.

**`off`** (default):
- All destructive tools prompt for confirmation
- No auto-accept toggle is offered

**`safe`** (`auto_accept_mode: "safe"`):
- **ReadOnly** tools auto-execute (always, regardless of this setting)
- **Destructive** `write` and `edit` get a prompt — but pressing `A` during confirmation enables auto-accept for the rest of the session
- **Destructive** `run` is NEVER auto-accepted, even with `A`

**`all`** (`auto_accept_mode: "all"`):
- **Destructive** `write` and `edit` auto-execute without prompting
- **Destructive** `run` still prompts — it is NEVER auto-accepted, even in `all` mode

### `run` Tool Special Rule

The `run` tool can **never** be auto-accepted, even with `/autoaccept all` and pressing `A`. This is a hard rule — if commands are safe, they pass the safety checker and auto-execute. If they're not, you must confirm them individually.

---

## Security Best Practices

### For Users

1. **Sandbox**: Run TinyHarness inside a Docker container or VM for untrusted models
2. **Review before confirming**: Always read the proposed command before pressing `Y`
3. **Use the deny list**: Block commands you never want auto-executed: `/command deny "git push --force"`
4. **Per-project settings**: Set `auto_accept_mode: "off"` for sensitive projects
5. **Limit tools by mode**: Use `planning` mode for analysis, switch to `agent` only when you need destructive operations
6. **Check the audit log**: `/audit last` shows recently executed commands; `/audit session` shows all in this session

### For Project Maintainers

1. **Commit a `.tinyharness/config.json`** with appropriate deny patterns:
   ```json
   {
     "denied_command_prefixes": ["git push --force", "rm -rf"],
     "auto_accept_mode": "off"
   }
   ```
2. **Use project-local skills** with `disable-model-invocation: true` for sensitive procedures
3. **Keep `TINYHARNESS.md` up to date** — the better the AI understands your project, the less likely it is to propose dangerous commands

### LLM Limitations

- LLMs can hallucinate or misunderstand context
- They may propose commands that are technically safe but semantically dangerous (e.g. `git reset --hard` without understanding the consequences)
- **You are responsible** for all operations performed by the AI. Review everything before confirming.

---

## Audit Log

Track executed commands with `/audit`:

| Command | Output |
|---------|--------|
| `/audit last` | Most recent command execution |
| `/audit session` | All commands executed in this session |
| `/audit clear` | Clear the audit log for this session |

Each entry includes:
- Command text
- Tool arguments
- Execution timestamp
- Exit code
- Duration

---

## Safety Checker Internals

Located in `src/agent/safety.rs`. Key functions:

- `is_safe_command(command, safe_list, deny_list)` — main checker
- `strip_safe_descriptor_redirections(command)` — pre-processing step

### Edge Cases Handled

- **Word boundaries**: `cdx` doesn't match the `cd` prefix. Must be `cd `, `cd=`, or end-of-string after the prefix.
- **Whitespace**: Leading/trailing whitespace is trimmed.
- **Empty commands**: Empty strings are safe.
- **Mixed safe/unsafe chains**: Every part of a `&&`/`||` chain must be safe.
- **Deny priority**: Deny list beats safe list, always.
- **Redirection stripping order**: Descriptor redirections are stripped before `>` and `<` checks, preventing false positives from `2>&1`.
