#!/usr/bin/env python3
"""Fake Muse (`muse serve`) for zeron-harness tests.

Speaks just enough of the MSP JSON-RPC wire: initialize, model/list,
skill/list, session/start, session/resume, turn/start (scripted replies
driven by the prompt text), turn/steer, turn/interrupt. Driven by
crates/harness/tests/muse.rs.
"""
import json
import sys

if "--version" in sys.argv:
    print("Muse Code 0.0.0-fake")
    sys.exit(0)

SESSION_ID = "01900000-0000-7000-8000-000000000001"


def emit(frame):
    print(json.dumps(frame), flush=True)


def result(req_id, value):
    emit({"jsonrpc": "2.0", "id": req_id, "result": value})


def notify(method, params):
    emit({"jsonrpc": "2.0", "method": method, "params": params})


def session_obj():
    return {
        "sessionId": SESSION_ID,
        "status": "idle",
        "activeTurnId": None,
        "workspaceRoot": "/tmp",
        "providerId": "fake",
        "modelId": None,
        "turnCount": 0,
        "kind": "root",
    }


def run_turn(params):
    parts = params.get("input", [])
    text = " ".join(p.get("text", "") for p in parts if p.get("type") == "text")
    skills = [p.get("selector", "") for p in parts if p.get("type") == "skill"]
    turn_id = params.get("commandId", "turn-1")
    notify("turn/started", {"sessionId": SESSION_ID, "viewCursor": "1", "sourceRange": {}, "turnId": turn_id, "commandId": turn_id})
    if skills:
        notify("item/started", {"sessionId": SESSION_ID, "viewCursor": "2", "item": {"itemId": "m1", "kind": "agentMessage", "status": "inProgress", "text": ""}})
        notify("item/delta", {"sessionId": SESSION_ID, "viewCursor": "3", "itemId": "m1", "field": "text", "delta": f"skill:{skills[0]}"})
        notify("item/completed", {"sessionId": SESSION_ID, "viewCursor": "4", "sourceRange": {}, "item": {"itemId": "m1", "kind": "agentMessage", "status": "completed", "text": f"skill:{skills[0]}"}})
    elif "scenario:tool" in text:
        notify("item/started", {"sessionId": SESSION_ID, "viewCursor": "2", "item": {"itemId": "t1", "kind": "toolCall", "status": "inProgress", "tool": "muse.bash", "args": json.dumps({"command": "echo hi"})}})
        notify("item/completed", {"sessionId": SESSION_ID, "viewCursor": "3", "sourceRange": {}, "item": {"itemId": "t1", "kind": "toolCall", "status": "completed", "tool": "muse.bash", "args": "{}", "visibleOutput": "hi"}})
        notify("item/started", {"sessionId": SESSION_ID, "viewCursor": "4", "item": {"itemId": "m1", "kind": "agentMessage", "status": "inProgress", "text": ""}})
        notify("item/delta", {"sessionId": SESSION_ID, "viewCursor": "5", "itemId": "m1", "field": "text", "delta": "done"})
        notify("item/completed", {"sessionId": SESSION_ID, "viewCursor": "6", "sourceRange": {}, "item": {"itemId": "m1", "kind": "agentMessage", "status": "completed", "text": "done"}})
    elif "scenario:fail" in text:
        notify("turn/completed", {"sessionId": SESSION_ID, "viewCursor": "9", "sourceRange": {}, "turnId": turn_id, "terminal": "failed", "error": {"kind": "test", "message": "fake failure", "retryable": False}})
        return
    elif "scenario:ask" in text:
        # Server-initiated question; the harness must receipt it, ask the
        # user through request_input, and answer with userInput/answer.
        emit({"jsonrpc": "2.0", "id": 900, "method": "userInput/request", "params": {
            "sessionId": SESSION_ID, "userInputId": "ui-1", "itemId": "i1", "toolCallId": "tc1", "toolName": "ask", "turnId": turn_id, "viewCursor": "2",
            "questions": [{"id": "q1", "header": "Pick", "question": "Which one?", "options": [{"label": "Alpha"}, {"label": "Beta"}], "selection": {"mode": "single", "minSelections": 1, "maxSelections": 1}}],
        }})
        return  # turn completes only after the answer arrives
    else:
        notify("item/started", {"sessionId": SESSION_ID, "viewCursor": "2", "item": {"itemId": "m1", "kind": "agentMessage", "status": "inProgress", "text": ""}})
        notify("item/delta", {"sessionId": SESSION_ID, "viewCursor": "3", "itemId": "m1", "field": "text", "delta": "echo: "})
        notify("item/delta", {"sessionId": SESSION_ID, "viewCursor": "4", "itemId": "m1", "field": "text", "delta": text})
        notify("item/completed", {"sessionId": SESSION_ID, "viewCursor": "5", "sourceRange": {}, "item": {"itemId": "m1", "kind": "agentMessage", "status": "completed", "text": f"echo: {text}"}})
    notify("turn/completed", {"sessionId": SESSION_ID, "viewCursor": "9", "sourceRange": {}, "turnId": turn_id, "terminal": "completed"})


