"""Stage A: the text and image nodes of the app's layout tree that the
reference capture actually shows (a node's box must hold ink)."""
import json, re, sys
import numpy as np
from PIL import Image

from lib import REFS, P
ref = REFS[P]
S = ref.shape[1] / 1320
tree = json.load(open("tree.json"))
hidden = {int(m.group(1)) for m in re.finditer(r"@(\d+)[^\n]* hidden\s*$", open("tree.txt").read(), re.M)}

def ink(box):
    x, y, w, h = (int(round(v * S)) for v in box)
    patch = ref[max(y, 0):y + h, max(x, 0):x + w].reshape(-1, 3)
    if len(patch) == 0: return 0
    base = np.median(patch, axis=0)
    return int(np.abs(patch - base).sum(axis=1).max())

badge = next((n["box"] for n in tree if n.get("id") == "updates-button" and n["n"] not in hidden), None)
out = []
for i, n in enumerate(tree):
    if n["n"] in hidden: continue
    leaf = i + 1 >= len(tree) or tree[i + 1]["depth"] <= n["depth"]
    x, y, w, h = n["box"]
    if w <= 0 or h <= 0: continue
    if x >= 1320 or y >= 840: continue
    if badge and badge[0] - 2 <= x <= badge[0] + badge[2] and y < 40: continue  # the agents' updates badge: not part of the picture
    kind = "img" if n.get("role") == "img" else "text" if n.get("text") and (n.get("role") == "text" or (leaf and n.get("role") is None)) else None
    if not kind or ink(n["box"]) < 10: continue
    out.append({"n": n["n"], "kind": kind, "box": n["box"], **({"text": n["text"]} if kind == "text" else {}), **({"id": n["id"]} if n.get("id") else {})})
json.dump(out, open("nodes.json", "w"), ensure_ascii=False)
texts = [o for o in out if o["kind"] == "text"]
print(len(out), "nodes;", len(texts), "texts;", sum(o["kind"] == "img" for o in out), "imgs")
print(" | ".join(f'{t["text"][:18]}@{t["box"][0]},{t["box"][1]}' for t in texts))
