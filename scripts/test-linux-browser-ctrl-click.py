#!/usr/bin/env python3
"""Exercise real WebKit pointer clicks without launching a system browser.

Requires a graphical DISPLAY and the Linux browser build dependencies. The
helper runs offscreen; its external-open packets are inspected by this test.
"""
import json
import os
from pathlib import Path
import queue
import shlex
import struct
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = b'''<!doctype html><style>a { display: block; width: 300px; height: 40px; }</style>
<a id="same" href="/destination?q=full#fragment">Same window</a>
<a id="new" href="/destination?q=new#fragment" target="_blank">New window</a>
<a id="blocked" href="http://user:pass@127.0.0.1/blocked">Credentials</a>'''
        self.send_response(200)
        self.send_header("Content-Type", "text/html")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def main():
    source = Path(__file__).resolve().parents[1] / "crates/ui/src/browser/linux/helper.c"
    flags = shlex.split(subprocess.check_output(
        ["pkg-config", "--cflags", "--libs", "webkit2gtk-4.1", "json-glib-1.0"], text=True))
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    origin = f"http://127.0.0.1:{server.server_port}"
    with tempfile.TemporaryDirectory(prefix="zeron-ctrl-click-") as directory:
        binary = Path(directory) / "zeron-webkit"
        subprocess.run(shlex.split(os.environ.get("CC", "cc")) + [
            "-std=c11", "-O2", str(source), "-o", str(binary), *flags], check=True)
        with open(Path(directory) / "helper.log", "w+") as log:
            process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=log)
            events = queue.Queue()

            def read_exact(length):
                result = bytearray()
                while len(result) < length:
                    chunk = process.stdout.read(length - len(result))
                    if not chunk:
                        raise EOFError("WebKit helper stopped")
                    result.extend(chunk)
                return bytes(result)

            def read_packets():
                try:
                    while True:
                        header = read_exact(9)
                        kind, page, length = struct.unpack("<cII", header)
                        payload = read_exact(length)
                        if kind != b"F":  # Pixel frames aren't needed for routing assertions.
                            events.put((kind, page, payload))
                except Exception as error:
                    events.put(error)

            threading.Thread(target=read_packets, daemon=True).start()

            def send(command, **fields):
                payload = json.dumps(dict(id=1, cmd=command, **fields)).encode()
                process.stdin.write(struct.pack("<I", len(payload)) + payload)
                process.stdin.flush()

            def wait_for(predicate, timeout=15):
                deadline = time.monotonic() + timeout
                while time.monotonic() < deadline:
                    event = events.get(timeout=max(0.01, deadline - time.monotonic()))
                    if isinstance(event, Exception):
                        log.seek(0)
                        raise RuntimeError(log.read()) from event
                    if predicate(event):
                        return event
                raise TimeoutError("Expected WebKit packet did not arrive")

            def evaluate(script):
                send("eval", script=script)
                return json.loads(wait_for(lambda event: event[0] == b"J")[2])

            def click(element, control):
                x, y = evaluate(f"(()=>{{const r=document.getElementById('{element}').getBoundingClientRect();return [r.x+10,r.y+10]}})()")
                for command in ["move", "down", "up"]:
                    send(command, x=x, y=y, button=1, mods=4 if control else 0)

            try:
                send("create")
                send("load", url=origin + "/")
                wait_for(lambda event: event[0] == b"S" and
                         json.loads(event[2]).get("url") == origin + "/" and
                         not json.loads(event[2]).get("loading"))
                for element, query in [("same", "full"), ("new", "new")]:
                    click(element, True)
                    event = wait_for(lambda event: event[0] in (b"O", b"N"))
                    assert event[0] == b"O", "Ctrl+click must request an external browser"
                    assert event[2].decode() == origin + f"/destination?q={query}#fragment"
                    assert evaluate("location.href") == origin + "/", "Ctrl+click navigated internally"
                click("blocked", True)
                assert evaluate("location.href") == origin + "/"
                while not events.empty():
                    event = events.get_nowait()
                    assert not isinstance(event, Exception)
                    assert event[0] not in (b"O", b"N"), "Rejected destination escaped validation"
                click("new", False)
                event = wait_for(lambda event: event[0] in (b"O", b"N"))
                assert event[0] == b"N", "Normal target=_blank click must retain its internal tab behavior"
                click("same", False)
                wait_for(lambda event: event[0] == b"S" and
                         json.loads(event[2]).get("url") == origin + "/destination?q=full#fragment")
                print("PASS: Ctrl+click external routing, full URL, unchanged page, rejected credentials, and normal navigation")
            finally:
                process.terminate()
                process.wait(timeout=5)
    server.shutdown()


if __name__ == "__main__":
    main()
