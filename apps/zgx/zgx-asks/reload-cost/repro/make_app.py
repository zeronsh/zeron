"""make_app.py: writes app/ (150 modules of 40 small functions, all required by app.luau,
which prints LOADED on each load) for run.sh."""
import os
here = os.path.dirname(os.path.abspath(__file__)); app = os.path.join(here, "app"); N = 150
os.makedirs(os.path.join(app, "mods"), exist_ok=True)
body = "\n".join(f"""local function f{j}(x: number): number
	local t = {{ a = x, b = x * 2, c = "s{j}" }}
	if t.a > t.b then return t.a - {j} else return t.b + {j} end
end""" for j in range(40))
for i in range(N):
    open(os.path.join(app, "mods", f"m{i}.luau"), "w").write(f"-- module {i}\n{body}\nreturn {{ name = \"m{i}\", f = f0 }}\n")
reqs = "\n".join(f'\trequire("./mods/m{i}"),' for i in range(N))
open(os.path.join(app, "app.luau"), "w").write(f'print("LOADED")\nlocal mods = {{\n{reqs}\n}}\nreturn ui.column {{ ui.text {{ `{{#mods}} modules` }} }}\n')
