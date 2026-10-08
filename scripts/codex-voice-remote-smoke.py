#!/usr/bin/env python3
"""Split-host voice diagnostic. Offline tests never start Codex or audio.

Live mode uses an explicit local helper and Codex app-server over SSH. Only
sanitized stage/counter results are printed; no SDP, transcript or raw errors.
This diagnostic does not use Zeron's production relay or certify its MCP routing.
"""
import argparse
import asyncio
import contextlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import struct
import sys
import uuid

MAX_SDP = 64 * 1024
MAX_FRAME = 128 * 1024
MAX_RPC = 1024 * 1024


class ProbeError(Exception):
    """A fixed diagnostic code, never a provider's error message."""


def validate_sdp(value):
    if not isinstance(value, str) or not 0 < len(value.encode()) <= MAX_SDP:
        raise ProbeError('invalid_sdp')
    lines = value.replace('\r\n', '\n').splitlines()
    if not lines or lines[0] != 'v=0' or not any(x.startswith('m=audio ') for x in lines):
        raise ProbeError('invalid_sdp')
    return value


def helper_environment():
    # Match the harness allowlist. In particular, no API key or loader overrides.
    allowed = {'HOME', 'USERPROFILE', 'SYSTEMROOT', 'WINDIR', 'TEMP', 'TMP',
               'TMPDIR', 'XDG_RUNTIME_DIR', 'PULSE_SERVER', 'PULSE_COOKIE',
               'PIPEWIRE_REMOTE', 'DBUS_SESSION_BUS_ADDRESS', 'HTTP_PROXY',
               'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY', 'SSL_CERT_FILE',
               'SSL_CERT_DIR', 'REQUESTS_CA_BUNDLE', 'CURL_CA_BUNDLE'}
    env = {k: v for k, v in os.environ.items() if k.upper() in allowed}
    for name in ('GST_PLUGIN_PATH', 'GST_PLUGIN_PATH_1_0',
                 'GST_PLUGIN_SYSTEM_PATH', 'GST_PLUGIN_SYSTEM_PATH_1_0'):
        env[name] = ''
    env.update(GST_REGISTRY=os.devnull, GST_REGISTRY_UPDATE='no', GST_REGISTRY_FORK='no')
    return env


def ssh_command(host, argv):
    if not host or host.startswith('-') or not re.fullmatch(r'[\w.@:-]+', host):
        raise ProbeError('invalid_ssh_host')
    # SSH accepts one remote shell command, so quote each argument for that shell.
    return ['ssh', '-T', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8',
            host, 'exec ' + shlex.join(argv)]


async def spawn(argv, **kwargs):
    return await asyncio.create_subprocess_exec(
        *argv, stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        stderr=asyncio.subprocess.DEVNULL, limit=MAX_RPC, **kwargs)


async def reap(child):
    if child.returncode is None:
        with contextlib.suppress(ProcessLookupError):
            child.terminate()
        try:
            await asyncio.wait_for(child.wait(), 2)
        except asyncio.TimeoutError:
            with contextlib.suppress(ProcessLookupError):
                child.kill()
            await child.wait()


async def metadata(argv, pattern, **kwargs):
    child = await spawn(argv, **kwargs)
    try:
        # Bounded read, not communicate(): an unexpected program cannot fill RAM.
        data = await asyncio.wait_for(child.stdout.read(513), 8)
        await asyncio.wait_for(child.wait(), 2)
        value = data.decode().strip()
        if child.returncode != 0 or len(data) > 512 or not re.fullmatch(pattern, value):
            raise ProbeError('invalid_build_metadata')
        return value
    finally:
        await reap(child)


class Helper:
    def __init__(self, child, timeout=20):
        self.child = child
        self.timeout = timeout
        self.broken = False

    async def exchange(self, message, expected):
        async def operation():
            payload = json.dumps(message).encode()
            if len(payload) > MAX_FRAME:
                raise ProbeError('helper_frame_overflow')
            self.child.stdin.write(struct.pack('>I', len(payload)) + payload)
            await self.child.stdin.drain()
            size = struct.unpack('>I', await self.child.stdout.readexactly(4))[0]
            if not 0 < size <= MAX_FRAME:
                raise ProbeError('helper_frame_overflow')
            reply = json.loads(await self.child.stdout.readexactly(size))
            if not isinstance(reply, dict) or reply.get('type') != expected:
                raise ProbeError('helper_protocol')
            return reply
        if self.broken:
            raise ProbeError('helper_closed')
        try:
            return await asyncio.wait_for(operation(), self.timeout)
        except BaseException:
            # Never reuse a pipe after a timed-out/cancelled partial frame.
            self.broken = True
            await reap(self.child)
            raise


