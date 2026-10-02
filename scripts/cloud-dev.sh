#!/usr/bin/env bash
# A Cloud to click through in the desktop app, with this checkout's code:
#   - `wrangler dev`: edge + vault, dev auth;
#   - a Zeron window (debug build) signed in as a fresh dev account.
# Your running app and ~/.zeron are untouched: everything has its own data
# dir and ports.
#
# MODE=local (default): fake sandboxes. A supervisor runs a real
#   `zeron headless` on this computer for every sandbox that is up (own HOME
#   and data dir), so each chat run on Cloud gets its own session machine.
#   Repositories check out under $STATE/projects (not /home/user) and
#   sessions of one repository share that folder.
# MODE=boat: real Boat sandboxes. The edge is exposed through a temporary
#   Cloudflare quick tunnel (sandboxes must reach it), and serves this
#   checkout's Linux build — run scripts/build-linux-dev.sh first — as the
#   release they install. Needs BOAT_API_KEY in edge/.dev.vars. Sandboxes are
#   billed; on exit every session machine is deleted (the account, its
#   connections and the window's projects stay for the next run).
#   The tunnel URL is public and dev auth trusts any bearer: keep the run
#   short.
#
# Needs edge/vault/.dev.vars: VAULT_KEK, and GITHUB_APP_* for GitHub (see
# edge/vault/README.md, "The GitHub App").
#
# In the window: Settings → Cloud → turn Cloud on, connect GitHub and sign in
# to Codex / Claude for Cloud; then add a project whose GitHub repository the
# Zeron GitHub App is installed on, pick Cloud in the checkout menu of a new
# chat there, and send.
#
# Run: scripts/cloud-dev.sh  |  MODE=boat scripts/cloud-dev.sh   (Ctrl-C stops)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
MODE="${MODE:-local}"
EDGE_PORT="${EDGE_PORT:-27699}"
GUI_PORT="${GUI_PORT:-27861}"
SESSION_PORT_BASE="${SESSION_PORT_BASE:-27900}"
RUN="$(date +%s)"
ORG_ID="org_clouddev"
if [[ "$MODE" == boat ]]; then
  # One account and window across runs: GitHub, Claude/Codex and the
  # projects you added stay (the edge and vault keep their state in
  # edge/.wrangler); only session machines are deleted on exit.
  STATE="${STATE:-/tmp/zeron-cloud-dev/boat}"
  USER_ID="${USER_ID:-user_cdboat}"
else
  # Fake sandboxes live in the edge's memory, so each run starts fresh.
  STATE="${STATE:-/tmp/zeron-cloud-dev/$RUN}"
  USER_ID="user_cd$RUN"
fi
BEARER="$USER_ID@$ORG_ID"
EDGE="http://127.0.0.1:$EDGE_PORT"
WRANGLER_PATTERN="wrangler dev -c wrangler.jsonc -c vault/wrangler.jsonc --port $EDGE_PORT"

case "$MODE" in local | boat) ;; *) echo "MODE must be local or boat" >&2; exit 1 ;; esac
grep -q '^VAULT_KEK=' "$ROOT/edge/vault/.dev.vars" 2>/dev/null || {
  echo "edge/vault/.dev.vars needs VAULT_KEK (see edge/vault/README.md)" >&2
  exit 1
}
if [[ "$MODE" == boat ]]; then
  grep -q '^BOAT_API_KEY=' "$ROOT/edge/.dev.vars" 2>/dev/null || {
    echo "MODE=boat needs BOAT_API_KEY in edge/.dev.vars" >&2
    exit 1
  }
  VERSION="$(cat "$ROOT/target/package-dev/latest.txt" 2>/dev/null || true)"
  TARBALL="$ROOT/target/package-dev/zeron-$VERSION-linux-x86_64.tar.gz"
  [[ -n "$VERSION" && -f "$TARBALL" ]] || {
    echo "MODE=boat needs a Linux build: run scripts/build-linux-dev.sh" >&2
    exit 1
  }
  command -v cloudflared >/dev/null || { echo "MODE=boat needs cloudflared" >&2; exit 1; }
fi
for port in $EDGE_PORT $GUI_PORT; do
  if lsof -nP -iTCP:"$port" -sTCP:LISTEN >/dev/null 2>&1; then
    echo "port $port is in use" >&2
    exit 1
  fi
done
mkdir -p "$STATE/gui" "$STATE/projects" "$STATE/sessions" "$STATE/bin"

