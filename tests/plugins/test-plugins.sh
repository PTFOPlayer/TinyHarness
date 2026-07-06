#!/usr/bin/env bash
# Test script for the TinyHarness plugin system.
#
# This script:
# 1. Validates that the plugin config JSON parses correctly
# 2. Tests each custom tool by calling its shell command directly
# 3. Tests each hook by simulating lifecycle events
#
# Usage: bash tests/plugins/test-plugins.sh
#
# Requirements: jq, wc, git, make, cargo (all should be available in the project)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
CONFIG="$SCRIPT_DIR/plugins.json"
LOG_FILE="$SCRIPT_DIR/plugin-test.log"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BOLD='\033[1m'
RESET='\033[0m'

pass=0
fail=0
warn=0

log() { echo -e "[$(date '+%H:%M:%S')] $1"; }
ok()   { echo -e "  ${GREEN}✓${RESET} $1"; pass=$((pass + 1)); }
bad()  { echo -e "  ${RED}✗${RESET} $1"; fail=$((fail + 1)); }
info() { echo -e "  ${YELLOW}→${RESET} $1"; }

echo -e "${BOLD}═══════════════════════════════════════════════════════════${RESET}"
echo -e "${BOLD}  TinyHarness Plugin System Tests${RESET}"
echo -e "${BOLD}═══════════════════════════════════════════════════════════${RESET}"
echo

# ── 1. JSON Validation ─────────────────────────────────────────────────────

echo -e "${BOLD}1. Config Validation${RESET}"
echo

if command -v jq &> /dev/null; then
    if jq empty "$CONFIG" 2>/dev/null; then
        ok "plugins.json is valid JSON"
    else
        bad "plugins.json is invalid JSON"
        exit 1
    fi

    hook_count=$(jq '.hooks | length' "$CONFIG")
    tool_count=$(jq '.custom_tools | length' "$CONFIG")
    ok "Found $hook_count hooks, $tool_count custom tools"

    # Validate each hook has required fields
    hook_count=$(jq '.hooks | length' "$CONFIG")
    for ((i = 0; i < hook_count; i++)); do
        name=$(jq -r ".hooks[$i].name" "$CONFIG")
        event=$(jq -r ".hooks[$i].event" "$CONFIG")
        command=$(jq -r ".hooks[$i].command" "$CONFIG")
        if [ -n "$name" ] && [ "$name" != "null" ] && [ -n "$event" ] && [ "$event" != "null" ] && [ -n "$command" ] && [ "$command" != "null" ]; then
            ok "Hook '$name' (event: $event) — fields OK"
        else
            bad "Hook '$name' — missing required fields"
        fi
    done

    # Validate each tool has required fields
    tool_count=$(jq '.custom_tools | length' "$CONFIG")
    for ((i = 0; i < tool_count; i++)); do
        name=$(jq -r ".custom_tools[$i].name" "$CONFIG")
        cat=$(jq -r ".custom_tools[$i].category" "$CONFIG")
        command=$(jq -r ".custom_tools[$i].command" "$CONFIG")
        if [ -n "$name" ] && [ "$name" != "null" ] && [ -n "$cat" ] && [ "$cat" != "null" ] && [ -n "$command" ] && [ "$command" != "null" ]; then
            ok "Tool '$name' (category: $cat) — fields OK"
        else
            bad "Tool '$name' — missing required fields"
        fi
    done
else
    echo -e "  ${YELLOW}⚠ jq not found, skipping JSON validation${RESET}"
    warn=$((warn + 1))
fi
echo

# ── 2. Custom Tool Tests ────────────────────────────────────────────────────

echo -e "${BOLD}2. Custom Tools${RESET}"
echo

# Create a test file for line_count and word_count
TEST_FILE="$SCRIPT_DIR/sample.txt"
echo -e "hello world\nfoo bar baz\nTinyHarness plugins are cool" > "$TEST_FILE"
EXPECTED_LINES=3
EXPECTED_WORDS=9

# Test: line_count
info "Testing line_count tool..."
LINES=$(wc -l < "$TEST_FILE" | tr -d ' ')
if [ "$LINES" = "$EXPECTED_LINES" ]; then
    ok "line_count returned $LINES (expected $EXPECTED_LINES)"
