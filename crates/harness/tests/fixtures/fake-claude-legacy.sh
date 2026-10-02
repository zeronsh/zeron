#!/bin/sh
# Fake PRE-2.1 Claude Code CLI: an install that predates the undocumented
# `--thinking-display` flag, so its argument parser rejects the launch.
#
# Everything else matches fake-claude.sh closely enough to drive a full turn:
# the point is that the harness must drop the flag and the turn must still
# land. Driven by crates/harness/tests/claude.rs.

for arg in "$@"; do
  if [ "$arg" = "--thinking-display" ]; then
    printf '%s\n' "error: unknown option '--thinking-display'" >&2
    exit 2
  fi
done

read -r first || exit 1
emit() { printf '%s\n' "$1"; }

emit '{"type":"system","subtype":"init","model":"claude-fable-5","tools":[],"cwd":"/tmp","session_id":"sess-legacy"}'
emit '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"pondering"}}}'
emit '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"legacy"}}}'
emit '{"type":"result","subtype":"success","result":"ran without --thinking-display","usage":{"input_tokens":1,"output_tokens":1},"session_id":"sess-legacy"}'
