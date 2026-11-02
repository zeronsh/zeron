#!/usr/bin/env python3
"""ACP contract fixture: saved policy differs from reported ask mode."""
import json
import sys

mode_written = False
model = "model-a"
pending = None

def emit(value):
    print(json.dumps(value), flush=True)

def option(id, category, values, current):
    return dict(id=id, name=id, category=category, type="select",
                currentValue=current, options=[dict(value=v, name=v) for v in values])

for line in sys.stdin:
    msg = json.loads(line)
    method, params = msg.get("method"), msg.get("params", {})
    result = {}
    if method == "initialize":
        result = dict(protocolVersion=1, agentCapabilities=dict(loadSession=True))
    elif method in ("session/new", "session/load"):
        mode_written = False
        result = dict(sessionId="fx-session", configOptions=[
            option("provider", "model", ["backend"], "backend"),
            option("model", "model", ["model-a", "model-b"], model),
            option("mode", "mode", ["ask", "auto"], "ask"),
            option("effort", "thought_level", ["low", "medium", "high"], "medium")])
    elif method == "session/set_config_option":
        key, value = params["configId"], params["value"]
        if key == "mode" and value == "ask":
            mode_written = True
        elif key == "model" and value != "rejected-model":
            model = value
        else:
            emit(dict(id=msg["id"], error=dict(code=-32602, message="invalid selection")))
            continue
    elif method == "session/prompt":
        if not mode_written or model != "model-b":
            emit(dict(id=msg["id"], error=dict(code=-32603, message="unsafe or wrong model")))
            continue
        if params.get("prompt", [{}])[0].get("text") == "permission":
            pending = msg["id"]
            emit(dict(id="approve", method="session/request_permission", params=dict(
                sessionId="fx-session",
                toolCall=dict(toolCallId="write-1", title="Write file", kind="edit"),
                options=[dict(optionId="allow", name="Allow", kind="allow_once"),
                         dict(optionId="deny", name="Deny", kind="reject_once")])))
            continue
        emit(dict(method="session/update", params=dict(sessionId="fx-session",
            update=dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="fx reply")))))
        result = dict(stopReason="end_turn")
    elif method is None:
        if msg.get("id") == "approve":
            selected = msg.get("result", {}).get("outcome", {}).get("optionId", "deny")
            emit(dict(method="session/update", params=dict(sessionId="fx-session",
                update=dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=selected)))))
            emit(dict(id=pending, result=dict(stopReason="end_turn")))
        continue
    if "id" in msg:
        emit(dict(id=msg["id"], result=result))
