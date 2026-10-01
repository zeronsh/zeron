#!/bin/sh
exec python3 -u -c '
import json, os, signal, subprocess, sys, time

def emit(value):
    print(json.dumps(value), end="\r\n", flush=True)
def update(text):
    emit({"method":"session/update","params":{"sessionId":"pi-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}}})
loaded = False
for line in sys.stdin:
    req = json.loads(line)
    method, params = req["method"], req.get("params", {})
    result = {}
    if method == "initialize":
        result = {"protocolVersion":1,"agentCapabilities":{"loadSession":True}}
    elif method == "session/new":
        result = {"sessionId":"pi-session", "configOptions":[{"id":"model","category":"model","type":"select","currentValue":"mock/old","options":[{"value":"mock/model","name":"Mock"},{"value":"mock/reject","name":"Rejected"}]},{"id":"thought_level","category":"thought_level","type":"select","currentValue":"off","options":[{"value":"xhigh","name":"Extra high"}]}]}
    elif method == "session/load":
        loaded = True
        if params["sessionId"] == "missing":
            emit({"id":req["id"],"error":{"code":-1,"message":"session missing"}})
            continue
        assert params["sessionId"] == "pi-session"
    elif method == "session/set_config_option":
        if params["value"] == "mock/reject":
            emit({"id":req["id"],"error":{"code":-1,"message":"model unavailable"}})
            continue
        assert params["value"] in ("mock/model", "xhigh"), params
    elif method == "session/prompt":
        text = params["prompt"][0]["text"]
        if text == "require-resume":
            assert loaded, "engine did not load stored Pi session"
        if text == "inherited-pipe-crash":
            subprocess.Popen(["sleep", "300"])
            print("pi fatal: inherited pipe context", file=sys.stderr, flush=True)
            sys.exit(25)
        if text == "crash":
            print("pi fatal: last stderr context", file=sys.stderr, flush=True)
            sys.exit(23)
        if text == "signal-crash":
            print("pi fatal: signal context", file=sys.stderr, flush=True)
            os.kill(os.getpid(), signal.SIGKILL)
        if text == "frames":
            print("not-json\r", flush=True)
            update("x" * (1024 * 1024 + 17))
        if text == "tree":
            child = subprocess.Popen(["sh", "-c", "trap \"\" TERM; sleep 300 & echo $!; wait"], stdout=subprocess.PIPE, text=True)
            update("tree:" + str(child.pid))
            update("tree:" + child.stdout.readline().strip())
            signal.signal(signal.SIGTERM, signal.SIG_IGN)
            time.sleep(300)
        if text == "interrupt-error":
            update("working")
            cancel = json.loads(sys.stdin.readline())
            assert cancel["method"] == "session/cancel"
            for _ in range(2):
                emit({"id":req["id"],"error":{"code":-1,"message":"aborted"}})
            continue
        update("reply:" + text)
        result = {"stopReason":"error" if text == "error" else "end_turn"}
        if text == "idle-crash":
            emit({"id":req["id"],"result":result})
            time.sleep(.1)
            sys.exit(24)
    if "id" in req:
        emit({"id":req["id"],"result":result})
'
