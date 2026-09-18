#!/usr/bin/env python3
"""Run a reproducible Cloudflare transport profile with an owned local proxy."""
import argparse
import json
from pathlib import Path
import subprocess
import sys

PROFILES = {
    'stream': (2048, 600, 'stream', []),
    'very-slow': (1024, 1200, 'stream', []),
    'outage': (2048, 600, 'outage', []),
    'http': (2048, 600, 'http', ['--block-ws', '--lose-post-ack']),
    'upload': (2048, 600, 'upload', []),
    'catchup': (2048, 600, 'catchup', []),
}
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--binary', required=True)
parser.add_argument('--origin', required=True)
parser.add_argument('--profile', choices=PROFILES, required=True)
parser.add_argument('--output', required=True)
args = parser.parse_args()
rate, delay, mode, extra = PROFILES[args.profile]
proxy = subprocess.Popen([sys.executable, str(Path(__file__).with_name('transport-proxy.py')),
    '--upstream', args.origin, '--rate', str(rate), '--latency-ms', str(delay), *extra],
    stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
try:
    startup = json.loads(proxy.stdout.readline())
    result = subprocess.run([args.binary, startup['listen'], args.origin, mode],
                            capture_output=True, text=True, timeout=420)
    report = {'profile': args.profile, 'rate_bytes_per_second_per_direction': rate,
              'added_one_way_ms': delay, 'exit_code': result.returncode,
              'stdout': result.stdout, 'stderr': result.stderr}
except Exception as exc:
    report = {'profile': args.profile, 'exit_code': 1, 'error': str(exc)}
finally:
    proxy.terminate()
    try:
        _, errors = proxy.communicate(timeout=5)
    except subprocess.TimeoutExpired:
        proxy.kill()
        _, errors = proxy.communicate()
    report['proxy_stderr'] = errors
Path(args.output).write_text(json.dumps(report, indent=2)+'\n')
print(json.dumps(report, indent=2))
sys.exit(report['exit_code'])
