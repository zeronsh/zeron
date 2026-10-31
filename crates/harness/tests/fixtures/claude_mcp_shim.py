#!/usr/bin/env python3
"""H5 native Claude + production command builder; synthetic profiles, zero tokens.
Usage: claude_mcp_shim.py SCRATCH CLAUDE PROXY HARNESS_FIXTURE
All children use an allowlisted environment. Never reads a real config/keychain.
"""
import http.server
import json
import os
from pathlib import Path
import selectors
import shlex
import signal
import statistics
import subprocess
import sys
import threading
import time

ROOT, CLI, PROXY, HARNESS = [Path(p).resolve() for p in sys.argv[1:]]
FAKE = PROXY.with_name('fake-mcp')
# System-managed config cannot be isolated by CLAUDE_CONFIG_DIR. Refuse it.
for managed in [Path('/etc/claude-code/managed-mcp.json'), Path('/etc/claude-code/managed-settings.json'),
                *Path('/etc/claude-code/managed-settings.d').glob('*')]:
    assert not managed.exists(), f'fixture refuses machine-wide Claude configuration: {managed}'
CONFIG, PROJECT, PLUGIN = ROOT / 'home/.claude', ROOT / 'project', ROOT / 'plugin'
DATA = ROOT / 'data'
for p in [CONFIG, PROJECT, PLUGIN / '.claude-plugin', ROOT / 'tmp', DATA]:
    p.mkdir(parents=True, exist_ok=True)
TOOL = 'mcp__plugin_playwright_playwright__echo'
SPAWNS, CHILDREN = ROOT / 'spawns', ROOT / 'children'


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def lines(path):
    return path.read_text().splitlines() if path.exists() else []


def alive(pid):
    try:
        return Path(f'/proc/{pid}/stat').read_text().split(') ')[1].split()[0] != 'Z'
    except FileNotFoundError:
        return False


def wait_until(predicate, label, timeout=15):
    deadline = time.monotonic() + timeout
    while not predicate():
        assert time.monotonic() < deadline, label
        time.sleep(.05)


def tree(pid):
    children = {}
    for stat in Path('/proc').glob('[0-9]*/stat'):
        try:
            fields = stat.read_text().split(') ')[1].split()
            if fields[0] != 'Z':
                children.setdefault(int(fields[1]), []).append(int(stat.parent.name))
        except (FileNotFoundError, ProcessLookupError):
            pass
    result = set()
    pending = [pid]
    while pending:
        for child in children.get(pending.pop(), []):
            if child not in result:
                result.add(child)
                pending.append(child)
    return result


def pss(pids):
    total = 0
    for pid in pids:
        try:
            for line in Path(f'/proc/{pid}/smaps_rollup').read_text().splitlines():
                if line.startswith('Pss:'):
                    total += int(line.split()[1])
        except (FileNotFoundError, ProcessLookupError):
            pass
    return total


# A real runtime launches the Rust fake; wrapper and ballast both count in PSS.
launcher = ROOT / 'server.js'
launcher.write_text('const {spawn}=require("child_process");\n'
    f'const p=spawn({json.dumps(str(FAKE))},process.argv.slice(2),{{stdio:"inherit",env:process.env}});\n'
    'p.on("exit",(code,signal)=>{if(signal)process.kill(process.pid,signal);else process.exit(code)});\n')


def server(label):
    return {'type': 'stdio', 'command': 'npx' if label == 'entry' else 'node',
        'args': ['--no-install', '-c', f'node {shlex.quote(str(launcher))} {label}'] if label == 'entry' else [str(launcher), label], 'env': {
        'FAKE_SPAWN_MARK': str(SPAWNS), 'FAKE_CHILD_MARK': str(CHILDREN),
        'FAKE_DUMP_ENV': str(ROOT / f'{label}-env.json'), 'FAKE_REQUEST_LOG': str(ROOT / f'{label}-requests'),
        'FAKE_BALLAST_MIB': '150', 'FAKE_GRANDCHILD': '1', 'FAKE_WATCHDOG': '1',
        'CAPTURE_ENTRY': '${CAPTURE_EXPAND}'}}


