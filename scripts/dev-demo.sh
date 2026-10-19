#!/usr/bin/env bash
# One-command demo: boots a seeded engine daemon + the headed app, offline.
# Made for judging look & feel with real input — no edge, no auth needed.
#
#   scripts/dev-demo.sh            # build, seed demo data, open the app
#   scripts/dev-demo.sh --slow     # pace mock streams (~10s) to watch streaming
#
# Everything lives under /tmp/zeron-demo-*; re-runs reuse it. Ctrl-C cleans up.
# A second demo beside a running one (another worktree's) picks its own
# state and port: ZERON_DEMO_DIR=/tmp/zeron-demo-b ZERON_DEMO_PORT=27931.
set -euo pipefail
cd "$(dirname "$0")/.."

DEMO_DIR="${ZERON_DEMO_DIR:-/tmp/zeron-demo}"
DAEMON_DIR="$DEMO_DIR-daemon"
UI_DIR="$DEMO_DIR-ui"
IPC="${ZERON_DEMO_PORT:-27921}"
# Demo projects live in ~/github when it exists, else in throwaway folders
# (a space must point at a real folder).
PROJECTS_DIR="$HOME/github"
[[ -d "$PROJECTS_DIR" ]] || PROJECTS_DIR=/tmp/zeron-demo-projects
DELAY=""
[[ "${1:-}" == "--slow" ]] && DELAY=350

echo "▸ building (first run takes a few minutes)…"
cargo build -p zeron -q

echo "▸ starting engine daemon on :$IPC"
env ZERON_DATA_DIR="$DAEMON_DIR" ZERON_IPC_PORT=$IPC ZERON_HARNESS=mock \
  ${DELAY:+ZERON_MOCK_DELAY_MS=$DELAY} RUST_LOG=warn \
  ./target/debug/zeron headless &
DAEMON_PID=$!
trap 'kill $DAEMON_PID 2>/dev/null || true' EXIT
for _ in $(seq 1 40); do
  (exec 3<>/dev/tcp/127.0.0.1/$IPC) 2>/dev/null && { exec 3>&-; break; }
  sleep 0.25
done

probe() { cargo run -q -p zeron-rpc --example rpc_probe -- "ws://127.0.0.1:$IPC" "$@"; }

if [[ ! -f "$DAEMON_DIR/.demo-seeded" ]]; then
  echo "▸ seeding demo chats"
  DEV=$(probe LocalDevice '{}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["deviceId"])')
  # One space per demo folder, created up-front (chats join by space id);
  # a folder already registered keeps its first space, so each is listed once.
  declare -A SPACES=()
  for project in zeron soccertcg aether; do
    sid=$(uuidgen | tr 'A-Z' 'a-z')
    mkdir -p "$PROJECTS_DIR/$project"
    probe Mutate "{\"op\":\"createSpace\",\"spaceId\":\"$sid\",\"deviceId\":\"$DEV\",\"path\":\"$PROJECTS_DIR/$project\"}" >/dev/null
    SPACES[$project]="$sid"
  done
  seed() { # title project branch age_hours run
    local id; id=$(uuidgen | tr 'A-Z' 'a-z')
    local sid="${SPACES[$2]}"
    probe Mutate "{\"op\":\"createChat\",\"chatId\":\"$id\",\"spaceId\":\"$sid\",\"config\":{\"harness\":\"mock\",\"model\":\"fable-5\",\"reasoning\":null,\"sandbox\":\"workspace-write\"}}" >/dev/null
    probe Mutate "{\"op\":\"renameChat\",\"chatId\":\"$id\",\"title\":\"$1\"}" >/dev/null
    probe Mutate "{\"op\":\"setChatBranch\",\"chatId\":\"$id\",\"branch\":\"$3\"}" >/dev/null
    if [[ "$5" == run ]]; then
      probe QueueCommand "{\"chatId\":\"$id\",\"command\":{\"kind\":\"run\",\"messageId\":\"$(uuidgen)\",\"request\":{\"prompt\":\"Walk me through the streaming pipeline\",\"model\":null,\"reasoning\":null,\"modelOptions\":{},\"cwd\":\"/tmp\",\"sandbox\":\"workspace-write\",\"autoApprove\":true,\"resume\":null}}}" >/dev/null
      sleep 1
    fi
    probe Mutate "{\"op\":\"setChatActivity\",\"chatId\":\"$id\",\"lastMessageAt\":$(( ($(date +%s) - $4*3600) * 1000 ))}" >/dev/null
  }
  seed "Native Zeron Rust Rewrite"    zeron zeron/main                 0  run
  seed "Rebalance Player Stats Caps"  soccertcg    zeron/rebalance-player-stat-caps  2  run
  seed "Craft Premium TCG Experience" soccertcg    zeron/craft-premium-tcg-exp       26 skip
  seed "Initial Context Exploration"  zeron        zeron/initial-context-exploration 14 skip
  seed "Soccer TCG Repo Creation"     aether       aether/main                       48 skip
  touch "$DAEMON_DIR/.demo-seeded"
