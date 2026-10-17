#!/usr/bin/env python3
"""Offline app-server contract. Fail immediately on any inference call."""
import json
import pathlib
import sys

if "generate-json-schema" in sys.argv:
    root = pathlib.Path(sys.argv[sys.argv.index("--out") + 1]) / "v2"
    root.mkdir(parents=True, exist_ok=True)
    (root / "ThreadForkParams.json").write_text(json.dumps({"properties": {"threadId": {}, "lastTurnId": {}}}))
    sys.exit(0)
source = ""
turns = [{"id": "t1", "status": "completed", "items": [{"text": "U1/A1"}]}, {"id": "t2", "status": "completed", "items": [{"text": "U2/A2"}]}]
for line in sys.stdin:
    message = json.loads(line)
    method = message["method"]
    with open("fork-wire.jsonl", "a") as log:
        log.write(line)
    if "id" not in message:
        continue
    params = message.get("params", {})
    if method == "initialize":
        result = {}
    elif method == "thread/read":
        if params["threadId"] != "child":
            source = params["threadId"]
            history = [dict(t) for t in turns]
            if source == "active":
                history[0]["status"] = "inProgress"
        else:
            history = turns if source == "ignore-boundary" else turns[:1]
        result = {"thread": {"id": params["threadId"], "turns": history}}
    elif method == "thread/fork":
        assert params["lastTurnId"] == "t1"
        assert params["ephemeral"] is False
        result = {"thread": {"id": "" if source == "empty-id" else "child", "sessionId": source}}
    else:
        raise AssertionError("Unexpected method: " + method)
    print(json.dumps({"id": message["id"], "result": result}), flush=True)
