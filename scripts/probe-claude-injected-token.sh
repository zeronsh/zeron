#!/usr/bin/env bash
# probe-claude-injected-token.sh — open question for the Cloud credential broker
# (docs/design/cloud-device.md): does a LONG-RUNNING Claude Code process pick up
# a rewritten credential after the access token it started with expires?
#
# The broker writes a vault `claude` grant into the CLI's own
# `$CLAUDE_CONFIG_DIR/.credentials.json` as
#   {"claudeAiOauth": {"accessToken", "refreshToken": "", "expiresAt", ...}}
# (no refresh token — the vault owns it) and rewrites the file ~10 min before
# `expiresAt`. A persistent stream-json session may outlive one access token.
#
# Modes
#   file (default)  Start a persistent `claude --print --input-format stream-json`
#                   session whose credentials file says the token expires in
#                   CLAUDE_PROBE_EXPIRES_IN seconds (default 120). Run turn 1, wait
#                   past that expiry, rewrite the file with token 2 (fresh
#                   expiresAt), run turn 2. PASS = turn 2 succeeds, i.e. the
#                   running process re-read the file instead of trying (and
#                   failing) a refresh with the empty refresh token.
#                   Token 1 and token 2 may be the SAME real token: the probe
#                   tests the CLI's local expiry handling, not the server's.
#   env             Same session, but token 1 rides CLAUDE_CODE_OAUTH_TOKEN (no
#                   credentials file). After turn 1 the file is written with
#                   token 2; then the script waits for YOU to revoke token 1
#                   (or for CLAUDE_PROBE_ENV_WAIT seconds, e.g. past its real
#                   expiry). PASS = turn 2 succeeds after token 1 stopped
#                   working — only meaningful if token 1 really was revoked or
#                   expired. Expected: FAIL (an env token can't rotate in a
#                   running process), which is why the broker uses the file.
#   both            file, then env.
#
# Usage — tokens are supplied by the operator AT RUNTIME, never committed,
# never echoed (this script runs with `set +x` and prints only lengths):
#   CLAUDE_PROBE_TOKEN_1='sk-ant-oat01-…' \
#   [CLAUDE_PROBE_TOKEN_2='sk-ant-oat01-…'] \      # default: token 1
#   [CLAUDE_PROBE_EXPIRES_IN=120] \                # file mode: fake expiry, seconds
#   [CLAUDE_PROBE_ENV_WAIT=0] \                    # env mode: 0 = wait for Enter
#   [CLAUDE_PROBE_MODEL=haiku] \
#   [CLAUDE_PROBE_SUBSCRIPTION=max] \
#   [CLAUDE_BIN=claude] \
#     scripts/probe-claude-injected-token.sh [file|env|both]
#
# Everything happens in a throwaway CLAUDE_CONFIG_DIR (deleted on exit); the
# operator's own ~/.claude login is never read or touched. Needs python3.
# Exit status: 0 = every requested mode passed, 1 = a mode failed, 2 = usage.

set -euo pipefail
set +x

mode="${1:-file}"
case "$mode" in file | env | both) ;; *)
  echo "usage: $0 [file|env|both]" >&2
  exit 2
  ;;
esac

: "${CLAUDE_PROBE_TOKEN_1:?set CLAUDE_PROBE_TOKEN_1 to a real Claude access token (never commit it)}"
token1="$CLAUDE_PROBE_TOKEN_1"
token2="${CLAUDE_PROBE_TOKEN_2:-$CLAUDE_PROBE_TOKEN_1}"
expires_in="${CLAUDE_PROBE_EXPIRES_IN:-120}"
env_wait="${CLAUDE_PROBE_ENV_WAIT:-0}"
model="${CLAUDE_PROBE_MODEL:-haiku}"
subscription="${CLAUDE_PROBE_SUBSCRIPTION:-max}"
claude_bin="${CLAUDE_BIN:-claude}"
turn_timeout=180

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 2; }
command -v "$claude_bin" >/dev/null || { echo "claude CLI not found ($claude_bin)" >&2; exit 2; }
echo "claude: $("$claude_bin" --version 2>/dev/null || echo unknown)"
echo "token 1: ${#token1} chars; token 2: ${#token2} chars$([ "$token1" = "$token2" ] && echo ' (same token)')"

now_ms() { python3 -c 'import time; print(int(time.time() * 1000))'; }

# write_credentials DIR TOKEN EXPIRES_AT_MS — the broker's exact shape.
write_credentials() {
  PROBE_TOKEN="$2" PROBE_EXPIRES="$3" PROBE_SUB="$subscription" python3 - "$1" <<'PY'
import json, os, sys, tempfile
d = sys.argv[1]
creds = {"claudeAiOauth": {
    "accessToken": os.environ["PROBE_TOKEN"],
    "refreshToken": "",
    "expiresAt": int(os.environ["PROBE_EXPIRES"]),
    "scopes": ["user:inference", "user:profile", "user:sessions:claude_code"],
    "subscriptionType": os.environ["PROBE_SUB"],
}}
fd, tmp = tempfile.mkstemp(dir=d)
with os.fdopen(fd, "w") as f:
    json.dump(creds, f)
os.chmod(tmp, 0o600)
os.replace(tmp, os.path.join(d, ".credentials.json"))
PY
}

