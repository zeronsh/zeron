#!/usr/bin/env python3
"""Fake `codex app-server` for the handoff tests (crates/harness/tests/codex_handoff.rs).

Speaks newline-framed JSON-RPC 2.0 over stdio and keeps ONE thread
(`th-handoff`) for its whole life, so a run that is frozen, handed over and
adopted talks to the same process. It polices what an adopter must not do:
a second `initialize` or `thread/start`, a reused or decreasing request id,
or a `turn/steer` naming a turn that is not the active one each produce an
`error` notification (which the harness surfaces as an `Error` event).

The `turn/start` RESPONSE is the only place a turn id appears: no
`turn/started` notification is sent, so an adopter only knows the active turn
if it was carried over.

What a turn does is chosen by its prompt text:

- `hang` says `hanging` and leaves the turn open;
- `slow-start` answers `turn/start` after 1.5 s, then behaves like `hang`;
- `ask` says `asking` and sends `item/tool/requestUserInput` with the
  JSON-RPC id `srv-ask-7` and one question `pick`; the answer (a response
  with that id) is said back as `answered: <labels>` and ends the turn;
- `partial` says `part-start`, writes the next text frame in two halves
  with a pause between them, then ends the turn;
- `spawn` sends content for child thread `child-a` BEFORE its spawn is
  known (the harness must buffer it), binds child `child-b` (spawn item
  `spawn-b`) with one line of content, says `spawned` and leaves the turn
  open;
- anything else says `echo: <text>` and ends the turn.

A `turn/steer` (answered after 1.5 s for `slow-steer`) says `steer: <text>`;
`end` also ends the turn; `bind` binds `child-a` (spawn item `spawn-a`), sends
`b after` for `child-b` and ends the turn.

`turn/interrupt` is answered, then after 1 s the turn is aborted. The
notification `test/die {code}` writes `dying` to stderr and exits with `code`.
Exits when stdin closes.
"""

import json
import sys
import time

THREAD = "th-handoff"


def emit(frame):
    frame.setdefault("jsonrpc", "2.0")
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()


def notify(method, params):
    emit({"method": method, "params": params})


def reply(rid, result):
    emit({"id": rid, "result": result})


def text(t, thread=THREAD, item="msg"):
    notify("item/agentMessage/delta", {"threadId": thread, "itemId": item, "delta": t})


def error(message):
    notify("error", {"threadId": THREAD, "error": {"message": message}})


def activity(spawn_id, child):
    item = {
        "type": "subAgentActivity",
        "id": spawn_id,
        "kind": "started",
        "agentThreadId": child,
        "agentPath": "/root/" + child,
    }
    notify("item/started", {"threadId": THREAD, "item": item})
    notify("item/completed", {"threadId": THREAD, "item": item})


class Server:
    def __init__(self):
        self.initialized = False
        self.thread_started = False
        self.last_id = 0
        self.turns = 0
        self.active = None

    def check_id(self, rid):
        if not isinstance(rid, int) or rid <= self.last_id:
            error("request id %r reused or out of order (last %r)" % (rid, self.last_id))
        else:
            self.last_id = rid

    def complete(self):
        if self.active:
            notify("turn/completed", {
                "threadId": THREAD,
                "turn": {"id": self.active, "status": "completed"},
            })
            self.active = None

    def start_turn(self, rid, said):
        self.turns += 1
        self.active = "t-%d" % self.turns
        if said == "slow-start":
            time.sleep(1.5)
        reply(rid, {"turn": {"id": self.active}})
        if said in ("hang", "slow-start"):
            text("hanging")
        elif said == "ask":
            text("asking")
            emit({
                "id": "srv-ask-7",
                "method": "item/tool/requestUserInput",
                "params": {
                    "threadId": THREAD,
                    "questions": [{
                        "id": "pick",
                        "header": "Choice",
                        "question": "Pick one",
                        "options": [{"label": "A"}, {"label": "B"}],
                    }],
                },
            })
        elif said == "partial":
            text("part-start")
            line = json.dumps({
                "jsonrpc": "2.0",
                "method": "item/agentMessage/delta",
                "params": {"threadId": THREAD, "itemId": "msg", "delta": "partial-done"},
            }) + "\n"
            half = len(line) // 2
            sys.stdout.write(line[:half])
            sys.stdout.flush()
            time.sleep(1.0)
            sys.stdout.write(line[half:])
            sys.stdout.flush()
            self.complete()
        elif said == "spawn":
            text("early child", thread="child-a", item="a-early")
            activity("spawn-b", "child-b")
            text("b before", thread="child-b", item="b-1")
            text("spawned")
        else:
            text("echo: " + said)
            self.complete()

    def steer(self, rid, params, said):
        expected = params.get("expectedTurnId")
        if expected != self.active:
            error("turn/steer expected %r but the active turn is %r" % (expected, self.active))
            emit({"id": rid, "error": {"code": -32600, "message": "no such turn"}})
            return
        if said == "slow-steer":
            time.sleep(1.5)
        reply(rid, {})
        text("steer: " + said)
        if said == "end":
            self.complete()
        elif said == "bind":
            activity("spawn-a", "child-a")
            text("b after", thread="child-b", item="b-2")
            self.complete()

    def answer(self, frame):
        if frame.get("id") != "srv-ask-7":
            text("wrong response id %r" % frame.get("id"))
            return
        answers = frame.get("result", {}).get("answers", {})
        labels = answers.get("pick", {}).get("answers", [])
        text("answered: " + ",".join(labels))
        self.complete()

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
        if rid is None:
            return  # `initialized` and other notifications
        self.check_id(rid)
        if method == "initialize":
            if self.initialized:
                error("initialize sent twice")
            self.initialized = True
            reply(rid, {"userAgent": "fake-codex-handoff"})
        elif method == "config/read":
            reply(rid, {"config": {}})
        elif method in ("thread/start", "thread/resume"):
            if self.thread_started:
                error("a second thread was started")
            self.thread_started = True
            reply(rid, {"thread": {"id": THREAD}})
        elif method == "turn/start":
            if params.get("threadId") != THREAD:
                error("turn/start on thread %r" % params.get("threadId"))
            said = params.get("input", [{}])[0].get("text", "").strip()
            self.start_turn(rid, said)
        elif method == "turn/steer":
            said = params.get("input", [{}])[0].get("text", "").strip()
            self.steer(rid, params, said)
        elif method == "turn/interrupt":
            reply(rid, {})
            time.sleep(1.0)
            if self.active:
                notify("turn/aborted", {"threadId": THREAD, "turn": {"id": self.active}})
                self.active = None
        else:
            emit({"id": rid, "error": {"code": -32601, "message": "unknown method " + method}})


def main():
    sys.stderr.write("fake-codex: started\n")
    sys.stderr.flush()
    server = Server()
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
        server.handle(frame)


main()
