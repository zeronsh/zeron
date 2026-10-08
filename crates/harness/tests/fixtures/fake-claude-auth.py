#!/usr/bin/env python3
"""A CLI whose native login recovers between subprocess launches."""
import json
from pathlib import Path
import signal
import sys
import time


def emit(frame):
    print(json.dumps(frame), flush=True)


first = sys.stdin.readline()
attempts = Path("attempts")
attempt = int(attempts.read_text()) + 1 if attempts.exists() else 1
attempts.write_text(str(attempt))
Path(f"args-{attempt}.json").write_text(json.dumps(sys.argv[1:]))
Path(f"prompt-{attempt}.json").write_text(first)
emit({"type": "system", "subtype": "init", "cwd": str(Path.cwd()),
      "session_id": f"auth-session-{attempt}", "model": "fixture", "tools": []})

if "scenario:auth-cancel-during-retry" in first:
    signal.signal(signal.SIGTERM, lambda *_: Path("reaping").write_text("true"))
if "scenario:auth-after-completed" in first:
    emit({"type": "result", "subtype": "success", "result": "Done",
          "session_id": f"auth-session-{attempt}"})
    emit(json.loads(sys.stdin.readline()))

if "scenario:auth-after-text" in first:
    emit({"type": "stream_event", "event": {"type": "content_block_delta",
          "delta": {"type": "text_delta", "text": "Already started"}}})
if "scenario:auth-after-reasoning" in first:
    emit({"type": "stream_event", "event": {"type": "content_block_delta",
          "delta": {"type": "thinking_delta", "thinking": ""}}})
if "scenario:auth-after-unknown-frame" in first:
    emit({"type": "future_action"})
if "scenario:auth-after-tool" in first:
    emit({"type": "control_request", "request_id": "tool-1", "request": {
        "subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "echo work"}}})
    sys.stdin.readline()
    Path("tool-ran").write_text("once")

if attempt == 1 or "scenario:auth-persistent" in first:
    emit({"type": "assistant", "error": "authentication_failed",
          "parent_tool_use_id": "subagent-1" if "scenario:auth-subagent" in first else None,
          "is_api_error_message": True, "message": {
              "content": [{"type": "text", "text": "API Error: 401 Authentication rejected"}],
              "usage": {"input_tokens": 0,
                        "output_tokens": 1 if "scenario:auth-with-output-usage" in first else 0}}})
    emit({"type": "result", "subtype": "error_during_execution", "is_error": True,
          "errors": ["Authentication rejected"], "session_id": f"auth-session-{attempt}"})
    if "scenario:auth-cancel-during-retry" in first:
        time.sleep(30)
else:
    emit({"type": "stream_event", "event": {"type": "content_block_delta",
          "delta": {"type": "text_delta", "text": "Recovered"}}})
    emit({"type": "result", "subtype": "success", "result": "Recovered",
          "session_id": f"auth-session-{attempt}"})
