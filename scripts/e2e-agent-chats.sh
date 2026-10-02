#!/usr/bin/env bash
# Two-device e2e for agent-spawned chats: real edge (wrangler dev, dev auth),
# two headless engines (one user, two devices), and the zeron-mcp
# agent_chats_e2e example driving real `zeron mcp` stdio servers:
#
#   the coordinator chat's agent on engine A spawns a top-level chat and a
#   side chat on engine B; B's engine runs both (mock harness); list_chats /
#   read_chat agree from both engines; follow-ups, interrupts, validation and
#   the spawn-depth guard are exercised across the two devices.
#
# Usage: scripts/e2e-agent-chats.sh [evidence.json]
# Env:   ZERON_E2E_EDGE_PORT (default 27740), ZERON_E2E_KEEP_LOGS=1 to keep logs,
#        ZERON_E2E_AGENT_MODEL=opencode/<model> (+ ZERON_E2E_AGENT_HARNESS, default
#        opencode) to also have a real agent on A make the create_chat call.
# Ports/dirs differ from e2e-smoke.sh so both can run side by side.

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
command -v cargo >/dev/null 2>&1 || PATH="$HOME/.cargo/bin:$PATH"
EDGE_PORT="${ZERON_E2E_EDGE_PORT:-27740}"
EDGE_URL="http://localhost:${EDGE_PORT}"
TOKEN="alice@org1"
ORG="org1"
A_PORT=27851
B_PORT=27852
A_DIR=/tmp/e2e-agent-chats-a
B_DIR=/tmp/e2e-agent-chats-b
EVIDENCE="${1:-}"
LOG_DIR="$(mktemp -d /tmp/zeron-e2e-agent-chats-logs.XXXXXX)"
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"

EDGE_PID=""
A_PID=""
B_PID=""
STATUS=1

cleanup() {
  for pid in "$A_PID" "$B_PID"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid" 2>/dev/null || true
    fi
  done
  [[ -n "$EDGE_PID" ]] && kill -- -"$EDGE_PID" 2>/dev/null || true
  sleep 1
  for pid in "$A_PID" "$B_PID"; do
    [[ -n "$pid" ]] && kill -9 "$pid" 2>/dev/null || true
  done
  [[ -n "$EDGE_PID" ]] && kill -9 -- -"$EDGE_PID" 2>/dev/null || true
  rm -rf "$A_DIR" "$B_DIR"
  if [[ "$STATUS" -ne 0 ]]; then
    echo "--- engine A log (tail) ---"; tail -n 40 "$LOG_DIR/engine-a.log" 2>/dev/null || true
    echo "--- engine B log (tail) ---"; tail -n 40 "$LOG_DIR/engine-b.log" 2>/dev/null || true
    echo "--- edge log (tail) ---"; tail -n 40 "$LOG_DIR/edge.log" 2>/dev/null || true
  fi
  if [[ "${ZERON_E2E_KEEP_LOGS:-0}" != "1" ]]; then
    rm -rf "$LOG_DIR"
  else
    echo "logs kept in $LOG_DIR"
  fi
}
trap cleanup EXIT

wait_for() { # wait_for <description> <timeout_s> <command...>
  local what="$1" timeout="$2"; shift 2
  local waited=0
  until "$@" >/dev/null 2>&1; do
    sleep 1
    waited=$((waited + 1))
    if [[ "$waited" -ge "$timeout" ]]; then
      echo "FAIL: timed out waiting for $what" >&2
      exit 1
    fi
  done
}

# ── 1. Edge worker (wrangler dev, dev auth: bearer == user@org) ────────────────
if curl -sf -m 3 "$EDGE_URL/health" | grep -q '"auth":"dev"'; then
  echo "edge: reusing healthy dev-mode worker on :$EDGE_PORT"
else
  echo "edge: starting wrangler dev on :$EDGE_PORT"
  set -m
  bash -c "cd '$ROOT/edge' && exec npx wrangler dev --port '$EDGE_PORT' --var AUTH_MODE:dev" \
    >"$LOG_DIR/edge.log" 2>&1 &
  EDGE_PID=$!
  set +m
  wait_for "edge /health" 90 curl -sf -m 3 "$EDGE_URL/health"
fi

# ── 2. Build the binaries ──────────────────────────────────────────────────────
echo "build: zeron + agent_chats_e2e"
(cd "$ROOT" && cargo build -q -p zeron)
(cd "$ROOT" && cargo build -q -p zeron-mcp --example agent_chats_e2e)
ZERON="$TARGET/debug/zeron"
DRIVER="$TARGET/debug/examples/agent_chats_e2e"

# ── 3. Two headless engines, one user, two devices ─────────────────────────────
rm -rf "$A_DIR" "$B_DIR"
mkdir -p "$A_DIR" "$B_DIR"

start_engine() { # start_engine <data_dir> <ipc_port> <name> <log>
  ZERON_DATA_DIR="$1" ZERON_IPC_PORT="$2" ZERON_DEVICE_NAME="$3" \
    ZERON_EDGE_URL="$EDGE_URL" ZERON_EDGE_TOKEN="$TOKEN" ZERON_ORG_ID="$ORG" \
    ZERON_HARNESS=mock RUST_LOG=info \
    "$ZERON" headless >"$4" 2>&1 &
}

start_engine "$A_DIR" "$A_PORT" "laptop-a" "$LOG_DIR/engine-a.log"; A_PID=$!
start_engine "$B_DIR" "$B_PORT" "gpu-box-b" "$LOG_DIR/engine-b.log"; B_PID=$!

wait_for "engine A ipc :$A_PORT" 60 bash -c "exec 3<>/dev/tcp/127.0.0.1/$A_PORT"
wait_for "engine B ipc :$B_PORT" 60 bash -c "exec 3<>/dev/tcp/127.0.0.1/$B_PORT"
echo "engines: A pid=$A_PID ipc=:$A_PORT  B pid=$B_PID ipc=:$B_PORT"

# ── 4. Drive the agent through real `zeron mcp` servers on both engines ───────
if "$DRIVER" "$ZERON" "$A_PORT" "$B_PORT" $EVIDENCE; then
  STATUS=0
else
  STATUS=$?
fi
exit "$STATUS"
