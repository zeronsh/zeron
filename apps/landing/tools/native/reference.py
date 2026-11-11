#!/usr/bin/env python3
"""Draw the real app's new chat (the reference the web hero is held to).

One fixture window of a release zeron-zgx, on a scratch HOME / data dir
(never ~/.zeron), ZGX_CHECK=0, no engine; driven over its inspect socket in
one quick pass, then killed. Pictures are zgx's own `snapshot` of the root.

usage: reference.py <zeron-zgx> <ui dir from make-ui.py> <out dir> [shots]
  shots: comma list of name=x:y (pointer 0..1), default rest=0.5:0.5
  env DUMP=1 also writes the layout tree (tree.txt).
"""
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
from pathlib import Path

binary, ui, out = Path(sys.argv[1]), Path(sys.argv[2]), Path(sys.argv[3])
shots = sys.argv[4] if len(sys.argv) > 4 else "rest=0.5:0.5"
out.mkdir(parents=True, exist_ok=True)

scratch = Path(tempfile.mkdtemp(prefix="landing-ref-"))
home = scratch / "home"
(home / "Library/Application Support/zeron").mkdir(parents=True)
for marker in ("onboarded", "welcomed"):
    (home / "Library/Application Support/zeron" / marker).touch()
tmp = scratch / "t"
tmp.mkdir()
env = {
    # (No keychain: a scratch HOME with the real PATH pops dialogs.)
    "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
    "HOME": str(home),
    "TMPDIR": str(tmp) + "/",
    "ZERON_DATA_DIR": str(scratch / "data"),
    "ZERON_ZGX_ENGINE_SPAWN": "0",
    "ZERON_ZGX_ENGINE_PORT": "27897",
    "ZERON_ZGX_ENGINE_DATA": str(scratch / "engine"),
    "ZERON_ZGX_UI": str(ui),
    "ZGX_CHECK": "0",
    "ZGX_RENDER_WHEN_COVERED": "1",
}
log = open(out / "app.log", "w")
proc = subprocess.Popen([str(binary), "fixture=landing_hero_fixture.luau", "--inspect"], env=env, stdout=log, stderr=log, cwd=str(scratch))


def sock_path():
    for d in tmp.rglob(f"{proc.pid}.sock"):
        return d
    return None


def cmd(obj, timeout=30):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    s.connect(str(sock_path()))
    s.sendall((json.dumps(obj) + "\n").encode())
    buf = b""
    while not buf.endswith(b"\n"):
        d = s.recv(1 << 22)
        if not d:
            break
        buf += d
    s.close()
    return json.loads(buf)


def wait(seconds):
    end = time.time() + seconds
    while time.time() < end:
        if proc.poll() is not None:
            raise SystemExit(f"the app quit: see {out / 'app.log'}")
        time.sleep(0.05)


try:
    for _ in range(200):
        if sock_path():
            break
        wait(0.1)
    else:
        raise SystemExit("no inspect socket")
    wait(2.5)
    print("errors:", cmd({"cmd": "errors"}).get("text"))
    if os.environ.get("DUMP"):
        r = cmd({"cmd": "tree", "target": "@1", "all": True, "budget": 100000})
        (out / "tree.txt").write_text(r.get("text", ""))
        (out / "tree.json").write_text(json.dumps(r.get("data")))
        if os.environ.get("DUMP") == "nodes":
            nodes = {}
            for line in r.get("data") or []:
                n = line["n"]
                nodes[n] = cmd({"cmd": "node", "target": f"@{n}"}).get("data")
            (out / "nodes.json").write_text(json.dumps(nodes, indent=1))
    # SETS="name=json;name=json": states set before the shots.
    for item in filter(None, os.environ.get("SETS", "").split(";")):
        k, v = item.split("=", 1)
        print("set", k, cmd({"cmd": "set", "name": k, "value": json.loads(v)}).get("text"))
    if os.environ.get("SETS"):
        wait(1.5)
    # MOTION="x:y": from rest, the pointer to x, y; snapshots as fast as
    # they come for 1.2 s, each with its time since the set (motion.json).
    if os.environ.get("MOTION"):
        mx, my = (float(v) for v in os.environ["MOTION"].split(":"))
        times = []
        t0 = time.monotonic()
        cmd({"cmd": "set", "name": "shell_wallpaper_px", "value": mx})
        cmd({"cmd": "set", "name": "shell_wallpaper_py", "value": my})
        t_set = (time.monotonic() + t0) / 2
        # (The layers' boxes from the tree, to the point: fast enough to
        # sample a spring; pictures are too slow, ~270 ms each.)
        while time.monotonic() - t_set < 1.2:
            before = time.monotonic()
            r = cmd({"cmd": "tree", "target": "#parallax-scene", "all": True})
            after = time.monotonic()
            boxes = [l["box"] for l in r.get("data") or [] if l.get("role") == "img" and "id" not in l]
            times.append({"t": (before + after) / 2 - t_set, "spread": after - before, "boxes": boxes})
        (out / "motion.json").write_text(json.dumps(times, indent=1))
        shots = ""
    for shot in filter(None, shots.split(",")):
        name, xy = shot.split("=")
        x, y = (float(v) for v in xy.split(":"))
        for state, v in (("shell_wallpaper_px", x), ("shell_wallpaper_py", y)):
            r = cmd({"cmd": "set", "name": os.environ.get("STATE_PREFIX", "") + state, "value": v})
            if "set" not in str(r.get("text", "")):
                print("set:", r)
        wait(float(os.environ.get("SETTLE", "2.5")))
        r = cmd({"cmd": "snapshot", "target": "@1"})
        path = (r.get("data") or {}).get("path") if isinstance(r, dict) else None
        if path:
            shutil.copy(path, out / f"{name}.png")
        else:
            print("snapshot:", str(r)[:400])
        print(name, "done")
finally:
    proc.send_signal(signal.SIGTERM)
    try:
        proc.wait(5)
    except subprocess.TimeoutExpired:
        proc.kill()
    shutil.rmtree(scratch, ignore_errors=True)
