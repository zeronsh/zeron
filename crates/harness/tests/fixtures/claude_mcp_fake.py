#!/usr/bin/env python3
"""Credential-free, zero-model MCP fixture for MCP_VERIFICATION.md."""
import json
import os
import sys


def send(value):
    print(json.dumps(value), flush=True)


if os.getenv("FAKE_DUMP_ENV"):
    # Only synthetic values; never dump the full inherited environment.
    keys = ["CAPTURE_ENTRY", "CAPTURE_PARENT", "CAPTURE_EXPAND", "CAPTURE_ROOT",
            "CLAUDE_PLUGIN_ROOT", "CLAUDE_PLUGIN_DATA"]
    with open(os.environ["FAKE_DUMP_ENV"], "w") as f:
        json.dump({k: os.getenv(k) for k in keys}, f)

for line in sys.stdin:
    req = json.loads(line)
    method = req.get("method")
    if method == "initialize":
        caps = {"tools": {}, "prompts": {}}
        if os.getenv("FAKE_CHANNEL"):
            caps["experimental"] = {"claude/channel": {}}
        result = {"protocolVersion": "2024-11-05", "capabilities": caps,
                  "serverInfo": {"name": "capture-fixture", "version": "1.0.0"}}
    elif method == "tools/list":
        result = {"tools": [{"name": "browser_click", "description": "Synthetic click",
                             "inputSchema": {"type": "object", "properties": {}}}]}
    elif method == "prompts/list":
        result = {"prompts": []}
    elif method == "tools/call":
        if os.getenv("FAKE_CALL_LOG"):
            with open(os.environ["FAKE_CALL_LOG"], "a") as f:
                f.write(req.get("params", {}).get("name", "unknown") + "\n")
        result = {"content": [{"type": "text", "text": "fixture ok"}]}
    elif method == "notifications/initialized":
        if os.getenv("FAKE_CHANNEL"):
            # Emitting a channel event may schedule a model turn. The runner's
            # API endpoint is a local trap and its key is deliberately invalid.
            send({"jsonrpc": "2.0", "method": "notifications/claude/channel",
                  "params": {"content": "fixture channel event", "meta": {"test": "true"}}})
        continue
    elif "id" not in req:
        continue
    else:
        result = {}
    send({"jsonrpc": "2.0", "id": req["id"], "result": result})
