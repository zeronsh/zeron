#!/usr/bin/env python3
"""Native Oh My Pi RPC session persistence fixture for engine dispatch tests."""
import json
import os
from pathlib import Path
import sys
import threading
import time
import uuid

if '--version' in sys.argv:
    print('omp/18.11.1')
    sys.exit(0)
if sys.argv[1:3] == ['models', '--json']:
    print(json.dumps({'models': [{'provider': 'mock', 'id': 'mock', 'name': 'Mock', 'kind': 'chat', 'reasoning': False, 'thinking': None}]}))
    sys.exit(0)

resumed = '--resume' in sys.argv
path = Path(sys.argv[sys.argv.index('--resume') + 1]) if resumed else Path.cwd() / 'session.jsonl'
# OMP writes the file at the first turn: a padded title slot, then the header.
session = json.loads(path.read_text().splitlines()[1])['id'] if resumed else str(uuid.uuid4())
ephemeral = '--no-session' in sys.argv

def emit(frame):
    print(json.dumps(frame), flush=True)

def persist():
    if not ephemeral and not path.exists():
        title = {'type': 'title', 'v': 1, 'title': '', 'pad': ' ' * 64}
        header = {'type': 'session', 'version': 3, 'id': session, 'cwd': str(Path.cwd())}
        path.write_text(json.dumps(title) + '\n' + json.dumps(header) + '\n')

def crash():
    time.sleep(0.15)
    os._exit(7)

emit({'type': 'ready', 'protocolVersion': 1, 'supportedProtocolVersions': [1], 'maxFrameBytes': 1048576})
dialog = None
for line in sys.stdin:
    req = json.loads(line)
    kind = req['type']
    if kind == 'prompt' and req['message'] == '/question':
        dialog = req
        emit({'id': req['id'], 'type': 'response', 'command': 'prompt', 'success': True})
        emit({'type': 'extension_ui_request', 'id': 'question', 'method': 'select', 'title': 'Choose', 'options': ['first', 'second'], 'timeout': 200})
        continue
    if kind == 'extension_ui_response':
        assert req.get('cancelled') is True
        emit({'type': 'prompt_result', 'id': dialog['id'], 'agentInvoked': False, 'status': 'completed', 'sessionSettled': True})
        continue
    data = {}
    if kind == 'get_state':
        data = {'sessionId': session, 'sessionFile': str(path), 'isStreaming': False, 'isCompacting': False, 'model': {'id': 'mock'}}
        if ephemeral:
            del data['sessionFile']
    if kind == 'get_available_commands':
        data = {'commands': []}
    if kind in ('get_commands', 'clear_queue'):
        emit({'id': req.get('id'), 'type': 'response', 'command': kind, 'success': False, 'error': 'Unknown command: ' + kind})
        continue
    if kind == 'prompt' and req['message'] == 'require-resume' and not resumed:
        emit({'id': req.get('id'), 'type': 'response', 'command': kind, 'success': False, 'error': 'session must be resumed'})
        emit({'type': 'prompt_result', 'id': req['id'], 'agentInvoked': False, 'status': 'error', 'error': {'message': 'session must be resumed', 'retryable': False}, 'sessionSettled': True})
        continue
    emit({'id': req.get('id'), 'type': 'response', 'command': kind, 'success': True, 'data': data})
    if kind == 'prompt':
        emit({'type': 'agent_start'})
        persist()
        emit({'type': 'message_start', 'message': {'role': 'user'}})
        emit({'type': 'message_end', 'message': {'role': 'assistant', 'content': [{'type': 'text', 'text': 'reply:' + req['message']}], 'stopReason': 'stop'}})
        emit({'type': 'agent_end', 'messages': [], 'isTerminal': True, 'yielded': True})
        emit({'type': 'prompt_result', 'id': req['id'], 'agentInvoked': True, 'status': 'completed', 'sessionSettled': True})
        emit({'type': 'session_settled'})
        if req['message'] == 'idle-crash':
            threading.Thread(target=crash, daemon=True).start()
