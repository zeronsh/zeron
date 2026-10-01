#!/usr/bin/env python3
"""Fake ACP agent for the permission-policy tests in tests/acp_policy.rs.

Advertises a Devin-style `mode` config select (normal/plan/ask/bypass,
current `bypass`), or when launched through a name containing "legacy"
only Hermes-style legacy `modes` (default/accept_edits/dont_ask, current
`dont_ask`). Records the
mode the client selects, then for each tool call listed in the prompt (a
JSON array) sends `session/request_permission` with Devin's option set
(widening allows first, reject last) and records the outcome. The turn's
only text is a JSON report: {"mode": ..., "answers": [...]}.
"""

import json
import os
import sys

OPTIONS = [
    {"optionId": "switch_bypass", "name": "Yes, switch to bypass mode", "kind": "allow_always"},
    {"optionId": "allow_always_global", "name": "Always (all projects)", "kind": "allow_always"},
    {"optionId": "allow_session", "name": "Allow for session", "kind": "allow_always"},
    {"optionId": "allow_once", "name": "Allow", "kind": "allow_once"},
    {"optionId": "reject_once", "name": "Reject", "kind": "reject_once"},
]


def emit(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


def read():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    return json.loads(line)


legacy = "legacy" in os.path.basename(sys.argv[0])
mode = "dont_ask" if legacy else "bypass"
sid = "policy-session"

while True:
    msg = read()
    method = msg.get("method")
    if method == "initialize":
        emit({"id": msg["id"], "result": {"protocolVersion": 1, "agentCapabilities": {}}})
    elif method == "session/new":
        result = {"sessionId": sid}
        if legacy:
            result["modes"] = {
                "currentModeId": mode,
                "availableModes": [
                    {"id": "default", "name": "Default"},
                    {"id": "accept_edits", "name": "Accept Edits"},
                    {"id": "dont_ask", "name": "Don't Ask"},
                ],
            }
        else:
            result["configOptions"] = [{
                "id": "mode", "name": "Session Mode", "category": "mode", "type": "select",
                "currentValue": mode,
                "options": [{"value": v, "name": v} for v in ["normal", "plan", "ask", "bypass"]],
            }]
        emit({"id": msg["id"], "result": result})
    elif method == "session/set_config_option":
        if msg["params"].get("configId") == "mode":
            mode = msg["params"].get("value")
        emit({"id": msg["id"], "result": {}})
    elif method == "session/set_mode":
        mode = msg["params"].get("modeId")
        emit({"id": msg["id"], "result": {}})
    elif method == "session/prompt":
        calls = json.loads(msg["params"]["prompt"][0]["text"])
        answers = []
        for index, call in enumerate(calls):
            request_id = 1000 + index
            options = call.pop("options", OPTIONS)
            emit({"id": request_id, "method": "session/request_permission", "params": {
                "sessionId": sid,
                "toolCall": dict({"toolCallId": "t%d" % index}, **call),
                "options": options,
            }})
            while True:
                reply = read()
                if reply.get("id") == request_id and "method" not in reply:
                    break
            outcome = reply.get("result", {}).get("outcome", {})
            answers.append(outcome.get("optionId") or outcome.get("outcome"))
        report = json.dumps({"mode": mode, "answers": answers})
        emit({"method": "session/update", "params": {"sessionId": sid, "update": {
            "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": report}}}})
        emit({"id": msg["id"], "result": {"stopReason": "end_turn"}})
        # One turn per run: exiting ends the run's stream.
        sys.exit(0)
    elif "id" in msg and method is not None:
        emit({"id": msg["id"], "error": {"code": -32601, "message": "unsupported"}})
