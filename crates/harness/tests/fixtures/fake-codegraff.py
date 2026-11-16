#!/usr/bin/env python3
"""ACP contract fixture for Codegraff (`graff acp`)."""
import json
import sys

model = "chatgpt-new/gpt-5.4"
client_name = None

def emit(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    msg = json.loads(line)
    method, params = msg.get("method"), msg.get("params", {})
    result = {}
    if method == "initialize":
        client_name = params.get("clientInfo", {}).get("name")
        result = dict(
            protocolVersion=1,
            agentCapabilities=dict(loadSession=True),
            authMethods=[dict(id="graff-login", name="graff login", type="terminal", args=["login"])],
        )
    elif method in ("session/new", "session/load"):
        result = dict(
            sessionId="codegraff-session",
            configOptions=[
                dict(
                    id="model",
                    name="Model",
                    category="model",
                    type="select",
                    currentValue=model,
                    options=[
                        dict(value="chatgpt-new/gpt-5.4", name="GPT-5.4 \u00b7 chatgpt-new"),
                        dict(value="anthropic/claude-sonnet-4-6", name="Claude Sonnet 4.6 \u00b7 anthropic"),
                    ],
                ),
                dict(
                    id="mode",
                    name="Mode",
                    category="mode",
                    type="select",
                    currentValue="ask",
                    options=[dict(value="ask", name="Ask"), dict(value="auto", name="Auto")],
                ),
                dict(
                    id="effort",
                    name="Reasoning",
                    category="thought_level",
                    type="select",
                    currentValue="medium",
                    options=[
                        dict(value="low", name="Low"),
                        dict(value="medium", name="Medium"),
                        dict(value="high", name="High"),
                    ],
                ),
            ],
        )
    elif method == "session/set_config_option":
        key, value = params["configId"], params["value"]
        if key == "model" and value != "anthropic/rejected-model":
            model = value
        elif key in ("mode", "effort"):
            pass
        else:
            emit(dict(id=msg["id"], error=dict(code=-32602, message="invalid selection")))
            continue
    elif method == "session/prompt":
        if client_name != "zeron":
            emit(dict(id=msg["id"], error=dict(code=-32602, message=f"unexpected clientInfo {client_name}")))
            continue
        emit(
            dict(
                method="session/update",
                params=dict(
                    sessionId="codegraff-session",
                    update=dict(
                        sessionUpdate="agent_message_chunk",
                        content=dict(type="text", text=f"codegraff:{model}"),
                    ),
                ),
            )
        )
        result = dict(stopReason="end_turn")
    if "id" in msg:
        emit(dict(id=msg["id"], result=result))
