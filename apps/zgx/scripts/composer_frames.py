#!/usr/bin/env python3
"""Record the isolated composer fixture on Linux X11 and check its text region.

Needs Xvfb, xdotool and ffmpeg. Run normal and --damage-check separately: the
comparison repaint changes rendering behavior. Output is local diagnostic data.
"""
import argparse
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import time


def run(binary, output, damage_check):
    for tool in ("Xvfb", "xdotool", "ffmpeg"):
        if not shutil.which(tool):
            raise SystemExit(f"required executable missing: {tool}")
    output.mkdir(parents=True, exist_ok=True)
    events = []
    processes = []
    logs = []

    def launch(args, name, **kwargs):
        log = (output / name).open("w")
        logs.append(log)
        process = subprocess.Popen(args, stdout=log, stderr=subprocess.STDOUT, **kwargs)
        processes.append(process)
        return process

    try:
        read, write = os.pipe()
        launch(["Xvfb", "-displayfd", str(write), "-screen", "0", "1280x900x24",
                "-nolisten", "tcp"], "xvfb.log", pass_fds=(write,))
        os.close(write)
        display = ":" + os.read(read, 64).decode().strip()
        os.close(read)
        env = os.environ.copy()
        env.pop("WAYLAND_DISPLAY", None)
        env.pop("ZGX_DAMAGE_CHECK", None)
        env.update(DISPLAY=display, XDG_CONFIG_HOME=str(output / "config"),
                   XDG_DATA_HOME=str(output / "data"), ZERON_ZGX_ENGINE_SPAWN="0",
                   ZERON_ZGX_ENGINE_PORT="27963", ZERON_ZGX_ENGINE_DATA=str(output / "engine"))
        if damage_check:
            env["ZGX_DAMAGE_CHECK"] = "1"
        app = launch([str(binary), "fixture=composer_transition_preview.luau", "--inspect"],
                     "app.log", env=env)

        def x(*args, check=True):
            return subprocess.run(["xdotool", *map(str, args)], env=env,
                                  capture_output=True, text=True, check=check)

        window = None
        for _ in range(100):
            if app.poll() is not None:
                raise RuntimeError("app exited before its window appeared; see app.log")
            found = x("search", "--onlyvisible", "--pid", app.pid, check=False)
            if found.returncode == 0 and found.stdout.strip():
                window = found.stdout.splitlines()[-1]
                break
            time.sleep(.1)
        if window is None:
            raise RuntimeError("window did not appear within 10 seconds")
        x("windowsize", window, 1200, 800)
        x("windowfocus", "--sync", window)
        # Allow first presentation/font setup; all timings below are native
        # input pacing for a recording, never product readiness deadlines.
        time.sleep(1)
        video = output / "frames.mkv"
        recording = launch(["ffmpeg", "-y", "-loglevel", "error", "-f", "x11grab",
                            "-framerate", "60", "-video_size", "1280x900", "-i", display,
                            "-t", "12", "-c:v", "ffv1", "-threads", "2", str(video)],
                           "capture.log", env=env)
        start = time.monotonic()

        def click(name, at, delay):
            events.append({"seconds": round(time.monotonic() - start, 3), "action": name})
            x("mousemove", at, 32, "click", 1)
            time.sleep(delay)

        time.sleep(.5)
        click("ten lines", 115, .08)
        click("interrupt with eighty", 195, .06)
        click("reverse to short", 43, .08)
        click("eighty lines", 195, .5)
        click("attachment header", 279, .4)
        click("short with header", 43, .4)
        click("remove header", 279, .4)
        click("long draft for scroll/edit", 195, .4)
        x("mousemove", 150, 640, "click", 1)
        x("key", "ctrl+End")
        x("click", "--repeat", 5, "--delay", 30, 4)
        x("type", "--delay", 20, " typed after scrolling")
        events.append({"seconds": round(time.monotonic() - start, 3), "action": "scroll and character typing"})
        click("short", 43, .25)
        click("wrap", 470, .4)
        click("reverse wrap", 43, .08)
        click("interrupt wrap", 470, .3)
        click("reduce motion", 380, .1)
        click("short snap", 43, .05)
        click("long snap", 195, .2)
        # Move the pointer away before sampling CPU; caret blink and
        # engine reconnects remain valid background work in this fixture.
        x("mousemove", 900, 400, "click", 1)
        time.sleep(.5)

        def cpu_ticks():
            fields = Path(f"/proc/{app.pid}/stat").read_text().rsplit(")", 1)[1].split()
            return int(fields[11]) + int(fields[12])

        idle = []
        for _ in range(3):
            before, at = cpu_ticks(), time.monotonic()
            time.sleep(.6)
            idle.append(100 * (cpu_ticks() - before) / os.sysconf("SC_CLK_TCK") / (time.monotonic() - at))
        recording.wait(timeout=20)
        if recording.returncode:
            raise RuntimeError("capture failed; see capture.log")
        # This fixed fixture region excludes toolbar/footer/buttons. A
        # minimum > 50 cannot be satisfied by a caret alone. It detects whole
        # text-region disappearance, not every local glyph/clip defect.
        decode = subprocess.Popen(["ffmpeg", "-loglevel", "error", "-i", str(video),
                                   "-vf", "crop=540:360:50:390,format=gray", "-f", "rawvideo", "-"],
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        threshold = bytes(int(i > 120) for i in range(256))
        counts = []
        frame_size = 540 * 360
        while True:
            frame = decode.stdout.read(frame_size)
            if not frame:
                break
            if len(frame) != frame_size:
                raise RuntimeError("incomplete decoded frame")
            counts.append(frame.translate(threshold).count(b"\1"))
        if decode.wait():
            raise RuntimeError(decode.stderr.read().decode())
        app_log = (output / "app.log").read_text()
        stale = re.findall(r"ZGX_DAMAGE_CHECK frame .*", app_log)
        report = {"platform": platform.platform(), "content_size": [1200, 800],
                  "capture_size": [1280, 900], "fps": 60, "frames": len(counts),
                  "theme": "zeron-dark", "zoom": "100%", "transparency": "default frosted",
                  "damage_check": damage_check, "minimum_text_pixels": min(counts, default=0),
                  "suspicious_frames": [i for i, count in enumerate(counts) if count <= 50],
                  "damage_mismatches": stale, "idle_cpu_percent": idle,
                  "events": events, "text_pixels_by_frame": counts,
                  "limitations": "Whole-region check; native macOS/Windows and original glitch reproduction unverified."}
        (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({k: v for k, v in report.items() if k not in ("events", "text_pixels_by_frame")}, indent=2))
        if len(counts) < 600 or report["suspicious_frames"] or stale:
            raise SystemExit("frame validation failed; inspect report.json and frames.mkv")
    finally:
        for process in reversed(processes):
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
        for log in logs:
            log.close()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True,
                        help="development zeron-zgx build; beta omits the fixture CLI")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--damage-check", action="store_true")
    args = parser.parse_args()
    run(args.binary.resolve(), args.output.resolve(), args.damage_check)
