#!/usr/bin/env python3
"""Fake `opencode serve` (1.x wire) for crates/harness/tests/opencode_handoff.rs.

A real child process, so a run frozen for a live update hands over a real
server pid that must survive (not be killed) and be adopted. It keeps ONE
state for its whole life: a run that is frozen, handed over and adopted talks
to the same server.

- `serve --port N --hostname H`: listens on H:N; every request must carry
  HTTP Basic `opencode:$OPENCODE_SERVER_PASSWORD` (401 otherwise).
- `/global/health` answers `{healthy, version: "1.18.33"}` (the `/api/*`
  probes 404, like a 1.x server).
- `/global/event` is a broadcast SSE bus with NO replay, like the real one.
- a prompt (`POST /session/{id}/prompt_async`) runs a one-step turn: busy,
  an assistant message saying `echo: <text>`, idle.
- REST: `/session/status`, `/session/{id}/message`, `/session/{id}/children`,
  `/permission`, `/question` answer from the same state.

Writes one line to stderr at start (`fake-opencode: started`). Never writes
the password anywhere.
"""

import base64
import json
import os
import queue
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PASSWORD = os.environ.get("OPENCODE_SERVER_PASSWORD", "")
EXPECTED = "Basic " + base64.b64encode(("opencode:" + PASSWORD).encode()).decode()
SESSION = "ses_fake"

lock = threading.Lock()
subscribers = []
statuses = {}
messages = {SESSION: []}
counter = [0]


def now_ms():
    return int(time.time() * 1000)


def next_id(prefix):
    counter[0] += 1
    return f"{prefix}_{counter[0]:04d}"


def publish(payload):
    frame = ("data: " + json.dumps({"directory": "/", "payload": payload}) + "\n\n").encode()
    with lock:
        for q in list(subscribers):
            q.put(frame)


def run_turn(session, text):
    statuses[session] = {"type": "busy"}
    publish({"type": "session.status", "properties": {"sessionID": session, "status": {"type": "busy"}}})
    time.sleep(0.05)
    message = next_id("msg")
    info = {"id": message, "sessionID": session, "role": "assistant", "time": {"created": now_ms()}}
    part = {"id": next_id("prt"), "sessionID": session, "messageID": message, "type": "text",
            "text": "echo: " + text}
    with lock:
        messages.setdefault(session, []).append({"info": info, "parts": [part]})
    publish({"type": "message.updated", "properties": {"info": info}})
    publish({"type": "message.part.updated", "properties": {"part": part}})
    time.sleep(0.05)
    statuses[session] = {"type": "idle"}
    publish({"type": "session.status", "properties": {"sessionID": session, "status": {"type": "idle"}}})


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def reply(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def authorized(self):
        if self.headers.get("authorization") != EXPECTED:
            self.reply(401, {"error": "unauthorized"})
            return False
        return True

    def body(self):
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length) if length else b""
        try:
            return json.loads(raw) if raw else None
        except ValueError:
            return None

    def do_GET(self):
        if not self.authorized():
            return
        path = self.path.split("?")[0]
        if path == "/global/health":
            return self.reply(200, {"healthy": True, "version": "1.18.33"})
        if path == "/global/event":
            q = queue.Queue()
            with lock:
                subscribers.append(q)
            try:
                self.send_response(200)
                self.send_header("content-type", "text/event-stream")
                self.send_header("cache-control", "no-cache")
                self.send_header("connection", "close")
                self.end_headers()
                self.wfile.write(b'data: {"payload":{"type":"server.connected","properties":{}}}\n\n')
                self.wfile.flush()
                while True:
                    try:
                        frame = q.get(timeout=0.5)
                    except queue.Empty:
                        frame = b": keepalive\n\n"
                    self.wfile.write(frame)
                    self.wfile.flush()
            except (BrokenPipeError, ConnectionResetError, OSError):
                pass
            finally:
                with lock:
                    subscribers.remove(q)
                self.close_connection = True
            return
        if path == "/provider":
            return self.reply(200, {"all": [], "default": {}})
        if path == "/command":
            return self.reply(200, [])
        if path == "/session/status":
            return self.reply(200, statuses)
        if path in ("/permission", "/question"):
            return self.reply(200, [])
        if path.startswith("/session/") and path.endswith("/message"):
            session = path.split("/")[2]
            with lock:
                return self.reply(200, messages.get(session, []))
        if path.startswith("/session/") and path.endswith("/children"):
            return self.reply(200, [])
        return self.reply(404, {"missing": path})

    def do_POST(self):
        if not self.authorized():
            return
        path = self.path.split("?")[0]
        body = self.body() or {}
        if path == "/session":
            return self.reply(200, {"id": SESSION})
        if path.endswith("/prompt_async"):
            session = path.split("/")[2]
            text = "".join(p.get("text", "") for p in body.get("parts", []) if p.get("type") == "text")
            self.send_response(204)
            self.send_header("content-length", "0")
            self.end_headers()
            threading.Thread(target=run_turn, args=(session, text), daemon=True).start()
            return
        if path.endswith("/abort"):
            return self.reply(200, True)
        return self.reply(404, {"missing": path})


def main():
    args = sys.argv[1:]
    if not args or args[0] != "serve":
        print("usage: serve --port N --hostname H", file=sys.stderr)
        sys.exit(2)
    port = int(args[args.index("--port") + 1])
    host = args[args.index("--hostname") + 1]
    server = ThreadingHTTPServer((host, port), Handler)
    server.daemon_threads = True
    sys.stderr.write("fake-opencode: started\n")
    sys.stderr.flush()
    server.serve_forever()


if __name__ == "__main__":
    main()
