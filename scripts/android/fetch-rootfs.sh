#!/bin/bash
# Download the pinned Alpine minirootfs for each Android ABI and stage it as
# the :runtime module's asset (assets/rootfs-<abi>.tar.gz), which the app
# extracts into filesDir/runtime/rootfs on first start.
#
#   scripts/android/fetch-rootfs.sh [out_dir]     # default target/android-runtime
#
# Bumping: change ALPINE_VERSION and both sha256s (from the .sha256 next to
# each tarball), and bump ROOTFS_RELEASE in apps/android/runtime Bootstrap.kt.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/target/android-runtime}"
ALPINE_VERSION=3.24.2
ALPINE_BRANCH="v${ALPINE_VERSION%.*}"
MIRROR="${ALPINE_MIRROR:-https://dl-cdn.alpinelinux.org/alpine}"

# arch abi sha256
PINS=(
  "aarch64 arm64-v8a 9bf70a7f18ea44094cbb5f70c58f9af129c8214745743db0e68e5502cc2ce773"
  "x86_64 x86_64 c5ca053cfe1d85c5b96dff8b9bc57045f7f184a30ffb6b65776409ca90388677"
)

die() { echo "error: $*" >&2; exit 1; }
for tool in curl sha256sum; do
  command -v "$tool" >/dev/null || die "$tool not found"
done

mkdir -p "$OUT/assets" "$OUT/cache/alpine"
for pin in "${PINS[@]}"; do
  read -r arch abi sha <<<"$pin"
  file="alpine-minirootfs-$ALPINE_VERSION-$arch.tar.gz"
  cached="$OUT/cache/alpine/$file"
  if ! { [[ -f "$cached" ]] && echo "$sha  $cached" | sha256sum -c --status; }; then
    echo "fetch $file"
    curl -fsSL --retry 3 -o "$cached.part" "$MIRROR/$ALPINE_BRANCH/releases/$arch/$file" \
      || { rm -f "$cached.part"; die "download of $file failed"; }
    echo "$sha  $cached.part" | sha256sum -c --status \
      || { rm -f "$cached.part"; die "sha256 mismatch for $file (expected $sha)"; }
    mv "$cached.part" "$cached"
  fi
  cp "$cached" "$OUT/assets/rootfs-$abi.tar.gz"
  echo "rootfs ($abi): $OUT/assets/rootfs-$abi.tar.gz"
done
