"""Stage B: which font each text node is set in, found by measuring it in
the browser with the page's own font files against the app's box."""
import os, pathlib
ROOT = pathlib.Path(__file__).resolve().parents[3]
CHROME = os.environ.get("CHROME", "chromium")
import json, subprocess, re, html
nodes = [n for n in json.load(open("nodes.json")) if n["kind"] == "text"]
FONTS = f"{ROOT}/apps/landing/src/assets/fonts"
page = f"""<!doctype html><meta charset=utf-8><style>
@font-face {{ font-family: G; src: url(file://{FONTS}/geist.woff2); font-weight: 100 900; }}
@font-face {{ font-family: M; src: url(file://{FONTS}/geist-mono.woff2); font-weight: 100 900; }}
</style><pre id=out>pending</pre><script>
const nodes = {json.dumps(nodes)};
(async () => {{
  for (const f of ["G", "M"]) for (const w of [400, 500, 600]) await document.fonts.load(`${{w}} 13px ${{f}}`);
  const ctx = document.createElement("canvas").getContext("2d");
  const out = {{}};
  for (const n of nodes) {{
    const [x, y, w, h] = n.box, best = [];
    for (const f of ["G", "M"]) for (const wt of [400, 500, 600]) for (let s = 10; s <= 18; s += 1) {{
      ctx.font = `${{wt}} ${{s}}px ${{f}}`;
      const m = ctx.measureText(n.text).width;
      const natural = Math.ceil(1.4 * s) === h;
      best.push([Math.abs(m - w) + (natural ? 0 : 0.75), f, wt, s, +m.toFixed(2)]);
    }}
    best.sort((a, b) => a[0] - b[0]);
    out[n.n] = best.slice(0, 3);
  }}
  document.getElementById("out").textContent = JSON.stringify(out);
}})();
</script>"""
open("fit.html", "w").write(page)
dom = subprocess.run([CHROME, "--headless=new", "--no-sandbox", "--disable-gpu", "--allow-file-access-from-files", "--virtual-time-budget=4000", "--dump-dom", "file://" + __import__("os").path.abspath("fit.html")], capture_output=True, text=True).stdout
fits = json.loads(html.unescape(re.search(r'<pre id="out">(.*?)</pre>', dom, re.S).group(1)))
json.dump(fits, open("fits.json", "w"))
for n in nodes:
    if n["box"][3] > 24: continue
    a, b = fits[str(n["n"])][0], fits[str(n["n"])][1]
    print(f'{n["text"][:26]:26} box {n["box"][2]}x{n["box"][3]}  -> {a[1]} {a[2]} {a[3]}px err {a[0]:.2f} (w {a[4]})   next {b[1]} {b[2]} {b[3]} err {b[0]:.2f}')