log() { printf '[cloud-dev %s] %s\n' "$(date +%H:%M:%S)" "$*"; }
session_ids() {
  curl -s "$EDGE/cloud/$ORG_ID/sessions" -H "Authorization: Bearer $BEARER" |
    python3 -c 'import sys,json; [print(s["chatId"]) for s in json.load(sys.stdin).get("sessions", [])]' 2>/dev/null || true
}
GUI_PID=""
TUNNEL_PID=""
cleanup() {
  log "stopping"
  if [[ "$MODE" == boat ]] && curl -sf "$EDGE/health" >/dev/null 2>&1; then
    # Delete every session machine (final usage is metered first); the
    # workflows run inside wrangler, so wait for them.
    log "deleting this account's Boat session machines"
    for chat in $(session_ids); do
      curl -s -X DELETE "$EDGE/cloud/$ORG_ID/sessions/$chat" -H "Authorization: Bearer $BEARER" >/dev/null || true
    done
    for _ in $(seq 1 60); do
      [[ -z "$(session_ids)" ]] && break
      sleep 3
    done
    log "session machines left: $(session_ids | wc -l | tr -d ' ')"
  fi
  for pidfile in "$STATE"/sessions/*/pid; do
    [[ -f "$pidfile" ]] && kill "$(cat "$pidfile")" 2>/dev/null || true
  done
  if [[ -n "$GUI_PID" ]]; then
    kill "$GUI_PID" 2>/dev/null || true
    for _ in 1 2 3 4 5; do kill -0 "$GUI_PID" 2>/dev/null || break; sleep 1; done
    kill -9 "$GUI_PID" 2>/dev/null || true
  fi
  [[ -n "$TUNNEL_PID" ]] && kill "$TUNNEL_PID" 2>/dev/null || true
  pkill -f "$WRANGLER_PATTERN" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 0' INT TERM

log "building zeron"
(cd "$ROOT" && cargo build -q -p zeron --bin zeron) > "$STATE/build.log" 2>&1 || {
  tail -40 "$STATE/build.log"
  exit 1
}

PROVIDER=fake
SANDBOX_EDGE="$EDGE"
if [[ "$MODE" == boat ]]; then
  PROVIDER=boat
  log "release $VERSION into the local edge's release bucket"
  for file in "$ROOT/target/package-dev/latest.txt" "$TARBALL"; do
    (cd "$ROOT/edge" && npx wrangler r2 object put "comet-native-releases/$(basename "$file")" \
      --file "$file" --local -c wrangler.jsonc) >> "$STATE/r2.log" 2>&1 || { tail -20 "$STATE/r2.log"; exit 1; }
  done
  log "tunnel to :$EDGE_PORT"
  cloudflared tunnel --no-autoupdate --url "$EDGE" > "$STATE/tunnel.log" 2>&1 &
  TUNNEL_PID=$!
  for _ in $(seq 1 60); do
    SANDBOX_EDGE="$(grep -oE 'https://[a-z0-9-]+\.trycloudflare\.com' "$STATE/tunnel.log" | head -1 || true)"
    [[ -n "$SANDBOX_EDGE" ]] && break
    sleep 1
  done
  [[ -n "$SANDBOX_EDGE" ]] || { tail -20 "$STATE/tunnel.log"; exit 1; }
  log "sandboxes reach the edge at $SANDBOX_EDGE"
fi

# Local session machines are processes here: no /home/user to clone into.
local_vars=()
[[ "$MODE" == local ]] && local_vars=(--var "CLOUD_PROJECTS_ROOT:$STATE/projects")
log "edge + vault on :$EDGE_PORT ($PROVIDER sandboxes)"
(cd "$ROOT/edge" && npx wrangler dev -c wrangler.jsonc -c vault/wrangler.jsonc --port "$EDGE_PORT" \
  --ip 127.0.0.1 --var AUTH_MODE:dev --var "SANDBOX_PROVIDER:$PROVIDER" --var "CLOUD_EDGE_URL:$SANDBOX_EDGE" \
  ${local_vars[@]+"${local_vars[@]}"} > "$STATE/wrangler.log" 2>&1 &)
for _ in $(seq 1 90); do curl -sf "$EDGE/health" >/dev/null && break; sleep 2; done
curl -sf "$EDGE/health" >/dev/null || { tail -30 "$STATE/wrangler.log"; exit 1; }
if [[ "$MODE" == boat ]]; then
  # A fresh quick-tunnel hostname can take a minute or two to resolve.
  for _ in $(seq 1 90); do
    served="$(curl -sf "$SANDBOX_EDGE/releases/latest.txt" || true)"
    [[ "$served" == "$VERSION" ]] && break
    sleep 2
  done
  [[ "$served" == "$VERSION" ]] || { echo "the tunnel doesn't serve release $VERSION (got '$served')" >&2; exit 1; }
  log "the tunnel serves release $VERSION"
fi

log "Zeron window as $USER_ID (data: $STATE/gui)"
gui_env=(ZERON_DATA_DIR="$STATE/gui/data" ZERON_IPC_PORT="$GUI_PORT" ZERON_EDGE_URL="$EDGE"
  ZERON_EDGE_TOKEN="$BEARER" ZERON_ORG_ID="$ORG_ID" ZERON_DEVICE_NAME="Cloud dev"
  ZERON_OPEN_ROUTE=settings/cloud ZERON_NO_LOGIN_SHELL=1 RUST_LOG=warn)
(cd "$STATE/gui" && env "${gui_env[@]}" "$ROOT/target/debug/zeron" > "$STATE/gui/ui.log" 2>&1 &
  echo $! > "$STATE/gui/pid")
GUI_PID="$(cat "$STATE/gui/pid")"

if [[ "$MODE" == boat ]]; then
  log "ready — use the Zeron window; Ctrl-C to stop (deletes session machines)"
  last=""
  while kill -0 "$GUI_PID" 2>/dev/null; do
    now="$(curl -s "$EDGE/cloud/$ORG_ID/sessions" -H "Authorization: Bearer $BEARER" | python3 -c '
import sys, json
try:
    for s in json.load(sys.stdin).get("sessions", []):
        error = " (" + s["error"] + ")" if s.get("error") else ""
        print(s["chatId"][:8] + " " + s["state"] + error)
except Exception:
    pass' 2>/dev/null || true)"
    [[ "$now" != "$last" && -n "$now" ]] && log "sessions: $(tr '\n' ';' <<< "$now")"
    last="$now"
    sleep 5
  done
  log "the Zeron window closed"
  exit 0
fi

# Local session machines have no keychain (real ones run Linux): CLIs that
# look one up through `security` are told "not found" / "failed" and fall
# back to their credential files, instead of macOS asking to create one.
cat > "$STATE/bin/security" <<'SECURITY'
#!/bin/sh
case "$1" in find-generic-password | find-internet-password | delete-generic-password) exit 44 ;; esac
exit 1
SECURITY
chmod +x "$STATE/bin/security"

next_port="$SESSION_PORT_BASE"
alive() { [[ -f "$1/pid" ]] && kill -0 "$(cat "$1/pid")" 2>/dev/null; }

# A session machine: the sandbox's own env (enrollment, chat, repository,
# branch, path), its own HOME and data dir, so credentials and git config
# stay inside it. The data dir survives a stop, like a sandbox's disk.
start_session() {
  local id="$1" dir="$STATE/sessions/$1"
  mkdir -p "$dir/home" "$dir/data"
  if [[ ! -f "$dir/port" ]]; then
    while lsof -nP -iTCP:"$next_port" -sTCP:LISTEN >/dev/null 2>&1; do next_port=$((next_port + 1)); done
    echo "$next_port" > "$dir/port"
    next_port=$((next_port + 1))
  fi
  local -a env_args=()
  while IFS= read -r line; do env_args+=("$line"); done < "$dir/env"
  # GIT_CONFIG_NOSYSTEM: like a clean Linux sandbox, no system git helper
  # (Apple git's osxkeychain) — this machine's keychain is never touched.
  (cd "$dir" && env -i PATH="$STATE/bin:$PATH" HOME="$dir/home" USER="$USER" GIT_CONFIG_NOSYSTEM=1 \
    ZERON_NO_LOGIN_SHELL=1 ZERON_DATA_DIR="$dir/data" \
    ZERON_IPC_PORT="$(cat "$dir/port")" RUST_LOG=info "${env_args[@]}" \
    "$ROOT/target/debug/zeron" headless >> "$dir/engine.log" 2>&1 & echo $! > "$dir/pid")
  log "session machine up: chat $(grep '^ZERON_CLOUD_CHAT=' "$dir/env" | cut -d= -f2) ($id, :$(cat "$dir/port"), log $dir/engine.log)"
}

stop_session() {
  local id="$1" dir="$STATE/sessions/$1"
  kill "$(cat "$dir/pid")" 2>/dev/null || true
  rm -f "$dir/pid"
  log "session machine stopped: $id"
}

log "ready — use the Zeron window; Ctrl-C to stop"
while kill -0 "$GUI_PID" 2>/dev/null; do
  # One line per session sandbox: "<id> <state>"; env written once.
  sandboxes="$(curl -sf "$EDGE/dev/cloud/fake-sandboxes" -H "Authorization: Bearer $BEARER" | python3 -c "
import json, os, sys
root = sys.argv[1]
for s in json.load(sys.stdin).get('sandboxes', []):
    env = s.get('env') or {}
    if not env.get('ZERON_CLOUD_CHAT'):
        continue
    d = os.path.join(root, s['id'])
    os.makedirs(d, exist_ok=True)
    path = os.path.join(d, 'env')
    if not os.path.exists(path):
        with open(path, 'w') as f:
            f.write(''.join(f'{k}={v}\n' for k, v in env.items() if k.startswith('ZERON_')))
    print(s['id'], s['state'])
" "$STATE/sessions" 2>/dev/null || true)"
  while read -r id state; do
    [[ -z "$id" ]] && continue
    dir="$STATE/sessions/$id"
    case "$state" in
      provisioning | ready) alive "$dir" || start_session "$id" ;;
      *) if alive "$dir"; then stop_session "$id"; fi ;;
    esac
  done <<< "$sandboxes"
  # A sandbox that vanished (deleted) stops too.
  for dir in "$STATE"/sessions/*/; do
    id="$(basename "$dir")"
    if alive "$dir" && ! grep -q "^$id " <<< "$sandboxes"; then stop_session "$id"; fi
  done
  sleep 2
done
log "the Zeron window closed"
