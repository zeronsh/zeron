#!/usr/bin/env python3
"""Fake ACP agent for the durability cases in tests/acp_hardening.rs.

The scenario comes from FAKE_ACP_MODE in the run environment, so it can shape
the handshake itself (auth, resume, crashes), not only the prompt.
"""
import json
import os
import subprocess
import sys

MODE = os.environ.get("FAKE_ACP_MODE", "")
SID = "s-1"


def emit(frame):
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()


def say(text):
    emit({"method": "session/update", "params": {"sessionId": SID, "update": {
        "sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}}}})


def fail(ident, code, message):
    emit({"id": ident, "error": {"code": code, "message": message}})


def initialize_result(params):
    caps = {"loadSession": True, "promptCapabilities": {"image": MODE == "image"}}
    result = {"protocolVersion": 1, "agentCapabilities": caps}
    if MODE == "resume":
        caps["loadSession"] = False
        caps["sessionCapabilities"] = {"resume": {}}
    if MODE == "auth":
        result["authMethods"] = [
            {"id": "cached_token", "name": "cached_token"},
            {"id": "browser", "name": "Sign in"},
        ]
        result["_meta"] = {"defaultAuthMethodId": "cached_token"}
    if MODE == "auth-terminal":
        result["authMethods"] = [{"id": "tty", "name": "Terminal login", "type": "terminal"}]
        result["_meta"] = {"defaultAuthMethodId": "tty"}
    if MODE == "grok-meta":
        assert "clientType" not in params.get("_meta", {}), params
    return result


authed = False
prompt = None
for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get("method")
    ident = frame.get("id")
    params = frame.get("params") or {}
    if method is None:
        # A client response: the answer to the question dialog.
        if ident == 91 and prompt is not None:
            outcome = frame["result"]["outcome"]["outcome"]
            say(f"question {outcome}")
            emit({"id": prompt, "result": {"stopReason": "cancelled"}})
            prompt = None
        continue
    if method == "initialize":
        emit({"id": ident, "result": initialize_result(params)})
    elif method == "authenticate":
        if params.get("methodId") == "cached_token":
            authed = True
            emit({"id": ident, "result": {}})
        else:
            fail(ident, -32602, "unexpected auth method")
    elif method in ("session/new", "session/load", "session/resume"):
        if MODE == "handshake-crash":
            # A descendant outside the process group keeps stdout open after
            # the adapter dies: EOF never comes.
            subprocess.Popen(["sleep", "8"], start_new_session=True)
            sys.stderr.write("adapter blew up\n")
            sys.stderr.flush()
            os._exit(3)
        if MODE.startswith("auth") and not authed:
            fail(ident, -32000, "Authentication required")
            continue
        if MODE == "resume" and method != "session/resume":
            fail(ident, -32601, f"{method} is not supported")
            continue
        if method == "session/new":
            result = {"sessionId": SID}
            if MODE == "config-hang":
                result["configOptions"] = [{
                    "id": "model", "name": "Model", "category": "model", "type": "select",
                    "currentValue": "grok-4.5",
                    "options": [{"value": "grok-4.5", "name": "Grok 4.5"},
                                {"value": "grok-4-fast", "name": "Grok 4 Fast"}]}]
            emit({"id": ident, "result": result})
        else:
            SID = params["sessionId"]
            emit({"id": ident, "result": {}})
    elif method == "session/set_config_option":
        if MODE != "config-hang":
            emit({"id": ident, "result": {}})
    elif method == "session/set_model":
        emit({"id": ident, "result": {}})
    elif method == "session/prompt":
        blocks = params["prompt"]
        if MODE == "resume":
            say(f"resumed {SID}")
        elif MODE.startswith("auth"):
            say("signed in")
        elif MODE in ("image", "image-unsupported"):
            kinds = [b.get("mimeType") or b["type"] for b in blocks]
            say(" ".join(kinds))
        elif MODE in ("stop-wedge", "grok-meta"):
            say("working")
            prompt = ident
            continue
        elif MODE == "question":
            say("asking")
            prompt = ident
            emit({"id": 91, "method": "session/request_permission", "params": {
                "sessionId": SID, "toolCall": {"toolCallId": "q1", "title": "Pick one"},
                "options": [{"optionId": "a", "name": "A"}, {"optionId": "b", "name": "B"}]}})
            continue
        emit({"id": ident, "result": {"stopReason": "end_turn"}})
    elif method == "session/cancel":
        if MODE == "grok-meta" and prompt is not None:
            trigger = params.get("_meta", {}).get("cancelTrigger")
            stop = "cancelled" if trigger == "ctrl_c" else "refusal"
            emit({"id": prompt, "result": {"stopReason": stop}})
            prompt = None
        # stop-wedge ignores the cancel; question waits for the dialog answer.