write(PLUGIN / '.claude-plugin/plugin.json', {'name': 'playwright', 'version': '1.0.0'})
plugin = server('plugin')
plugin['env']['CAPTURE_ROOT'] = '${CLAUDE_PLUGIN_ROOT}'
write(PLUGIN / '.mcp.json', {'playwright': plugin})
(CONFIG / 'plugins').mkdir()
write(CONFIG / 'plugins/installed_plugins.json', {'version': 2, 'plugins': {'playwright@fixture': [{
    'scope': 'user', 'installPath': str(PLUGIN), 'version': '1.0.0',
    'installedAt': '2026-10-30T00:00:00Z', 'lastUpdated': '2026-10-30T00:00:00Z'}]}})
(ROOT / '.claude-plugin').mkdir()
write(ROOT / '.claude-plugin/marketplace.json', {'name': 'fixture', 'owner': {'name': 'Fixture'},
    'plugins': [{'name': 'playwright', 'source': './plugin', 'description': 'Synthetic'}]})
write(CONFIG / 'plugins/known_marketplaces.json', {'fixture': {'source': {'source': 'directory', 'path': str(ROOT)},
    'installLocation': str(ROOT), 'lastUpdated': '2026-10-30T00:00:00Z'}})
write(CONFIG / '.claude.json', {'mcpServers': {'entry': server('entry')}})


def settings(allow):
    write(CONFIG / 'settings.json', {'enabledPlugins': {'playwright@fixture': True},
        'permissions': {'allow': ['Bash(*)'] + ([TOOL] if allow else [])}})


settings(True)
API = []
SHELL = ROOT / 'shell.json'
# Exercise argv0, inherited env, exit status and shell-child PATH visibility.
SHELL_COMMAND = ('node -e ' + shlex.quote('require("fs").writeFileSync(' + json.dumps(str(SHELL)) +
    ',JSON.stringify({argv0:process.argv0,entry:process.env.CAPTURE_EXPAND,path:process.env.PATH}));process.exit(37)') +
    '; test $? = 37 && npx --version > ' + shlex.quote(str(ROOT / 'npx-version')) +
    ' && python3 -c ' + shlex.quote('import sys; sys.exit(29)') + '; test $? = 29')


class Fixture(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers.get('Content-Length', '0'))))
        API.append({'path': self.path, 'tools': [x.get('name') for x in payload.get('tools', [])],
                    'spawns': len(lines(SPAWNS))})
        self.send_response(200)
        if 'count_tokens' in self.path:
            self.send_header('Content-Type', 'application/json'); self.end_headers()
            self.wfile.write(b'{"input_tokens":0}'); return
        assert '/messages' in self.path
        results = sum(c.get('type') == 'tool_result' for m in payload.get('messages', [])
                      if isinstance(m.get('content'), list) for c in m['content'])
        name, args = ('Bash', {'command': SHELL_COMMAND}) if results == 0 else (TOOL, {})
        content = [{'type': 'tool_use', 'id': f'toolu_fixture_{results}', 'name': name, 'input': args}] if results < 2 else [
            {'type': 'text', 'text': 'fixture complete'}]
        message = {'id': 'msg_fixture', 'type': 'message', 'role': 'assistant',
            'model': payload.get('model', 'claude-haiku-4-5'), 'content': [], 'stop_reason': None,
            'stop_sequence': None, 'usage': {'input_tokens': 0, 'output_tokens': 0}}
        self.send_header('Content-Type', 'text/event-stream'); self.end_headers()
        events = [('message_start', {'type': 'message_start', 'message': message})]
        for index, block in enumerate(content):
            start = {**block, 'input': {}} if block['type'] == 'tool_use' else {'type': 'text', 'text': ''}
            delta = {'type': 'input_json_delta', 'partial_json': json.dumps(block['input'])} if block['type'] == 'tool_use' else {
                'type': 'text_delta', 'text': block['text']}
            events += [('content_block_start', {'type': 'content_block_start', 'index': index, 'content_block': start}),
                       ('content_block_delta', {'type': 'content_block_delta', 'index': index, 'delta': delta}),
                       ('content_block_stop', {'type': 'content_block_stop', 'index': index})]
        events += [('message_delta', {'type': 'message_delta', 'delta': {
            'stop_reason': 'tool_use' if results < 2 else 'end_turn', 'stop_sequence': None},
            'usage': {'output_tokens': 0}}), ('message_stop', {'type': 'message_stop'})]
        for event, value in events:
            self.wfile.write(f'event: {event}\ndata: {json.dumps(value)}\n\n'.encode())
        self.wfile.flush()
    def log_message(self, *args):
        pass


http = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
threading.Thread(target=http.serve_forever, daemon=True).start()
ENV = {'PATH': '/usr/bin:/bin', 'HOME': str(ROOT / 'home'), 'CLAUDE_CONFIG_DIR': str(CONFIG),
    'TMPDIR': str(ROOT / 'tmp'), 'XDG_CONFIG_HOME': str(ROOT / 'xdg'), 'XDG_DATA_HOME': str(DATA),
    'XDG_CACHE_HOME': str(ROOT / 'cache'), 'GIT_CONFIG_GLOBAL': '/dev/null',
    'ZERON_DATA_DIR': str(DATA), 'CODEX_HOME': str(ROOT / 'codex'), 'ZERON_MCP_PROXY_CHECK_MS': '20',
    'ANTHROPIC_API_KEY': 'invalid-zeron-zero-token-verification',
    'ANTHROPIC_BASE_URL': f'http://127.0.0.1:{http.server_port}',
    'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1', 'CLAUDE_CODE_AUTO_CONNECT_IDE': '0',
    'CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL': '1', 'CAPTURE_EXPAND': 'synthetic-entry-value'}
subprocess.run(['git', 'init', '-q', str(PROJECT)], env=ENV, check=True)
assert not (CONFIG / '.credentials.json').exists()
SPEC = ROOT / 'fixture.json'
BASE = {'request': {'prompt': '', 'model': 'haiku', 'reasoning': None, 'cwd': str(PROJECT),
    'sandbox': 'workspace-write', 'resume': None, 'access': 'supervised'}, 'dataDir': str(DATA),
    'flags': ['--settings', '{"disableAllHooks":true}']}


