#!/bin/bash
# Build the Rust mobile core (crates/mobile) for Android and generate its
# Kotlin bindings — the same library the iOS app links.
#
#   scripts/android/build-core.sh [out_dir]
#
# Kotlin generation needs only the host toolchain. The .so build needs the
# Android NDK + cargo-ndk (`cargo install cargo-ndk`,
# `rustup target add aarch64-linux-android x86_64-linux-android`); it is
# skipped with a note when they are missing. Run by the app's `buildCore`
# Gradle task (apps/android). `ZERON_ANDROID_ABIS` picks the ABIs (default
# arm64-v8a x86_64).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/android-core}"
mkdir -p "$OUT/kotlin" "$OUT/jniLibs"
cd "$ROOT"

cargo build --locked -p zeron-mobile --lib --profile mobile
cargo build --locked -p zeron-mobile --bin uniffi-bindgen --features bindgen --profile mobile
HOST_LIB="$ROOT/target/mobile/libzeron_mobile.$([[ "$(uname)" == Darwin ]] && echo dylib || echo so)"
"$ROOT/target/mobile/uniffi-bindgen" generate --library "$HOST_LIB" --language kotlin \
  --no-format --out-dir "$OUT/kotlin"
echo "kotlin bindings: $OUT/kotlin"

if command -v cargo-ndk >/dev/null && [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
  ABIS=()
  for abi in ${ZERON_ANDROID_ABIS:-arm64-v8a x86_64}; do ABIS+=(-t "$abi"); done
  cargo ndk "${ABIS[@]}" -o "$OUT/jniLibs" \
    build --locked -p zeron-mobile --lib --profile mobile
  echo "jniLibs: $OUT/jniLibs"
else
  echo "note: cargo-ndk / ANDROID_NDK_HOME not found — skipped the .so build"
fi
