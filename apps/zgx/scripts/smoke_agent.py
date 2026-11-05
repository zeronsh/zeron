#!/usr/bin/env python3
"""The smoke run's agent: a fake ACP agent (as Grok) that lives across turns.

crates/harness/tests/fixtures/fake-acp.sh plays one scripted turn and exits
(each harness test spawns its own); the engine keeps an agent's process for
the chat's next turn, so a second message to it would wait on a process that
is gone. This one answers every prompt, the way fake-acp.sh's `happy`
scenario does, and stays:

- the reply streams in pieces (a few words at a time, 120 ms apart), so the
  transcript shows it arriving;
- one `execute` tool (`echo smoke`) with its output, and one `edit` with an
  inline diff, so a reply has tool rows to open;
- a plan (the composer's todo chip);
- `end_turn`.

Models are advertised as fake-acp.sh does (grok-4-fast, grok-4.5), so the
catalog lists them. Stdout carries only ACP frames; nothing reads stderr.
"""
import json
import sys
import time

SESSION = {"id": "smoke-1"}
MODELS = {
    "models": {
        "availableModels": [
            {"modelId": "grok-4-fast", "name": "Grok 4 Fast", "description": "Fast tier"},
            {"modelId": "grok-4.5", "name": "Grok 4.5"},
        ],
        "currentModelId": "grok-4-fast",
    },
    "configOptions": [
        {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": "grok-4-fast",
         "options": [{"value": "grok-4-fast", "name": "Grok 4 Fast"}, {"value": "grok-4.5", "name": "Grok 4.5"}]},
        {"id": "effort", "name": "Reasoning effort", "category": "thought_level", "type": "select", "currentValue": "high",
         "options": [{"value": "low", "name": "Low"}, {"value": "medium", "name": "Medium"}, {"value": "high", "name": "High"}]},
    ],
}


def emit(frame):
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()


def update(body):
    emit({"method": "session/update", "params": {"sessionId": SESSION["id"], "update": body}})


def prompt_text(params):
    for block in params.get("prompt") or []:
        if block.get("type") == "text":
            return block.get("text") or ""
    return ""


def turn(ident, params, turns):
    words = prompt_text(params).split("\n")[0][:60]
    reply = f"Smoke reply {turns}: heard “{words}”. Streaming in a few pieces so the transcript shows it arrive."
    pieces = reply.split(" ")
    for at in range(0, len(pieces), 3):
        text = " ".join(pieces[at:at + 3]) + ("" if at + 3 >= len(pieces) else " ")
        update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text}})
        time.sleep(0.12)
    update({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "checking the project"}})
    update({"sessionUpdate": "tool_call", "toolCallId": f"t{turns}a", "title": "echo smoke", "kind": "execute",
            "status": "pending", "rawInput": {"command": "echo smoke"}})
    update({"sessionUpdate": "tool_call_update", "toolCallId": f"t{turns}a", "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "smoke\n"}}]})
    update({"sessionUpdate": "tool_call", "toolCallId": f"t{turns}b", "title": "edit notes.txt", "kind": "edit",
            "status": "completed",
            "content": [{"type": "diff", "path": "notes.txt", "oldText": "first line\nsecond line\n",
                         "newText": "first line\nsecond line, edited\n"}]})
    update({"sessionUpdate": "plan", "entries": [
        {"content": "read", "priority": "high", "status": "completed"},
        {"content": "reply", "priority": "high", "status": "completed"},
    ]})
    update({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": " Done."}})
    emit({"id": ident, "result": {"stopReason": "end_turn"}})


def main():
    turns = 0
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            frame = json.loads(line)
        except ValueError:
            continue
        method, ident = frame.get("method"), frame.get("id")
        params = frame.get("params") or {}
        if method is None:
            continue
        if method == "initialize":
            emit({"id": ident, "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}}})
        elif method == "session/new":
            emit({"id": ident, "result": {"sessionId": SESSION["id"], **MODELS}})
        elif method == "session/load":
            SESSION["id"] = params.get("sessionId") or SESSION["id"]
            emit({"id": ident, "result": MODELS})
        elif method == "session/prompt":
            turns += 1
            turn(ident, params, turns)
        elif method == "session/cancel":
            pass
        elif ident is not None:
            # session/set_model, session/set_config_option, session/set_mode, …
            emit({"id": ident, "result": {}})


if __name__ == "__main__":
    main()