fi

# A session fanning out 40 subagents with side chats under it, for the
# session card's lanes. Each launch fans out again; settled ones fold away.
FANOUT_ID_FILE="$DAEMON_DIR/.fanout-chat"
if [[ ! -f "$FANOUT_ID_FILE" ]]; then
  echo "▸ seeding the fan-out session"
  DEV=$(probe LocalDevice '{}' | python3 -c 'import json,sys;print(json.load(sys.stdin)["deviceId"])')
  sid=$(uuidgen | tr 'A-Z' 'a-z')
  mkdir -p "$PROJECTS_DIR/scouts"
  probe Mutate "{\"op\":\"createSpace\",\"spaceId\":\"$sid\",\"deviceId\":\"$DEV\",\"path\":\"$PROJECTS_DIR/scouts\"}" >/dev/null
  fanout=$(uuidgen | tr 'A-Z' 'a-z')
  probe Mutate "{\"op\":\"createChat\",\"chatId\":\"$fanout\",\"spaceId\":\"$sid\",\"config\":{\"harness\":\"mock\",\"model\":\"fable-5\",\"reasoning\":null,\"sandbox\":\"workspace-write\"}}" >/dev/null
  probe Mutate "{\"op\":\"renameChat\",\"chatId\":\"$fanout\",\"title\":\"Fan Out Forty Scouts\"}" >/dev/null
  for title in "Investigate the cadence regression" "Draft the migration notes" "Review the relay retry budget"; do
    side=$(uuidgen | tr 'A-Z' 'a-z')
    probe Mutate "{\"op\":\"createChat\",\"chatId\":\"$side\",\"spaceId\":\"$sid\",\"parentChatId\":\"$fanout\",\"config\":{\"harness\":\"mock\",\"model\":\"fable-5\",\"reasoning\":null,\"sandbox\":\"workspace-write\"}}" >/dev/null
    probe Mutate "{\"op\":\"renameChat\",\"chatId\":\"$side\",\"title\":\"$title\"}" >/dev/null
  done
  echo "$fanout" > "$FANOUT_ID_FILE"
fi
probe QueueCommand "{\"chatId\":\"$(cat "$FANOUT_ID_FILE")\",\"command\":{\"kind\":\"run\",\"messageId\":\"$(uuidgen)\",\"request\":{\"prompt\":\"/fanout 40\",\"model\":null,\"reasoning\":null,\"modelOptions\":{},\"cwd\":\"/tmp\",\"sandbox\":\"workspace-write\",\"autoApprove\":true,\"resume\":null}}}" >/dev/null

echo "▸ opening zeron (composer is live — type into it; --slow shows streaming)"
echo "  'Fan Out Forty Scouts' settles a subagent every 2.5s; send '/fanout 40' to replay"
ZERON_DATA_DIR="$UI_DIR" ZERON_IPC_PORT=$IPC RUST_LOG=warn ./target/debug/zeron
