# Plugin System Test Fixtures

This directory contains test plugins for the TinyHarness plugin system.

## Files

| File | Description |
|------|-------------|
| `plugins.json` | Sample plugin config with 7 hooks and 5 custom tools |
| `test-plugins.sh` | Shell script that validates the config and tests each plugin |

## What's Tested

### Custom Tools (5)

| Tool | Category | Command | Tests |
|------|----------|---------|-------|
| `line_count` | readonly | `wc -l < {path}` | Counts lines in a sample file |
| `word_count` | readonly | `wc -w < {path}` | Counts words in a sample file |
| `make_target` | destructive | `make {target}` | Dry run — checks `make` is available |
| `git_branch` | readonly | `git branch -a` | Lists branches in the repo |
| `cargo_test_filtered` | destructive | `cargo test -- {test_name}` | Dry run — checks `cargo` is available |

### Hooks (7)

| Hook | Event | Feature Tested |
|------|-------|---------------|
| `before-msg-echo` | `before_user_message` | `{user_input}` substitution + file logging |
| `after-msg-echo` | `after_user_message` | `{message_count}` substitution |
| `before-llm-inject-timestamp` | `before_llm_call` | `inject_stdout: true` |
| `after-llm-log` | `after_llm_response` | `{response}` substitution |
| `before-tool-log` | `before_tool_call` | `{tool}` and `{args}` substitution |
| `after-tool-log` | `after_tool_call` | `{tool}` and `{result}` substitution |
| `on-exit-goodbye` | `on_exit` | Basic execution on exit |

## Running the Tests

```bash
bash tests/plugins/test-plugins.sh
```

Requirements: `jq`, `git`, `make`, `cargo`

## Using the Config with TinyHarness

To test the plugins live with TinyHarness, copy the config to your project:

```bash
mkdir -p .tinyharness
cp tests/plugins/plugins.json .tinyharness/plugins.json
```

Then start TinyHarness. The custom tools will appear in the LLM's tool list,
and hooks will fire at each lifecycle event, writing to `.tinyharness/plugin-test.log`.