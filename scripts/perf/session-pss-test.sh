#!/usr/bin/env bash
# Fixture test for scripts/perf/session-pss.sh. Bash and awk in the tool;
# this driver uses python3 only to parse its JSON.

set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
tool=$root/scripts/perf/session-pss.sh
base=${SESSION_PSS_TEST_DIR:-$HOME/Documents/dev/tmp/zeron-perf/scratch/h13-session-pss}
mkdir -p "$base"
work=$(mktemp -d -p "$base" proc.XXXXXX)
trap 'rm -rf "$work"' EXIT
proc=$work/proc
mkdir -p "$proc"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

# pid ppid name pss(none|anon|kib) exe_target -- argv...
add() {
  local pid=$1 ppid=$2 name=$3 pss=$4 exe=$5
  shift 5
  mkdir -p "$proc/$pid"
  printf 'Name:\t%s\nState:\tS (sleeping)\nPPid:\t%s\n' "$name" "$ppid" >"$proc/$pid/status"
  if [[ $# -gt 0 ]]; then
    printf '%s\0' "$@" >"$proc/$pid/cmdline"
  else
    : >"$proc/$pid/cmdline"
  fi
  if [[ $exe != - ]]; then
    ln -s "$exe" "$proc/$pid/exe"
  fi
  if [[ $pss == none ]]; then
    return
  fi
  if [[ $pss == anon ]]; then
    printf 'Pss_Anon:           99999 kB\nPss_File:            88888 kB\n' >"$proc/$pid/smaps_rollup"
    return
  fi
  # Mode bits say this is readable, but opening it as a file fails.
  if [[ $pss == dir ]]; then
    mkdir -p "$proc/$pid/smaps_rollup"
    return
  fi
  printf 'Pss_Anon:           99999 kB\nPss:              %8d kB\nPss_File:                1 kB\n' "$pss" >"$proc/$pid/smaps_rollup"
}

long=$(printf 'x%.0s' {1..220})

# Session 10: claude, with the shapes the live machine actually has.
add 10 1 claude 10240 /opt/claude claude --print
add 11 10 claude 1024 /opt/claude claude --resume
# Process title is one argv and contains a slash. Comm is not "npm …",
# so the title itself has to carry the group.
add 20 10 node 2048 /usr/bin/node "npm exec @playwright/mcp@latest"
add 21 20 node-MainThread 4096 /usr/bin/node node /opt/pkg/node_modules/.bin/playwright-mcp
add 22 21 node-MainThread 512 /usr/bin/node node /opt/chrome-devtools-mcp/build/src/telemetry/watchdog/main.js --parent-pid=21
add 30 10 zeron 3072 /usr/bin/zeron zeron mcp 'Claude "Code"'
add 31 10 bash 1024 /usr/bin/bash bash -c "$long"
add 40 10 mystery none - mystery
add 41 40 bash 1024 /usr/bin/bash bash -c echo
add 70 10 npx none /usr/bin/npx npx -y some-mcp
add 71 70 serverbin 1024 /usr/bin/serverbin /usr/bin/serverbin
add 80 10 ghost dir /usr/bin/ghost ghost

# A second top-level harness.
add 60 1 grok 2048 /opt/grok grok agent
add 61 60 reins 1024 /usr/bin/reins reins mcp --via Codex

# Claude ships ugrep as the claude binary with argv0 ugrep. That is a tool.
add 32 31 claude 1024 /opt/claude ugrep -G --hidden
# Not harnesses: grok-bot, a comm/exe mismatch, and a top-level ugrep whose
# exe is the claude binary (must not become its own session).
add 50 1 grok-bot 50000 /opt/grok-bot "/opt/Grok Bot/grok-bot" --type=zygote
add 90 1 claude 40000 /usr/bin/ugrep ugrep -G --hidden
add 91 1 claude 40000 /opt/claude ugrep -G --hidden
mkdir -p "$proc/123abc"
printf 'nope\n' >"$proc/123abc/status"

run() {
  local err=$work/err
  set +e
  stdout=$(SESSION_PSS_PROC=$proc "$tool" "$@" 2>"$err")
  code=$?
  set -e
  err_text=$(cat "$err")
}

run
[[ $code -eq 0 ]] || fail "default exit $code"
[[ -z $err_text ]] || fail "stderr noise: $err_text"
echo "$stdout" | grep -E '^session 10 claude$' >/dev/null || fail "missing session 10"
echo "$stdout" | grep -E '^session 60 grok$' >/dev/null || fail "missing session 60"
echo "$stdout" | grep -E '^session (11|50|90) ' >/dev/null && fail "nested or non-harness listed as its own session"
echo "$stdout" | grep -E '^  harness +2 proc +11\.0 MiB$' >/dev/null || fail "harness total"
echo "$stdout" | grep -E '^  mcp servers +3 proc +8\.0 MiB$' >/dev/null || fail "mcp total"
echo "$stdout" | grep -E '^  npm/npx +1 proc +2\.0 MiB$' >/dev/null || fail "npm total"
echo "$stdout" | grep -E '^  other +4 proc +3\.5 MiB$' >/dev/null || fail "other total"
echo "$stdout" | grep -E '^  total +10 proc +24\.5 MiB$' >/dev/null || fail "session total"
echo "$stdout" | grep -E '^  skipped 3$' >/dev/null || fail "skipped count"
echo "$stdout" | grep -E '^all sessions +2 sessions +27\.5 MiB$' >/dev/null || fail "grand total"
echo "$stdout" | grep -F 'npm exec @playwright/mcp@latest' >/dev/null || fail "npm title missing"
echo "$stdout" | grep -F 'serverbin' >/dev/null || fail "npx child not reported"
echo "$stdout" | grep -E 'grok-bot' >/dev/null && fail "grok-bot included"
echo "$stdout" | grep -E '^session 91 ' >/dev/null && fail "ugrep exe became a session"
# Truncation keeps the record bounded and still marks that it was cut.
echo "$stdout" | grep -F 'xxx' >/dev/null || fail "long command lost"
echo "$stdout" | grep -F '...' >/dev/null || fail "long command not truncated"
# Watchdog stays out of the mcp group: the mcp block ends before 0.5 MiB.
awk '
  /^  mcp servers/ { inm=1; next }
  /^  npm\/npx/ { inm=0 }
  inm && /0\.5/ && /watchdog/ { bad=1 }
  END { exit bad ? 1 : 0 }
' <<<"$stdout" || fail "watchdog counted as an mcp server"
awk '
  /^  other/ { ino=1; next }
  /^  total/ { ino=0 }
  ino && /watchdog/ { seen=1 }
  END { exit seen ? 0 : 1 }
' <<<"$stdout" || fail "watchdog not in other"

run --json
[[ $code -eq 0 ]] || fail "json exit $code"
printf '%s\n' "$stdout" >"$work/all.json"
run 10 --json
printf '%s\n' "$stdout" >"$work/a.json"
run --json 10
printf '%s\n' "$stdout" >"$work/b.json"
cmp -s "$work/a.json" "$work/b.json" || fail "--json position changed output"

python3 - "$work/all.json" "$work/a.json" <<'PY'
import json, sys
all_doc = json.load(open(sys.argv[1]))
one = json.load(open(sys.argv[2]))
assert all_doc["session_count"] == 2
assert all_doc["total_pss_kib"] == 28160
assert all_doc["total_pss_mib"] == 27.5
assert all_doc["processes"] == 12
pids = [s["pid"] for s in all_doc["sessions"]]
assert pids == [10, 60], pids
s10, s60 = all_doc["sessions"]
assert s10["kind"] == "claude"
assert s10["total_pss_kib"] == 25088
assert s10["processes"] == 10
assert s10["skipped"] == 3
g = s10["groups"]
assert g["harness"]["pss_kib"] == 11264
assert [p["pid"] for p in g["harness"]["processes"]] == [10, 11]
assert g["mcp"]["pss_kib"] == 8192
assert [p["pid"] for p in g["mcp"]["processes"]] == [21, 30, 71]
assert g["npm"]["pss_kib"] == 2048
assert [p["pid"] for p in g["npm"]["processes"]] == [20]
assert g["other"]["pss_kib"] == 3584
assert [p["pid"] for p in g["other"]["processes"]] == [31, 32, 41, 22]
assert all(p["pid"] != 32 or p["command"].startswith("ugrep") for p in g["other"]["processes"])
for proc in g["other"]["processes"]:
    if proc["pid"] == 31:
        assert proc["command"].endswith("...")
        assert len(proc["command"]) == 160
assert s60["kind"] == "grok"
assert s60["groups"]["mcp"]["pss_kib"] == 1024
assert s60["groups"]["npm"]["pss_kib"] == 0
assert s60["skipped"] == 0
# Quoted argv survived JSON escaping.
assert g["mcp"]["processes"][1]["command"] == 'zeron mcp Claude "Code"'
assert one["session_count"] == 1
assert one["sessions"][0]["pid"] == 10
assert one["total_pss_kib"] == 25088
print("json ok")
PY

run 11
[[ $code -eq 0 ]] || fail "pid 11 exit $code"
echo "$stdout" | grep -E '^session 11 claude$' >/dev/null || fail "pid 11 session"
echo "$stdout" | grep -E '^session 10 ' >/dev/null && fail "pid 11 included its parent"
echo "$stdout" | grep -E '^  total +1 proc +1\.0 MiB$' >/dev/null || fail "pid 11 total"

run 21
echo "$stdout" | grep -E '^  mcp servers +1 proc +4\.0 MiB$' >/dev/null || fail "server root"
echo "$stdout" | grep -E '^  other +1 proc +0\.5 MiB$' >/dev/null || fail "server helper"
echo "$stdout" | grep -E '^  total +2 proc +4\.5 MiB$' >/dev/null || fail "server tree total"

run 999999
[[ $code -eq 0 ]] || fail "missing pid exit $code"
[[ -z $stdout ]] || fail "missing pid produced stdout: $stdout"
echo "$err_text" | grep -F 'not readable' >/dev/null || fail "missing pid silent"

run --json 999999
[[ $code -eq 0 ]] || fail "missing pid json exit $code"
python3 -c 'import json,sys; d=json.loads(sys.argv[1]); assert d["session_count"]==0 and d["total_pss_kib"]==0' "$stdout"

run --nope
[[ $code -eq 2 ]] || fail "bad flag exit $code"
run 10 11
[[ $code -eq 2 ]] || fail "two pids exit $code"
run abc
[[ $code -eq 2 ]] || fail "non-numeric exit $code"

echo "session-pss-test: ok"
