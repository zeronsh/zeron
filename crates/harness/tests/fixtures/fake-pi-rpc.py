#!/usr/bin/env python3
"""Native RPC session persistence fixture for engine dispatch tests."""
import json
import os
from pathlib import Path
import sys
import threading
import time
import uuid

if '--version' in sys.argv:
    print('0.85.1')
    sys.exit(0)

resumed = '--session' in sys.argv
path = Path(sys.argv[sys.argv.index('--session') + 1]) if resumed else Path.cwd() / 'session.jsonl'
if resumed:
    session = json.loads(path.read_text().splitlines()[0])['id']
else:
    session = str(uuid.uuid4())
    if '--no-session' not in sys.argv:
        path.write_text(json.dumps({'type': 'session', 'id': session, 'cwd': str(Path.cwd())}) + '\n')

def emit(frame):
    print(json.dumps(frame), flush=True)

def crash():
    time.sleep(0.15)
    os._exit(7)

dialog = None
for line in sys.stdin:
    req = json.loads(line)
    kind = req['type']
    if kind == 'prompt' and req['message'] == '/question':
        dialog = req
        emit({'type':'extension_ui_request','id':'question','method':'select','title':'Choose','options':['first','second'],'timeout':200})
        continue
    if kind == 'extension_ui_response':
        assert req.get('cancelled') is True
        emit({'id':dialog['id'],'type':'response','command':'prompt','success':True})
        continue
    data = {}
    if kind == 'get_state':
        data = {'sessionId': session, 'sessionFile': str(path), 'isStreaming': False, 'isCompacting': False, 'model': {'id': 'mock'}}
    if kind == 'get_commands':
        data = {'commands': []}
    success = not (kind == 'prompt' and req['message'] == 'require-resume' and not resumed)
    emit({'id': req.get('id'), 'type': 'response', 'command': kind, 'success': success, 'data': data, 'error': 'session must be resumed'})
    if kind == 'prompt' and success:
        emit({'type': 'agent_start'})
        emit({'type': 'message_start', 'message': {'role': 'user'}})
        emit({'type': 'message_end', 'message': {'role': 'assistant', 'content': [{'type': 'text', 'text': 'reply:' + req['message']}], 'stopReason': 'stop'}})
        emit({'type': 'agent_end'})
        emit({'type': 'agent_settled'})
        if req['message'] == 'idle-crash':
            threading.Thread(target=crash, daemon=True).start()