class Server:
    def __init__(self, child, timeout=30):
        self.child = child
        self.timeout = timeout
        self.pending = {}
        self.sequence = 0
        self.events = asyncio.Queue(128)
        self.failure = None
        self.reader = asyncio.create_task(self.read())

    async def read(self):
        code = 'server_closed'
        try:
            while line := await self.child.stdout.readline():
                frame = json.loads(line)
                if not isinstance(frame, dict):
                    raise ProbeError('server_protocol')
                future = self.pending.get(frame.get('id'))
                if future is not None:
                    if not future.done():
                        future.set_result(frame)
                elif 'id' in frame and 'method' in frame:
                    # Never auto-approve an interactive tool or permission request.
                    await self.send({'id': frame['id'], 'error': {
                        'code': -32601, 'message': 'Diagnostic does not handle server requests'}})
                elif frame.get('method', '').startswith('thread/realtime/'):
                    self.events.put_nowait(frame)
        except asyncio.CancelledError:
            raise
        except asyncio.QueueFull:
            code = 'event_overflow'
        except Exception:
            code = 'server_protocol'
        finally:
            self.failure = code
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(ProbeError(code))
            # Wake a notification waiter even when the queue had filled.
            if self.events.full():
                self.events.get_nowait()
            self.events.put_nowait(None)

    async def send(self, message):
        self.child.stdin.write((json.dumps(message) + '\n').encode())
        await self.child.stdin.drain()

    async def request(self, method, params):
        if self.failure:
            raise ProbeError(self.failure)
        self.sequence += 1
        identifier = self.sequence
        future = asyncio.get_running_loop().create_future()
        self.pending[identifier] = future
        try:
            async def operation():
                await self.send({'id': identifier, 'method': method, 'params': params})
                frame = await future
                if 'error' in frame or 'result' not in frame:
                    raise ProbeError('request_rejected')
                return frame['result']
            return await asyncio.wait_for(operation(), self.timeout)
        finally:
            self.pending.pop(identifier, None)

    async def event(self):
        if self.failure:
            raise ProbeError(self.failure)
        event = await self.events.get()
        if event is None:
            raise ProbeError(self.failure or 'server_closed')
        return event

    async def close(self):
        self.reader.cancel()
        await asyncio.gather(self.reader, return_exceptions=True)
        await reap(self.child)


class Observation:
    def __init__(self, thread, session):
        self.thread = thread
        self.session = session
        self.started = False
        self.answer = None
        self.finals = set()
        self.counts = {'userFinals': 0, 'assistantFinals': 0, 'handoffPromotions': 0}

    def accept(self, frame):
        p = frame.get('params') or {}
        if p.get('threadId') != self.thread:
            return
        if p.get('realtimeSessionId', self.session) != self.session:
            return
        method = frame.get('method')
        if method == 'thread/realtime/started':
            if p.get('realtimeSessionId') != self.session or p.get('version') != 'v3':
                raise ProbeError('session_mismatch')
            self.started = True
        elif method == 'thread/realtime/sdp':
            answer = validate_sdp(p.get('sdp'))
            if self.answer is not None and self.answer != answer:
                raise ProbeError('conflicting_answer')
            self.answer = answer
        elif method in ('thread/realtime/closed', 'thread/realtime/error'):
            raise ProbeError('provider_closed')
        elif method == 'thread/realtime/item/completed':
            item = p.get('item') or {}
            if item.get('realtimeSessionId') != self.session or not isinstance(item.get('id'), str):
                return
            key = item['id']
            if key in self.finals:
                return
            if len(self.finals) >= 4096:
                raise ProbeError('item_overflow')
            self.finals.add(key)
            if item.get('type') == 'transcriptSegment' and item.get('role') in ('user', 'assistant'):
                self.counts[item['role'] + 'Finals'] += 1
            elif item.get('type') == 'bemItemPromoted':
                self.counts['handoffPromotions'] += 1

    async def negotiate(self, server, timeout=90):
        async def wait():
            while not (self.started and self.answer is not None):
                self.accept(await server.event())
            # Catch a close already queued alongside the two successful events.
            while not server.events.empty():
                self.accept(await server.event())
            return self.answer
        return await asyncio.wait_for(wait(), timeout)


