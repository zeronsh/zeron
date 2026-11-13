#!/usr/bin/env python3
"""A stand-in `omp --mode rpc|rpc-ui`: replays an RPC scenario.

The frames scenarios emit are trimmed recordings of omp 18.20.0's RPC mode
(see crates/harness/src/omp/PROTOCOL.md); error shapes are omp's documented
ones. scenarios.py writes the scenario files.

A scenario is NDJSON, one step per line:
  {"expect": "<command type>", "bind": {"$NAME": ["id"]}, "equals": {...}, "skip": n}
      wait for the client to send that command (other frames are kept for
      later expects; housekeeping commands are answered by the defaults
      below as they come, the first `skip` of the expected kind too); bind
      values from it; `equals` fields must match exactly or the fake exits 97.
  {"emit": <frame>}   write a frame; "$NAME" strings are replaced by bound values.
  {"chunked": <frame>, "pieces": n}   write a frame as an rpc_chunk sequence.
  {"stderr": "..."} / {"sleep": ms} / {"exit": code}
  {"no_ready": true}  (first step only) do not write the `ready` frame.
After the last step it answers defaults until the client closes stdin.

Environment: OMP_FAKE_SCENARIO (a run's script; discovery runs without
one) and OMP_FAKE_LOG (a directory receiving launch.ndjson and
inbound.ndjson).
"""
import base64
import json
import os
import sys
import time

args = sys.argv[1:]
if args[:1] == ["--version"]:
    print("omp/18.20.0")
    sys.exit(0)

log = os.environ.get("OMP_FAKE_LOG")


def record(name, value):
    if log:
        with open(os.path.join(log, name), "a") as f:
            f.write(json.dumps(value) + "\n")


record("launch.ndjson", {"args": args, "cwd": os.getcwd()})


def write(frame):
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()


def arg(flag):
    return args[args.index(flag) + 1] if flag in args else None


session_dir = arg("--session-dir")
session = "01a1d49f-0000-7000-8000-000000000001"
resume = arg("--resume")
if resume:
    for line in open(resume):
        record_ = json.loads(line)
        if record_.get("type") == "session":
            session = record_["id"]
            break
values = {"$SESSION": session}
session_file = os.path.join(session_dir or "/nonexistent", "2026-11-13T00-00-00-000Z_%s.jsonl" % session)

MODEL = {"id": "gpt-5.6-luna", "name": "GPT-5.6 Luna", "provider": "openai-codex", "reasoning": True,
         "thinking": {"mode": "effort", "efforts": ["low", "medium", "high", "xhigh", "max"]},
         "contextWindow": 272000}


def state():
    return {"model": MODEL, "thinkingLevel": "high", "isStreaming": False, "isCompacting": False,
            "steeringMode": "all", "followUpMode": "one-at-a-time", "interruptMode": "wait",
            "sessionId": values["$SESSION"], "sessionFile": session_file, "autoCompactionEnabled": True,
            "messageCount": 0, "queuedMessageCount": 0, "hasPendingAsyncWork": False, "isSettled": True}


DEFAULTS = {
    "negotiate_protocol": lambda f: {"protocolVersion": 2},
    "set_event_filter": lambda f: {"events": None, "messageUpdates": f.get("messageUpdates", "full")},
    "set_ask_dialog": lambda f: {"enabled": f.get("enabled") is True},
    "set_steering_mode": lambda f: None,
    "set_interrupt_mode": lambda f: None,
    "set_host_tools": lambda f: {"toolNames": [t["name"] for t in f.get("tools", [])]},
    "set_model": lambda f: MODEL,
    "set_thinking_level": lambda f: None,
    "get_available_thinking_levels": lambda f: {"levels": ["off", "low", "medium", "high", "xhigh", "max"]},
    "get_available_models": lambda f: {"models": [MODEL]},
    "get_available_commands": lambda f: {"commands": [
        {"name": "compact", "description": "Compact context", "source": "builtin"},
        {"name": "skill:review", "description": "Review code", "source": "skill"}]},
    "get_state": lambda f: state(),
}


def answer_default(frame):
    kind = frame.get("type")
    if kind not in DEFAULTS:
        return False
    response = {"id": frame.get("id"), "type": "response", "command": kind, "success": True}
    data = DEFAULTS[kind](frame)
    if data is not None:
        response["data"] = data
    write(response)
    return True


def read_frame():
    line = sys.stdin.readline()
    if not line:
        return None
    frame = json.loads(line)
    record("inbound.ndjson", frame)
    return frame


scenario = os.environ.get("OMP_FAKE_SCENARIO")
steps = [json.loads(line) for line in open(scenario) if line.strip()] if scenario else []
buffered = []

if not (steps and steps[0].get("no_ready")):
    write({"type": "ready", "protocolVersion": 1, "supportedProtocolVersions": [1, 2],
           "maxFrameBytes": 1048576, "maxReassembledFrameBytes": 67108864})
    write({"type": "extension_ui_request", "id": "w0", "method": "setWidget", "widgetKey": "autoresearch"})


def take(kind, skip):
    for index, frame in enumerate(buffered):
        if frame.get("type") == kind:
            return buffered.pop(index)
    while True:
        frame = read_frame()
        if frame is None:
            sys.exit(0)
        if frame.get("type") == kind and skip == 0:
            return frame
        if frame.get("type") == kind:
            skip -= 1
        if answer_default(frame):
            continue
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
        frame = take(step["expect"], step.get("skip", 0))
        for name, path in step.get("bind", {}).items():
            value = frame
            for key in path:
                value = value[key]
            values[name] = value
        for key, want in substitute(step.get("equals", {})).items():
            if frame.get(key) != want:
                sys.stderr.write("fake omp: %s.%s = %r, expected %r\n" % (step["expect"], key, frame.get(key), want))
                sys.exit(97)
    elif "emit" in step:
        write(substitute(step["emit"]))
    elif "chunked" in step:
        data = json.dumps(substitute(step["chunked"])).encode()
        pieces = step.get("pieces", 3)
        size = -(-len(data) // pieces)
        parts = [data[i:i + size] for i in range(0, len(data), size)]
        for index, part in enumerate(parts):
            write({"type": "rpc_chunk", "chunkId": "rpc-1", "index": index, "count": len(parts),
                   "byteLength": len(data), "data": base64.b64encode(part).decode()})
    elif "stderr" in step:
        sys.stderr.write(step["stderr"] + "\n")
        sys.stderr.flush()
    elif "sleep" in step:
        time.sleep(step["sleep"] / 1000)
    elif "exit" in step:
        sys.exit(step["exit"])

for frame in buffered:
    answer_default(frame)
while True:
    frame = read_frame()
    if frame is None:
        sys.exit(0)
    answer_default(frame)
