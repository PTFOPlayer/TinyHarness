#!/usr/bin/env bash
# =============================================================================
# Sockudo AI Transport — Integration Test Runner
# =============================================================================
# Starts Sockudo in Docker (with AI Transport enabled) and runs Rust
# integration tests that exercise the SockudoProvider directly.
# Ollama is expected to be running locally on the host at port 11434.
#
# Usage:
#   ./tests/sockudo/run-test.sh up        Start the Sockudo container
#   ./tests/sockudo/run-test.sh pull      Pull a model into local Ollama
#   ./tests/sockudo/run-test.sh test      Run Rust integration tests
#   ./tests/sockudo/run-test.sh down      Stop and remove Sockudo container
#   ./tests/sockudo/run-test.sh run       Full cycle: up → wait → test → down
# =============================================================================

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# ── Config ──────────────────────────────────────────────────────────────────
SOCKUDO_URL="http://127.0.0.1:6001"
SOCKUDO_APP_ID="test-app"
OLLAMA_URL="http://127.0.0.1:11434"
TEST_MODEL="${SOCKUDO_TEST_MODEL:-qwen2.5:0.5b}"
DOCKER_COMPOSE_FILE="$SCRIPT_DIR/docker-compose.yml"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
RESET='\033[0m'

log()  { echo -e "${BLUE}[sockudo-test]${RESET} $*"; }
ok()   { echo -e "${GREEN}✔${RESET} $*"; }
fail() { echo -e "${RED}✘${RESET} $*" >&2; }
warn() { echo -e "${YELLOW}⚠${RESET} $*"; }

# ── Docker stack management ─────────────────────────────────────────────────

stack_up() {
    log "Starting Sockudo container..."
    sudo docker compose -f "$DOCKER_COMPOSE_FILE" up -d --wait
    ok "Sockudo container is up"
}

stack_down() {
    log "Stopping Sockudo container..."
    sudo docker compose -f "$DOCKER_COMPOSE_FILE" down -v --remove-orphans 2>/dev/null || true
    ok "Sockudo container stopped"
}

# ── Ollama model pull (local, not Docker) ───────────────────────────────────

pull_model() {
    local model="${1:-$TEST_MODEL}"
    log "Pulling model '$model' into local Ollama (this may take a while)..."
    ollama pull "$model"
    ok "Model '$model' is available in Ollama"
}

# ── Health checks ──────────────────────────────────────────────────────────

wait_for_sockudo() {
    log "Waiting for Sockudo to be healthy..."
    for i in $(seq 1 30); do
        if curl -sf "$SOCKUDO_URL/up/$SOCKUDO_APP_ID" >/dev/null 2>&1; then
            ok "Sockudo is healthy"
            return 0
        fi
        sleep 1
    done
    fail "Sockudo did not become healthy in 30s"
    return 1
}

wait_for_ollama() {
    log "Waiting for local Ollama to be healthy..."
    for i in $(seq 1 15); do
        if curl -sf "$OLLAMA_URL/api/tags" >/dev/null 2>&1; then
            ok "Ollama is healthy"
            return 0
        fi
        sleep 1
    done
    fail "Ollama did not respond at $OLLAMA_URL — is it running? Start it with: ollama serve"
    return 1
}

ensure_model() {
    log "Checking if model '$TEST_MODEL' is available in local Ollama..."
    if ollama list 2>/dev/null | grep -q "$TEST_MODEL"; then
        ok "Model '$TEST_MODEL' already available"
        return 0
    fi
    warn "Model '$TEST_MODEL' not found. Pulling..."
    pull_model "$TEST_MODEL"
}

# ── Run Rust integration tests ──────────────────────────────────────────────

run_tests() {
    log "Running Rust integration tests against live Sockudo..."
    echo ""

    cd "$PROJECT_ROOT"

    # Run the integration test file with --ignored flag (tests are #[ignore]
    # because they require a live Sockudo server)
    if cargo test -p tinyharness-lib --test sockudo_integration -- --ignored --nocapture 2>&1; then
        echo ""
        ok "All Rust integration tests passed!"
        return 0
    else
        echo ""
        fail "Some integration tests failed"
        return 1
    fi
}

# ── CLI ─────────────────────────────────────────────────────────────────────

case "${1:-run}" in
    up)
        stack_up
        wait_for_sockudo
        ;;
    pull)
        wait_for_ollama
        ensure_model
        ;;
    test)
        wait_for_sockudo
        run_tests
        ;;
    down)
        stack_down
        ;;
    run)
        wait_for_ollama
        ensure_model
        stack_up
        wait_for_sockudo
        run_tests
        rc=$?
        stack_down
        exit $rc
        ;;
    *)
        echo "Usage: $0 {up|pull|test|down|run}"
        echo ""
        echo "Commands:"
        echo "  up     Start Sockudo container"
        echo "  pull   Pull test model into local Ollama"
        echo "  test   Run Rust integration tests against live stack"
        echo "  down   Stop and remove Sockudo container"
        echo "  run    Full cycle: pull → up → test → down"
        exit 1
        ;;
esac