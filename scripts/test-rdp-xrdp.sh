#!/usr/bin/env bash
# Opt-in local interoperability lab. Only loopback is published; credentials are
# synthetic and exist only inside this disposable fixture. Never use real secrets.
set -euo pipefail
output="${1:?usage: test-rdp-xrdp.sh OUTPUT_DIRECTORY}"
runner="${CONTAINER_RUNTIME:-podman}"
port="${RDP_LAB_PORT:-33991}"
binary="${RDP_PROBE_BINARY:-target/release/examples/probe}"
name="zeron-rdp-lab-$$"
mkdir -p "$output"
cleanup() { "$runner" exec "$name" cat /tmp/zeron-rdp-input.txt >"$output/input.txt" 2>/dev/null || true; "$runner" exec "$name" cat /var/log/xrdp.log >"$output/xrdp.log" 2>/dev/null || true; "$runner" logs "$name" >"$output/container.log" 2>&1 || true; "$runner" rm -f "$name" >/dev/null 2>&1 || true; }
trap cleanup EXIT
"$runner" build -t zeron-rdp-lab:local -f crates/rdp/tests/lab/Containerfile crates/rdp/tests/lab >"$output/build.log" 2>&1
"$runner" run -d --name "$name" -p "127.0.0.1:$port:3389" zeron-rdp-lab:local >"$output/container-id"
for _ in $(seq 1 100); do
  if "$runner" exec "$name" awk '$2 ~ /:0D3D$/ && $4 == "0A" {found=1} END {exit !found}' /proc/net/tcp /proc/net/tcp6; then break; fi
  sleep .1
done
export ZERON_RDP_HOST=127.0.0.1 ZERON_RDP_PORT="$port" ZERON_RDP_USER=rdptest ZERON_RDP_PASSWORD=rdptest
export ZERON_RDP_KEYBOARD_LAYOUT=040a ZERON_RDP_LAB_TRUST=1 ZERON_RDP_LAB_INPUT=1
"$binary" >"$output/probe.log" 2>&1
"$runner" exec "$name" cat /tmp/zeron-rdp-input.txt >"$output/input.txt"
"$runner" exec "$name" cat /tmp/zeron-rdp-clipboard.txt >"$output/clipboard.txt"
python3 - "$output" <<'PY'
from pathlib import Path
import sys
p=Path(sys.argv[1])
assert (p/'input.txt').read_bytes() == 'niño con acento á\n'.encode()
assert (p/'clipboard.txt').read_bytes() == 'local ñ\r\nline two'.encode()
(p/'result.txt').write_text('ok\n')
PY
"$runner" exec "$name" cat /var/log/xrdp.log >"$output/xrdp.log"
