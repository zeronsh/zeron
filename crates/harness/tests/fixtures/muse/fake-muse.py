#!/usr/bin/env python3
"""A stand-in `muse serve`: replays an MSP scenario.

A scenario is NDJSON, one step per line:
  {"expect": "<method>" | "@reply", "bind": {"$NAME": ["params", "sessionId"]}}
      wait for the client to send that frame (frames arriving out of order are
      kept for later expects); bind values from it.
  {"emit": <frame>}   write a frame; "$NAME" strings are replaced by bound values.
  {"stderr": "..."} / {"sleep": ms} / {"exit": code}
After the last step it reads until the client closes stdin.

Environment: MUSE_FAKE_SCENARIO (session runs) and MUSE_FAKE_LOG (a directory
receiving launch.ndjson and inbound.ndjson). A read-only discovery host answers
model/list from models.json beside this script.
"""
import json
import os
import sys
import time

args = sys.argv[1:]
if args[:1] == ["--version"]:
    print("1.4.3-R5018.1")
    sys.exit(0)

log = os.environ.get("MUSE_FAKE_LOG")


def record(name, value):
    if log:
        with open(os.path.join(log, name), "a") as f:
            f.write(json.dumps(value) + "\n")


record("launch.ndjson", {
    "args": args,
    "env": {k: os.environ.get(k) for k in ("META_API_KEY", "MUSE_NO_AUTO_UPDATE")},
})


def write(frame):
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()


def read_frame():
    line = sys.stdin.readline()
    if not line:
        return None
    frame = json.loads(line)
    record("inbound.ndjson", frame)
    params = frame.get("params") or {}
    # The real host refuses a sticky workspace-root change on a steer (it may
    # launch no turn to bind it to): fail loudly, as a real run would.
    if frame.get("method") == "turn/start" and params.get("ifBusy") == "steer" \
            and "workspaceRoots" in params:
        sys.stderr.write("Invalid params: workspaceRoots cannot ride ifBusy \"steer\"\n")
        sys.exit(97)
    return frame


if "--disable-write" in args and not os.environ.get("MUSE_FAKE_SCENARIO"):
    # A read-only discovery host (a title run brings its own scenario).
    beside = os.path.join(os.path.dirname(os.path.abspath(__file__)), "models.json")
    models = json.load(open(beside)) if os.path.exists(beside) else {"providerId": "meta", "models": []}
    while True:
        frame = read_frame()
        if frame is None:
            sys.exit(0)
        if frame.get("method") == "initialize":
            write({"jsonrpc": "2.0", "id": frame["id"], "result": {
                "schema": {"version": 1, "fingerprint": "sha256:fake"},
                "grantedCapabilities": []}})
        elif frame.get("method") == "model/list":
            write({"jsonrpc": "2.0", "id": frame["id"], "result": models})

steps = [json.loads(line) for line in open(os.environ["MUSE_FAKE_SCENARIO"]) if line.strip()]
values = {}
buffered = []


def matches(frame, method):
    if method == "@reply":
        return "method" not in frame
    return frame.get("method") == method


def take(method):
    for index, frame in enumerate(buffered):
        if matches(frame, method):
            return buffered.pop(index)
    while True:
        frame = read_frame()
        if frame is None:
            sys.exit(0)
        if matches(frame, method):
            return frame
        buffered.append(frame)


def substitute(value):
    if isinstance(value, dict):
        return {k: substitute(v) for k, v in value.items()}
    if isinstance(value, list):
        return [substitute(v) for v in value]
    if isinstance(value, str):
        if value in values:
            return values[value]
        for name in sorted(values, key=len, reverse=True):
            if isinstance(values[name], str):
                value = value.replace(name, values[name])
    return value


for step in steps:
    if "expect" in step:
        frame = take(step["expect"])
        for name, path in step.get("bind", {}).items():
            value = frame
            for key in path:
                value = value[key]
            values[name] = value
    elif "emit" in step:
        write(substitute(step["emit"]))
    elif "stderr" in step:
        sys.stderr.write(step["stderr"] + "\n")
        sys.stderr.flush()
    elif "sleep" in step:
        time.sleep(step["sleep"] / 1000)
    elif "exit" in step:
        sys.exit(step["exit"])

while read_frame() is not None:
    pass
