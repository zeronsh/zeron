#!/usr/bin/env python3
"""Fake ACP agent for the handoff tests (crates/harness/tests/acp_handoff.rs).

Speaks newline-framed JSON-RPC 2.0 over stdio and keeps ONE session
(`acp-handoff`) for its whole life, so a run that is frozen, handed over and
adopted talks to the same process. It polices what an adopter must not do: a
second `initialize` or `session/new`, a reused or decreasing request id, a
repeated `_meta.promptId`, or a `session/prompt` while another is still
unanswered. Each violation is said back as an `agent_message_chunk` starting
with `PROTOCOL ERROR`.

No `_session/steering` extension is advertised (steers preempt, as Grok's, or
wait for the turn end) unless the script runs under a name ending in
`_steering.py`: then a `_session/steering` request is answered `injected`
after 1 s while a prompt is open (`promptRequired` otherwise), followed by
`steer: <text>` and the end of that prompt. `session/cancel` answers the open
prompt with `cancelled`.

What a prompt does is chosen by its text:

- `hang` says `hanging` and leaves the prompt open;
- `stubborn` says `hanging` and leaves the prompt open, ignoring cancels;
- `slow` says `working`, waits 1 s, says `finished` and answers the prompt:
  a response that lands while nobody reads;
- `ask` says `asking` and sends `session/request_permission` with the
  JSON-RPC id `perm-9` and two question options; the answer (a response with
  that id) is said back as `answered: <optionId>` and ends the prompt;
- `spawn` binds Grok subagent `sub-1` (spawn tool call `sp1`, child session
  `child-1`), says `spawned` and leaves the prompt open;
- `finish-sub` sends `subagent_finished` for `sub-1`, then acts as below;
- `grandchild` starts `sleep 60` in the agent's process group, says
  `grandchild: <pid>` and ends the prompt;
- `tmp` says `tmp: <$TMPDIR>` and ends the prompt;
- `echo-head` says the first half of a `<SYSTEM_MESSAGE>` wakeup block and
  leaves the prompt open;
- `selfcontinue` says `done-1`, ends the prompt, then keeps a tool call open
  (a self-continued turn) until a `session/cancel` closes it;
- `replay` first sends a STALE `_x.ai/session/prompt_complete` for the
  previous prompt id, then acts as below;
- anything else says `echo: <text>` (and `promptId: <id>` when one was
  sent) and ends the prompt.

The notification `test/say {text, end}` says `text` (`echo-tail`: the rest
of the wakeup block and ` after`) and, with `end`, ends the open prompt.

A prompt carrying `_meta.promptId` is ended with `_x.ai/session/prompt_complete`
just ahead of the response, as Grok does. `session/new` waits 1.5 s when its
cwd contains `slow-start`. The notification `test/die {code}` writes `dying`
to stderr and exits with `code`. Exits when stdin closes.
"""

import json
import os
import subprocess
import sys
import time

SID = "acp-handoff"
STEERING = sys.argv[0].endswith("_steering.py")
WAKEUP = "<SYSTEM_MESSAGE>\n[Message] timestamp=x sender=task priority=HIGH content=built\n</SYSTEM_MESSAGE>"


def emit(frame):
    frame.setdefault("jsonrpc", "2.0")
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()


def update(body):
    emit({"method": "session/update", "params": {"sessionId": SID, "update": body}})


def xnotify(body):
    emit({"method": "_x.ai/session_notification", "params": {"sessionId": SID, "update": body}})


def say(text):
    update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}})


def error(message):
    say("PROTOCOL ERROR: " + message)