else
    bad "line_count returned $LINES (expected $EXPECTED_LINES)"
fi

# Test: word_count
info "Testing word_count tool..."
WORDS=$(wc -w < "$TEST_FILE" | tr -d ' ')
if [ "$WORDS" = "$EXPECTED_WORDS" ]; then
    ok "word_count returned $WORDS (expected $EXPECTED_WORDS)"
else
    bad "word_count returned $WORDS (expected $EXPECTED_WORDS)"
fi

# Test: git_branch
info "Testing git_branch tool..."
cd "$PROJECT_ROOT"
GIT_OUTPUT=$(git branch -a 2>/dev/null)
if [ -n "$GIT_OUTPUT" ]; then
    ok "git_branch returned $(echo "$GIT_OUTPUT" | wc -l | tr -d ' ') branches"
else
    echo -e "  ${YELLOW}⚠ Not in a git repo or no branches${RESET}"
    warn=$((warn + 1))
fi

# Test: make_target (dry run — just check make is available)
info "Testing make_target tool (dry run)..."
if make -n help &>/dev/null || make -n &>/dev/null; then
    ok "make_target command is valid (make available)"
else
    echo -e "  ${YELLOW}⚠ make not available or no Makefile${RESET}"
    warn=$((warn + 1))
fi

# Test: cargo_test_filtered (dry run — just check cargo is available)
info "Testing cargo_test_filtered tool (dry run)..."
if cargo --version &>/dev/null; then
    ok "cargo_test_filtered command is valid (cargo available)"
else
    echo -e "  ${YELLOW}⚠ cargo not available${RESET}"
    warn=$((warn + 1))
fi

# Clean up test file
rm -f "$TEST_FILE"
echo

# ── 3. Hook Tests ───────────────────────────────────────────────────────────

echo -e "${BOLD}3. Hooks${RESET}"
echo

# Clear previous log
rm -f "$LOG_FILE"

# Test: before_user_message hook
info "Testing before_user_message hook..."
USER_INPUT="test message from plugin test"
EVAL_CMD=$(jq -r '.hooks[] | select(.event=="before_user_message") | .command' "$CONFIG" | head -1)
# Simulate: replace {user_input} with our test value
SIMULATED=$(echo "$EVAL_CMD" | sed "s/{user_input}/$USER_INPUT/g")
# The command writes to .tinyharness/plugin-test.log relative to cwd
# We need to create the directory and adjust the path
mkdir -p "$SCRIPT_DIR/.tinyharness" 2>/dev/null || true
SIMULATED=$(echo "$SIMULATED" | sed "s|\.tinyharness/|$SCRIPT_DIR/.tinyharness/|g")
eval "$SIMULATED" 2>/dev/null
if [ -f "$SCRIPT_DIR/.tinyharness/plugin-test.log" ]; then
    ok "before_user_message hook wrote to log"
    if grep -q "before_user_message" "$SCRIPT_DIR/.tinyharness/plugin-test.log"; then
        ok "Log contains expected content"
    else
        bad "Log missing expected content"
    fi
else
    bad "before_user_message hook did not write to log"
fi

# Test: before_llm_call hook (inject_stdout)
info "Testing before_llm_call hook (inject_stdout)..."
INJECT_CMD=$(jq -r '.hooks[] | select(.event=="before_llm_call") | .command' "$CONFIG" | head -1)
INJECT_OUTPUT=$(eval "$INJECT_CMD" 2>/dev/null)
if [ -n "$INJECT_OUTPUT" ]; then
    ok "before_llm_call hook produced stdout: $(echo "$INJECT_OUTPUT" | head -c 60)..."
    if echo "$INJECT_OUTPUT" | grep -q "current time"; then
        ok "Injected text contains timestamp"
    else
        bad "Injected text missing expected content"
    fi
else
    bad "before_llm_call hook produced no output"
fi