async def exercise(server, helper, build, cwd, report, seconds=0):
    """One fresh provider thread: no cross-generation SDP without session IDs."""
    thread = None
    started = False
    try:
        report['stage'] = 'initialize'
        await server.request('initialize', {
            'clientInfo': {'name': 'zeron-voice-remote-smoke', 'version': '1'},
            'capabilities': {'experimentalApi': True}})
        await server.send({'method': 'initialized'})
        account = await server.request('account/read', {'refreshToken': False})
        if (account.get('account') or {}).get('type') != 'chatgpt':
            raise ProbeError('chatgpt_required')
        config = (await server.request('config/read', {'includeLayers': False, 'cwd': cwd}))['config']
        if (config.get('model_provider') not in (None, 'openai')
                or config.get('model_providers', {}).get('openai')
                or any(config.get(k) for k in ('experimental_realtime_ws_base_url',
                       'experimental_realtime_webrtc_call_base_url', 'experimental_realtime_ws_model'))):
            raise ProbeError('custom_provider')
        await server.request('thread/realtime/listVoices', {})
        report['voicesBeforeThread'] = True
        report['stage'] = 'local_runtime'
        await helper.exchange({'type': 'hello', 'protocol': 1, 'buildCommit': build}, 'ready')
        await helper.exchange({'type': 'initializeRuntime'}, 'runtimeReady')
        offer = await helper.exchange({'type': 'startTransport'}, 'offer')
        sdp = validate_sdp(offer.get('sdp'))
        result = await server.request('thread/start', {
            'ephemeral': True, 'cwd': cwd, 'approvalPolicy': 'never', 'sandbox': 'read-only',
            'developerInstructions': 'Voice connectivity diagnostic. Stay silent until spoken to. Do not use tools.'})
        thread = result['thread']['id']
        if result.get('modelProvider') != 'openai':
            raise ProbeError('custom_provider')
        observation = Observation(thread, str(uuid.uuid4()))
        report['stage'] = 'negotiate'
        # Stop even if the start reply is lost after reaching the provider.
        started = True
        await server.request('thread/realtime/start', {
            'threadId': thread, 'transport': {'type': 'webrtc', 'sdp': sdp},
            'version': 'v3', 'realtimeSessionId': observation.session,
            'outputModality': 'audio', 'clientManagedHandoffs': False,
            'includeStartupContext': True})
        answer = await observation.negotiate(server)
        await helper.exchange({'type': 'applyAnswer', 'sdp': answer}, 'transportReady')
        report['transportReady'] = True
        report['stage'] = 'observe'
        if seconds:
            await helper.exchange({'type': 'openDevices'}, 'devicesOpened')
            await helper.exchange({'type': 'setAudioControls', 'controls': {
                'microphoneMuted': True, 'speakerSuppressed': True}}, 'audioControlsApplied')
            report['muteControlAccepted'] = True
            await helper.exchange({'type': 'setAudioControls', 'controls': {
                'microphoneMuted': False, 'speakerSuppressed': False}}, 'audioControlsApplied')
            report['devicesOpened'] = True
            print('Audio local activo durante la prueba; habla para verificar la respuesta.', file=sys.stderr)
        deadline = asyncio.get_running_loop().time() + (seconds or 1)
        while (remaining := deadline - asyncio.get_running_loop().time()) > 0:
            try:
                event = await asyncio.wait_for(server.event(), min(remaining, 0.25))
                observation.accept(event)
            except asyncio.TimeoutError:
                pass
        report.update(observation.counts)
        if seconds:
            report['bidirectionalFinalsObserved'] = all(
                observation.counts[key] > 0 for key in ('userFinals', 'assistantFinals'))
        report['stage'] = 'stop'
    finally:
        # Local audio closes first, including after cancellation or lost control.
        if not helper.broken:
            try:
                await asyncio.wait_for(helper.exchange({'type': 'close'}, 'closed'), 3)
                report['localClosed'] = True
            except Exception:
                report['localClosed'] = False
        await reap(helper.child)
        if started:
            try:
                await asyncio.wait_for(server.request('thread/realtime/stop', {'threadId': thread}), 8)
                report['stopAcknowledged'] = True
                async def closed():
                    while True:
                        event = await server.event()
                        if (event.get('method') == 'thread/realtime/closed'
                                and event.get('params', {}).get('threadId') == thread
                                and event['params'].get('reason') == 'requested'):
                            return
                await asyncio.wait_for(closed(), 3)
                report['providerClosed'] = True
            except Exception:
                report.setdefault('stopAcknowledged', False)
                report['providerClosed'] = False


