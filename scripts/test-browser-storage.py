#!/usr/bin/env python3
"""Exercise real browser storage with synthetic data across independent processes.

Linux: xvfb-run -a python3 scripts/test-browser-storage.py --helper /path/to/zeron-webkit
Native shell (Linux/macOS): python3 scripts/test-browser-storage.py --fixture /path/to/browser-fixture
"""
import argparse
import http.server
import json
import os
from pathlib import Path
import queue
import struct
import subprocess
import tempfile
import threading
import time
from urllib.parse import urlparse


PAGE = r"""<!doctype html><title>Storage fixture</title><script>
(async () => {
  const mode = location.pathname.slice(1);
  const check = (ok, message) => { if (!ok) throw Error(message); };
  const db = await new Promise((resolve, reject) => {
    const r = indexedDB.open('storage-fixture', 1);
    r.onupgradeneeded = () => r.result.createObjectStore('values');
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
  const transaction = (write, value) => new Promise((resolve, reject) => {
    const tx = db.transaction('values', write ? 'readwrite' : 'readonly');
    const store = tx.objectStore('values');
    const r = write ? (value === null ? store.delete('token') : store.put(value, 'token')) : store.get('token');
    tx.oncomplete = () => resolve(r.result);
    tx.onerror = () => reject(tx.error);
    tx.onabort = () => reject(tx.error || Error('transaction aborted'));
  });
  if (mode === 'write') {
    document.cookie = 'durable=fixture; Path=/; Max-Age=3600; SameSite=Lax';
    document.cookie = 'expired=fixture; Path=/; Max-Age=1';
    document.cookie = 'removed=fixture; Path=/; Max-Age=3600';
    document.cookie = 'removed=; Path=/; Max-Age=0';
    localStorage.setItem('token', 'fixture');
    await transaction(true, 'fixture');
  } else if (mode === 'delete') {
    document.cookie = 'durable=; Path=/; Max-Age=0';
    localStorage.removeItem('token');
    await transaction(true, null);
  } else {
    const present = mode === 'read';
    check(document.cookie.includes('durable=fixture') === present, 'persistent cookie');
    check(!document.cookie.includes('expired='), 'expired cookie returned');
    check(!document.cookie.includes('removed='), 'deleted cookie returned');
    check(localStorage.getItem('token') === (present ? 'fixture' : null), 'localStorage');
    check((await transaction(false)) === (present ? 'fixture' : undefined), 'IndexedDB');
    const cookies = await (await fetch('/cookies')).json();
    check(cookies.includes('httpOnlyToken=fixture') === present, 'HttpOnly cookie');
    check(!document.cookie.includes('httpOnlyToken'), 'HttpOnly cookie exposed');
  }
  db.close();
  document.title = 'storage:pass';
})().catch(error => { document.title = 'storage:fail:' + error.message; });
</script>"""


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        mode = urlparse(self.path).path
        self.send_response(200)
        self.send_header('Cache-Control', 'no-store')
        if mode == '/cookies':
            body = json.dumps(self.headers.get('Cookie', '')).encode()
            self.send_header('Content-Type', 'application/json')
        else:
            body = PAGE.encode()
            self.send_header('Content-Type', 'text/html')
            if mode in ('/write', '/delete'):
                age = 3600 if mode == '/write' else 0
                self.send_header('Set-Cookie', f'httpOnlyToken=fixture; Path=/; HttpOnly; SameSite=Lax; Max-Age={age}')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


