#!/usr/bin/env python3
"""Renders the live-update motion graphic (live-update.gif) and the static
architecture diagram (architecture.png). Pure PIL + ffmpeg; run from this
directory:  python3 generate.py

The animation is schematic, not a screen recording: it shows what the design
guarantees (same PID, same agent and shell processes, no refused connection)
while the engine image and the window are replaced. The real-process evidence
is in scripts/live-update-demo.sh and apps/zeron/tests/live_handoff.rs.
"""
import math, os, subprocess, tempfile, shutil
from PIL import Image, ImageDraw, ImageFont

W, H, FPS = 960, 540, 20
BG = (14, 17, 23)
PANEL = (24, 29, 39)
LINE = (52, 60, 78)
TEXT = (226, 232, 240)
MUTED = (139, 150, 170)
GREEN = (74, 222, 128)
BLUE = (96, 165, 250)
AMBER = (251, 191, 36)
VIOLET = (167, 139, 250)
RED = (248, 113, 113)

def font(size, bold=False):
    names = ["/usr/share/fonts/noto/NotoSans-Bold.ttf" if bold else "/usr/share/fonts/noto/NotoSans-Regular.ttf",
             "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf" if bold else "/usr/share/fonts/TTF/DejaVuSans.ttf"]
    for n in names:
        if os.path.exists(n):
            return ImageFont.truetype(n, size)
    return ImageFont.load_default()

def mono(size):
    for n in ["/usr/share/fonts/noto/NotoSansMono-Regular.ttf", "/usr/share/fonts/TTF/DejaVuSansMono.ttf"]:
        if os.path.exists(n):
            return ImageFont.truetype(n, size)
    return ImageFont.load_default()

F_TITLE, F_H, F_B, F_S, F_M = font(30, True), font(19, True), font(15), font(13), mono(14)

def ease(t):
    t = max(0.0, min(1.0, t))
    return t * t * (3 - 2 * t)

def lerp(a, b, t):
    return a + (b - a) * t

def mix(c1, c2, t):
    return tuple(int(lerp(a, b, t)) for a, b in zip(c1, c2))

def box(d, xy, fill=PANEL, outline=LINE, r=12, width=2):
    d.rounded_rectangle(xy, r, fill=fill, outline=outline, width=width)

def text(d, xy, s, f=F_B, fill=TEXT, anchor="la"):
    d.text(xy, s, font=f, fill=fill, anchor=anchor)

def chip(d, x, y, label, color, f=F_S):
    w = d.textlength(label, font=f) + 20
    d.rounded_rectangle((x, y, x + w, y + 24), 12, fill=mix(BG, color, 0.22), outline=color, width=1)
    d.text((x + 10, y + 12), label, font=f, fill=color, anchor="lm")
    return w

def engine_card(d, x, y, w, h, version, version_color, alpha=1.0, pulse=0.0):
    outline = mix(LINE, version_color, 0.35 + 0.65 * pulse)
    box(d, (x, y, x + w, y + h), outline=outline, width=2 + int(2 * pulse))
    text(d, (x + 18, y + 14), "Engine host", F_H)
    chip(d, x + w - 92, y + 14, version, version_color)
    text(d, (x + 18, y + 46), "PID 41207", F_M, MUTED)

def child(d, x, y, label, pid, color, glow=0.0):
    box(d, (x, y, x + 210, y + 62), fill=mix(PANEL, color, 0.08 + 0.1 * glow), outline=mix(LINE, color, 0.5 + 0.5 * glow))
    text(d, (x + 14, y + 10), label, F_B)
    text(d, (x + 14, y + 34), pid, F_M, MUTED)
    d.ellipse((x + 186, y + 12, x + 198, y + 24), fill=color)

def window(d, x, y, w, h, version, color, lines, alpha=1.0):
    fill = mix(BG, PANEL, alpha)
    box(d, (x, y, x + w, y + h), fill=fill, outline=mix(BG, LINE, alpha))
    d.rounded_rectangle((x, y, x + w, y + 30), 12, fill=mix(BG, (34, 40, 54), alpha))
    for i, c in enumerate([(248, 113, 113), (251, 191, 36), (74, 222, 128)]):
        d.ellipse((x + 12 + i * 18, y + 10, x + 22 + i * 18, y + 20), fill=mix(BG, c, alpha))
    text(d, (x + w - 12, y + 15), version, F_S, mix(BG, color, alpha), "rm")
    yy = y + 44
    for s, c in lines:
        text(d, (x + 14, yy), s, F_S, mix(BG, c, alpha))
        yy += 22

