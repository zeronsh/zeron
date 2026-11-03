#!/bin/bash
# Build the Rust mobile core (crates/mobile) for Android and generate its
# Kotlin bindings — the same library the iOS app links.
#
#   scripts/android/build-core.sh [out_dir]
#
# The .so build needs the Android NDK + cargo-ndk (`cargo install cargo-ndk`,
# `rustup target add aarch64-linux-android x86_64-linux-android`).
# `ZERON_ANDROID_ABIS` picks the ABIs (default arm64-v8a x86_64);
# `ZERON_ANDROID_PROFILE` the core's Cargo profile (default `mobile`;
# `mobile-dist` for published builds: thin LTO, stripped, as iOS archives).
# Run by the app's `buildCore` Gradle task (apps/android).
#
# The Kotlin bindings are read out of the Android library just built, by
# UniFFI's generator on its own (crates/mobile-bindgen): the core isn't
# compiled for the host at all. Without the NDK only the bindings are made,
# from a host build of the core.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/android-core}"
mkdir -p "$OUT/kotlin" "$OUT/jniLibs"
cd "$ROOT"

LIB=""
if command -v cargo-ndk >/dev/null && [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
  ABIS=()
  for abi in ${ZERON_ANDROID_ABIS:-arm64-v8a x86_64}; do ABIS+=(-t "$abi"); done
  # The core links C++ (the terminal emulator and GPU paths): ship the NDK's
  # libc++_shared.so beside it.
  cargo ndk "${ABIS[@]}" --link-libcxx-shared -o "$OUT/jniLibs" \
    build --locked -p zeron-mobile --lib --profile "${ZERON_ANDROID_PROFILE:-mobile}"
  echo "jniLibs: $OUT/jniLibs"
  first="$(set -- ${ZERON_ANDROID_ABIS:-arm64-v8a x86_64}; echo "$1")"
  LIB="$OUT/jniLibs/$first/libzeron_mobile.so"
else
  echo "note: cargo-ndk / ANDROID_NDK_HOME not found — skipped the .so build"
  cargo build --locked -p zeron-mobile --lib --profile mobile
  LIB="$ROOT/target/mobile/libzeron_mobile.$([[ "$(uname)" == Darwin ]] && echo dylib || echo so)"
fi

cargo build --locked -p zeron-mobile-bindgen --profile mobile
"$ROOT/target/mobile/uniffi-bindgen" generate --library "$LIB" --language kotlin \
  --no-format --out-dir "$OUT/kotlin"
echo "kotlin bindings: $OUT/kotlin"