class Helper:
    def __init__(self, binary, profile, expect_error=False):
        self.process = subprocess.Popen([binary, '--profile', str(profile)], stdin=subprocess.PIPE,
                                        stdout=subprocess.PIPE)
        self.events = queue.Queue()
        self.thread = threading.Thread(target=self.receive, daemon=True)
        self.thread.start()
        kind, _, data = self.events.get(timeout=15)
        if expect_error:
            assert kind == b'E', (kind, data)
            assert self.process.wait(timeout=5) != 0
            self.process.stdin.close()
            self.process.stdout.close()
            return
        if kind != b'R':
            self.process.kill()
            self.process.wait()
            raise AssertionError(('helper startup failed', kind, data))

    def receive(self):
        def read(length):
            data = bytearray()
            while len(data) < length:
                part = self.process.stdout.read(length - len(data))
                if not part:
                    raise EOFError()
                data.extend(part)
            return bytes(data)
        try:
            while True:
                kind, page, length = struct.unpack('<cII', read(9))
                self.events.put((kind, page, read(length)))
        except EOFError:
            self.events.put((b'X', 0, b'helper exited'))

    def command(self, **command):
        data = json.dumps(command).encode()
        self.process.stdin.write(struct.pack('<I', len(data)) + data)
        self.process.stdin.flush()

    def page(self, page, url):
        self.command(cmd='create', id=page)
        self.command(cmd='load', id=page, url=url)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            kind, identity, data = self.events.get(timeout=max(.1, deadline - time.monotonic()))
            if kind == b'X':
                raise AssertionError(data)
            if kind != b'S' or identity != page:
                continue
            state = json.loads(data)
            assert not state.get('error'), state
            title = state.get('title', '')
            assert not title.startswith('storage:fail:'), title
            if title == 'storage:pass':
                return
        raise AssertionError('storage page timed out')

    def close(self):
        self.process.stdin.close()  # production shutdown uses EOF, too
        try:
            assert self.process.wait(timeout=5) == 0
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()
            raise
        finally:
            self.thread.join(timeout=2)
            self.process.stdout.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument('--helper', type=str)
    group.add_argument('--fixture', type=str)
    args = parser.parse_args()
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    origin = f'http://127.0.0.1:{server.server_port}'
    with tempfile.TemporaryDirectory(prefix='zeron-browser-storage-') as temporary:
        root = Path(temporary)
        def run(profile, mode):
            if args.helper:
                browser = Helper(args.helper, root / profile)
                try:
                    browser.page(1, origin + '/' + mode)
                    if mode == 'write':
                        time.sleep(2.2)  # expire the short-lived cookie
                        browser.command(cmd='close', id=1)
                        browser.page(2, origin + '/read')
                finally:
                    browser.close()
            else:
                env = dict(os.environ, ZERON_BROWSER_STORAGE_ROOT=str(root),
                           ZERON_BROWSER_STORAGE_PROFILE=profile,
                           ZERON_BROWSER_STORAGE_URL=origin + '/' + mode)
                command = [str(Path(args.fixture).resolve()), str(root / 'captures')]
                if os.uname().sysname == 'Darwin':
                    command.insert(0, str(Path(__file__).resolve().parent / 'run-macos-browser-fixture.sh'))
                subprocess.run(command, env=env, check=True, timeout=60)
            print(f'PASS {profile}: {mode}', flush=True)

        try:
            for profile, mode in [('a', 'write'), ('a', 'read'), ('b', 'empty'),
                                  ('a', 'read'), ('a', 'delete'), ('a', 'empty')]:
                run(profile, mode)
            if args.helper:
                browser = Helper(args.helper, root / 'a')
                try:
                    Helper(args.helper, root / 'a', expect_error=True)
                    other = Helper(args.helper, root / 'b')
                    other.close()
                finally:
                    browser.close()
                bad = root / 'not-a-directory'
                bad.write_text('storage must fail')
                Helper(args.helper, bad, expect_error=True)
                assert (root / 'a' / 'cookies.sqlite').stat().st_mode & 0o077 == 0
                print('PASS profile locking, independent profiles, storage errors, file permissions', flush=True)
        finally:
            if args.fixture and os.uname().sysname == 'Darwin':
                for profile in ('a', 'a-other', 'b'):
                    env = dict(os.environ, ZERON_BROWSER_STORAGE_ROOT=str(root),
                               ZERON_BROWSER_STORAGE_PROFILE=profile,
                               ZERON_BROWSER_STORAGE_URL='cleanup')
                    subprocess.run([str(Path(__file__).resolve().parent / 'run-macos-browser-fixture.sh'),
                                    str(Path(args.fixture).resolve()), str(root / 'captures')],
                                   env=env, check=True, timeout=60)
    server.shutdown()


if __name__ == '__main__':
    main()