class Session:
    def __init__(self, name, lazy, expected_status="connected"):
        self.name, self.frames, self.errors, self.sent, self.known = name, [], [], [], set()
        spec = {**BASE, 'plan': {'proxyCommand': str(PROXY), 'runDir': str(DATA / 'run/mcp' / name)} if lazy else None}
        write(SPEC, spec)
        self.p = subprocess.Popen([str(HARNESS), str(CLI), str(SPEC), 'launch'], cwd=PROJECT, env=ENV,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        self.sel = selectors.DefaultSelector()
        for stream in [self.p.stdout, self.p.stderr]:
            os.set_blocking(stream.fileno(), False)
            self.sel.register(stream, selectors.EVENT_READ, bytearray())
        sessions.append(self)
        self.control('initialize')
        for _ in range(80):
            self.status = self.control('mcp_status')
            rows = self.rows()
            if len(rows) == 2 and all(r['status'] == expected_status for r in rows):
                break
            time.sleep(.1)
        assert len(self.rows()) == 2 and all(r['status'] == expected_status for r in self.rows()), self.status
        self.known |= tree(self.p.pid)
        self.cli_pid = next(p for p in tree(self.p.pid) if Path(f'/proc/{p}/exe').resolve() == CLI)
        self.cli_env = dict(part.split(b'=', 1) for part in Path(f'/proc/{self.cli_pid}/environ').read_bytes().split(b'\0') if b'=' in part)

    def rows(self):
        return self.status['response']['response']['mcpServers']

    def send(self, value):
        self.sent.append(value)
        self.p.stdin.write((json.dumps(value) + '\n').encode()); self.p.stdin.flush()

    def wait(self, predicate, timeout=40):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            for key, _ in self.sel.select(.1):
                chunk = os.read(key.fileobj.fileno(), 65536)
                if not chunk:
                    self.sel.unregister(key.fileobj); continue
                key.data.extend(chunk)
                while b'\n' in key.data:
                    raw, _, rest = key.data.partition(b'\n'); key.data[:] = rest
                    if key.fileobj is self.p.stderr:
                        self.errors.append(raw.decode(errors='replace')); continue
                    try:
                        frame = json.loads(raw)
                    except ValueError:
                        continue
                    self.frames.append(frame)
                    if frame.get('type') == 'control_request' and frame.get('request', {}).get('subtype') == 'can_use_tool':
                        self.send({'type': 'control_response', 'response': {'subtype': 'success',
                            'request_id': frame['request_id'], 'response': {'behavior': 'deny', 'message': 'fixture negative control'}}})
                    if predicate(frame):
                        return frame
            if self.p.poll() is not None:
                raise RuntimeError(('fixture exited', self.p.returncode, self.errors))
        raise TimeoutError((self.name, self.errors, self.frames[-5:]))

    def control(self, subtype, **fields):
        rid = f'fixture-{len(self.sent)}'
        self.send({'type': 'control_request', 'request_id': rid, 'request': {'subtype': subtype, **fields}})
        return self.wait(lambda f: f.get('response', {}).get('request_id') == rid)

    def turn(self, allow):
        before = len(API)
        self.send({'type': 'user', 'message': {'role': 'user', 'content': 'local deterministic shim fixture'}})
        result = self.wait(lambda f: f.get('type') == 'result')
        assert not result.get('is_error'), result
        assert result.get('usage', {}).get('input_tokens') == 0 and result.get('usage', {}).get('output_tokens') == 0
        assert result.get('total_cost_usd') == 0
        prompts = [f['request']['tool_name'] for f in self.frames if f.get('type') == 'control_request'
                   and f.get('request', {}).get('subtype') == 'can_use_tool']
        assert prompts == ([] if allow else [TOOL]), prompts
        init = next(f for f in self.frames if f.get('type') == 'system' and f.get('subtype') == 'init')
        assert TOOL in init['tools']
        shell = json.loads(SHELL.read_text())
        assert shell['argv0'] == 'node' and shell['entry'] == 'synthetic-entry-value', shell
        assert lines(ROOT / 'npx-version')
        self.known |= tree(self.p.pid)
        return init['tools'], API[before:]

    def measure(self):
        # Select only the two MCP launch roots, excluding Claude/driver and any
        # transient shell snapshot or Git children the CLI starts for itself.
        cli_pid = self.cli_pid
        descendants = tree(cli_pid)
        # npm changes its process title, so identify eager roots by the fake
        # server PID marks, and cold roots by the small proxy executable.
        seeds = {int(p) for p in lines(SPAWNS)} & descendants
        for pid in descendants:
            try:
                if Path(f'/proc/{pid}/exe').resolve() == PROXY:
                    seeds.add(pid)
            except (FileNotFoundError, ProcessLookupError):
                pass
        roots = set()
        for pid in seeds:
            while pid in descendants:
                parent = int(Path(f'/proc/{pid}/stat').read_text().split(') ')[1].split()[1])
                if parent == cli_pid:
                    roots.add(pid)
                    break
                pid = parent
        assert len(roots) == 2, roots
        values = []
        for _ in range(7):
            pids = set(roots)
            for root in roots:
                pids |= tree(root)
            self.known |= pids
            values.append(pss(pids)); time.sleep(.05)
        return {'median_kib': statistics.median(values), 'samples_kib': values,
                'pids': sorted(pids), 'mcp_roots': sorted(roots)}

    def close(self):
        self.known |= tree(self.p.pid)
        self.known |= {int(p) for p in lines(SPAWNS) + lines(CHILDREN) + lines(ROOT / 'channel-spawns') + lines(ROOT / 'channel-children')}
        self.p.terminate()
        try:
            self.p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(self.p.pid, signal.SIGKILL); self.p.wait(); raise
        self.sel.close()
        write(ROOT / f'{self.name}.json', {'frames': self.frames, 'stderr': self.errors, 'exit': self.p.returncode})
        wait_until(lambda: not any(alive(p) for p in self.known), f'{self.name}: leftover pids {self.known}')
        run_dir = DATA / 'run/mcp' / self.name
        assert not list(run_dir.glob('shim-*')) and not [p for p in run_dir.glob('*') if p.name.isdigit()]


sessions = []
try:
    baseline = Session('baseline', False)
    baseline_pss = baseline.measure()
    rows = baseline.status
    names = sorted((r['name'], [t['name'] for t in r.get('tools', [])]) for r in baseline.rows())
    tools, _ = baseline.turn(True)
    baseline.close(); sessions.remove(baseline)
    spec = {**BASE, 'status': rows}; write(SPEC, spec)
    subprocess.run([str(HARNESS), str(CLI), str(SPEC), 'capture'], env=ENV, cwd=PROJECT, check=True)
    # Both escape hatches leave PATH entirely unmodified.
    ENV['ZERON_MCP_LAZY'] = '0'
    disabled = Session('disabled', True)
    assert b'ZERON_MCP_SHIM_DIR' not in disabled.cli_env
    disabled.close(); sessions.remove(disabled)
    ENV.pop('ZERON_MCP_LAZY')
    warm = Session('warm', True)
    wait_until(lambda: not any(alive(int(p)) for p in lines(SPAWNS) + lines(CHILDREN)), 'discovery children must stop')
    warm.close(); sessions.remove(warm)
    count = len(lines(SPAWNS))
    lazy = Session('lazy', True)
    assert len(lines(SPAWNS)) == count, 'warm-cache startup spawned a real server'
    assert sorted((r['name'], [t['name'] for t in r.get('tools', [])]) for r in lazy.rows()) == names
    lazy_pss = lazy.measure()
    write(ROOT / 'pss.json', {'baseline': baseline_pss, 'lazy': lazy_pss})
    assert len(lines(SPAWNS)) == count
    lazy_tools, requests = lazy.turn(True)
    assert set(lazy_tools) == set(tools)
    messages = [r for r in requests if '/messages' in r['path'] and 'count_tokens' not in r['path']]
    assert len(messages) == 3 and messages[0]['spawns'] == count and messages[1]['spawns'] == count, messages
    assert len(lines(SPAWNS)) == count + 1, 'first MCP call must start exactly one server'
    for label in ['entry', 'plugin']:
        env = json.loads((ROOT / f'{label}-env.json').read_text())
        assert env['CAPTURE_ENTRY'] == 'synthetic-entry-value'
        if label == 'plugin':
            assert env['CLAUDE_PLUGIN_ROOT'] == str(PLUGIN) and env['CAPTURE_ROOT'] == str(PLUGIN)
            assert env.get('CLAUDE_PLUGIN_DATA'), env
    lazy.close(); sessions.remove(lazy)
    settings(False)
    count = len(lines(SPAWNS))
    denied = Session('denied', True)
    denied.turn(False)
    assert len(lines(SPAWNS)) == count, 'denied call must not start a server'
    denied.close(); sessions.remove(denied)
    # Channel capability comes from initialize, not invented mcp_status fields.
    settings(True)
    channel_config = json.loads((PLUGIN / '.mcp.json').read_text())
    channel_config['playwright']['env'].update({'FAKE_CHANNEL': '1', 'FAKE_BALLAST_MIB': '0',
        'FAKE_SPAWN_MARK': str(ROOT / 'channel-spawns'), 'FAKE_CHILD_MARK': str(ROOT / 'channel-children')})
    write(PLUGIN / '.mcp.json', channel_config)
    channel_base = Session('channel-baseline', False)
    channel_rows = channel_base.status
    channel_base.close(); sessions.remove(channel_base)
    write(SPEC, {**BASE, 'status': channel_rows})
    subprocess.run([str(HARNESS), str(CLI), str(SPEC), 'capture'], env=ENV, cwd=PROJECT, check=True)
    channel_results = []
    for name, expect_proxy in [('channel-first', True), ('channel-later', False)]:
        before = len(lines(ROOT / 'channel-spawns'))
        session = Session(name, True)
        time.sleep(.6)  # Many 20 ms proxy checks: a channel must never idle-stop.
        channel_pids = [int(p) for p in lines(ROOT / 'channel-spawns')][before:]
        assert len(channel_pids) == 1 and alive(channel_pids[0])
        pid, ancestors = channel_pids[0], []
        while pid != session.cli_pid:
            pid = int(Path(f'/proc/{pid}/stat').read_text().split(') ')[1].split()[1])
            ancestors.append(pid)
        proxied = any(Path(f'/proc/{p}/exe').resolve() == PROXY for p in ancestors)
        assert proxied == expect_proxy, (name, ancestors, proxied)
        response = session.control('mcp_call', tool=TOOL, arguments={}, result_not_for_model=True)
        assert response['response']['subtype'] == 'success' and 'echo' in json.dumps(response), response
        assert len(lines(ROOT / 'channel-spawns')) == before + 1 and alive(channel_pids[0])
        channel_results.append({'case': name, 'proxied': proxied, 'one_live_server': True, 'call_forwarded': True})
        session.close(); sessions.remove(session)
    # A transient initialization failure after a previously connected capture
    # must switch this very session to eager forwarding, then exec on later runs.
    ENV['FAKE_INIT_ERROR'] = '1'
    failures = []
    for name, expected_proxies in [('discovery-failed', 2), ('failed-later', 0)]:
        before = len(lines(SPAWNS)) + len(lines(ROOT / 'channel-spawns'))
        failed = Session(name, True, expected_status='failed')
        helpers = [p for p in tree(failed.p.pid) if Path(f'/proc/{p}/exe').resolve() == PROXY]
        assert len(helpers) <= expected_proxies, (name, helpers)
        # Claude may close a failed connection before mcp_status is returned.
        spawned = len(lines(SPAWNS)) + len(lines(ROOT / 'channel-spawns')) - before
        skipped = sum('Skipping connection' in r.get('error', '') for r in failed.rows())
        attempts = len(failed.rows()) - skipped
        assert spawned == attempts * (2 if expected_proxies else 1), (name, spawned, skipped)
        failures.append({'case': name, 'proxy_count': len(helpers), 'real_starts': spawned,
                         'claude_cached_failures_skipped': skipped})
        failed.close(); sessions.remove(failed)
    ENV.pop('FAKE_INIT_ERROR')
    result = {'baseline_mcp_pss': baseline_pss, 'lazy_mcp_pss': lazy_pss,
        'saved_kib': baseline_pss['median_kib'] - lazy_pss['median_kib'], 'server_names_and_tools': names,
        'checks': ['catalog names', 'zero servers before call', 'one server on first call', 'plugin/entry env',
                   'allow and deny permission rules', 'Bash node/npx/python argv0/env/exit', 'descendant/watchdog teardown'],
        'channel_cases': channel_results, 'failure_cases': failures, 'zero_tokens': True, 'all_children_gone': True}
    write(ROOT / 'result.json', result)
    print(json.dumps(result), flush=True)
finally:
    for session in sessions:
        session.close()
    http.shutdown()
