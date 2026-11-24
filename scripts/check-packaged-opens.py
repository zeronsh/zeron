#!/usr/bin/env python3
"""Fails if a packaged Zeron doesn't open its first window, or opens one from
a bundle whose zgx.json was changed after the build.

  check-packaged-opens.py <folder with the package's `zeron`> [--macos-app]

Nothing of the person's is used or shown: a copy of the package without its
engine (so none is started), a home and data folder thrown away after, an IPC
port nothing serves (never Zeron's own), proxies that go nowhere, and no
screen: an Xvfb of its own on Linux, zgx's offscreen windows on macOS. On
Linux its session bus is its own too, one that starts no services (the
desktop's portals, which would outlive the run). Each run is its own process
group, ended with it.
"""

import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

OPENED = "first frame presented"
MARK = re.compile(r"profile mark .* ms: (.+)$")
ENGINES = ("zeron-engine", "Zeron Engine")
OUR_PORTS = {27654, 27655, 27871}
WAIT = 90
BUS = """<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={path}</listen>
  <policy context="default">
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
    <allow own="*"/>
  </policy>
</busconfig>
"""


def free_port():
    while True:
        with socket.socket() as found:
            found.bind(("127.0.0.1", 0))
            port = found.getsockname()[1]
        if port not in OUR_PORTS:
            return port


def bus(run):
    """A session bus of its own in `run`, and its address."""
    path, config = os.path.join(run, "bus"), os.path.join(run, "bus.conf")
    with open(config, "w") as file:
        file.write(BUS.format(path=path))
    daemon = subprocess.Popen(
        ["dbus-daemon", f"--config-file={config}", "--nofork", "--nopidfile"],
        env={"PATH": "/usr/bin:/bin"}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
    )
    ended = time.monotonic() + 10
    while not os.path.exists(path):
        if daemon.poll() is not None or time.monotonic() > ended:
            end(daemon)
            sys.exit("its own session bus (dbus-daemon) didn't start")
        time.sleep(0.05)
    return daemon, f"unix:path={path}"


def end(process):
    """Ends `process` and all it started."""
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


def opened(binary, work, display):
    """Whether `binary` presents a first frame, and what it said."""
    home = tempfile.mkdtemp(prefix="home-", dir=work)
    run = tempfile.mkdtemp(prefix="run-", dir=work)
    os.chmod(run, 0o700)
    nowhere = "http://127.0.0.1:9"
    env = {
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin",
        "HOME": home,
        "ZERON_DATA_DIR": os.path.join(home, "data"),
        "ZERON_IPC_PORT": str(free_port()),
        "XDG_RUNTIME_DIR": run,
        "TMPDIR": run,
        "ZGX_PROFILE": "1",
        "HTTP_PROXY": nowhere,
        "HTTPS_PROXY": nowhere,
        "ALL_PROXY": nowhere,
    }
    daemon = None
    if display:
        env["DISPLAY"] = display
        daemon, env["DBUS_SESSION_BUS_ADDRESS"] = bus(run)
    else:
        env["ZGX_OFFSCREEN"] = "1"
    os.makedirs(env["ZERON_DATA_DIR"])
    log = os.path.join(work, "said.log")
    with open(log, "wb") as said:
        process = subprocess.Popen([binary], env=env, stdin=subprocess.DEVNULL, stdout=said, stderr=said, start_new_session=True)
    ended = time.monotonic() + WAIT
    ok = False
    try:
        while time.monotonic() < ended:
            with open(log, errors="replace") as said:
                ok = any((found := MARK.search(line)) and found.group(1).strip() == OPENED for line in said)
            if ok or process.poll() is not None:
                break
            time.sleep(0.2)
    finally:
        end(process)
        if daemon:
            end(daemon)
    with open(log, errors="replace") as said:
        return ok, said.read()


def main() -> int:
    macos = "--macos-app" in sys.argv
    package = [arg for arg in sys.argv[1:] if not arg.startswith("--")][0]
    work = tempfile.mkdtemp(prefix="zeron-opens-")
    screen = None
    try:
        copy = os.path.join(work, "package")
        shutil.copytree(package, copy, symlinks=True, ignore=shutil.ignore_patterns(*ENGINES))
        binary = os.path.join(copy, "MacOS/zeron" if macos else "zeron")
        root = os.path.join(copy, "Resources") if macos else copy
        display = None
        if not macos:
            read, write = os.pipe()
            screen = subprocess.Popen(
                ["Xvfb", "-displayfd", str(write), "-screen", "0", "1600x1000x24", "-nolisten", "tcp"],
                pass_fds=[write], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True,
            )
            os.close(write)
            with os.fdopen(read) as told:
                display = ":" + told.readline().strip()
        if macos:
            subprocess.run(["codesign", "--force", "--sign", "-", binary], check=True, capture_output=True)
        ok, said = opened(binary, work, display)
        if not ok:
            print(f"{package} didn't open its first window:\n{said[-3000:]}", file=sys.stderr)
            return 1
        print(f"{package}: opened its first window")
        if not os.path.exists(os.path.join(root, "manifest.json")):
            return 0
        # A bundle: its windows are its checked files', or it doesn't open.
        with open(os.path.join(root, "ui", "zgx.json"), "a") as config:
            config.write("\n")
        ok, said = opened(binary, work, display)
        if ok or "isn't the file its bundle's manifest names" not in said:
            print(f"{package}: opened, or said nothing, with its zgx.json changed since the build:\n{said[-3000:]}", file=sys.stderr)
            return 1
        print(f"{package}: refuses a zgx.json changed since the build")
        return 0
    finally:
        if screen:
            end(screen)
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