granted = []

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    req_id = msg.get("id")
    if method == "initialize":
        wanted = msg.get("params", {}).get("capabilities", {}).get("requestedCapabilities", [])
        granted = [c for c in wanted if c == "sessionMcp"]
        result(req_id, {"serverInfo": {"name": "muse", "version": "0.0.0-fake"}, "userAgent": "fake", "museHome": "/tmp", "platformFamily": "unix", "platformOs": "macos", "schema": {"version": 1, "fingerprint": "fake"}, "grantedCapabilities": granted, "experimentalApi": False})
    elif method == "model/list":
        result(req_id, {"providerId": "fake", "profileId": None, "source": "fake", "models": [
            {"modelId": "muse-fake-1", "displayLabel": "Fake One", "providerId": "fake", "profileId": None, "releaseDate": None, "description": None, "contextLimit": 100000, "outputLimit": None, "cost": None, "isActive": False, "isDefault": True, "variants": "unknown"},
        ]})
    elif method in ("session/start", "session/resume"):
        if (msg["params"].get("config") or {}).get("mcpServers") and "sessionMcp" not in granted:
            emit({"jsonrpc": "2.0", "id": req_id, "error": {"code": -32010, "message": "session MCP configuration requires the sessionMcp capability", "data": {"kind": "capabilityRequired", "capability": "sessionMcp"}}})
            continue
        if method == "session/resume" and msg["params"].get("sessionId") == "missing-session":
            emit({"jsonrpc": "2.0", "id": req_id, "error": {"code": -32000, "message": "no such session"}})
            continue
        result(req_id, {"session": session_obj(), "viewCursor": "", "history": {"mode": "none"}, "pendingRequests": []})
    elif method == "skill/list":
        result(req_id, {"skills": [{"selector": "fake-skill", "displayName": "Fake Skill", "description": "A fake skill", "source": "user"}]})
    elif method == "turn/start":
        result(req_id, {"commandId": msg["params"]["commandId"], "status": "accepted", "turnId": msg["params"]["commandId"], "startedNewTurn": True, "disposition": "started"})
        run_turn(msg["params"])
    elif method == "turn/steer":
        result(req_id, {"commandId": msg["params"]["commandId"], "status": "accepted", "turnId": msg["params"]["expectedTurnId"], "startedNewTurn": False, "disposition": "steered"})
    elif method == "turn/interrupt":
        result(req_id, {"commandId": msg["params"]["commandId"], "status": "accepted"})
    elif method == "session/setModel":
        result(req_id, {"commandId": msg["params"]["commandId"], "status": "accepted"})
    elif method == "userInput/answer":
        # The harness answered our question; finish the turn with the label.
        labels = []
        for answer in msg["params"].get("answers", []):
            if "selectedLabel" in answer:
                labels.append(answer["selectedLabel"])
            labels.extend(answer.get("selectedLabels", []))
        result(req_id, {"commandId": msg["params"]["commandId"], "status": "accepted"})
        notify("item/completed", {"sessionId": SESSION_ID, "viewCursor": "6", "sourceRange": {}, "item": {"itemId": "m1", "kind": "agentMessage", "status": "completed", "text": "picked: " + ",".join(labels)}})
        notify("turn/completed", {"sessionId": SESSION_ID, "viewCursor": "9", "sourceRange": {}, "turnId": "turn-1", "terminal": "completed"})
    elif req_id is not None and method is None:
        pass  # a receipt for our server request
    elif req_id is not None:
        result(req_id, {})
