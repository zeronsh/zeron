#!/usr/bin/python3
"""Offline native app-server with WebRTC signaling and canonical voice events."""
import json, pathlib, sys, threading, time
root=pathlib.Path(__file__).resolve().parents[1]
session=None
account_initialized=False
thread='idle-thread'
wire_lock=threading.Lock()
def send(v):
    with wire_lock:print(json.dumps(v),flush=True)
def notify(method,params):send({'method':method,'params':dict(threadId=thread,**params)})
def identity_watch():
    while True:
        if (root/'identity-updated').exists():
            (root/'identity-updated').unlink()
            send({'method':'account/updated','params':{'authMode':'chatgpt'}})
            (root/'identity-notified').write_text('true')
        if (root/'replay-transcripts').exists() and session:
            (root/'replay-transcripts').unlink()
            for role,item,text in [('user','u1','Make a local Codex task'),('assistant','a1','I can do that.')]:
                notify('thread/realtime/item/completed',{'item':{'type':'transcriptSegment','id':item,'realtimeSessionId':session,'role':role,'text':text}})
            (root/'replay-sent').write_text('true')
        time.sleep(0.01)
threading.Thread(target=identity_watch,daemon=True).start()
for line in sys.stdin:
    frame=json.loads(line);method=frame.get('method')
    with (root/'voice-wire.jsonl').open('a') as out:out.write(json.dumps(frame)+'\n')
    if 'id' not in frame:continue
    result={}
    if method=='config/read':
        result={'config':json.loads((root/'project-config').read_text()) if frame.get('params',{}).get('cwd') and (root/'project-config').exists() else {}}
    elif method=='account/read':
        if account_initialized and (root/'probe-delay').exists():
            (root/'probing').write_text('true')
            while not (root/'probe-release').exists():time.sleep(0.01)
        result={'account':{'type':(root/'account-mode').read_text().strip() if (root/'account-mode').exists() else 'chatgpt'}}
        if not account_initialized:
            # Native Codex announces its initial account before answering the
            # first account/read, even when no login or account switch occurred.
            send({'method':'account/updated','params':{'authMode':'chatgpt' if result['account']['type']=='chatgpt' else 'apikey','planType':None}})
            account_initialized=True
    elif method=='account/rateLimits/read':result=json.loads((root/'rate-limits').read_text()) if (root/'rate-limits').exists() else {'ordinaryUsageAllowed':True}
    elif method=='thread/realtime/listVoices':result={'voices':{'v1':['juniper','ember'],'v2':['alloy'],'defaultV1':'juniper','defaultV2':'alloy'}}
    elif method in ('thread/start','thread/resume'):result={'thread':{'id':thread,'turns':[]},'cwd':frame['params'].get('cwd',str(root)),'modelProvider':(root/'thread-provider').read_text().strip() if (root/'thread-provider').exists() else 'openai'}
    elif method=='turn/start':result={'turn':{'id':'text-turn'}}
    elif method=='thread/realtime/start':
        p=frame['params'];assert p['transport']=={'type':'webrtc','sdp':'fixture-offer'};assert p['version']=='v3';assert p['outputModality']=='audio';assert p['clientManagedHandoffs'] is False;assert p['includeStartupContext'] is True
        assert 'Zeron MCP' in p['realtimeStartInstructions'] and p['initialItems']==[{'role':'developer','text':p['realtimeStartInstructions']}]
        session=p['realtimeSessionId']
    elif method=='thread/realtime/appendAudio':raise RuntimeError('subscription voice must never append PCM')
    send({'id':frame['id'],'result':result})
    if method=='thread/realtime/start':
        notify('thread/realtime/started',{'realtimeSessionId':session,'version':'v3'})
        notify('thread/realtime/sdp',{'sdp':'fixture-answer'})
        for role,item,text in [('user','u1','Make a local Codex task'),('user','u1','Make a local Codex task'),('assistant','a1','I can do that.')]:
            notify('thread/realtime/item/transcript/delta',{'itemId':item,'delta':text})
            notify('thread/realtime/item/completed',{'item':{'type':'transcriptSegment','id':item,'realtimeSessionId':session,'role':role,'text':text}})
            notify('thread/realtime/transcript/done',{'role':role,'text':text})
        if (root/'conversation-only').exists():continue
        notify('turn/started',{'turn':{'id':'native-task'}})
        notify('item/agentMessage/delta',{'turnId':'native-task','itemId':'task-reply','delta':'Native task output'})
        notify('thread/realtime/item/completed',{'item':{'type':'bemItemPromoted','id':'promotion','realtimeSessionId':session,'turnId':'native-task','itemId':'task-reply','presentation':{'type':'wholeItem'}}})
    elif method=='thread/realtime/stop':
        if (root/'spontaneous-close').exists():notify('thread/realtime/closed',{'reason':'transport_closed'})
        def closed():
            time.sleep(0.1 if (root/'delayed-close').exists() else 0)
            notify('thread/realtime/closed',{'reason':'requested'})
        threading.Thread(target=closed,daemon=True).start()
    elif method=='turn/start':notify('turn/completed',{'turn':{'id':'text-turn','status':'completed'}})
