# Packaging

## Linux (implemented)

```sh
scripts/package-linux.sh            # release build (thin LTO, stripped)
PROFILE=debug scripts/package-linux.sh   # fast smoke package
```

Produces `target/package/glitch-flow-<version>-linux-<arch>.tar.gz` containing:

- `glitch-flow` — the binary (headed by default; `glitch-flow headless` runs the engine alone)
- `glitch-flow.desktop` — XDG desktop entry
- `glitch-flow.png` — 1024×1024 Glitch Flow app icon
- `install.sh` — installs into `~/.local/{bin,share/applications,share/icons}`

The release profile in the root `Cargo.toml` sets `lto = "thin"` and
`strip = "symbols"` for distribution builds.

## macOS

```sh
scripts/package-macos.sh    # → target/package/glitch-flow-<version>-macos-<arch>.dmg
```

Builds the release binary, assembles `Glitch Flow.app` (Info.plist + icns), ad-hoc
signs it (set `CODESIGN_IDENTITY` for a real Developer ID), and wraps it in a
dmg. The auto-update tarball contains `Glitch Flow.app` for this local client's
update flow. The manual steps it automates, for reference
(run on a macOS host — gpui needs Metal; no cross-build from Linux):

1. Build the universal (or per-arch) binary:
   ```sh
   cargo build --release -p glitch-flow --target aarch64-apple-darwin
   cargo build --release -p glitch-flow --target x86_64-apple-darwin
   lipo -create -output glitch-flow \
     target/aarch64-apple-darwin/release/glitch-flow \
     target/x86_64-apple-darwin/release/glitch-flow
   ```
2. Assemble the bundle:
   ```sh
   mkdir -p "Glitch Flow.app/Contents/MacOS" "Glitch Flow.app/Contents/Resources"
   cp glitch-flow "Glitch Flow.app/Contents/MacOS/glitch-flow"
   sed "s/__VERSION__/$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')/" \
     dist/macos/Info.plist > "Glitch Flow.app/Contents/Info.plist"
   ```
3. Icon: generate `glitch-flow.icns` from `dist/macos/icon-1024.png` (the macOS-shaped
   variant of the artwork — squircle mask, margins, and shadow pre-baked, since
   `sips` can't apply an alpha mask) and place it at
   `Glitch Flow.app/Contents/Resources/glitch-flow.icns`:
   ```sh
   mkdir glitch-flow.iconset && sips -z 256 256 dist/macos/icon-1024.png --out glitch-flow.iconset/icon_256x256.png
   iconutil -c icns glitch-flow.iconset -o "Glitch Flow.app/Contents/Resources/glitch-flow.icns"
   ```
4. Sign + notarize (required for distribution):
   ```sh
   codesign --deep --force --options runtime --sign "Developer ID Application: …" "Glitch Flow.app"
   xcrun notarytool submit "Glitch Flow.zip" --keychain-profile … --wait
   xcrun stapler staple "Glitch Flow.app"
   ```
5. Ship as a `.dmg` (`hdiutil create -volname "Glitch Flow" -srcfolder "Glitch Flow.app" -ov -format UDZO "Glitch Flow.dmg"`).
