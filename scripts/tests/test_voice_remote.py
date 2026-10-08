"""Offline checks for the actual subprocess pipes, bounds and cleanup paths."""
import asyncio
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('remote_probe', Path(__file__).parents[1] / 'codex-voice-remote-smoke.py')
probe = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(probe)
FIXTURE = Path(__file__).with_name('voice_remote_fixture.py')
SDP = 'v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n'


class ContractTests(unittest.TestCase):
    def test_cli_requires_live_before_resolving_or_spawning_anything(self):
        result = subprocess.run([sys.executable, str(SPEC.origin),
            '--ssh-host', 'does-not-exist.invalid', '--remote-codex', '/no/codex',
            '--remote-cwd', '/no/cwd', '--helper', '/no/helper'], capture_output=True, text=True)
        self.assertEqual(result.returncode, 2)
        self.assertIn('--live is required', result.stderr)
        self.assertNotIn('Traceback', result.stderr)

    def test_sdp_bounds_are_bytes_and_reject_malformed(self):
        for value in (None, '', 'secret', SDP + 'é' * (probe.MAX_SDP // 2)):
            with self.subTest(value=type(value).__name__), self.assertRaisesRegex(probe.ProbeError, '^invalid_sdp$'):
                probe.validate_sdp(value)
        self.assertEqual(probe.validate_sdp(SDP), SDP)
        self.assertEqual(len(probe.validate_sdp(SDP + ' ' * (probe.MAX_SDP - len(SDP))).encode()), probe.MAX_SDP)

    def test_ssh_quotes_remote_command_and_rejects_option_injection(self):
        args = ['env', '-u', 'OPENAI_API_KEY', '/tmp/a $(touch nope)/codex', 'app-server']
        command = probe.ssh_command('user@fedora', args)
        self.assertEqual(shlex.split(command[-1]), ['exec'] + args)
        self.assertIn('BatchMode=yes', command)
        for host in ('-oProxyCommand=bad', 'host;bad', 'host\ncmd', ''):
            with self.assertRaises(probe.ProbeError):
                probe.ssh_command(host, args)

    def test_helper_does_not_inherit_credentials_or_loader_overrides(self):
        with patch.dict(os.environ, {'OPENAI_API_KEY': 'SECRET', 'DYLD_INSERT_LIBRARIES': 'SECRET',
                                     'LD_PRELOAD': 'SECRET', 'GST_PLUGIN_PATH': 'SECRET', 'HOME': '/tmp'}, clear=True):
            env = probe.helper_environment()
        self.assertNotIn('SECRET', env.values())
        self.assertEqual(env['HOME'], '/tmp')
        self.assertEqual(env['GST_PLUGIN_PATH'], '')

    def test_observation_correlates_and_deduplicates_without_retaining_text(self):
        observation = probe.Observation('thread', 'session')
        frame = {'method': 'thread/realtime/item/completed', 'params': {
            'threadId': 'other', 'item': {'id': 'i1', 'type': 'transcriptSegment',
                'realtimeSessionId': 'session', 'role': 'user', 'text': 'SECRET'}}}
        observation.accept(frame)
        frame['params']['threadId'] = 'thread'
        frame['params']['item']['realtimeSessionId'] = 'old-session'
        observation.accept(frame)
        self.assertEqual(observation.counts['userFinals'], 0)
        frame['params']['item']['realtimeSessionId'] = 'session'
        observation.accept(frame)
        observation.accept(frame)
        self.assertEqual(observation.counts['userFinals'], 1)
        self.assertNotIn('SECRET', repr(vars(observation)))


class PipeTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.children = []
        self.servers = []

    async def asyncTearDown(self):
        for server in self.servers:
            await server.close()
        for child in self.children:
            await probe.reap(child)
            self.assertIsNotNone(child.returncode)

    async def child(self, role, mode):
        child = await probe.spawn([sys.executable, str(FIXTURE), role, mode])
        self.children.append(child)
        return child

    async def server(self, mode='normal', timeout=1):
        server = probe.Server(await self.child('server', mode), timeout=timeout)
        self.servers.append(server)
        return server

    async def helper(self, mode='normal', timeout=1):
        return probe.Helper(await self.child('helper', mode), timeout=timeout)

    async def test_complete_split_pipes_with_both_notification_orders(self):
        for mode in ('normal', 'reverse'):
            report = {}
            await probe.exercise(await self.server(mode), await self.helper(), 'fixture', '/tmp', report)
            self.assertTrue(report['transportReady'])
            self.assertTrue(report['voicesBeforeThread'])
            self.assertTrue(report['localClosed'])
            self.assertTrue(report['stopAcknowledged'])
            self.assertTrue(report['providerClosed'])
            self.assertEqual(report['userFinals'], 1)
            self.assertEqual(report['assistantFinals'], 1)
            self.assertNotIn('SECRET', json.dumps(report))
            self.assertNotIn('devicesOpened', report)

    async def test_provider_rejection_does_not_expose_payload(self):
        with self.assertRaisesRegex(probe.ProbeError, '^request_rejected$'):
            await (await self.server('reject')).request('initialize', {})

    async def test_closed_server_wakes_pending_request(self):
        with self.assertRaisesRegex(probe.ProbeError, '^server_closed$'):
            await (await self.server('disconnect')).request('initialize', {})

    async def test_request_timeout_removes_pending_future(self):
        server = await self.server('hang', timeout=0.1)
        with self.assertRaises(asyncio.TimeoutError):
            await server.request('initialize', {})
        self.assertEqual(server.pending, {})

    async def test_bad_auth_and_config_fail_before_transport(self):
        for mode, error in [('apikey', 'chatgpt_required'), ('custom', 'custom_provider')]:
            report = {}
            with self.assertRaisesRegex(probe.ProbeError, '^' + error + '$'):
                await probe.exercise(await self.server(mode), await self.helper(), 'fixture', '/tmp', report)
            self.assertNotIn('transportReady', report)
            self.assertTrue(report['localClosed'])

    async def test_failed_negotiation_closes_helper_and_stops_provider(self):
        for mode, error in [('closed', 'provider_closed'), ('invalid-answer', 'invalid_sdp')]:
            report = {}
            with self.assertRaisesRegex(probe.ProbeError, '^' + error + '$'):
                await probe.exercise(await self.server(mode), await self.helper(), 'fixture', '/tmp', report)
            self.assertTrue(report['localClosed'])
            self.assertTrue(report['stopAcknowledged'])

    async def test_lost_start_reply_still_stops_provider(self):
        report = {}
        with self.assertRaises(asyncio.TimeoutError):
            await probe.exercise(await self.server('lost-start', timeout=0.15),
                                 await self.helper(), 'fixture', '/tmp', report)
        self.assertTrue(report['localClosed'])
        self.assertTrue(report['stopAcknowledged'])

    async def test_cancel_during_start_reaps_and_stops(self):
        report = {}
        helper = await self.helper()
        task = asyncio.create_task(probe.exercise(await self.server('lost-start'), helper, 'fixture', '/tmp', report))
        async def reached_start():
            while report.get('stage') != 'negotiate':
                await asyncio.sleep(0.005)
        await asyncio.wait_for(reached_start(), 1)
        task.cancel()
        with self.assertRaises(asyncio.CancelledError):
            await task
        self.assertTrue(report['localClosed'])
        self.assertTrue(report['stopAcknowledged'])
        self.assertIsNotNone(helper.child.returncode)

    async def test_helper_truncated_oversized_and_hung_frames_are_reaped(self):
        for mode, error in [('truncated', asyncio.IncompleteReadError),
                            ('oversize', probe.ProbeError), ('hang', asyncio.TimeoutError)]:
            helper = await self.helper(mode, timeout=0.1)
            with self.assertRaises(error):
                await helper.exchange({'type': 'hello'}, 'ready')
            self.assertIsNotNone(helper.child.returncode)
            with self.assertRaisesRegex(probe.ProbeError, '^helper_closed$'):
                await helper.exchange({'type': 'close'}, 'closed')

    async def test_negotiation_ignores_old_thread_and_session_and_times_out(self):
        class Silent:
            events = asyncio.Queue()
            async def event(self):
                return await self.events.get()
        server = Silent()
        await server.events.put({'method': 'thread/realtime/started', 'params': {
            'threadId': 'old', 'realtimeSessionId': 'session', 'version': 'v3'}})
        await server.events.put({'method': 'thread/realtime/started', 'params': {
            'threadId': 'thread', 'realtimeSessionId': 'old', 'version': 'v3'}})
        observation = probe.Observation('thread', 'session')
        with self.assertRaises(asyncio.TimeoutError):
            await observation.negotiate(server, timeout=0.05)
        self.assertFalse(observation.started)

    async def test_queued_close_prevents_successful_negotiation(self):
        class Buffered:
            events = asyncio.Queue()
            async def event(self):
                return await self.events.get()
        server = Buffered()
        for method, params in [('started', {'realtimeSessionId': 'session', 'version': 'v3'}),
                               ('sdp', {'sdp': SDP}), ('closed', {'reason': 'transport_closed'})]:
            await server.events.put({'method': 'thread/realtime/' + method,
                                     'params': dict(threadId='thread', **params)})
        with self.assertRaisesRegex(probe.ProbeError, '^provider_closed$'):
            await probe.Observation('thread', 'session').negotiate(server)

    async def test_conflicting_duplicate_answer_is_rejected(self):
        observation = probe.Observation('thread', 'session')
        frame = {'method': 'thread/realtime/sdp', 'params': {'threadId': 'thread', 'sdp': SDP}}
        observation.accept(frame)
        observation.accept(frame)
        frame['params']['sdp'] = SDP + 'a=sendrecv\r\n'
        with self.assertRaisesRegex(probe.ProbeError, '^conflicting_answer$'):
            observation.accept(frame)


if __name__ == '__main__':
    unittest.main()
