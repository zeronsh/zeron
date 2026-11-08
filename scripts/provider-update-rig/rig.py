#!/usr/bin/env python3
"""Provider-update rig (macOS, Linux): the real zeron-engine binary on a
throwaway port and data dir, a fake HOME, a GUI-like PATH, and sandboxed
installs. See README.md.

  RIG_DIR=/tmp/rig RIGCLIENT=…/rigclient rig.py setup
  RIG_DIR=/tmp/rig RIGCLIENT=…/rigclient rig.py run <engine-binary> <label> <scenario>...

Nothing outside RIG_DIR is touched: HOME, ZERON_DATA_DIR, npm/pnpm/bun
homes and caches all live under each scenario's own directory.
"""
import json, os, shutil, signal, socket, subprocess, sys, time

S = os.environ["RIG_DIR"]
CLIENT = os.environ["RIGCLIENT"]
NODE_T = os.path.join(S, "tools/node-template")
SYSNODE = os.path.join(S, "tools/sysnode/bin")
PNPM = os.path.join(S, "tools/pnpm/bin/pnpm")
BUN = os.path.join(S, "tools/bun/bin/bun")
GUI_PATH = "/usr/bin:/bin:/usr/sbin:/sbin"
PI = "@earendil-works/pi-coding-agent"
PI_OLD = "1.4.0"
ALL = ["claude-code", "codex", "cursor", "devin", "grok", "hermes", "pi", "opencode",
       "antigravity", "muse", "fx"]
PINS = {"claude-code": "CLAUDE_CODE_EXECUTABLE", "codex": "CODEX_EXECUTABLE", "cursor": "CURSOR_SDK_SHIM_EXECUTABLE",
        "devin": "DEVIN_EXECUTABLE", "grok": "GROK_EXECUTABLE", "hermes": "HERMES_EXECUTABLE", "pi": "PI_EXECUTABLE",
        "opencode": "OPENCODE_EXECUTABLE", "antigravity": "ANTIGRAVITY_ACP_EXECUTABLE", "muse": "MUSE_EXECUTABLE",
        "fx": "FX_EXECUTABLE"}


def free_port():
    s = socket.socket(); s.bind(("127.0.0.1", 0)); p = s.getsockname()[1]; s.close(); return p


def run(cmd, env, cwd=None, check=True):
    r = subprocess.run(cmd, env=env, cwd=cwd or S, capture_output=True, text=True)
    if check and r.returncode != 0:
        raise SystemExit(f"setup failed: {cmd}\n{r.stdout}\n{r.stderr}")
    return r.stdout.strip()


def write(path, text, mode=0o755):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w") as f:
        f.write(text)
    os.chmod(path, mode)


# ---- scenarios: each sets up an install in `home` and returns
# (harness, engine env additions, launcher, apply-watch seconds, extra) ----

def npm_nvm(home):
    """Pi installed with an nvm Node's own npm; the engine's PATH has no nvm."""
    prefix = os.path.join(home, ".nvm/versions/node/v24")
    shutil.copytree(NODE_T, prefix, symlinks=True)
    env = {"HOME": home, "PATH": f"{prefix}/bin:{GUI_PATH}", "npm_config_cache": f"{home}/.npm"}
    run([f"{prefix}/bin/npm", "install", "-g", f"{PI}@{PI_OLD}", "--no-audit", "--no-fund"], env)
    return "pi", {"PI_EXECUTABLE": f"{prefix}/bin/pi"}, f"{prefix}/bin/pi", 240, {}


def pnpm_home_under(platform=sys.platform):
    """Where `pnpm setup` puts PNPM_HOME on this system, under HOME."""
    return "Library/pnpm" if platform == "darwin" else ".local/share/pnpm"


