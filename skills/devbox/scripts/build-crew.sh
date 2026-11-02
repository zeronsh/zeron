#!/usr/bin/env bash
# Build Crew's Linux executable on a devbox from a pinned Ashler-AI/comet source archive.
# Source, Cargo downloads and build output stay under ~/.cache, which Boat snapshots skip,
# so a build never grows what every pause saves and every resume restores. Only the
# finished executables are installed, into a directory named for the commit and digest.
# Usage: build-crew.sh ARCHIVE ARCHIVE_SHA256 COMMIT
set -euo pipefail
die() { echo "build-crew: $*" >&2; exit 1; }
[[ $# == 3 ]] || die "usage: build-crew.sh ARCHIVE ARCHIVE_SHA256 COMMIT"
archive="$1"; expected="$2"; commit="$3"
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || die "COMMIT must be a full 40-hex commit"
[[ "$expected" =~ ^[0-9a-f]{64}$ ]] || die "ARCHIVE_SHA256 must be 64 lowercase hex digits"
cache="${XDG_CACHE_HOME:-$HOME/.cache}/crew-build"
case "$(realpath -m "$archive")" in
  "$cache"/*) ;;
  *) die "keep the source archive under $cache, outside the snapshot" ;;
esac
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ "${CREW_DEVBOX_SETUP_PROTECTED:-}" != 1 ]]; then
  exec python3 "$here/keepalive.py" env CREW_DEVBOX_SETUP_PROTECTED=1 bash "$0" "$@"
fi
[[ "$(sha256sum "$archive" | cut -d' ' -f1)" == "$expected" ]] || die "archive digest does not match"

# The release workflow's Linux build dependencies.
packages=(libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-dev libxcb1-dev libx11-xcb-dev
  libfontconfig1-dev libfreetype-dev libasound2-dev libvulkan-dev pkg-config cmake
  libwebkit2gtk-4.1-dev libjson-glib-dev)
missing=()
for package in "${packages[@]}"; do
  dpkg-query -W -f='${Status}' "$package" 2>/dev/null | grep -q 'install ok installed' || missing+=("$package")
done
if (( ${#missing[@]} )); then
  sudo apt-get update -qq
  sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "${missing[@]}"
fi
command -v cargo >/dev/null || die "cargo is required; install the stable Rust toolchain"

src="$cache/src-$commit"
unpack="$cache/.unpack-$commit"
rm -rf "$src" "$unpack"
mkdir -p "$unpack"
tar -xzf "$archive" -C "$unpack" --no-same-owner
root="$unpack"
if [[ ! -f "$root/Cargo.toml" ]]; then
  # GitHub tarballs wrap the tree in one top-level directory; `git archive` does not.
  entries=("$unpack"/*)
  [[ ${#entries[@]} == 1 && -f "${entries[0]}/Cargo.toml" ]] || die "archive does not contain the Crew workspace"
  root="${entries[0]}"
fi
mv -T "$root" "$src"
rm -rf "$unpack"
export CARGO_HOME="$cache/cargo" CARGO_TARGET_DIR="$cache/target"
# Linux executables carry the production identity; devboxes pass staging coordinates at launch.
export COMET_DEFAULT_ENVIRONMENT=production COMET_PACKAGE_ENVIRONMENT=production ASHLER_INCREMENTAL_TSC_CHECKS=false
(cd "$src" && nice -n 10 cargo build --release --locked -p zeron --bin zeron --bin zeron-engine)

version="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\([^"]*\)"/\1/p' "$src/Cargo.toml")"
[[ -n "$version" ]] || die "workspace version not found"
digest="$(sha256sum "$CARGO_TARGET_DIR/release/zeron" | cut -d' ' -f1)"
root="$HOME/.local/lib/crew"
dest="$root/$version-src-${commit:0:12}-$digest"
mkdir -p "$root"
if [[ ! -x "$dest/comet" ]]; then
  # Stage beside the final name, then rename: an interrupted install leaves nothing trusted.
  stage="$(mktemp -d "$root/.build-$version-XXXXXX")"
  trap 'rm -rf "$stage"' EXIT
  install -m 755 "$CARGO_TARGET_DIR/release/zeron" "$stage/comet"
  install -m 755 "$CARGO_TARGET_DIR/release/zeron-engine" "$stage/zeron-engine"
  chmod 755 "$stage"
  mv -T "$stage" "$dest"
fi
[[ "$(sha256sum "$dest/comet" | cut -d' ' -f1)" == "$digest" ]] || die "installed executable digest does not match the build"
"$dest/comet" --version >/dev/null || die "the built executable does not start"
echo "CREW_BINARY=$dest/comet"
echo "CREW_BINARY_SHA256=$digest"
echo "CREW_SOURCE_COMMIT=$commit"