def frame_scene(t):
    """t in seconds -> PIL image."""
    im = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(im)
    text(d, (40, 28), "Live updates", F_TITLE)
    text(d, (40, 68), "A new release installs itself. Nothing that is running notices.", F_B, MUTED)

    # timeline (seconds): 0-2 steady v0.2.99, 2-3.2 update lands, 3.2-5 exec, 5-6.5 window swap, 6.5-9 steady v0.3.0
    ver_old, ver_new = "v0.2.99", "v0.3.0"
    exec_t = ease((t - 3.2) / 0.6)
    swapped = t >= 3.4
    engine_ver = ver_new if swapped else ver_old
    engine_col = GREEN if swapped else BLUE
    pulse = 0.0
    if 3.1 <= t <= 4.3:
        pulse = math.sin((t - 3.1) / 1.2 * math.pi)
    # engine + children
    ex, ey = 500, 130
    engine_card(d, ex, ey, 420, 330, engine_ver, engine_col, pulse=pulse)
    if 2.4 <= t < 3.2:
        text(d, (ex + 18, ey + 72), "freezing agents and terminals…", F_S, AMBER)
    elif 3.2 <= t < 4.4:
        text(d, (ex + 18, ey + 72), "exec > " + ver_new + "  (same PID)", F_S, AMBER)
    elif t >= 4.4:
        text(d, (ex + 18, ey + 72), "adopted 2 processes · resumed", F_S, GREEN)
    else:
        text(d, (ex + 18, ey + 72), "serving", F_S, MUTED)
    glow = pulse
    child(d, ex + 18, ey + 108, "Agent · Claude Code", "PID 41310  (same)" if swapped else "PID 41310", VIOLET, glow)
    child(d, ex + 18, ey + 186, "Terminal · zsh", "PID 41355  (same)" if swapped else "PID 41355", AMBER, glow)
    child(d, ex + 18, ey + 264 - 0, "Dev server · :3000", "PID 41402  (same)" if swapped else "PID 41402", GREEN, glow) if False else None
    text(d, (ex + 250, ey + 130), "turn in flight", F_S, MUTED)
    text(d, (ex + 250, ey + 208), "sleep 300 &", F_M, MUTED)
    # window
    phase = ease((t - 5.0) / 1.2)
    old_lines = [("agent: refactoring parser…", TEXT), ("> edit src/lib.rs", MUTED), ("? Proceed with rename?", AMBER)]
    new_lines = old_lines + [("you: yes", BLUE), ("agent: done", GREEN)]
    if t < 5.0:
        window(d, 40, 130, 410, 330, ver_old, BLUE, old_lines)
    elif t < 6.2:
        window(d, 40, 130, 410, 330, ver_old, BLUE, old_lines, alpha=1 - phase)
        window(d, 40 + int(lerp(30, 0, phase)), 130, 410, 330, ver_new, GREEN, old_lines, alpha=phase)
    else:
        window(d, 40, 130, 410, 330, ver_new, GREEN, new_lines if t > 7.0 else old_lines)
    # connection line
    col = mix(LINE, GREEN, 0.7)
    gap = 3.15 <= t <= 3.75
    y = 300
    x0, x1 = 450, 500
    d.line((x0, y, x1, y), fill=AMBER if gap else col, width=3)
    if gap:
        text(d, (475, y - 24), "queued", F_S, AMBER, "mm")
    # banner
    if t < 2.0:
        chip(d, 40, 480, "v0.3.0 released", MUTED)
    elif t < 3.2:
        chip(d, 40, 480, "downloaded · verified · installed — no prompt", AMBER)
    elif t < 5.0:
        chip(d, 40, 480, "engine handed off in place · 0 refused connections", GREEN)
    elif t < 6.5:
        chip(d, 40, 480, "window swapped while idle", GREEN)
    else:
        chip(d, 40, 480, "Updated to v0.3.0", GREEN)
    text(d, (920, 495), "same PIDs · same agent · same shell", F_S, MUTED, "rm")
    return im