# send_turn N — one user message on the persistent session's stdin.
send_turn() {
  python3 -c 'import json,sys; print(json.dumps({"type":"user","message":{"role":"user","content":"This is an automated credential probe. Reply with exactly: PROBE-%s" % sys.argv[1]}}))' "$1" >&3
}

# wait_result N — wait for the Nth `result` frame; print its verdict line.
wait_result() {
  local n="$1" deadline=$((SECONDS + turn_timeout))
  while [ "$(grep -c '"type":"result"' "$out" 2>/dev/null || true)" -lt "$n" ]; do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "claude exited before turn $n finished"
      return 1
    fi
    if [ "$SECONDS" -ge "$deadline" ]; then
      echo "timed out waiting for turn $n"
      return 1
    fi
    sleep 1
  done
  python3 - "$out" "$n" <<'PY'
import json, sys
results = [json.loads(l) for l in open(sys.argv[1]) if '"type":"result"' in l]
r = results[int(sys.argv[2]) - 1]
text = str(r.get("result", ""))[:200].replace("\n", " ")
ok = not r.get("is_error") and f"PROBE-{sys.argv[2]}" in text
print(("ok" if ok else "error") + f": subtype={r.get('subtype')} is_error={r.get('is_error')} result={text!r}")
sys.exit(0 if ok else 1)
PY
}

run_mode() {
  local which="$1"
  work="$(mktemp -d)"
  out="$work/out.jsonl"
  local cfg="$work/claude"
  mkdir -p "$cfg" && chmod 700 "$cfg"
  echo '{"hasCompletedOnboarding": true}' >"$cfg/.claude.json"
  mkfifo "$work/in"

  # `env` takes every -u before any NAME=value.
  local -a env_args=(-u ANTHROPIC_API_KEY -u ANTHROPIC_AUTH_TOKEN)
  if [ "$which" = file ]; then
    write_credentials "$cfg" "$token1" $(($(now_ms) + expires_in * 1000))
    env_args+=(-u CLAUDE_CODE_OAUTH_TOKEN CLAUDE_CONFIG_DIR="$cfg")
  else
    env_args+=(CLAUDE_CONFIG_DIR="$cfg" CLAUDE_CODE_OAUTH_TOKEN="$token1")
  fi
  env "${env_args[@]}" \
    "$claude_bin" --print --input-format stream-json --output-format stream-json \
    --verbose --model "$model" <"$work/in" >"$out" 2>"$work/err.log" &
  pid=$!
  exec 3>"$work/in"

  echo "[$which] turn 1…"
  send_turn 1
  if ! wait_result 1; then
    echo "[$which] FAIL: turn 1 did not succeed — check the token (stderr: $(tail -c 300 "$work/err.log" | tr '\n' ' '))"
    return 1
  fi

  if [ "$which" = file ]; then
    echo "[$which] waiting ${expires_in}s + 15s for the injected token's expiresAt to pass…"
    sleep $((expires_in + 15))
    write_credentials "$cfg" "$token2" $(($(now_ms) + 3600 * 1000))
    echo "[$which] rewrote .credentials.json with token 2"
  else
    write_credentials "$cfg" "$token2" $(($(now_ms) + 3600 * 1000))
    if [ "$env_wait" -gt 0 ]; then
      echo "[$which] wrote token 2 to .credentials.json; waiting ${env_wait}s for token 1 to stop working…"
      sleep "$env_wait"
    else
      echo "[$which] wrote token 2 to .credentials.json."
      read -r -p "[$which] Revoke token 1 now (or let it expire), then press Enter… " _ </dev/tty
    fi
  fi

  echo "[$which] turn 2…"
  send_turn 2
  if wait_result 2; then
    echo "[$which] PASS: the running process kept working with the rewritten credential"
    return 0
  fi
  echo "[$which] FAIL: turn 2 failed (stderr tail: $(tail -c 300 "$work/err.log" | tr '\n' ' '))"
  return 1
}

cleanup() {
  exec 3>&- 2>/dev/null || true
  [ -n "${pid:-}" ] && kill "$pid" 2>/dev/null || true
  [ -n "${work:-}" ] && rm -rf "$work"
}

trap cleanup EXIT INT TERM
status=0
modes=("$mode")
[ "$mode" = both ] && modes=(file env)
for m in "${modes[@]}"; do
  pid=""
  work=""
  if run_mode "$m"; then :; else status=1; fi
  cleanup
done
exit "$status"
