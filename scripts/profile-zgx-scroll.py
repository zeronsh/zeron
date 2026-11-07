#!/usr/bin/env python3
"""Repeatable Linux CPU/RAM measurements of the real zgx transcript.

Use a dedicated Xvfb display. CPU is percent of one core, including renderer
threads. RSS/PSS are sampled from smaps_rollup; neither is GPU memory. The
fixture runs without a chat engine. This does not measure physical scanout.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import socket
import statistics
import subprocess
import threading
import time


def request(pid, **fields):
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(45)
        stream.connect(f"/tmp/zgx-inspect-{os.getuid()}/{pid}.sock")
        stream.sendall((json.dumps(fields) + "\n").encode())
        answer = json.loads(stream.makefile().readline())
    if not answer.get("ok"):
        raise RuntimeError(answer)
    return answer


def tape(hz):
    lines = ["# zgx wheel tape 1"]
    for i in range(hz * 4 + 1):
        delta = -900 / hz if i < hz or 2 * hz <= i < 3 * hz else 900 / hz
        phase = "started" if i == 0 else "ended" if i == hz * 4 else "moved"
        lines.append(f"{i * 1000 / hz:.6f} pixel 0 {0 if i == hz * 4 else delta:.6f} {phase}")
    return "\n".join(lines) + "\n"


def cpu_seconds(pid):
    # comm may contain spaces and parentheses; fields after its final ')' start
    # with state (field 3). utime/stime are fields 14/15.
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")


def memory(pid):
    values = {}
    for line in Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
        key, _, value = line.partition(":")
        if key in ("Rss", "Pss"):
            values[key.lower() + "_mib"] = int(value.split()[0]) / 1024
    return values


def measure(pid, action):
    samples = []
    stop = threading.Event()

    def sample():
        while not stop.is_set():
            samples.append(memory(pid))
            stop.wait(0.1)

    thread = threading.Thread(target=sample)
    before_cpu, before = cpu_seconds(pid), time.monotonic()
    thread.start()
    try:
        result = action()
    finally:
        stop.set()
        thread.join()
    elapsed = time.monotonic() - before
    consumed = cpu_seconds(pid) - before_cpu
    return {
        "elapsed_s": elapsed,
        "cpu_s": consumed,
        "cpu_percent_one_core": 100 * consumed / elapsed,
        "memory_samples": len(samples),
        **{
            f"{key}_{stat}": fn(s[key] for s in samples)
            for key in ("rss_mib", "pss_mib")
            for stat, fn in (("median", statistics.median), ("peak", max))
        },
        "result": result,
    }


def position(pid):
    request(pid, cmd="scroll", target="#transcript", dy=1000000)
    request(pid, cmd="scroll", target="#transcript", dy=-1500)
    time.sleep(0.4)


def run(args, label, binary, index):
    directory = args.out / f"{index:02d}-{label}"
    directory.mkdir(parents=True)
    env = os.environ.copy()
    # Clear measurement toggles so the two binaries use their own defaults.
    for key in list(env):
        if key.startswith("ZGX_") or key.startswith("ZERON_ZGX_"):
            env.pop(key)
    env.update(
        DISPLAY=args.display,
        WAYLAND_DISPLAY="",
        WINIT_UNIX_BACKEND="x11",
        ZERON_ZGX_UI=str(args.ui),
        XDG_CONFIG_HOME=str(directory / "config"),
        XDG_DATA_HOME=str(directory / "data"),
        VK_ICD_FILENAMES="/usr/share/vulkan/icd.d/lvp_icd.x86_64.json",
        LP_NUM_THREADS="4",
    )
    with (directory / "app.log").open("w") as log:
        process = subprocess.Popen(
            [str(binary), "fixture=chat_fixture.luau", "--inspect"],
            env=env, stdout=log, stderr=log,
        )
        try:
            deadline = time.monotonic() + 30
            sock = Path(f"/tmp/zgx-inspect-{os.getuid()}/{process.pid}.sock")
            while not sock.exists():
                if process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError(f"App did not start; see {directory / 'app.log'}")
                time.sleep(0.1)
            time.sleep(1)
            request(process.pid, cmd="command", id="go_s1")
            request(process.pid, cmd="frames", keep=3000)
            # Warm all rows the measured stroke will cross, in both directions.
            position(process.pid)
            request(process.pid, cmd="replay", target="#transcript", tape=tape(120), repeat=2)
            errors = request(process.pid, cmd="errors")["text"]
            if errors != "none":
                raise RuntimeError(errors)
            time.sleep(2)
            phases = {"idle_before": measure(process.pid, lambda: time.sleep(3))}
            for hz in (60, 120):
                position(process.pid)
                phases[f"scroll_{hz}"] = measure(
                    process.pid,
                    lambda hz=hz: request(
                        process.pid, cmd="replay", target="#transcript",
                        tape=tape(hz), repeat=args.replays,
                    ),
                )
            time.sleep(2)
            phases["idle_after"] = measure(process.pid, lambda: time.sleep(3))
            return {"label": label, "launch": index, "phases": phases}
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--variant", action="append", required=True, metavar="LABEL=BINARY")
    parser.add_argument("--ui", type=Path, required=True, help="Frozen copy of apps/zgx/ui")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--display", default=":109")
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--replays", type=int, default=2)
    args = parser.parse_args()
    if args.rounds < 1 or not 1 <= args.replays <= 20:
        parser.error("rounds must be positive; replays must be in 1..20")
    args.ui = args.ui.resolve()
    args.out.mkdir(parents=True)
    variants = [(label, Path(binary).resolve()) for label, binary in
                (entry.split("=", 1) for entry in args.variant)]
    report = {
        "host": platform.platform(),
        "renderer": "Vulkan llvmpipe, 4 threads; Xvfb; nominal 60Hz fallback",
        "cpu_units": "percent of ONE core; all app threads; engine excluded",
        "ram_units": "MiB of process RSS/PSS; GPU memory excluded",
        "sample_interval_s": 0.1,
        "replays_per_phase": args.replays,
        "binaries": {label: {"path": str(binary), "sha256": hashlib.sha256(binary.read_bytes()).hexdigest()}
                     for label, binary in variants},
        "ui_sha256": hashlib.sha256(b"".join(
            str(path.relative_to(args.ui)).encode() + b"\0" + path.read_bytes()
            for path in sorted(args.ui.rglob("*")) if path.is_file()
        )).hexdigest(),
        "runs": [],
    }
    for round_index in range(args.rounds):
        # Two variants and two rounds give A/B/B/A.
        for label, binary in (variants if round_index % 2 == 0 else variants[::-1]):
            result = run(args, label, binary, len(report["runs"]))
            report["runs"].append(result)
            (args.out / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            print(label, {phase: {k: round(v, 2) for k, v in data.items()
                                 if k in ("cpu_percent_one_core", "rss_mib_median", "pss_mib_median")}
                          for phase, data in result["phases"].items()}, flush=True)


if __name__ == "__main__":
    main()
