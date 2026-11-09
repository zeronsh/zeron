#!/bin/sh
# Copies example plugins into a Zeron data dir's plugins folder and signs
# them as that engine's account (`zeron plugin sign`), for a scratch engine:
#
#   ./install.sh <data-dir> <port> [plugin ...]     (default: every one here)
#
# The engine must be running (its IPC on <port>, its token in <data-dir>).
# Never the production engine: port 27654, ~/.zeron and ~/.comet-native
# (and anything under them) are refused.
# `focus` is meant to be signed by another account: install it on that
# account's engine, then copy its signed folder over (see rig/).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
data=${1:?usage: install.sh <data-dir> <port> [plugin ...]}
port=${2:?usage: install.sh <data-dir> <port> [plugin ...]}
shift 2
case "$port" in 27654) echo "refusing the production engine's port" >&2; exit 1 ;; esac
[ -d "$data" ] || { echo "no such data dir: $data (the engine makes it)" >&2; exit 1; }
real=$(cd "$data" && pwd -P)
for prod in "$HOME/.zeron" "$HOME/.comet-native"; do
  prod_real=$( (cd "$prod" 2>/dev/null && pwd -P) || echo "$prod")
  for p in "$prod" "$prod_real"; do
    case "$real" in "$p" | "$p"/*) echo "refusing the production engine's data ($prod)" >&2; exit 1 ;; esac
  done
done
if [ $# -eq 0 ]; then
  for dir in "$here"/*/; do
    if [ -f "$dir/zeron-plugin.json" ]; then set -- "$@" "$(basename "$dir")"; fi
  done
fi
zeron=${ZERON_BIN:-$root/target/debug/zeron}
mkdir -p "$data/plugins"
for id in "$@"; do
  rm -rf "$data/plugins/$id"
  cp -R "$here/$id" "$data/plugins/$id"
  cp "$root/apps/zgx/plugins-sdk/zeron.luau" "$data/plugins/$id/zeron.luau"
done
sleep 1
for id in "$@"; do
  env -u ZERON_IPC_TOKEN ZERON_IPC_PORT="$port" ZERON_DATA_DIR="$data" ZERON_IPC_TOKEN_FILE="$data/ipc-token" "$zeron" plugin sign "$id"
done
env -u ZERON_IPC_TOKEN ZERON_IPC_PORT="$port" ZERON_DATA_DIR="$data" ZERON_IPC_TOKEN_FILE="$data/ipc-token" "$zeron" plugin list