def pnpm_global(home):
    """Pi installed with `pnpm add -g` (PNPM_HOME set in the user's shell only)."""
    pnpm_home = os.path.join(home, pnpm_home_under())
    os.makedirs(pnpm_home)
    env = {"HOME": home, "PNPM_HOME": pnpm_home, "PATH": f"{pnpm_home}:{SYSNODE}:{GUI_PATH}"}
    run([PNPM, "add", "-g", f"{PI}@{PI_OLD}"], env)
    return ("pi", {"PI_EXECUTABLE": f"{pnpm_home}/pi"}, f"{pnpm_home}/pi", 240,
            {"PATH_EXTRA": os.path.dirname(PNPM)})


def bun_global(home):
    """Pi installed with `bun add -g` (bun's default BUN_INSTALL, ~/.bun)."""
    bun_home = os.path.join(home, ".bun")
    os.makedirs(f"{bun_home}/bin")
    shutil.copy2(BUN, f"{bun_home}/bin/bun")
    env = {"HOME": home, "BUN_INSTALL": bun_home, "PATH": f"{bun_home}/bin:{SYSNODE}:{GUI_PATH}"}
    run([f"{bun_home}/bin/bun", "add", "-g", f"{PI}@{PI_OLD}"], env)
    return "pi", {"PI_EXECUTABLE": f"{bun_home}/bin/pi"}, f"{bun_home}/bin/pi", 240, {}


def hermes_channel(home):
    """Fake Hermes on a release channel (its real `--check` wording)."""
    root = os.path.join(home, ".hermes")
    write(f"{root}/state", "old\n", 0o644)
    write(f"{root}/bin/hermes", f"""#!/bin/sh
state=$(cat '{root}/state')
case "$1:$2" in
  --version:) if [ "$state" = new ]; then echo 'Hermes Agent v0.20.1'; else echo 'Hermes Agent v0.20.0'; fi ;;
  update:--check)
    echo '→ Update channel: stable'
    if [ "$state" = new ]; then echo '✓ Up to date with the latest release (v2026.11.2).';
    else echo '→ Selected release available: v2026.11.2'; echo '  Run `hermes update` to install it.'; fi ;;
  update:--yes) echo new > '{root}/state'; echo '→ Switching to source commit 1a2b3c4d5e' ;;
  *) exit 2 ;;
esac
""")
    return "hermes", {"HERMES_EXECUTABLE": f"{root}/bin/hermes"}, f"{root}/bin/hermes", 30, {}


def grok_prompt(home):
    """Fake Grok whose updater stops to ask, with nobody to answer."""
    root = os.path.join(home, ".grok")
    write(f"{root}/bin/grok", """#!/bin/sh
case "$1:$2" in
  version:) echo 'grok 1.0.0' ;;
  update:--check) echo 'update available: 1.0.0 -> 1.1.0' ;;
  update:) echo 'Downloading grok 1.1.0...'; printf 'An existing install was found. Replace it? [y/N] '; sleep 3600 ;;
  *) exit 2 ;;
esac
""")
    return "grok", {"GROK_EXECUTABLE": f"{root}/bin/grok"}, f"{root}/bin/grok", 45, {}


def grok_fails(home):
    """Fake Grok whose updater touches its install, then fails (like npm's EACCES)."""
    root = os.path.join(home, ".grok")
    write(f"{root}/bin/grok", """#!/bin/sh
case "$1:$2" in
  version:) echo 'grok 1.0.0' ;;
  update:--check) echo 'update available: 1.0.0 -> 1.1.0' ;;
  update:) printf '\\n# partly replaced\\n' >> "$0"; echo 'npm error code EACCES' >&2;
           echo 'npm error syscall rename' >&2; exit 1 ;;
  *) exit 2 ;;
esac
""")
    return "grok", {"GROK_EXECUTABLE": f"{root}/bin/grok"}, f"{root}/bin/grok", 8, {"view_after": True}


