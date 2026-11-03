#!/usr/bin/env python3
"""A fake agent from the ACP registry, signing in the ways real ones do
(`FAKE_SIGNIN`), from their wire shapes on Nov 2 2026:

  droid   a session answers auth_required with a device code ("Your code:
          …"); `authenticate` then opens the pairing page with the system's
          `open` and waits for the pairing ($HOME/paired); before a code,
          `authenticate` answers {} at once and signs nothing in.
  droid-stale  as droid, but holding credentials it rejects: a session
          answers "Authentication failed" with no code, and `authenticate`
          does nothing, until `logout` (offered as auth.logout) clears them.
  droid-cli  `authenticate` never signs anything in; it says it signs in in
          a terminal (agentCapabilities._meta["terminal-auth"]): its own CLI,
          run bare.
  kimi    one method in the older `_meta["terminal-auth"]` form, its own
          `command` and `args: ["login"]`: a device code printed, then done.
  gemini  `authenticate` opens its page with the system's `open` (never
          $BROWSER) and waits for its loopback callback; a request that
          isn't the callback ends the sign-in.

Driven by tests/acp_registry_sign_in.rs."""
import json
import os
import socket
import subprocess
import sys
import time

mode = os.environ.get("FAKE_SIGNIN", "droid")
code = None
signed_in = False
stale = mode == "droid-stale"
if len(sys.argv) > 1 and sys.argv[1] == "login":
    print("Please visit the following URL to finish authorization.", flush=True)
    print("Verification URL: https://auth.example.com/device?user_code=ABCD-1234", flush=True)
    sys.exit(0)
if "--acp" not in sys.argv:
    # The agent's own CLI, bare: interactive, it needs a terminal.
    print("Welcome! Type /login to sign in.", flush=True)
    sys.exit(3)


def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    try:
        message = json.loads(line)
    except ValueError:
        continue
    method, mid = message.get("method"), message.get("id")
    if method == "initialize":
        caps = {"loadSession": True}
        if mode in ("droid", "droid-stale", "droid-cli"):
            caps["_meta"] = {"terminal-auth": True}
        if mode in ("droid", "droid-stale"):
            caps["auth"] = {"logout": {}}
            methods = [{"id": "device-pairing", "name": "Login"}, {"id": "api-key", "name": "API key"}]
        elif mode == "kimi":
            methods = [{"id": "login", "name": "Login with Kimi account", "_meta": {"terminal-auth": {
                "command": os.path.abspath(sys.argv[0]), "args": ["login"], "label": "Login", "env": {}, "type": "terminal"}}}]
        else:
            methods = [{"id": "oauth-personal", "name": "Log in with Google"}]
        send({"jsonrpc": "2.0", "id": mid, "result": {"protocolVersion": 1, "agentCapabilities": caps, "authMethods": methods}})
    elif method == "authenticate":
        if mode in ("droid", "droid-stale") and code:
            subprocess.run(["open", f"https://auth.example.com/device?user_code={code}"], check=False)
            paired = os.path.join(os.environ.get("HOME", "/"), "paired")
            while not os.path.exists(paired):
                time.sleep(0.05)
            signed_in = True
            send({"jsonrpc": "2.0", "id": mid, "result": {}})
        elif mode == "gemini":
            server = socket.socket()
            server.bind(("127.0.0.1", 0))
            server.listen(1)
            port = server.getsockname()[1]
            subprocess.run(["open", f"https://accounts.example.com/auth?redirect_uri=http%3A%2F%2F127.0.0.1%3A{port}%2Foauth2callback"], check=False)
            conn, _ = server.accept()
            request = conn.recv(4096).decode("latin-1")
            conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
            conn.close()
            if "/oauth2callback?code=" in request:
                signed_in = True
                send({"jsonrpc": "2.0", "id": mid, "result": {}})
            else:
                send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32000, "message": "OAuth callback not received. Unexpected request: /"}})
        else:
            send({"jsonrpc": "2.0", "id": mid, "result": {}})
    elif method == "session/new":
        if mode == "droid-stale" and stale:
            send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32000, "message": "Authentication required: Authentication failed. Please log in using /login or set a valid FACTORY_API_KEY environment variable."}})
        elif mode in ("droid", "droid-stale") and not signed_in:
            code = "WXYZ-9876"
            send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32000, "message": f"Authentication required: \n\nYour code: {code}\n\nClick the \"Login\" button to authenticate."}})
        elif mode == "droid-cli":
            send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32000, "message": "Authentication required: Please log in using /login"}})
        elif mode == "gemini" and not signed_in:
            send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32000, "message": "Authentication required"}})
        else:
            send({"jsonrpc": "2.0", "id": mid, "result": {"sessionId": "s-1"}})
    elif method == "logout":
        stale = False
        send({"jsonrpc": "2.0", "id": mid, "result": {}})
    elif mid is not None:
        send({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "unsupported"}})
