#!/usr/bin/env python3
"""Fake Claude Code CLI for the handoff tests (crates/harness/tests/claude_handoff.rs).

Speaks stream-json over stdio and keeps ONE conversation for its whole life,
so a run that is frozen, handed over and adopted talks to the same process.
Deterministic: what it does is chosen by the text of each user line.

- the first user line starts the session (`system:init`, session id
  `sess-handoff`) and is then handled like any other;
- a steer (a user line with a `uuid`) is replayed first, as the real CLI
  does with `--replay-user-messages`;
- `ask` asks one AskUserQuestion over the control channel (request id
  `ask-1`) and says `answered: <label>` with the answer it gets back;
- `partial` says `part-start`, then writes the next text frame in two
  halves with a pause between them, then ends the turn;
- `hang` says `hanging` and leaves the turn open;
- `absorb-next` ends its turn; the NEXT steer is then answered with a bare
  result and never replayed (the CLI absorbing a steer);
- `die N` writes `dying` to stderr and exits with status N;
- anything else says `echo: <text>` and ends the turn.

An `interrupt` control request is ignored (the run escalates to signals).
Exits when stdin closes.
"""

import json
import os
import sys
import time

SESSION = "sess-handoff"


def emit(frame):
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()


def text(t):
    return {
        "type": "stream_event",
        "parent_tool_use_id": None,
        "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": t}},
    }


def result(r="ok"):
    return {
        "type": "result",
        "subtype": "success",
        "result": r,
        "errors": [],
        "usage": {"input_tokens": 1, "output_tokens": 1},
        "session_id": SESSION,
    }


def content_of(frame):
    content = frame.get("message", {}).get("content", "")
    if isinstance(content, list):
        return " ".join(b.get("text", "") for b in content if b.get("type") == "text")
    return content


def read_frame():
    while True:
        line = sys.stdin.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            continue
        try:
            return line, json.loads(line)
        except ValueError:
            continue


def main():
    sys.stderr.write("fake-claude: started\n")
    sys.stderr.flush()
    started = False
    absorb = False
    while True:
        raw, frame = read_frame()
        kind = frame.get("type")
        if kind == "control_request":
            continue  # interrupt: ignored on purpose
        if kind != "user":
            continue
        if not started:
            started = True
            emit({
                "type": "system",
                "subtype": "init",
                "model": "claude-fake",
                "tools": [],
                "cwd": os.getcwd(),
                "session_id": SESSION,
            })
        if frame.get("uuid"):
            if absorb:
                absorb = False
                emit(result("absorbed"))
                continue
            sys.stdout.write(raw + "\n")
            sys.stdout.flush()
        said = content_of(frame).strip()
        if said == "ask":
            emit(text("asking"))
            emit({
                "type": "control_request",
                "request_id": "ask-1",
                "request": {
                    "subtype": "can_use_tool",
                    "tool_name": "AskUserQuestion",
                    "input": {"questions": [{
                        "header": "Choice",
                        "question": "Pick one",
                        "options": ["A", "B"],
                        "multiSelect": False,
                    }]},
                },
            })
            while True:
                _, reply = read_frame()
                if reply.get("type") == "control_response":
                    break
            response = reply.get("response", {})
            if response.get("request_id") != "ask-1":
                emit(text("wrong request id"))
            else:
                answers = response.get("response", {}).get("updatedInput", {}).get("answers", {})
                emit(text("answered: %s" % answers.get("Pick one", "<none>")))
            emit(result())
        elif said == "partial":
            emit(text("part-start"))
            line = json.dumps(text("partial-done")) + "\n"
            half = len(line) // 2
            sys.stdout.write(line[:half])
            sys.stdout.flush()
            time.sleep(1.0)
            sys.stdout.write(line[half:])
            sys.stdout.flush()
            emit(result())
        elif said == "hang":
            emit(text("hanging"))
        elif said == "absorb-next":
            absorb = True
            emit(text("ready"))
            emit(result())
        elif said.startswith("die "):
            sys.stderr.write("dying\n")
            sys.stderr.flush()
            sys.exit(int(said.split()[1]))
        else:
            emit(text("echo: " + said))
            emit(result())


main()
