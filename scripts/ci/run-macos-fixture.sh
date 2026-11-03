#!/bin/bash
# Runs a native macOS GUI fixture (browser-fixture / preview-fixture) with guards against a flaky,
# heavily loaded GitHub runner VM (3 vCPU). The fixture binary, its arguments and assertions are
# untouched; only the wrapper changes:
#  * bounded retries: each attempt has its own timeout (instead of one long hang) and a failed attempt
#    is reported with a ::warning:: so flakes stay visible. A real regression fails every attempt.
# The preview fixture deliberately gets no /etc/hosts entries: the app routes *.localhost preview
# hostnames through WebKit's per-domain proxy, and an NSURLErrorDomain -1003 means that path failed,
# which is what the fixture exists to catch.
# usage: run-macos-fixture.sh <fixture-binary> <capture-dir>
set -uo pipefail
ROOT="${ZERON_ROOT:-$(cd "$(dirname "$0")/../.." && pwd)}"
BINARY="$1"
OUT="$2"
ATTEMPTS="${FIXTURE_ATTEMPTS:-3}"
ATTEMPT_TIMEOUT="${FIXTURE_ATTEMPT_TIMEOUT:-70}"

# macOS has no coreutils `timeout`; do it in bash.
run_with_timeout() {
  local secs=$1; shift
  "$@" &
  local pid=$!
  ( sleep "$secs"; kill -TERM "$pid" 2>/dev/null; sleep 3; kill -KILL "$pid" 2>/dev/null ) &
  local watchdog=$!
  wait "$pid"; local rc=$?
  kill "$watchdog" 2>/dev/null; wait "$watchdog" 2>/dev/null
  return $rc
}

for attempt in $(seq 1 "$ATTEMPTS"); do
  rm -rf "$OUT"; mkdir -p "$OUT"
  echo "=== $(basename "$BINARY") attempt $attempt/$ATTEMPTS ==="
  if run_with_timeout "$ATTEMPT_TIMEOUT" bash "$ROOT/scripts/run-macos-browser-fixture.sh" "$BINARY" "$OUT" && test -f "$OUT/result.txt"; then
    echo "$(basename "$BINARY") passed on attempt $attempt"
    [ "$attempt" -gt 1 ] && echo "::warning::$(basename "$BINARY") needed $attempt attempts (flaky on this runner)"
    exit 0
  fi
  echo "::warning::$(basename "$BINARY") attempt $attempt failed"
  # Is the preview proxy listening? (Nothing for the browser fixture.)
  lsof -nP -iTCP:7331 -sTCP:LISTEN || true
  mkdir -p "$OUT.failed-$attempt"
  cp -R "$OUT"/. "$OUT.failed-$attempt"/ 2>/dev/null || true
  pkill -f 'vite/bin/vite.js' 2>/dev/null || true
  pkill -f 'node api.js' 2>/dev/null || true
  pkill -f 'Fixture.app/Contents/MacOS/fixture' 2>/dev/null || true
  sleep 2
done
echo "$(basename "$BINARY") failed after $ATTEMPTS attempts"
exit 1
