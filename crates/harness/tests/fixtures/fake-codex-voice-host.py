#!/usr/bin/python3
"""Offline Codex native helper. Length-prefixed controls; never opens hardware/network."""
import json, pathlib, struct, sys, os, time
root=pathlib.Path(__file__).resolve().parents[3]
if sys.argv[1:] == ['--build-commit']:
    print('fixture-build');sys.exit(0)
log=root/'helper-wire.jsonl'
(root/'helper.pid').write_text(str(os.getpid()))
def receive():
    header=sys.stdin.buffer.read(4)
    if not header:return None
    length=struct.unpack('>I',header)[0]
    if length>128*1024:raise ValueError('overflow')
    return json.loads(sys.stdin.buffer.read(length))
def send(value):
    payload=json.dumps(value).encode();sys.stdout.buffer.write(struct.pack('>I',len(payload))+payload);sys.stdout.buffer.flush()
try:
    while (frame:=receive()) is not None:
        with log.open('a') as out:out.write(json.dumps(frame)+'\n')
        kind=frame['type']
        if kind=='hello':assert frame=={'type':'hello','protocol':1,'buildCommit':'fixture-build'};send({'type':'ready'})
        elif kind=='initializeRuntime':send({'type':'runtimeReady'})
        elif kind=='startTransport':send({'type':'offer','sdp':'fixture-offer'})
        elif kind=='applyAnswer':assert frame['sdp']=='fixture-answer';send({'type':'transportReady'})
        elif kind=='openDevices':
            manifest=pathlib.Path(__file__).resolve().parents[1]/'manifest.json'
            version=json.loads(manifest.read_text())['appVersion'] if manifest.exists() else '0.159.0'
            expected={'type':'openDevices'}
            if tuple(map(int,version.split('.'))) >= (0,161,0):expected['selection']={}
            assert frame==expected
            if (root/'helper-mode').exists() and (root/'helper-mode').read_text().strip()=='exit-open':sys.exit(25)
            send({'type':'devicesOpened'})
        elif kind=='setAudioControls':
            if (root/'helper-mode').exists() and (root/'helper-mode').read_text().strip()=='exit-controls':sys.exit(26)
            if (root/'helper-mode').exists() and (root/'helper-mode').read_text().strip()=='stall-controls':
                (root/'controls-stalled').write_text('true');time.sleep(60)
            send({'type':'audioControlsApplied'})
        elif kind=='inspectAudio':send({'type':'audioState','state':{'microphonePeak':1024,'speakerPeak':0}})
        elif kind=='close':send({'type':'closed'});break
        else:raise ValueError('unexpected command')
finally:
    with log.open('a') as out:out.write('{"type":"exited"}\n')
