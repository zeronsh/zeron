#!/usr/bin/env python3
"""Fake zeron cursor shim for cursor_handoff.rs: ONE conversation for the whole
life of the process, speaking the shim's JSONL protocol (see
crates/harness/src/cursor/shim.mjs) over real pipes. Every reply carries this
process's parent pid, because the real shim's watchdog exits when the parent
changes: an exec-in-place handoff must keep it constant.
"""
import json
import os
import sys
import time

PARENT = os.getppid()


def emit(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


def turn(prompt):
    emit({"ev": "text", "text": f"echo: {prompt}"})
    emit({"ev": "text", "text": f"ppid={os.getppid()}"})
    emit({"ev": "turn", "status": "finished"})


sys.stderr.write("fake-cursor: started\n")
sys.stderr.flush()
first = json.loads(sys.stdin.readline())
emit({"ev": "ready", "agentId": "agent-handoff", "model": "composer-2.5"})
if first.get("prompt") == "partial":
    emit({"ev": "text", "text": "part-start"})
    # Half a frame, then a pause: a freeze lands between the halves.
    sys.stdout.write('{"ev":"text","te')
    sys.stdout.flush()
    time.sleep(1.0)
    sys.stdout.write('xt":"part-end"}\n')
    sys.stdout.flush()
    emit({"ev": "turn", "status": "finished"})
else:
    turn(first.get("prompt"))

for line in sys.stdin:
    if os.getppid() != PARENT:
        sys.stderr.write("fake-cursor: parent changed, exiting\n")
        sys.exit(9)
    op = json.loads(line)
    if op.get("op") == "steer":
        emit({"ev": "steered"})
        turn(op.get("prompt"))
    elif op.get("op") == "interrupt":
        # Slow to wind down, so the run stays in its interrupting state.
        time.sleep(3)
        emit({"ev": "turn", "status": "cancelled"})
        sys.exit(0)
