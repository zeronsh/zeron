#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"
python3 - "$@" <<'PY'
import json
import subprocess
import sys
import tempfile

checks = {
    "hub": (["-p", "zeron-private"], [[]]),
    "engine": (["-p", "zeron-engine", "--lib", "--test", "private_workspace", "--test", "local_first", "--test", "local_profiles", "--test", "local_import", "--test", "workspace_sync", "--test", "relay_delivery"], [[]]),
    "documents": (["-p", "zeron-doc", "-p", "zeron-proto", "--lib"], [[]]),
    "sync": (["-p", "zeron-sync", "--features", "mock-server", "--lib", "--test", "registry_client"], [[]]),
    "rpc": (["-p", "zeron-rpc", "--lib", "--test", "device_room"], [[]]),
    "ui": (["-p", "zeron-ui", "--lib"], [["private"], ["settings::workspace::tests"], ["settings::devices::tests"], ["nav_"]]),
}
requested = set(sys.argv[1:])
if unknown := requested - checks.keys():
    raise SystemExit(f"Unknown checks: {', '.join(sorted(unknown))}. Choose from: {', '.join(checks)}")
# A running desktop discovers project listeners. Keep test listeners outside
# the project directory so its independent preview probes cannot reach them.
with tempfile.TemporaryDirectory(prefix="zeron-private-check-") as run_dir:
    for name, (cargo_args, test_runs) in checks.items():
        if requested and name not in requested:
            continue
        command = ["cargo", "test", "--locked", *cargo_args, "--no-run", "--message-format=json"]
        print("Building:", " ".join(command), flush=True)
        build = subprocess.Popen(command, stdout=subprocess.PIPE, text=True)
        binaries = set()
        for line in build.stdout:
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                print(line, end="")
                continue
            if event.get("reason") == "compiler-message":
                print(event["message"].get("rendered", ""), end="")
            if event.get("reason") == "compiler-artifact" and event.get("profile", {}).get("test") and event.get("executable"):
                binaries.add(event["executable"])
        if build.wait() != 0:
            raise SystemExit(build.returncode)
        if not binaries:
            raise SystemExit("Cargo produced no test executables")
        for binary in sorted(binaries):
            for test_args in test_runs:
                subprocess.run([binary, *test_args], cwd=run_dir, check=True)
PY
