#!/usr/bin/env python3
"""Opt-in live-test proxy: count production shim starts and inject an outage.

Set ZERON_CURSOR_STRESS_LAUNCH to the JSON printed by cursor_stability_probe
resolve, ZERON_CURSOR_STRESS_COUNTER to a disposable log path, and
ZERON_CURSOR_STRESS_OUTAGE_FLAG to a disposable flag path. This proxy does not
read or log credentials, prompts, or model payloads.
"""
import json
import os
import sys

with open(os.environ["ZERON_CURSOR_STRESS_LAUNCH"]) as source:
    launch = json.load(source)
with open(os.environ["ZERON_CURSOR_STRESS_COUNTER"], "a") as counter:
    counter.write(" ".join(sys.argv[1:]) + "\n")
flag = os.environ.get("ZERON_CURSOR_STRESS_OUTAGE_FLAG")
if flag and os.path.exists(flag):
    print(json.dumps({"ev": "fatal", "message": "Injected rate limit exceeded during stability test"}), flush=True)
    sys.exit(1)
os.execv(launch["exe"], [launch["exe"]] + launch["args"] + sys.argv[1:])