def cursor_native(home):
    """Fake cursor-agent in the native layout, two releases behind the feed."""
    old = "2026.08.11-e8db854"
    base = os.path.join(home, ".local/share/cursor-agent/versions")
    feed = subprocess.run(["curl", "-fsSL", "https://cursor.com/install"], capture_output=True, text=True).stdout
    new = feed.split("downloads.cursor.com/lab/")[1].split("/")[0]
    for v in (old,):
        write(f"{base}/{v}/cursor-agent", f"""#!/bin/sh
case "$1" in
  --version) echo '{v}' ;;
  update) mkdir -p '{base}/{new}'; printf '#!/bin/sh\\necho {new}\\n' > '{base}/{new}/cursor-agent';
          chmod +x '{base}/{new}/cursor-agent'; ln -sfn '{base}/{new}/cursor-agent' "$HOME/.local/bin/cursor-agent"; echo 'Updated.' ;;
  *) exit 2 ;;
esac
""")
    os.makedirs(f"{home}/.local/bin")
    os.symlink(f"{base}/{old}/cursor-agent", f"{home}/.local/bin/cursor-agent")
    return "cursor", {}, f"{home}/.local/bin/cursor-agent", 30, {}


def system_git(home):
    """Linux: a CLI the distribution's package manager owns. `git` stands in
    (its `version` answers like Grok's); nothing is applied, only checked."""
    return "grok", {"GROK_EXECUTABLE": "/usr/bin/git"}, "/usr/bin/git", 3, {"check_only": True}


SCENARIOS = {f.__name__: f for f in [npm_nvm, pnpm_global, bun_global, hermes_channel,
                                     grok_prompt, grok_fails, cursor_native, system_git]}


def client(*args, timeout=None):
    r = subprocess.run([CLIENT, *args], capture_output=True, text=True, timeout=timeout)
    return r.stdout.strip() + (("\n" + r.stderr.strip()) if r.returncode else "")


def engine_env(home, data, port, path, harness, extra_env, root):
    """The engine's whole environment. Every provider but the target is pinned
    to a path that doesn't exist: discovery also searches fixed places such as
    /usr/local/bin, and the engine checks every provider as it starts, before
    the rig can disable any. A target with no pin of its own (cursor_native) is
    found by discovery under the fake HOME."""
    pins = {var: os.path.join(root, "not-installed", h) for h, var in PINS.items() if h != harness}
    return {
        "HOME": home, "PATH": path, "TMPDIR": os.environ.get("TMPDIR", "/tmp"),
        "ZERON_DATA_DIR": data, "ZERON_IPC_PORT": str(port), "ZERON_NO_LOGIN_SHELL": "1",
        "ZERON_WORKOS_CLIENT_ID": "", "ZERON_EDGE_URL": "http://127.0.0.1:1",
        "ZERON_DISABLE_NOTIFICATIONS": "1", "ZERON_AUTO_UPDATE": "0",
        "XDG_CONFIG_HOME": f"{home}/.config", "XDG_DATA_HOME": f"{home}/.local/share",
        "XDG_CACHE_HOME": f"{home}/.cache", "RUST_LOG": "warn",
        **pins, **extra_env,
    }


