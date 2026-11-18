#!/usr/bin/env bash
# The plugin store end to end, on this machine: the real edge worker (its
# bundle, in Miniflare, with the store's D1 migrations, local R2 and a
# WorkOS fake: edge.mjs) and real `zeron headless` engines and the real CLI
# (crates/engine/tests/plugin_store_e2e.rs), each on scratch folders.
# Prints `PASS <step> …` / `FAIL <step> …` and exits non-zero on a failure.
# Leaves nothing running; touches no real data (a scratch HOME and data
# folder for every engine, nothing persisted by the edge, no network: the
# edge's outbound fetches other than the WorkOS fake are refused).
#
# Usage: scripts/plugin-store-e2e/run.sh
# Env:
#   E2E_EDGE_DIR   the edge to run (default: this checkout's edge/)
#   CARGO_TARGET_DIR  as cargo takes it
#   E2E_KEEP=1     keep the scratch folder (engine logs) afterwards

set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
EDGE_DIR="$(cd "${E2E_EDGE_DIR:-$ROOT/edge}" && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/zeron-store-e2e.XXXXXX")"
EDGE_PID=""

cleanup() {
  local status=$?
  if [[ -n "$EDGE_PID" ]] && kill -0 "$EDGE_PID" 2>/dev/null; then
    kill "$EDGE_PID" 2>/dev/null || true
    wait "$EDGE_PID" 2>/dev/null || true
  fi
  if [[ "${E2E_KEEP:-}" == 1 ]]; then
    echo "scratch kept: $WORK"
  else
    rm -rf "$WORK"
  fi
  exit "$status"
}
trap cleanup EXIT INT TERM

echo "== the edge: $EDGE_DIR"
[[ -d "$EDGE_DIR/node_modules" ]] || (cd "$EDGE_DIR" && npm ci --no-audit --no-fund)
# (wrangler wants the assets folder to exist; it's not committed.)
mkdir -p "$EDGE_DIR/public"
(cd "$EDGE_DIR" && npx wrangler deploy --dry-run --outdir "$WORK/bundle" > "$WORK/bundle.log" 2>&1) ||
  { cat "$WORK/bundle.log"; echo "FAIL 0 the edge didn't bundle"; exit 1; }

echo "== zeron"
cargo build -p zeron --bin zeron
TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
ZERON_BIN="$TARGET/debug/zeron"

node "$ROOT/scripts/plugin-store-e2e/edge.mjs" "$EDGE_DIR" "$WORK/bundle" "$WORK/edge.json" > "$WORK/edge.log" 2>&1 &
EDGE_PID=$!
for _ in $(seq 1 120); do
  [[ -f "$WORK/edge.json" ]] && break
  kill -0 "$EDGE_PID" 2>/dev/null || { cat "$WORK/edge.log"; echo "FAIL 0 the edge didn't start"; exit 1; }
  sleep 0.25
done
[[ -f "$WORK/edge.json" ]] || { cat "$WORK/edge.log"; echo "FAIL 0 the edge didn't start in time"; exit 1; }
EDGE_URL="$(node -e 'console.log(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).url)' "$WORK/edge.json")"
echo "== the edge serves $EDGE_URL"

mkdir -p "$WORK/devices"
set +e
E2E_EDGE_JSON="$WORK/edge.json" E2E_ZERON_BIN="$ZERON_BIN" E2E_WORK_DIR="$WORK/devices" \
  cargo test -p zeron-engine --test plugin_store_e2e -- --ignored --nocapture --test-threads=1 2>&1 | tee "$WORK/run.log"
status=${PIPESTATUS[0]}
set -e
echo
echo "== summary"
grep -oE '(PASS|FAIL) [0-9]+ .*' "$WORK/run.log" || true
if [[ $status -ne 0 ]]; then
  echo "== the edge's log"
  tail -40 "$WORK/edge.log"
  echo "== engine logs in $WORK/devices/*/engine.log (E2E_KEEP=1 keeps them)"
fi
exit "$status"
