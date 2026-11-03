#!/usr/bin/env python3
"""Explicit live, subscription WebRTC connectivity check. No microphone/device opens.
This can consume normal Codex voice usage. Offline CI must never invoke --live.
SDP, account details, native errors and media are never printed or persisted.
"""
import argparse, asyncio, json, os, pathlib, shutil, struct, uuid
async def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live',action='store_true',help='Authorize a short real Codex voice call using normal plan usage')
    parser.add_argument('--devices',action='store_true',help='Also open default audio devices with microphone muted and play a short greeting')
    args=parser.parse_args()
    if not args.live:parser.error('Pass --live explicitly; use codex-voice-probe.py for read-only discovery')
    binary=pathlib.Path(shutil.which('codex') or '').resolve()
    root=binary.parent.parent
    helper=root/'codex-resources/voice/bin/codex-voice-host'
    if not helper.is_file():raise RuntimeError('packaged native runtime unavailable')
    children=[];pending={};notifications=asyncio.Queue(32);reader=None;thread=None;server=None
    result={'chatgpt':False,'helperReady':False,'transportReady':False,'microphoneOpened':False,'apiFallback':False}
    stage='helper'
    async def stop(child):
        if child.returncode is None:
            child.terminate()
            try:await asyncio.wait_for(child.wait(),3)
            except asyncio.TimeoutError:child.kill();await child.wait()
    async def exchange(host,v):
        p=json.dumps(v).encode();host.stdin.write(struct.pack('>I',len(p))+p);await host.stdin.drain()
        n=struct.unpack('>I',await host.stdout.readexactly(4))[0]
        if not 0<n<=128*1024:raise RuntimeError('invalid helper frame')
        return json.loads(await host.stdout.readexactly(n))
    async def request(method,params):
        identifier=len(pending)+1+request.counter;request.counter+=1
        f=asyncio.get_running_loop().create_future();pending[identifier]=f
        server.stdin.write((json.dumps({'id':identifier,'method':method,'params':params})+'\n').encode());await server.stdin.drain()
        try:
            v=await asyncio.wait_for(f,25)
            if 'error' in v:raise RuntimeError('native request rejected')
            return v['result']
        finally:pending.pop(identifier,None)
    request.counter=0
    try:
        environment={k:v for k,v in os.environ.items() if k.upper() not in ('OPENAI_API_KEY','LD_PRELOAD','LD_LIBRARY_PATH','GST_PLUGIN_PATH','GST_PLUGIN_SYSTEM_PATH')}
        environment.update(GST_PLUGIN_PATH='',GST_PLUGIN_PATH_1_0='',GST_PLUGIN_SYSTEM_PATH='',GST_PLUGIN_SYSTEM_PATH_1_0='',GST_REGISTRY='/dev/null',GST_REGISTRY_UPDATE='no',GST_REGISTRY_FORK='no')
        for directory in ('/usr/lib/x86_64-linux-gnu/alsa-lib','/usr/lib64/alsa-lib','/usr/lib/alsa-lib'):
            if pathlib.Path(directory).is_dir():environment['ALSA_PLUGIN_DIR']=directory;break
        commit_child=await asyncio.create_subprocess_exec(str(helper),'--build-commit',stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.DEVNULL,env=environment)
        children.append(commit_child)
        commit=(await commit_child.communicate())[0].decode().strip()
        host=await asyncio.create_subprocess_exec(str(helper),stdin=asyncio.subprocess.PIPE,stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.DEVNULL,env=environment)
        children.append(host)
        assert (await exchange(host,{'type':'hello','protocol':1,'buildCommit':commit}))['type']=='ready'
        assert (await exchange(host,{'type':'initializeRuntime'}))['type']=='runtimeReady'
        result['helperReady']=True
        offer=await exchange(host,{'type':'startTransport'})
        assert offer['type']=='offer'
        stage='auth'
        server=await asyncio.create_subprocess_exec(str(binary),'app-server',stdin=asyncio.subprocess.PIPE,stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.DEVNULL,env=environment,limit=8*1024*1024)
        children.append(server)
        async def read():
            while line:=await server.stdout.readline():
                v=json.loads(line)
                if v.get('id') in pending:
                    f=pending[v['id']]
                    if not f.done():f.set_result(v)
                elif v.get('method','').startswith('thread/realtime/'):
                    notifications.put_nowait(v)
        reader=asyncio.create_task(read())
        await request('initialize',{'clientInfo':{'name':'zeron-voice-smoke','version':'1'},'capabilities':{'experimentalApi':True}})
        server.stdin.write(b'{"method":"initialized"}\n');await server.stdin.drain()
        account=await request('account/read',{'refreshToken':False})
        assert account.get('account',{}).get('type')=='chatgpt';result['chatgpt']=True
        config=(await request('config/read',{'includeLayers':False,'cwd':str(pathlib.Path.cwd())}))['config']
        assert config.get('model_provider','openai') in ('openai',None)
        assert not config.get('model_providers',{}).get('openai')
        assert not any(config.get(k) for k in ('experimental_realtime_ws_base_url','experimental_realtime_webrtc_call_base_url','experimental_realtime_ws_model'))
        started=await request('thread/start',{'ephemeral':True,'cwd':str(pathlib.Path.cwd()),'approvalPolicy':'never','sandbox':'read-only','developerInstructions':'Connectivity check. Do not use tools or perform tasks. Stay silent until user speech.'})
        thread=started['thread']['id']
        assert started['modelProvider']=='openai'
        stage='nativeCall'
        session=str(uuid.uuid4())
        await request('thread/realtime/start',{'threadId':thread,'transport':{'type':'webrtc','sdp':offer['sdp']},'version':'v3','realtimeSessionId':session,'outputModality':'audio','clientManagedHandoffs':False,'includeStartupContext':False})
        accepted=False;answer=None
        while not (accepted and answer):
            v=await asyncio.wait_for(notifications.get(),30)
            if v['method']=='thread/realtime/started':assert v['params']['version']=='v3';accepted=True
            elif v['method']=='thread/realtime/sdp':answer=v['params']['sdp']
            elif v['method'] in ('thread/realtime/error','thread/realtime/closed'):raise RuntimeError('native call failed')
        stage='applyAnswer'
        assert (await exchange(host,{'type':'applyAnswer','sdp':answer}))['type']=='transportReady'
        result['transportReady']=True
        if args.devices:
            stage='audioDevices'
            assert (await exchange(host,{'type':'openDevices'}))['type']=='devicesOpened'
            result['devicesOpened']=True
            assert (await exchange(host,{'type':'setAudioControls','controls':{'microphoneMuted':True,'speakerSuppressed':False}}))['type']=='audioControlsApplied'
            result['microphoneMuted']=True
            stage='playout'
            await request('thread/realtime/appendSpeech',{'threadId':thread,'text':'Hola.'})
            for _ in range(60):
                audio=await exchange(host,{'type':'inspectAudio'})
                if audio['state']['speakerPeak']>0:result['speakerAudioReceived']=True;break
                await asyncio.sleep(0.1)
            if not result.get('speakerAudioReceived'):raise RuntimeError('no playout observed')
        else:
            await asyncio.sleep(1)
        stage='stop'
        await request('thread/realtime/stop',{'threadId':thread});thread=None
        assert (await exchange(host,{'type':'close'}))['type']=='closed'
        await asyncio.wait_for(host.wait(),3)
        result['stopped']=True
    except Exception:
        result['failedStage']=stage
    finally:
        if server and thread:
            try:await request('thread/realtime/stop',{'threadId':thread})
            except Exception:pass
        if reader:reader.cancel();await asyncio.gather(reader,return_exceptions=True)
        for child in reversed(children):await stop(child)
        print(json.dumps(result,indent=2))
if __name__=='__main__':asyncio.run(asyncio.wait_for(main(),120))
