#!/bin/bash
# One-command APK (arm64-v8a; add x86_64 for emulators), demo mode by default.
#
# With the release key present this builds the shipped `release` variant
# (not debuggable, much faster UI); without it, a debug-key debug build.
#
#   scripts/android/build-apk.sh
#   ZERON_WITH_X86_64=1 scripts/android/build-apk.sh   # also x86_64 (emulator)
#
# Version: apps/android/version.properties (ZERON_VERSION_CODE / _NAME override).
# Signing: $ZERON_KEYSTORE_PROPERTIES or ~/zeron-keys/keystore.properties if present.
#
# Requires: Rust, cargo-ndk, the Android SDK (platforms;android-35,
# build-tools;35.0.0) and NDK r27. Set ANDROID_HOME (or ANDROID_SDK_ROOT).
# On Windows, run this from Git Bash or WSL; see apps/android/README.md.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
SDK="${ANDROID_SDK_ROOT:-${ANDROID_HOME:-}}"
if [[ -z "$SDK" ]]; then
  echo "error: set ANDROID_HOME to the Android SDK" >&2
  exit 1
fi
export ANDROID_HOME="$SDK"
export ANDROID_SDK_ROOT="$SDK"

ABIS="arm64-v8a"
[[ "${ZERON_WITH_X86_64:-}" == "1" ]] && ABIS="arm64-v8a x86_64"
export ZERON_ANDROID_ABIS="$ABIS"
export ZERON_REQUIRE_NDK=1
# Drop last run's libraries so an ABI that isn't built now can't be copied.
rm -rf "$ROOT/target/android-core/jniLibs"
"$ROOT/scripts/android/build-core.sh" "$ROOT/target/android-core"

APP_JNI="$ROOT/apps/android/app/src/main/jniLibs"
rm -rf "$APP_JNI"
mkdir -p "$APP_JNI"
NDK="${ANDROID_NDK_HOME:-}"
if [[ -z "$NDK" && -d "$SDK/ndk" ]]; then
  NDK="$(find "$SDK/ndk" -mindepth 1 -maxdepth 1 -type d | sort | tail -1)"
fi
STRIP=""
if [[ -n "$NDK" ]]; then
  # Windows builds ship llvm-strip.exe; a bare -name match misses it and the
  # debug-info-heavy .so ships inside the release APK (45MB instead of 18).
  STRIP="$(find "$NDK/toolchains/llvm/prebuilt" -name 'llvm-strip*' \( -type f -o -type l \) 2>/dev/null | head -1 || true)"
fi
for abi in $ABIS; do
  src="$ROOT/target/android-core/jniLibs/$abi/libzeron_mobile.so"
  if [[ ! -f "$src" ]]; then
    echo "error: missing $src — cargo-ndk did not produce $abi" >&2
    exit 1
  fi
  mkdir -p "$APP_JNI/$abi"
  cp "$src" "$APP_JNI/$abi/libzeron_mobile.so"
  if [[ -n "$STRIP" ]]; then
    "$STRIP" --strip-all "$APP_JNI/$abi/libzeron_mobile.so" || true
  fi
done

KOTLIN_SRC="$ROOT/target/android-core/kotlin/uniffi/zeron_core/zeron_core.kt"
if [[ -f "$KOTLIN_SRC" ]]; then
  mkdir -p "$ROOT/apps/android/app/src/main/java/uniffi/zeron_core"
  cp "$KOTLIN_SRC" "$ROOT/apps/android/app/src/main/java/uniffi/zeron_core/zeron_core.kt"
  python3 "$ROOT/scripts/android/patch-uniffi-kotlin.py" "$ROOT/apps/android/app/src/main/java/uniffi/zeron_core/zeron_core.kt"
fi

# Properties treats '\' as an escape; a Windows path breaks lintVital's SDK
# lookup ("filename, directory name, or volume label syntax is incorrect").
printf 'sdk.dir=%s\n' "${SDK//\\//}" > "$ROOT/apps/android/local.properties"
cd "$ROOT/apps/android"
if [[ ! -x ./gradlew ]]; then
  if command -v gradle >/dev/null; then
    gradle wrapper --gradle-version 8.11.1
  else
    echo "error: ./gradlew is missing and gradle is not on PATH" >&2
    exit 1
  fi
fi
chmod +x ./gradlew
# Sign with the one stable release key when it's available (never in the repo).
KEYPROPS="${ZERON_KEYSTORE_PROPERTIES:-$HOME/zeron-keys/keystore.properties}"
GRADLE_ARGS=()
if [[ -f "$KEYPROPS" ]]; then
  echo "Signing with $KEYPROPS"
  GRADLE_ARGS+=("-PzeronKeystoreProperties=$KEYPROPS")
else
  echo "warning: $KEYPROPS not found; using the debug key (in-app updates won't install over releases)" >&2
fi
[[ "${ZERON_WITH_X86_64:-}" == "1" ]] && GRADLE_ARGS+=("-PzeronWithX86_64=true")
[[ -n "${ZERON_VERSION_CODE:-}" ]] && GRADLE_ARGS+=("-PzeronVersionCode=$ZERON_VERSION_CODE")
[[ -n "${ZERON_VERSION_NAME:-}" ]] && GRADLE_ARGS+=("-PzeronVersionName=$ZERON_VERSION_NAME")
if [[ -f "$KEYPROPS" ]]; then
  ./gradlew :app:assembleRelease --no-daemon "${GRADLE_ARGS[@]}"
  APK="$ROOT/apps/android/app/build/outputs/apk/release/app-release.apk"
  # R8's mapping, to read crash stack traces; kept beside the signing key,
  # never published.
  MAPPING="$ROOT/apps/android/app/build/outputs/mapping/release/mapping.txt"
  if [[ -f "$MAPPING" ]]; then
    NAME="${ZERON_VERSION_NAME:-$(sed -n 's/^versionName=//p' "$ROOT/apps/android/version.properties")}"
    MAPDIR="${ZERON_MAPPING_DIR:-$HOME/zeron-keys/mappings}"
    mkdir -p "$MAPDIR"
    cp "$MAPPING" "$MAPDIR/$NAME-mapping.txt"
    echo "R8 mapping: $MAPDIR/$NAME-mapping.txt"
  fi
else
  ./gradlew :app:assembleDebug --no-daemon "${GRADLE_ARGS[@]}"
  APK="$ROOT/apps/android/app/build/outputs/apk/debug/app-debug.apk"
fi
echo "APK: $APK"