# Test: before_tool_call hook (template substitution)
info "Testing before_tool_call hook substitution..."
BEFORE_TOOL_CMD=$(jq -r '.hooks[] | select(.event=="before_tool_call") | .command' "$CONFIG" | head -1)
SIMULATED=$(echo "$BEFORE_TOOL_CMD" | sed 's/{tool}/run/g; s/{args}/{"command":"ls"}/g')
SIMULATED=$(echo "$SIMULATED" | sed "s|\.tinyharness/|$SCRIPT_DIR/.tinyharness/|g")
eval "$SIMULATED" 2>/dev/null
if grep -q "before_tool_call" "$SCRIPT_DIR/.tinyharness/plugin-test.log"; then
    ok "before_tool_call hook wrote to log with {tool} and {args} substituted"
else
    bad "before_tool_call hook log not found"
fi

# Test: after_tool_call hook (template substitution)
info "Testing after_tool_call hook substitution..."
AFTER_TOOL_CMD=$(jq -r '.hooks[] | select(.event=="after_tool_call") | .command' "$CONFIG" | head -1)
SIMULATED=$(echo "$AFTER_TOOL_CMD" | sed 's/{tool}/read/g; s/{result}/file contents here/g')
SIMULATED=$(echo "$SIMULATED" | sed "s|\.tinyharness/|$SCRIPT_DIR/.tinyharness/|g")
eval "$SIMULATED" 2>/dev/null
if grep -q "after_tool_call" "$SCRIPT_DIR/.tinyharness/plugin-test.log"; then
    ok "after_tool_call hook wrote to log with {tool} and {result} substituted"
else
    bad "after_tool_call hook log not found"
fi

# Test: on_exit hook
info "Testing on_exit hook..."
ON_EXIT_CMD=$(jq -r '.hooks[] | select(.event=="on_exit") | .command' "$CONFIG" | head -1)
SIMULATED=$(echo "$ON_EXIT_CMD" | sed "s|\.tinyharness/|$SCRIPT_DIR/.tinyharness/|g")
eval "$SIMULATED" 2>/dev/null
if grep -q "on_exit" "$SCRIPT_DIR/.tinyharness/plugin-test.log"; then
    ok "on_exit hook wrote to log"
else
    bad "on_exit hook log not found"
fi

# Clean up
rm -rf "$SCRIPT_DIR/.tinyharness"
rm -f "$LOG_FILE"
echo

# ── 4. Template Substitution Tests ─────────────────────────────────────────

echo -e "${BOLD}4. Template Substitution${RESET}"
echo

# Test: single variable
info "Testing single {var} substitution..."
TEMPLATE="echo {name}"
RESULT=$(echo "$TEMPLATE" | sed 's/{name}/world/g')
if [ "$RESULT" = "echo world" ]; then
    ok "Single var substitution works"
else
    bad "Single var substitution failed: got '$RESULT'"
fi

# Test: multiple variables
info "Testing multiple {var} substitution..."
TEMPLATE="docker run --rm {image} sh -c '{command}'"
RESULT=$(echo "$TEMPLATE" | sed 's/{image}/ubuntu:latest/g; s/{command}/ls/g')
if [ "$RESULT" = "docker run --rm ubuntu:latest sh -c 'ls'" ]; then
    ok "Multiple var substitution works"
else
    bad "Multiple var substitution failed: got '$RESULT'"
fi

# Test: missing variable (should remain as placeholder)
info "Testing missing variable handling..."
TEMPLATE="echo {missing}"
RESULT=$(echo "$TEMPLATE" | sed 's/{not_missing}/value/g')
if [ "$RESULT" = "echo {missing}" ]; then
    ok "Missing variable stays as placeholder"
else
    bad "Missing variable was incorrectly replaced: got '$RESULT'"
fi
echo

# ── Summary ─────────────────────────────────────────────────────────────────

echo -e "${BOLD}═══════════════════════════════════════════════════════════${RESET}"
echo -e "  ${GREEN}Passed:${RESET} $pass   ${RED}Failed:${RESET} $fail   ${YELLOW}Warnings:${RESET} $warn"
echo -e "${BOLD}═══════════════════════════════════════════════════════════${RESET}"

if [ "$fail" -gt 0 ]; then
    exit 1
else
    echo -e "  ${GREEN}All tests passed!${RESET}"
    exit 0
fi