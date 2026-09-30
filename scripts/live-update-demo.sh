#!/usr/bin/env bash
# A real live update, narrated: a real `zeron headless` engine with a running
# (scripted) agent parked on a question and a real shell is replaced, in place,
# by a second copy of the binary. The engine keeps its PID, the agent and the
# shell stay its children, the parked question still answers, and not one
# connection to the engine's port is refused.
#
#   scripts/live-update-demo.sh          # Unix only; needs python3 and a Rust toolchain
#
# It runs the end-to-end test in apps/zeron/tests/live_handoff.rs with narration
# turned on. The agent is the fake Claude CLI the harness tests use, so no
# account or network is involved.
set -euo pipefail
cd "$(dirname "$0")/.."
echo "Live update demo (real processes; the agent is a scripted stand-in for an LLM CLI)"
echo
E2E_SHOW=1 cargo test -q -p zeron --test live_handoff \
  handoff_keeps_agent_and_shell_alive_and_the_transcript_continuous \
  -- --nocapture --test-threads=1 2>&1 | grep -E '^» |^test result|panicked' || true
