#!/usr/bin/env python3
"""V-5: actual CLI permission path driven by a deterministic local HTTP fixture.
No model service, usable key, or real profile. All API responses are fixed JSON.
Usage: python3 claude_mcp_permission_probe.py NEW_SCRATCH CLAUDE
"""
import http.server
import json
import os
from pathlib import Path
import selectors
import signal
import subprocess
import sys
import threading
import time

root, cli = map(lambda p: Path(p).resolve(), sys.argv[1:])
root.mkdir(parents=True, exist_ok=True)
fake = Path(__file__).with_name('claude_mcp_fake.py').resolve()
tool = 'mcp__plugin_playwright_playwright__browser_click'
records = []


class Fixture(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        payload = json.loads(self.rfile.read(int(self.headers.get('Content-Length', '0'))))
        # Do not persist prompts/headers. There is no forwarding code or model.
        entry = {'path': self.path, 'tools': [x.get('name') for x in payload.get('tools', [])]}
        records.append(entry)
        if 'count_tokens' in self.path:
            self.send_response(200)
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(b'{"input_tokens":0}')
            return
        if '/messages' not in self.path:
            self.send_response(404)
            self.end_headers()
            return
        # First message returns one fixed tool call, subsequent messages end.
        is_first = not any(isinstance(m.get('content'), list) and any(
            c.get('type') == 'tool_result' for c in m['content']) for m in payload.get('messages', []))
        content = [{'type': 'tool_use', 'id': 'toolu_fixture', 'name': tool, 'input': {}}] if is_first else [
            {'type': 'text', 'text': 'fixture complete'}]
        message = {'id': 'msg_fixture', 'type': 'message', 'role': 'assistant',
                   'model': payload.get('model', 'claude-haiku-4-5'), 'content': content,
                   'stop_reason': 'tool_use' if is_first else 'end_turn', 'stop_sequence': None,
                   'usage': {'input_tokens': 0, 'output_tokens': 0}}
        self.send_response(200)
        if payload.get('stream'):
            self.send_header('Content-Type', 'text/event-stream')
            self.end_headers()
            start = {**message, 'content': [], 'stop_reason': None}
            events = [('message_start', {'type': 'message_start', 'message': start})]
            for index, block in enumerate(content):
                events.append(('content_block_start', {'type': 'content_block_start', 'index': index, 'content_block': block}))
                events.append(('content_block_stop', {'type': 'content_block_stop', 'index': index}))
            events.extend([('message_delta', {'type': 'message_delta', 'delta': {
                'stop_reason': message['stop_reason'], 'stop_sequence': None}, 'usage': {'output_tokens': 0}}),
                ('message_stop', {'type': 'message_stop'})])
            for event, value in events:
                self.wfile.write(f'event: {event}\ndata: {json.dumps(value)}\n\n'.encode())
            self.wfile.flush()
        else:
            self.send_header('Content-Type', 'application/json')
            self.end_headers()
            self.wfile.write(json.dumps(message).encode())
    def log_message(self, *args):
        pass


http = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Fixture)
threading.Thread(target=http.serve_forever, daemon=True).start()
try:
    for allow in [True, False]:
        case = root / ('allowed' if allow else 'no-rule')
        config = case / 'home/.claude'
        project = case / 'project'
        for p in [config, project, project / '.git', case / 'tmp']:
            p.mkdir(parents=True, exist_ok=True)
        (config / 'settings.json').write_text(json.dumps({'permissions': {'allow': [tool] if allow else []}}))
        env = {'PATH': '/usr/bin:/bin', 'HOME': str(case / 'home'), 'CLAUDE_CONFIG_DIR': str(config),
               'TMPDIR': str(case / 'tmp'), 'XDG_CONFIG_HOME': str(case / 'xdg'),
               'XDG_DATA_HOME': str(case / 'data'), 'XDG_CACHE_HOME': str(case / 'cache'),
               'GIT_CONFIG_GLOBAL': '/dev/null', 'ANTHROPIC_API_KEY': 'invalid-zeron-zero-token-verification',
               'ANTHROPIC_BASE_URL': f'http://127.0.0.1:{http.server_port}',
               'CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC': '1', 'CLAUDE_CODE_AUTO_CONNECT_IDE': '0',
               'CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL': '1'}
        assert not (config / '.credentials.json').exists()
        mcp = {'mcpServers': {'plugin_playwright_playwright': {'command': sys.executable,
            'args': [str(fake)], 'env': {'FAKE_CALL_LOG': str(case / 'calls')}}}}
        args = [str(cli), '-p', '--input-format', 'stream-json', '--output-format', 'stream-json',
                '--verbose', '--permission-mode', 'default', '--permission-prompt-tool', 'stdio',
                '--model', 'haiku', '--settings', '{"disableAllHooks":true}',
                '--strict-mcp-config', '--mcp-config', json.dumps(mcp)]
        process = subprocess.Popen(args, env=env, cwd=project, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, start_new_session=True)
        sel = selectors.DefaultSelector()
        for stream in [process.stdout, process.stderr]:
            os.set_blocking(stream.fileno(), False)
            sel.register(stream, selectors.EVENT_READ, bytearray())
        frames, errors, sent = [], [], []

        def send(value):
            sent.append(value)
            process.stdin.write((json.dumps(value) + '\n').encode())
            process.stdin.flush()

        def wait_for(predicate, timeout=25):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                for key, _ in sel.select(.1):
                    chunk = os.read(key.fileobj.fileno(), 65536)
                    if not chunk:
                        sel.unregister(key.fileobj)
                        continue
                    key.data.extend(chunk)
                    while b'\n' in key.data:
                        line, _, remaining = key.data.partition(b'\n')
                        key.data[:] = remaining
                        if key.fileobj is process.stderr:
                            errors.append(line.decode(errors='replace'))
                            continue
                        try:
                            frame = json.loads(line)
                        except ValueError:
                            continue
                        frames.append(frame)
                        if frame.get('type') == 'control_request' and frame.get('request', {}).get('subtype') == 'can_use_tool':
                            send({'type': 'control_response', 'response': {'subtype': 'success',
                                'request_id': frame['request_id'], 'response': {'behavior': 'deny',
                                'message': 'fixture negative control: no allow rule'}}})
                        if predicate(frame):
                            return frame
                if process.poll() is not None:
                    raise RuntimeError('CLI exited before fixture result')
            raise TimeoutError('fixture result timed out')

        try:
            send({'type': 'control_request', 'request_id': 'init', 'request': {'subtype': 'initialize'}})
            wait_for(lambda f: f.get('response', {}).get('request_id') == 'init')
            # The only user message is sent to our loopback fixture, never a model.
            send({'type': 'user', 'message': {'role': 'user', 'content': 'local deterministic permission probe'}})
            result = wait_for(lambda f: f.get('type') == 'result')
            init = next(f for f in frames if f.get('type') == 'system' and f.get('subtype') == 'init')
            prompts = [f for f in frames if f.get('type') == 'control_request'
                       and f.get('request', {}).get('subtype') == 'can_use_tool']
            calls = (case / 'calls').read_text().splitlines() if (case / 'calls').exists() else []
            record = {'command': args, 'environment': {'ANTHROPIC_BASE_URL': env['ANTHROPIC_BASE_URL'],
                'ANTHROPIC_API_KEY': 'invalid fixture key', 'HOME': env['HOME'], 'CLAUDE_CONFIG_DIR': str(config)},
                'system_init_tool_present': tool in init['tools'],
                'permission_requests': [p['request'].get('tool_name') for p in prompts],
                'calls': calls, 'result_usage': result.get('usage'),
                'result_is_error': result.get('is_error'), 'total_cost_usd': result.get('total_cost_usd'), 'stderr': errors,
                'frames': frames, 'api_requests': list(records)}
            (case / 'result.json').write_text(json.dumps(record, indent=2) + '\n')
            print(json.dumps({k: v for k, v in record.items() if k not in ['command', 'environment', 'frames', 'api_requests']}), flush=True)
            assert record['system_init_tool_present']
            assert bool(prompts) != allow
            assert bool(calls) == allow
            assert not result.get('is_error')
            assert result.get('usage', {}).get('input_tokens') == 0
            assert result.get('usage', {}).get('output_tokens') == 0
            assert result.get('total_cost_usd') == 0
        finally:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            sel.close()
finally:
    http.shutdown()
