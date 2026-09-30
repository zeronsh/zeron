#!/bin/bash
# Build the static musl engine that runs inside the on-device proot guest and
# stage it as jniLibs/<abi>/libzeron.so (docs/android.md § Runtime contract).
#
#   scripts/android/build-engine.sh [out_dir]     # default target/android-runtime
#
# Needs rustup targets aarch64-/x86_64-unknown-linux-musl, cargo-zigbuild and
# zig. ZERON_ANDROID_ABIS="x86_64" limits the build (e.g. emulator-only).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/android-runtime}"
ABIS="${ZERON_ANDROID_ABIS:-arm64-v8a x86_64}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target/android-guest}"

die() { echo "error: $*" >&2; exit 1; }
command -v cargo >/dev/null || die "cargo not found"
command -v cargo-zigbuild >/dev/null || die "cargo-zigbuild not found (cargo install cargo-zigbuild)"
command -v zig >/dev/null || die "zig not found on PATH (https://ziglang.org/download/)"

cd "$ROOT"
for abi in $ABIS; do
  case "$abi" in
    arm64-v8a) triple=aarch64-unknown-linux-musl ;;
    x86_64) triple=x86_64-unknown-linux-musl ;;
    *) die "unsupported ABI $abi" ;;
  esac
  if command -v rustup >/dev/null && ! rustup target list --installed | grep -qx "$triple"; then
    die "rust target $triple missing (rustup target add $triple)"
  fi
  cargo zigbuild --locked -p zeron --no-default-features --release --target "$triple"
  mkdir -p "$OUT/jniLibs/$abi"
  install -m 0755 "$CARGO_TARGET_DIR/$triple/release/zeron" "$OUT/jniLibs/$abi/libzeron.so"
  echo "engine ($abi): $OUT/jniLibs/$abi/libzeron.so"
done
