# The machines card's state, in one quick pass (reference.py SCRIPT_EACH,
# just before each snapshot: popups close once the window loses focus):
# Cloud on (the stub's own command), anything open closed, the composer's
# Work in picker opened from its place chip, and its Device row's list of
# machines beside it.
import re
if not globals().get("offered"):
    cmd({"cmd": "command", "id": "cloud_stub.offer"})
    wait(1.0)
for _ in range(2):
    cmd({"cmd": "key", "keys": "Escape"})
    wait(0.15)
print(cmd({"cmd": "click", "target": 'role:button "Branch"'}).get("text"))
wait(0.7)
print(cmd({"cmd": "click", "target": 'role:menuitem "Device"'}).get("text"))
wait(0.7)
ws = cmd({"cmd": "windows"}).get("text", "")
print(ws)
# (Each popup's tree, for the record and the specs.)
for m in re.finditer(r"popup @(\d+)", ws):
    t = cmd({"cmd": "tree", "target": f"@{m.group(1)}", "all": True, "budget": 100000})
    (out / f"popup-{m.group(1)}-tree.txt").write_text(t.get("text", ""))
# Snapshotted here, at once (a wait lets the window lose focus and the
# popups close): the window with the picker open, and each popup with its
# box, as compose-machines.py reads them.
import shutil
r = cmd({"cmd": "snapshot", "target": "@1"})
if (r.get("data") or {}).get("path"):
    shutil.copy(r["data"]["path"], out / "rest-open.png")
(out / "open-windows.txt").write_text(ws)
for m in re.finditer(r"popup @(\d+)", ws):
    p = cmd({"cmd": "snapshot", "target": f"@{m.group(1)}"})
    if (p.get("data") or {}).get("path"):
        shutil.copy(p["data"]["path"], out / f"rest-popup-{m.group(1)}.png")
