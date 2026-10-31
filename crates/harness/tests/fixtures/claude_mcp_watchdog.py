#!/usr/bin/env python3
"""V-P1: initialize the installed Chrome MCP, kill only its private group.
No browser/tool call. Telemetry goes only to a local fixture HTTP server.
Usage: python3 claude_mcp_watchdog.py SCRATCH CHROME_MCP_PACKAGE_ROOT
"""
import http.server
import json
import os
from pathlib import Path
import selectors
import shlex
import signal
import subprocess
import sys
import threading
import time

root, package = map(lambda p: Path(p).resolve(), sys.argv[1:])
root.mkdir(parents=True, exist_ok=True)
for name in ["home", "tmp", "config", "cache", "data"]:
    (root / name).mkdir(exist_ok=True)


class Telemetry(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", "0")))
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"{}")
    def log_message(self, *args):
        pass


http = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Telemetry)
threading.Thread(target=http.serve_forever, daemon=True).start()
args = ["/usr/bin/node", str(package / "build/src/bin/chrome-devtools-mcp.js"),
        f"--clearcut-endpoint=http://127.0.0.1:{http.server_port}"]
env = {"PATH": "/usr/bin:/bin", "HOME": str(root / "home"), "TMPDIR": str(root / "tmp"),
       "XDG_CONFIG_HOME": str(root / "config"), "XDG_DATA_HOME": str(root / "data"),
       "XDG_CACHE_HOME": str(root / "cache")}
p = subprocess.Popen(args, env=env, cwd=root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                     stderr=subprocess.PIPE, start_new_session=True)
watchdog = None
record = {"command": shlex.join(args), "package_version": json.loads((package / "package.json").read_text())["version"]}
try:
    p.stdin.write((json.dumps({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "zeron-verification", "version": "1"}}}) + "\n").encode())
    p.stdin.flush()
    selector = selectors.DefaultSelector()
    selector.register(p.stdout, selectors.EVENT_READ)
    if not selector.select(15):
        raise RuntimeError("Chrome MCP initialize timed out")
    record["initialize"] = json.loads(p.stdout.readline())
    selector.close()
    deadline = time.monotonic() + 5
    while watchdog is None and time.monotonic() < deadline:
        for entry in Path("/proc").iterdir():
            if not entry.name.isdigit():
                continue
            try:
                cmdline = entry.joinpath("cmdline").read_bytes().split(b"\0")
                if f"--parent-pid={p.pid}".encode() in cmdline and any(b"telemetry/watchdog/main.js" in s for s in cmdline):
                    watchdog = int(entry.name)
                    break
            except (OSError, ProcessLookupError):
                pass
        time.sleep(.02)
    if watchdog is None:
        raise RuntimeError("No watchdog appeared")
    record.update(server_pid=p.pid, server_pgid=os.getpgid(p.pid), watchdog_pid=watchdog,
                  watchdog_pgid=os.getpgid(watchdog))
    start = time.monotonic()
    os.killpg(p.pid, signal.SIGKILL)
    p.wait()
    deadline = start + 5
    exited = False
    while time.monotonic() < deadline:
        try:
            # A zombie has exited; the container's init may reap it later.
            stat = Path(f"/proc/{watchdog}/stat").read_text()
            if stat[stat.rfind(")") + 2:].split()[0] == "Z":
                exited = True
                break
        except FileNotFoundError:
            exited = True
            break
        time.sleep(.02)
    record.update(watchdog_exited=exited, elapsed_ms=round((time.monotonic() - start) * 1000, 1),
                  separate_group=record["server_pgid"] != record["watchdog_pgid"])
finally:
    if p.poll() is None:
        os.killpg(p.pid, signal.SIGKILL)
        p.wait()
    if watchdog:
        try:
            os.kill(watchdog, signal.SIGKILL)
        except ProcessLookupError:
            pass
    http.shutdown()
    record["stderr"] = p.stderr.read().decode(errors="replace")
    (root / "watchdog.json").write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps(record, indent=2))
