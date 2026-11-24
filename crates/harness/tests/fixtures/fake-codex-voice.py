#!/usr/bin/env python3
"""Offline app-server peer; any realtime start is a test failure."""
import json
import pathlib
import sys
for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get('method')
    with pathlib.Path('voice-wire.jsonl').open('a') as log:
        log.write(json.dumps(frame) + '\n')
    if 'id' not in frame:
        continue
    result = {}
    if method == 'config/read':
        # The user's config as a test lays it out (`config-read.json`), else none.
        seeded = pathlib.Path('config-read.json')
        result = json.loads(seeded.read_text()) if seeded.exists() else {'config': {}}
    elif method == 'plugin/installed':
        # The plugins a test installs (`plugins-installed.json`), else none.
        seeded = pathlib.Path('plugins-installed.json')
        result = json.loads(seeded.read_text()) if seeded.exists() else {'marketplaces': []}
    elif method == 'plugin/read':
        # One plugin's detail (`plugin-<name>.json`); `"silent": true` never
        # answers, as a remote catalog that doesn't.
        seeded = pathlib.Path(f"plugin-{frame['params']['pluginName']}.json")
        detail = json.loads(seeded.read_text()) if seeded.exists() else {'mcpServers': []}
        if detail.get('silent'):
            continue
        result = {'plugin': detail}
    elif method in ('thread/start', 'thread/resume'):
        result = {'thread': {'id': 'idle-thread', 'turns': []}}
    elif method == 'turn/start':
        result = {'turn': {'id': 'text-turn'}}
    elif method == 'thread/realtime/start':
        sys.exit(98)
    print(json.dumps({'id': frame['id'], 'result': result}), flush=True)
    if method == 'turn/start':
        print(json.dumps({'method': 'turn/completed', 'params': {'threadId': 'idle-thread', 'turn': {'id':'text-turn', 'status':'completed'}}}), flush=True)