class Agent:
    def __init__(self):
        self.initialized = False
        self.session = False
        self.last_id = 0
        self.open_prompt = None  # (rid, prompt id)
        self.stubborn = False
        self.seen_prompt_ids = []
        self.self_continued = False

    def check_id(self, rid):
        if not isinstance(rid, int) or rid <= self.last_id:
            error("request id %r reused or out of order (last %r)" % (rid, self.last_id))
        else:
            self.last_id = rid

    def end_prompt(self, stop="end_turn"):
        if self.open_prompt is None:
            return
        rid, pid = self.open_prompt
        self.open_prompt = None
        self.stubborn = False
        if pid is not None:
            emit({
                "method": "_x.ai/session/prompt_complete",
                "params": {"sessionId": SID, "promptId": pid, "stopReason": stop},
            })
        emit({"id": rid, "result": {"stopReason": stop}})

    def prompt(self, rid, params):
        if self.open_prompt is not None:
            error("session/prompt %r while %r is unanswered" % (rid, self.open_prompt[0]))
        if params.get("sessionId") != SID:
            error("session/prompt on session %r" % params.get("sessionId"))
        pid = (params.get("_meta") or {}).get("promptId")
        if pid is not None:
            if pid in self.seen_prompt_ids:
                error("promptId %r reused" % pid)
            previous = self.seen_prompt_ids[-1] if self.seen_prompt_ids else None
            self.seen_prompt_ids.append(pid)
        else:
            previous = None
        self.open_prompt = (rid, pid)
        blocks = params.get("prompt") or [{}]
        said = "\n\n".join(b.get("text", "") for b in blocks).strip()
        if said == "hang":
            say("hanging")
        elif said == "stubborn":
            self.stubborn = True
            say("hanging")
        elif said == "slow":
            say("working")
            time.sleep(1.0)
            say("finished")
            self.end_prompt()
        elif said == "ask":
            say("asking")
            emit({
                "id": "perm-9",
                "method": "session/request_permission",
                "params": {
                    "sessionId": SID,
                    "toolCall": {"toolCallId": "q1", "title": "Pick one"},
                    "options": [
                        {"optionId": "opt-a", "name": "A"},
                        {"optionId": "opt-b", "name": "B"},
                    ],
                },
            })
        elif said == "spawn":
            update({
                "sessionUpdate": "tool_call",
                "toolCallId": "sp1",
                "title": "spawn_subagent",
                "rawInput": {"description": "Count files", "prompt": "Count the files."},
                "_meta": {"x.ai/tool": {"version": 1, "name": "spawn_subagent", "kind": "task"}},
            })
            update({
                "sessionUpdate": "tool_call_update",
                "toolCallId": "sp1",
                "status": "completed",
                "rawOutput": {"type": "Text", "text": "Subagent started in background.\nsubagent_id: sub-1"},
            })
            xnotify({
                "sessionUpdate": "subagent_spawned",
                "subagent_id": "sub-1",
                "parent_session_id": SID,
                "child_session_id": "child-1",
                "description": "Count files",
            })
            say("spawned")
        elif said == "grandchild":
            child = subprocess.Popen(
                ["sleep", "60"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            say("grandchild: %d" % child.pid)
            self.end_prompt()
        elif said == "tmp":
            say("tmp: " + os.environ.get("TMPDIR", ""))
            self.end_prompt()
        elif said == "echo-head":
            say("before " + WAKEUP[:30])
        elif said == "selfcontinue":
            say("done-1")
            self.end_prompt()
            self.self_continued = True
            update({
                "sessionUpdate": "tool_call",
                "toolCallId": "bg-1",
                "title": "background",
                "status": "in_progress",
            })
        else:
            if said.startswith("replay"):
                if previous is None:
                    error("no previous prompt id to replay")
                else:
                    emit({
                        "method": "_x.ai/session/prompt_complete",
                        "params": {"sessionId": SID, "promptId": previous, "stopReason": "end_turn"},
                    })
                    # Let the stale completion reach the harness first.
                    time.sleep(0.3)
            if said.startswith("finish-sub"):
                xnotify({
                    "sessionUpdate": "subagent_finished",
                    "subagent_id": "sub-1",
                    "child_session_id": "child-1",
                    "status": "completed",
                    "output": "two files",
                })
            say("echo: " + said)
            if pid is not None:
                say("promptId: " + pid)
            self.end_prompt()

    def cancel(self):
        if self.self_continued:
            self.self_continued = False
            update({"sessionUpdate": "tool_call_update", "toolCallId": "bg-1", "status": "completed"})
        if self.open_prompt is not None and not self.stubborn:
            self.end_prompt("cancelled")

    def answer(self, frame):
        if frame.get("id") != "perm-9":
            say("wrong response id %r" % frame.get("id"))
            return
        outcome = (frame.get("result") or {}).get("outcome") or {}
        say("answered: " + str(outcome.get("optionId", outcome.get("outcome"))))
        self.end_prompt()

    def handle(self, frame):
        method = frame.get("method")
        rid = frame.get("id")
        if method is None:
            if rid is not None:
                self.answer(frame)
            return
        params = frame.get("params") or {}
        if method == "test/die":
            sys.stderr.write("dying\n")
            sys.stderr.flush()
            sys.exit(int(params.get("code", 1)))
        if method == "session/cancel":
            self.cancel()
            return
        if method == "test/say":
            if params.get("text") == "echo-tail":
                say(WAKEUP[30:] + " after")
            else:
                say(params.get("text", ""))
            if params.get("end"):
                self.end_prompt()
            return
        if rid is None:
            return
        self.check_id(rid)
        if method == "initialize":
            if self.initialized:
                error("initialize sent twice")
            self.initialized = True
            result = {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}}
            if STEERING:
                result["_meta"] = {"steering": {"supported": True}}
            emit({"id": rid, "result": result})
        elif method == "session/new":
            if self.session:
                error("a second session was started")
            self.session = True
            if "slow-start" in str(params.get("cwd", "")):
                time.sleep(1.5)
            emit({"id": rid, "result": {"sessionId": SID}})
        elif method == "session/prompt":
            self.prompt(rid, params)
        elif method == "_session/steering" and STEERING:
            said = "\n\n".join(b.get("text", "") for b in params.get("prompt") or [{}]).strip()
            if self.open_prompt is None:
                emit({"id": rid, "result": {"outcome": "promptRequired"}})
                return
            time.sleep(1.0)
            emit({"id": rid, "result": {"outcome": "injected"}})
            say("steer: " + said)
            self.end_prompt()
        else:
            emit({"id": rid, "error": {"code": -32601, "message": "unknown method " + method}})


def main():
    sys.stderr.write("fake-acp: started\n")
    sys.stderr.flush()
    agent = Agent()
    while True:
        line = sys.stdin.readline()
        if not line:
            return
        line = line.strip()
        if not line:
            continue
        try:
            frame = json.loads(line)
        except ValueError:
            continue
        agent.handle(frame)


main()
