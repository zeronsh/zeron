#!/usr/bin/python3
"""Initialize peer that, like the real CLI, outlives SIGTERM until stdin ends."""
import json
import signal
import sys

signal.signal(signal.SIGTERM, signal.SIG_IGN)
request = json.loads(sys.stdin.readline())
response = {
    "subtype": "success",
    "request_id": request["request_id"],
    "response": {"commands": [{"name": "compact", "description": "Compact"}]},
}
print(json.dumps({"type": "control_response", "response": response}), flush=True)
sys.stdin.read()
