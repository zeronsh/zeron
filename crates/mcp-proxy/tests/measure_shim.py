#!/usr/bin/env python3
"""Paired Linux pass-through latency, including exec. No server/profile/network.
Usage: python3 tests/measure_shim.py PROXY NEW_SCRATCH
"""
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time

proxy, root = [Path(p).resolve() for p in sys.argv[1:]]
shim, real = root / 'shim', root / 'real'
shim.mkdir(parents=True)
real.mkdir()
(shim / 'node').symlink_to(proxy)
(real / 'node').symlink_to('/usr/bin/true')
env = {'PATH': f'{shim}:{real}:/usr/bin:/bin', 'ZERON_MCP_SHIM_DIR': str(shim)}
# Warm filesystem/linker pages before timing. Shell descendants take this same path.
for _ in range(20):
    subprocess.run([str(shim / 'node')], env=env, check=True)
samples = []
for n in range(300):
    times = {}
    for name in (['real', 'shim'] if n % 2 == 0 else ['shim', 'real']):
        start = time.perf_counter_ns()
        subprocess.run([str((real if name == 'real' else shim) / 'node')], env=env, check=True)
        times[name] = (time.perf_counter_ns() - start) / 1e6
    samples.append(times)
result = {'samples': len(samples), 'baseline_median_ms': statistics.median(s['real'] for s in samples),
    'shim_median_ms': statistics.median(s['shim'] for s in samples),
    'paired_added_median_ms': statistics.median(s['shim'] - s['real'] for s in samples)}
(root / 'latency.json').write_text(json.dumps(result, indent=2) + '\n')
print(json.dumps(result))
