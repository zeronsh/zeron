#!/usr/bin/env python3
"""A fake agent from the ACP registry, for its sign-in: `initialize` offers
one method the agent runs itself; `authenticate` opens its page through
$BROWSER, an OAuth authorize URL whose redirect_uri is a loopback port this
agent listens on, and finishes once the browser lands there with a code
(as a real CLI's login does). Driven by tests/registry_login_tunnel.rs."""
import json
import os
import socket
import subprocess
import sys


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    method = message.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": message["id"], "result": {
            "protocolVersion": 1,
            "agentCapabilities": {},
            "authMethods": [{"id": "browser", "name": "Sign in with your browser"}],
        }})
    elif method == "authenticate":
        server = socket.socket()
        server.bind(("127.0.0.1", 0))
        server.listen(4)
        port = server.getsockname()[1]
        page = ("https://auth.example.com/authorize?client_id=fake&redirect_uri="
                f"http%3A%2F%2Flocalhost%3A{port}%2Fcallback")
        subprocess.run([os.environ["BROWSER"], page], check=False)
        print("Waiting for the browser…", file=sys.stderr, flush=True)
        while True:
            conn, _ = server.accept()
            request = conn.recv(4096).decode("latin-1")
            conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\nsigned in")
            conn.close()
            if "GET /callback?code=" in request:
                break
        send({"jsonrpc": "2.0", "id": message["id"], "result": {}})
    elif "id" in message:
        send({"jsonrpc": "2.0", "id": message["id"], "error": {"code": -32601, "message": "unsupported"}})