async def live(args):
    report = {'mode': 'split-host', 'gateG0Passed': False,
              'mcpRoutingVerified': False, 'localPlatform': platform.system() + ' ' + platform.machine()}
    children = []
    server = None
    try:
        report['stage'] = 'metadata'
        helper_path = Path(args.helper).resolve(strict=True)
        env = helper_environment()
        build = await metadata([str(helper_path), '--build-commit'], r'[a-zA-Z0-9._-]{1,128}', env=env)
        report['helperBuild'] = build
        report['remoteCodex'] = await metadata(
            ssh_command(args.ssh_host, ['env', '-u', 'OPENAI_API_KEY', args.remote_codex, '--version']),
            r'codex(?:-cli)? [0-9][a-zA-Z0-9.+_-]{0,100}')
        report['remotePlatform'] = await metadata(ssh_command(args.ssh_host, ['uname', '-sm']),
                                                 r'[a-zA-Z0-9 _.-]{1,100}')
        child = await spawn([str(helper_path)], env=env, cwd=str(helper_path.parent))
        children.append(child)
        remote = await spawn(ssh_command(args.ssh_host, [
            'env', '-u', 'OPENAI_API_KEY', args.remote_codex, 'app-server']))
        children.append(remote)
        server = Server(remote)
        await exercise(server, Helper(child), build, args.remote_cwd, report,
                       args.seconds if args.devices else 0)
        report['ok'] = (report.get('localClosed', False) and report.get('providerClosed', False)
                        and (not args.devices or report.get('bidirectionalFinalsObserved', False)))
    except ProbeError as error:
        report.update(ok=False, error=str(error))
    except asyncio.TimeoutError:
        report.update(ok=False, error='timeout')
    except asyncio.CancelledError:
        report.update(ok=False, error='cancelled')
    except Exception:
        report.update(ok=False, error='io_or_protocol')
    finally:
        if server:
            await server.close()
        for child in reversed(children):
            await reap(child)
        print(json.dumps(report, indent=2))
    return 0 if report.get('ok') else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--live', action='store_true', help='Use real subscription voice on the named host')
    parser.add_argument('--ssh-host', required=True, help='Explicit configured SSH host; no discovery')
    parser.add_argument('--remote-codex', required=True, help='Absolute Codex executable path on the host')
    parser.add_argument('--remote-cwd', required=True, help='Absolute existing directory on the host')
    parser.add_argument('--helper', required=True, help='Explicit local codex-voice-host; no local CLI lookup')
    parser.add_argument('--devices', action='store_true', help='Open LOCAL microphone/speaker for a bounded conversation')
    parser.add_argument('--seconds', type=int, default=30, help='Conversation duration, 1–120 seconds')
    args = parser.parse_args()
    if not args.live:
        parser.error('--live is required; offline: python3 -m unittest discover -s scripts/tests -p "test_voice_remote*.py"')
    if not 1 <= args.seconds <= 120:
        parser.error('--seconds must be between 1 and 120')
    if not all(x.startswith('/') for x in (args.remote_codex, args.remote_cwd, args.helper)):
        parser.error('Codex, helper and cwd must be absolute paths')
    return asyncio.run(live(args))


if __name__ == '__main__':
    sys.exit(main())
