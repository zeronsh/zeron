#!/usr/bin/env python3
"""Run only control requests in a new credential-free HOME. See MCP_VERIFICATION.md.

Usage: python3 .../claude_mcp_verify.py SCRATCH CLAUDE PROXY
The proxy argument is the WU1 standalone zeron-mcp-proxy binary.
"""
import json
import http.server
import os
from pathlib import Path
import selectors
import shlex
import signal
import subprocess
import sys
import time
import threading

ROOT, CLI, PROXY = map(lambda p: Path(p).resolve(), sys.argv[1:])
ROOT.mkdir(parents=True, exist_ok=True)
HOME = ROOT / "home"
CONFIG = HOME / ".claude"
PROJECT = ROOT / "project"
PLUGIN = ROOT / "plugin"
FAKE = Path(__file__).with_name("claude_mcp_fake.py").resolve()
for path in [CONFIG, PROJECT, PLUGIN / ".claude-plugin", PLUGIN / "commands",
             PLUGIN / "skills" / "capture-skill", CONFIG / "plugins", ROOT / "tmp"]:
    path.mkdir(parents=True, exist_ok=True)


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def server(**env):
    # Claude suppresses plugins whose command+args duplicate manual servers.
    args = [str(FAKE), env.get("CAPTURE_ENTRY", "fixture")]
    if "CAPTURE_ROOT" in env:
        args.append("${CLAUDE_PLUGIN_ROOT}")
    return {"type": "stdio", "command": sys.executable, "args": args, "env": env}


# No real config or credentials are read or copied. All fixture values are new.
ENV = {"PATH": "/usr/local/bin:/usr/bin:/bin", "HOME": str(HOME),
       "CLAUDE_CONFIG_DIR": str(CONFIG), "TMPDIR": str(ROOT / "tmp"),
       "XDG_CONFIG_HOME": str(ROOT / "xdg"), "XDG_DATA_HOME": str(ROOT / "data"),
       "XDG_CACHE_HOME": str(ROOT / "cache"), "ZERON_DATA_DIR": str(ROOT / "data"),
       "CODEX_HOME": str(ROOT / "codex"), "GIT_CONFIG_GLOBAL": "/dev/null",
       "ANTHROPIC_API_KEY": "invalid-zeron-zero-token-verification",
       "ANTHROPIC_BASE_URL": "http://127.0.0.1:1",
       "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
       "CLAUDE_CODE_AUTO_CONNECT_IDE": "0", "CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL": "1",
       "CAPTURE_PARENT": "parent-value", "CAPTURE_EXPAND": "expanded-value"}
subprocess.run(["git", "init", "-q", str(PROJECT)], env=ENV, check=True)
write(PLUGIN / ".claude-plugin" / "plugin.json", {"name": "playwright", "version": "1.0.0"})
write(PLUGIN / ".mcp.json", {"playwright": server(
    CAPTURE_ENTRY="${CAPTURE_EXPAND}", CAPTURE_ROOT="${CLAUDE_PLUGIN_ROOT}",
    FAKE_DUMP_ENV=str(ROOT / "plugin-env.json"))})
(PLUGIN / "commands" / "capture-command.md").write_text("---\ndescription: Capture command\n---\nSynthetic command; do not execute.\n")
(PLUGIN / "skills" / "capture-skill" / "SKILL.md").write_text("---\nname: capture-skill\ndescription: Capture skill\n---\nSynthetic skill; do not execute.\n")
write(CONFIG / ".claude.json", {"mcpServers": {"user_fixture": server(CAPTURE_ENTRY="user")},
    "projects": {str(PROJECT): {"mcpServers": {"local_fixture": server(CAPTURE_ENTRY="local")},
        "enableAllProjectMcpServers": True}}})
write(CONFIG / "settings.json", {"permissions": {"allow": [
    "mcp__plugin_playwright_playwright__browser_click"]}})
write(PROJECT / ".mcp.json", {"mcpServers": {"project_fixture": server(CAPTURE_ENTRY="project")}})