def architecture():
    im = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(im)
    text(d, (40, 28), "Architecture", F_TITLE)
    text(d, (40, 68), "Three layers, replaced independently — none by killing work.", F_B, MUTED)
    cols = [
        ("Window (UI process)", BLUE, ["replaced by a fast relaunch", "when idle or unfocused", "UI state carried across", "(ui-state.json)"], "attaches over ws://127.0.0.1"),
        ("Engine host", GREEN, ["replaced by execve in place", "same PID: children stay children", "IPC listener, lock, PTY masters,", "agent pipes survive; manifest is", "an unlinked anonymous file"], "spawns and owns"),
        ("Agents and shells", VIOLET, ["real processes, never restarted", "protocol state serialized and", "adopted by the new image", "rollback = re-exec the old binary"], ""),
    ]
    x = 40
    for title, color, lines, note in cols:
        box(d, (x, 130, x + 270, 430), outline=color)
        text(d, (x + 18, 146), title, F_H, color)
        yy = 190
        for s in lines:
            text(d, (x + 18, yy), s, F_S, TEXT)
            yy += 26
        if note:
            d.line((x + 270, 280, x + 305, 280), fill=MUTED, width=3)
            d.polygon([(x + 296, 272), (x + 296, 288), (x + 308, 280)], fill=MUTED)
        x += 305
    chip(d, 40, 460, "Windows: no engine handoff in v1 — install on quit, no prompt", AMBER)
    return im

def terminal_frames(lines, width=960, height=420):
    """Typewriter-style terminal recording of the REAL demo output."""
    f = mono(15)
    frames = []
    shown = []
    prompt = "$ scripts/live-update-demo.sh"

    def render(visible, cursor_line=None, typed=None):
        im = Image.new("RGB", (width, height), (12, 14, 20))
        d = ImageDraw.Draw(im)
        d.rounded_rectangle((0, 0, width, 34), 0, fill=(30, 35, 48))
        for i, c in enumerate([(248, 113, 113), (251, 191, 36), (74, 222, 128)]):
            d.ellipse((14 + i * 20, 12, 24 + i * 20, 22), fill=c)
        d.text((width // 2, 17), "zeron - live update demo (real processes)", font=F_S, fill=MUTED, anchor="mm")
        y = 48
        d.text((16, y), typed if typed is not None else prompt, font=f, fill=GREEN)
        y += 26
        for line in visible:
            col = TEXT
            if line.startswith("»"):
                col = TEXT
                if "unchanged" in line or "refused during the handoff: 0" in line or "answered" in line:
                    col = GREEN
                elif "handing" in line:
                    col = AMBER
            elif line.startswith("test result"):
                col = BLUE
            d.text((16, y), line, font=f, fill=col)
            y += 22
        return im

    for n in range(len(prompt) + 1):
        frames.append((render([], typed=prompt[:n]), 1))
    frames.append((render([]), 6))
    for line in lines:
        shown.append(line)
        frames.append((render(shown), 10 if line.startswith("»") else 6))
    frames.append((render(shown), 40))
    return frames


def main():
    out = os.path.dirname(os.path.abspath(__file__))
    tmp = tempfile.mkdtemp()
    try:
        total = 9.5
        n = int(total * FPS)
        for i in range(n):
            frame_scene(i / FPS).save(os.path.join(tmp, f"f{i:04d}.png"))
        pal = os.path.join(tmp, "pal.png")
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", os.path.join(tmp, "f%04d.png"),
                        "-vf", "palettegen=max_colors=96", pal], check=True)
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", os.path.join(tmp, "f%04d.png"),
                        "-i", pal, "-lavfi", "paletteuse=dither=bayer:bayer_scale=4", "-loop", "0",
                        os.path.join(out, "live-update.gif")], check=True)
        demo = os.path.join(out, "demo-output.txt")
        if os.path.exists(demo):
            lines = [l.rstrip("\n") for l in open(demo) if l.strip() and not l.startswith("Live update demo")]
            frames = terminal_frames(lines)
            n = 0
            for im, hold in frames:
                for _ in range(hold):
                    im.save(os.path.join(tmp, f"t{n:04d}.png"))
                    n += 1
            tpal = os.path.join(tmp, "tpal.png")
            subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", os.path.join(tmp, "t%04d.png"),
                            "-vf", "palettegen=max_colors=48", tpal], check=True)
            subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-framerate", str(FPS), "-i", os.path.join(tmp, "t%04d.png"),
                            "-i", tpal, "-lavfi", "paletteuse", "-loop", "0", os.path.join(out, "demo-terminal.gif")], check=True)
            frames[-1][0].save(os.path.join(out, "demo-terminal.png"))
        frame_scene(7.6).save(os.path.join(out, "after.png"))
        frame_scene(1.0).save(os.path.join(out, "before.png"))
        architecture().save(os.path.join(out, "architecture.png"))
    finally:
        shutil.rmtree(tmp)

if __name__ == "__main__":
    main()
