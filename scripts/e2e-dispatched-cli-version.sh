#!/usr/bin/env bash
# E2E: a real Zeron engine (headless) with pi and codex behind one dispatcher binary,
# the way mise shims work. Reads the engine's own "Model discovery" log lines.
#   A  Pi's version is Pi's, not Codex's (shared-shim cache collision)
#   B  updating Pi behind the shim (dispatcher unchanged) is noticed
#   C  a binary is not re-probed on every call
# usage: scripts/e2e-dispatched-cli-version.sh <zeron binary>   (needs pi, codex, node, uv)
set -uo pipefail
ZERON=$1
PI=$(command -v pi) CODEX=$(command -v codex) NODE=$(command -v node)
T=$(mktemp -d); PID=; trap 'kill $PID 2>/dev/null; rm -rf "$T"' EXIT
mkdir -p "$T/bin" "$T/node" "$T/data"; ln -s "$NODE" "$T/node/node"
cat >"$T/bin/dispatch" <<EOF
#!/bin/sh
name=\$(basename "\$0")
[ "\$1" = --version ] && { echo probe >>"$T/\$name.probes"; cat "$T/\$name.version"; exit 0; }
[ "\$name" = pi ] && exec "$PI" "\$@"
exec "$CODEX" "\$@"
EOF
chmod +x "$T/bin/dispatch"; ln -s dispatch "$T/bin/pi"; ln -s dispatch "$T/bin/codex"
# Above any real install, so the engine picks the dispatcher over a real pi it also finds.
echo 900.1.0 >"$T/pi.version"; echo 900.0.0 >"$T/codex.version"
cat >"$T/rpc.py" <<'EOF'
# /// script
# dependencies = ["websockets"]
# ///
import asyncio, json, sys, websockets
async def main():
    async with websockets.connect("ws://127.0.0.1:27903", max_size=None) as ws:
        await ws.send(json.dumps({"id": 1, "method": sys.argv[1], "params": json.loads(sys.argv[2])}))
        while json.loads(await ws.recv()).get("id") != 1:
            pass
asyncio.run(main())
EOF

PATH="$T/bin:$T/node:/usr/bin:/bin:/usr/sbin:/sbin" ZERON_DATA_DIR=$T/data ZERON_IPC_PORT=27903 \
  RUST_LOG=info NO_COLOR=1 "$ZERON" headless >"$T/log" 2>&1 & PID=$!
sleep 6
rpc() { uv run -q "$T/rpc.py" "$@" >/dev/null 2>&1; }
pi_version() { grep "Model discovery harness=Pi" "$T/log" | tail -1 | sed -E 's/.*binary_version=Some\("([^"]*)"\).*/\1/'; }

rpc ListModels '{"harness":"codex","force":true}'
rpc ListModels '{"harness":"pi","force":true}'
a=$(pi_version)
echo 900.1.1 >"$T/pi.version"
sleep 35
rpc ListModels '{"harness":"pi","force":true}'
b=$(pi_version)
for _ in 1 2 3 4 5; do rpc ListModels '{"harness":"codex"}'; done
probes=$(wc -l <"$T/codex.probes" | tr -d ' ')

ok=true
check() { if eval "$2"; then echo "PASS  $1"; else echo "FAIL  $1"; ok=false; fi; }
check "A pi version after a codex probe: $a (want 900.1.0)" '[ "$a" = 900.1.0 ]'
check "B pi version after updating pi behind the shim: $b (want 900.1.1)" '[ "$b" = 900.1.1 ]'
check "C codex --version probes over 7 calls in ~40s: $probes (want <= 3)" '[ "$probes" -le 3 ]'
$ok
