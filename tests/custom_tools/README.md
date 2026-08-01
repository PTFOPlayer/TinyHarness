# Custom Tools Test Fixtures

This directory contains test fixtures for the TinyHarness custom-tools system.

Custom tools are configured in `custom_tools.json` (global:
`~/.config/tinyharness/custom_tools.json`, project: `.tinyharness/custom_tools.json`)
and managed by `CustomToolManager`.

## Files

| File | Description |
|------|-------------|
| `custom_tools.json` | Sample custom-tools config with 5 custom tools |

## What's Tested

### Custom Tools (5)

| Tool | Category | Command | Tests |
|------|----------|---------|-------|
| `line_count` | readonly | `wc -l < {path}` | Counts lines in a sample file |
| `word_count` | readonly | `wc -w < {path}` | Counts words in a sample file |
| `make_target` | destructive | `make {target}` | Dry run — checks `make` is available |
| `git_branch` | readonly | `git branch -a` | Lists branches in the repo |
| `cargo_test_filtered` | destructive | `cargo test -- {test_name}` | Dry run — checks `cargo` is available |

The corresponding integration tests live in
`tinyharness-lib/tests/custom_tool_fixture_test.rs` and
`tinyharness-lib/tests/custom_tool_integration.rs`.

## Using the Config with TinyHarness

To test the custom tools live with TinyHarness, copy the config to your project:

```bash
mkdir -p .tinyharness
cp tests/custom_tools/custom_tools.json .tinyharness/custom_tools.json
```

Then start TinyHarness. The custom tools will appear in the LLM's tool list.
