#!/usr/bin/env python3
"""Linux proxy-only PSS, with a cached idle peer and an outstanding forwarded call.
Usage: python3 crates/mcp-proxy/tests/measure_pss.py TARGET/debug/zeron-mcp-proxy TARGET/debug/fake-mcp
"""
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import tempfile
import time

proxy, fake = map(lambda p: str(Path(p).resolve()), sys.argv[1:])
root = Path(os.environ["CARGO_TARGET_DIR"]) / "mcp-proxy-tests"
root.mkdir(exist_ok=True, parents=True)
with tempfile.TemporaryDirectory(dir=root, prefix="pss-") as directory:
    data = Path(directory)
    env = dict(os.environ, ZERON_DATA_DIR=directory, FAKE_SPAWN_MARK=str(data / "spawn"),
               ZERON_MCP_PROXY_CHECK_MS="20", FAKE_CALL_DELAY_MS="2000")
    init = {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
        "protocolVersion": "2025-03-26", "clientInfo": {"name": "pss", "version": "1"}, "capabilities": {}}}

    def start():
        return subprocess.Popen([proxy, "--data-dir", directory, "--server", "fake", "--mode", "lazy", "--", fake],
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                text=True, env=env)

    def send(peer, value):
        peer.stdin.write(json.dumps(value) + "\n")
        peer.stdin.flush()

    def response(peer, request_id):
        while True:
            line = peer.stdout.readline()
            if not line:
                raise RuntimeError("unexpected EOF")
            message = json.loads(line)
            if message.get("id") == request_id:
                return message

    def pss(peer):
        samples = []
        for _ in range(7):
            with open(f"/proc/{peer.pid}/smaps_rollup") as rollup:
                samples.append(next(int(line.split()[1]) for line in rollup if line.startswith("Pss:")))
            time.sleep(0.05)
        return {"median_kib": statistics.median(samples), "min_kib": min(samples), "max_kib": max(samples)}

    seed = start()
    send(seed, init)
    assert "result" in response(seed, 1)
    seed.stdin.close()
    assert seed.wait(timeout=5) == 0
    peer = start()
    try:
        send(peer, init)
        assert "result" in response(peer, 1)
        idle = pss(peer)
        assert len((data / "spawn").read_text().splitlines()) == 1
        send(peer, {"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "echo"}})
        time.sleep(0.1)
        forwarding = pss(peer)
        assert response(peer, 2)["result"]["isError"] is False
        print(json.dumps({"binary": proxy, "unit": "KiB PSS (proxy PID only)", "cached_idle": idle,
                          "forwarding": forwarding, "samples_per_state": 7}, indent=2))
    finally:
        peer.stdin.close()
        if peer.wait(timeout=5) != 0:
            raise RuntimeError("proxy failed")
