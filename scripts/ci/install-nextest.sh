#!/usr/bin/env bash
# Install a pinned cargo-nextest release (x86_64 Linux) after verifying its
# SHA-256, instead of trusting a third-party install action. To bump: change
# VERSION and SHA256 (from the release's .tar.gz asset).
set -euo pipefail
VERSION=0.9.146
SHA256=682c21b777c333e96fd532e114d3a5a894e0729ab88d94c0a9f20f8419695428

[ "$(uname -m)" = x86_64 ] || { echo "install-nextest.sh: x86_64 only" >&2; exit 1; }
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
curl --proto '=https' --tlsv1.2 -fsSL --retry 5 -o "$tmp/nextest.tar.gz" \
  "https://github.com/nextest-rs/nextest/releases/download/cargo-nextest-$VERSION/cargo-nextest-$VERSION-x86_64-unknown-linux-gnu.tar.gz"
echo "$SHA256  $tmp/nextest.tar.gz" | sha256sum -c -
mkdir -p "$HOME/.cargo/bin"
tar -xzf "$tmp/nextest.tar.gz" -C "$HOME/.cargo/bin" cargo-nextest
cargo nextest --version
