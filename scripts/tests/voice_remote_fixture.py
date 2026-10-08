#!/usr/bin/env python3
"""Independent subprocess fixtures. No network, hardware, credentials or wire logs."""
import json
import struct
import sys
import time

ROLE, MODE = sys.argv[1:]
SDP = 'v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n'


def send(value):
    payload = json.dumps(value).encode()
    if ROLE == 'helper':
        sys.stdout.buffer.write(struct.pack('>I', len(payload)) + payload)
        sys.stdout.buffer.flush()
    else:
        print(payload.decode(), flush=True)


def helper():
    while header := sys.stdin.buffer.read(4):
        frame = json.loads(sys.stdin.buffer.read(struct.unpack('>I', header)[0]))
        if MODE == 'hang':
            time.sleep(60)
        if MODE == 'oversize':
            sys.stdout.buffer.write(struct.pack('>I', 128 * 1024 + 1))
            sys.stdout.buffer.flush()
            return
        if MODE == 'truncated':
            sys.stdout.buffer.write(struct.pack('>I', 100) + b'{')
            sys.stdout.buffer.flush()
            return
        kind = frame['type']
        responses = {'hello': 'ready', 'initializeRuntime': 'runtimeReady',
                     'startTransport': 'offer', 'applyAnswer': 'transportReady',
                     'close': 'closed'}
        # Offline exercise must never open audio. Fail if it tries.
        if kind not in responses:
            raise RuntimeError('unexpected helper command')
        result = {'type': responses[kind]}
        if kind == 'startTransport':
            result['sdp'] = SDP if MODE != 'invalid-offer' else 'secret malformed offer'
        if kind == 'applyAnswer':
            assert frame['sdp'] == SDP
        send(result)
        if kind == 'close':
            return


def server():
    voices_checked = False
    for line in sys.stdin:
        frame = json.loads(line)
        if 'id' not in frame:
            continue
        method = frame['method']
        result = {}
        if MODE == 'disconnect':
            return
        if MODE == 'hang':
            time.sleep(60)
        if MODE == 'reject':
            send({'id': frame['id'], 'error': {'message': 'SECRET provider contents'}})
            continue
        if method == 'account/read':
            result = {'account': {'type': 'apikey' if MODE == 'apikey' else 'chatgpt',
                                  'email': 'SECRET@example.test'}}
        elif method == 'config/read':
            result = {'config': {'model_provider': 'custom'} if MODE == 'custom' else {}}
        elif method == 'thread/realtime/listVoices':
            voices_checked = True
        elif method == 'thread/start':
            assert voices_checked
            result = {'thread': {'id': 'fixture-thread'}, 'modelProvider': 'openai'}
        elif method == 'thread/realtime/start':
            p = frame['params']
            assert p['transport'] == {'type': 'webrtc', 'sdp': SDP}
            assert p['clientManagedHandoffs'] is False and p['version'] == 'v3'
            if MODE == 'lost-start':
                continue  # Still service stop after the caller's deadline.
        send({'id': frame['id'], 'result': result})
        if method == 'thread/realtime/stop':
            send({'method': 'thread/realtime/closed', 'params': {
                'threadId': 'fixture-thread', 'reason': 'requested'}})
        if method == 'thread/realtime/start':
            p = frame['params']
            def event(name, **params):
                send({'method': 'thread/realtime/' + name,
                      'params': dict(threadId='fixture-thread', **params)})
            if MODE == 'closed':
                event('closed', reason='SECRET error details')
                continue
            started = {'realtimeSessionId': p['realtimeSessionId'], 'version': 'v3'}
            if MODE == 'reverse':
                event('sdp', sdp=SDP)
                event('started', **started)
            else:
                event('started', **started)
                event('sdp', sdp=SDP if MODE != 'invalid-answer' else 'SECRET SDP')
            for _ in range(2):
                event('item/completed', item={'type': 'transcriptSegment', 'id': 'u1',
                      'realtimeSessionId': p['realtimeSessionId'], 'role': 'user', 'text': 'SECRET transcript'})
            event('item/completed', item={'type': 'transcriptSegment', 'id': 'a1',
                  'realtimeSessionId': p['realtimeSessionId'], 'role': 'assistant', 'text': 'SECRET answer'})


if ROLE == 'helper':
    helper()
else:
    server()