def probe(name, flags, requests=(), cwd=PROJECT):
    args = [str(CLI), "-p", "--input-format", "stream-json", "--output-format", "stream-json",
            "--verbose", "--settings", '{"disableAllHooks":true}', *flags]
    frames, stderr, sent = [], [], []
    p = subprocess.Popen(args, cwd=cwd, env=ENV, stdin=subprocess.PIPE,
                         stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    sel = selectors.DefaultSelector()
    for stream in [p.stdout, p.stderr]:
        os.set_blocking(stream.fileno(), False)
        sel.register(stream, selectors.EVENT_READ, bytearray())

    def send(subtype, **fields):
        req = {"type": "control_request", "request_id": f"capture-{len(sent)}",
               "request": {"subtype": subtype, **fields}}
        sent.append(req)
        p.stdin.write((json.dumps(req) + "\n").encode())
        p.stdin.flush()
        return req["request_id"]

    def read_until(req_id, timeout=12):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for key, _ in sel.select(min(.2, max(0, deadline - time.monotonic()))):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    sel.unregister(key.fileobj)
                    continue
                key.data.extend(chunk)
                while b"\n" in key.data:
                    raw, _, rest = key.data.partition(b"\n")
                    key.data[:] = rest
                    if key.fileobj is p.stderr:
                        stderr.append(raw.decode(errors="replace"))
                        continue
                    try:
                        frame = json.loads(raw)
                    except ValueError:
                        continue
                    frames.append(frame)
                    if frame.get("response", {}).get("request_id") == req_id:
                        return frame
            if p.poll() is not None:
                break
        return {"timeout_or_exit": p.poll(), "request_id": req_id}

    try:
        read_until(send("initialize"))
        status = read_until(send("mcp_status"))
        # Some releases return a starting row; wait using controls only.
        for _ in range(20):
            rows = status.get("response", {}).get("response", {}).get("mcpServers", [])
            if not any(r.get("status") in ["pending", "connecting"] for r in rows):
                break
            time.sleep(.15)
            status = read_until(send("mcp_status"))
        for req in requests:
            read_until(send(**req))
    finally:
        try:
            os.killpg(p.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            p.wait(timeout=3)
        except subprocess.TimeoutExpired:
            os.killpg(p.pid, signal.SIGKILL)
            p.wait()
        sel.close()
    record = {"command": shlex.join(args), "cwd": str(cwd), "requests": sent,
              "frames": frames, "stderr": stderr, "exit": p.returncode}
    write(ROOT / (name + ".json"), record)
    rows = status.get("response", {}).get("response", {}).get("mcpServers", [])
    print(name, json.dumps(rows), flush=True)
    return record


if __name__ == "__main__":
    print(subprocess.check_output([str(CLI), "--version"], env=ENV, text=True).strip())
    probe("baseline", ["--plugin-dir", str(PLUGIN)])
    caps = {"mcpServers": {"zeron_caps": {"command": str(PROXY), "args": ["--data-dir", str(ROOT / "data"), "--selftest"]}}}
    probe("strict", ["--plugin-dir", str(PLUGIN), "--strict-mcp-config", "--mcp-config", json.dumps(caps)])
    dynamic = {"mcpServers": {"plugin_playwright_playwright": server(
        CAPTURE_ENTRY="${CAPTURE_EXPAND}", FAKE_DUMP_ENV=str(ROOT / "dynamic-env.json"))}}
    probe("normalized", ["--strict-mcp-config", "--mcp-config", json.dumps(dynamic)],
          [{"subtype": "list_permission_rules"}, {"subtype": "get_settings"},
           {"subtype": "mcp_call", "tool": "mcp__plugin_playwright_playwright__browser_click",
            "arguments": {}, "result_not_for_model": True}])
    proxy_env = {"CAPTURE_ENTRY": "${CAPTURE_EXPAND}", "FAKE_DUMP_ENV": str(ROOT / "proxy-child-env.json")}
    probe("proxy-env", ["--strict-mcp-config", "--mcp-config", json.dumps({"mcpServers": {
        "proxy_fixture": {"command": str(PROXY), "args": ["--server", "proxy_fixture", "--mode", "keep-alive",
            "--data-dir", str(ROOT / "data"), "--run-dir", str(ROOT / "proxy-run"), "--", sys.executable, str(FAKE), "proxy-env"], "env": proxy_env}}})])
    probe("override", ["--mcp-config", json.dumps({"mcpServers": {
        "user_fixture": server(CAPTURE_ENTRY="override")}})])
    # An installed plugin becomes @inline only on the fallback launch.
    (ROOT / ".claude-plugin").mkdir(exist_ok=True)
    write(ROOT / ".claude-plugin/marketplace.json", {"name": "fixture", "owner": {"name": "Fixture"},
        "plugins": [{"name": "playwright", "source": "./plugin", "description": "Synthetic plugin"}]})
    write(CONFIG / "plugins/known_marketplaces.json", {"fixture": {
        "source": {"source": "directory", "path": str(ROOT)}, "installLocation": str(ROOT),
        "lastUpdated": "2026-10-30T00:00:00Z"}})
    write(CONFIG / "plugins/installed_plugins.json", {"version": 2, "plugins": {
        "playwright@fixture": [{"scope": "user", "installPath": str(PLUGIN), "version": "1.0.0",
                                "installedAt": "2026-10-30T00:00:00Z", "lastUpdated": "2026-10-30T00:00:00Z"}]}})
    settings = json.loads((CONFIG / "settings.json").read_text())
    settings["enabledPlugins"] = {"playwright@fixture": True}
    write(CONFIG / "settings.json", settings)
    probe("installed-plugin", [])
    probe("no-mcp-plugin", ["--settings", '{"disableAllHooks":true,"enabledPlugins":{"playwright@fixture":false}}',
                            "--plugin-dir-no-mcp", str(PLUGIN)])
    probe("connector-redeclare", ["--strict-mcp-config", "--mcp-config", json.dumps({"mcpServers": {
        "connector_fixture": {"type": "claudeai-proxy", "url": "http://127.0.0.1:1/mcp",
                              "id": "fixture-id", "displayName": "Fixture", "scope": "claudeai"}}})])
    probe("channel", ["--strict-mcp-config", "--mcp-config", json.dumps({"mcpServers": {
        "channel_fixture": server(FAKE_CHANNEL="1")}})])
    probe("channel-enabled", ["--strict-mcp-config", "--dangerously-load-development-channels", "server:channel_fixture",
        "--mcp-config", json.dumps({"mcpServers": {"channel_fixture": server(FAKE_CHANNEL="1")}})],
        [{"subtype": "channel_enable", "serverName": "channel_fixture"}])

    class OAuthRequired(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Bearer realm="fixture"')
            self.end_headers()
        do_GET = do_POST
        def log_message(self, *args):
            pass

    http = http.server.ThreadingHTTPServer(("127.0.0.1", 0), OAuthRequired)
    threading.Thread(target=http.serve_forever, daemon=True).start()
    remote = {"mcpServers": {"oauth_fixture": {"type": "http", "url": f"http://127.0.0.1:{http.server_port}/mcp"}}}
    probe("http-strict", ["--strict-mcp-config", "--mcp-config", json.dumps(remote)])
    http.shutdown()
    # Test distinct project keys in a real worktree and in its subdirectory.
    subprocess.run(["git", "-C", str(PROJECT), "-c", "user.name=Daniel Katulevskiy", "-c", "user.email=katulevskiy@gmail.com",
                    "commit", "-q", "--allow-empty", "-m", "fixture"], env=ENV, check=True)
    worktree = ROOT / "worktree"
    subprocess.run(["git", "-C", str(PROJECT), "worktree", "add", "-q", "-b", "fixture", str(worktree)], env=ENV, check=True)
    nested = worktree / "nested"
    nested.mkdir()
    config = json.loads((CONFIG / ".claude.json").read_text())
    for name, path in [("main_key", PROJECT), ("worktree_key", worktree), ("nested_key", nested)]:
        config.setdefault("projects", {})[str(path)] = {"mcpServers": {name: server(CAPTURE_ENTRY=name)}}
    write(CONFIG / ".claude.json", config)
    probe("worktree-key", [], cwd=worktree)
    probe("nested-key", [], cwd=nested)