def scenario(engine, label, name):
    root = os.path.join(S, "runs", f"{label}-{name}")
    shutil.rmtree(root, ignore_errors=True)
    home, data = os.path.join(root, "home"), os.path.join(root, "data")
    os.makedirs(home); os.makedirs(data)
    t0 = time.time()
    harness, extra_env, launcher, secs, extra = SCENARIOS[name](home)
    print(f"\n=== {label} :: {name} ({harness}) — setup {time.time()-t0:.1f}s")
    port = free_port()
    path = ":".join(p for p in [extra.get("PATH_EXTRA"), SYSNODE, GUI_PATH] if p)
    env = engine_env(home, data, port, path, harness, extra_env, root)
    log = open(os.path.join(root, "engine.log"), "w")
    proc = subprocess.Popen([engine], env=env, cwd=root, stdout=log, stderr=log, start_new_session=True)
    try:
        for h in ALL:
            client("call", str(port), data, "SetHarnessEnabled", json.dumps({"harness": h, "enabled": h == harness}))
        print("installed (asked directly):", run([launcher, "version" if harness == "grok" else "--version"], {"HOME": home, "PATH": path}, check=False).splitlines()[0:1])
        t = time.time()
        out = client("call", str(port), data, "CheckHarnessUpdates", json.dumps({"harness": harness}))
        try:
            rows = json.loads(out)
            row = next((r for r in rows if r.get("harness") == harness), rows)
            print(f"check ({time.time()-t:.1f}s): phase={row.get('phase')} installed={row.get('installedVersion')} "
                  f"latest={row.get('latestVersion')} canApply={row.get('canApply')} "
                  f"error={(row.get('error') or {}).get('message')} manual={row.get('manualCommand')}")
        except Exception:
            print(f"check ({time.time()-t:.1f}s): {out}")
        if extra.get("check_only"):
            print("status after the check:")
            print(client("watch", str(port), data, "WatchHarnessUpdates", "{}", "1", harness))
            return
        print(f"apply (watching {secs}s):")
        print(client("applywatch", str(port), data, harness, str(secs)))
        if extra.get("view_after"):
            print("a view opens (second WatchHarnessUpdates), watched 6s:")
            print(client("watch", str(port), data, "WatchHarnessUpdates", "{}", "6", harness))
        after = run([launcher, "version" if harness == "grok" else "--version"], {"HOME": home, "PATH": path}, check=False)
        print("installed afterwards (asked directly):", after.splitlines()[0:1])
    finally:
        client("call", str(port), data, "StopEngine", "{}")
        try:
            proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            pass
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        log.close()
        # An updater left asking (its own process group) goes too.
        subprocess.run(["pkill", "-f", root], capture_output=True)


def setup():
    """Copies of node and bun from PATH, plus npm and pnpm from the registry,
    all under RIG_DIR/tools. Reads the real tools, writes only there."""
    node = shutil.which("node")
    npm = shutil.which("npm")
    bun = shutil.which("bun")
    if not node or not npm or not bun:
        raise SystemExit("needs node, npm and bun on PATH to copy from")
    shutil.rmtree(os.path.join(S, "tools"), ignore_errors=True)
    os.makedirs(os.path.join(NODE_T, "bin"))
    shutil.copy2(os.path.realpath(node), os.path.join(NODE_T, "bin/node"))
    os.makedirs(SYSNODE)
    shutil.copy2(os.path.realpath(node), os.path.join(SYSNODE, "node"))
    os.makedirs(os.path.dirname(BUN))
    shutil.copy2(os.path.realpath(bun), BUN)
    home = os.path.join(S, "toolhome")
    os.makedirs(home, exist_ok=True)
    env = {"HOME": home, "PATH": f"{NODE_T}/bin:/usr/bin:/bin", "npm_config_cache": f"{home}/.npm"}
    # npm itself comes from the registry, not a copy: a distribution's npm needs
    # modules kept elsewhere on the system (Ubuntu: /usr/share/nodejs).
    run([npm, "install", "-g", "npm@10", "--prefix", NODE_T, "--no-audit", "--no-fund"], env)
    # npm hands its built-in config to the npm it installs, and a distribution's
    # says prefix=/usr/local. Global installs must land beside this node.
    builtin = os.path.join(NODE_T, "lib/node_modules/npm/npmrc")
    if os.path.exists(builtin):
        os.remove(builtin)
    prefix = run([f"{NODE_T}/bin/npm", "config", "get", "prefix"], env)
    if os.path.realpath(prefix) != os.path.realpath(NODE_T):
        raise SystemExit(f"the rig's npm installs into {prefix}, not {NODE_T}")
    run([f"{NODE_T}/bin/npm", "install", "-g", "pnpm@10", "--prefix", os.path.join(S, "tools/pnpm"),
         "--no-audit", "--no-fund"], env)
    print("tools ready under", os.path.join(S, "tools"))


if __name__ == "__main__":
    if sys.argv[1] == "setup":
        setup()
    else:
        engine, label, names = sys.argv[2], sys.argv[3], sys.argv[4:]
        for n in names:
            scenario(engine, label, n)
