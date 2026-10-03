#!/bin/bash
# Reproduce the unmodified upstream codex-core iOS compilation probe.
# Kept outside Zeron's Cargo workspace until the port actually works.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
REV=804d6306e84f570393cdd1eec94c41464f503b1a
CHECKOUT="${ZERON_CODEX_SPIKE_CHECKOUT:-$ROOT/target/native-agent-spike/codex}"
if [[ ! -d "$CHECKOUT/.git" ]]; then
  mkdir -p "$CHECKOUT"
  git -C "$CHECKOUT" init
  git -C "$CHECKOUT" remote add origin https://github.com/openai/codex.git
  git -C "$CHECKOUT" fetch --depth 1 origin "$REV"
  git -C "$CHECKOUT" checkout --detach FETCH_HEAD
fi
if [[ "$(git -C "$CHECKOUT" rev-parse HEAD)" != "$REV" ]]; then
  echo "error: expected Codex revision $REV in $CHECKOUT" >&2
  exit 1
fi
rustup target add --toolchain stable aarch64-apple-ios
cd "$CHECKOUT/codex-rs"
# +stable is explicit so the checkout's rust-toolchain.toml cannot silently
# select a different toolchain from the one with the installed iOS target.
cargo +stable check --locked -p codex-core -p codex-app-server-client --target aarch64-apple-ios
