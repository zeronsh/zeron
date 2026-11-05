# Packaging

## Linux (implemented)

```sh
scripts/package-linux.sh            # release build (thin LTO, stripped)
PROFILE=debug scripts/package-linux.sh   # fast smoke package
```

Produces `target/package/zeron-<version>-linux-<arch>.tar.gz` containing:

- `zeron` — the binary (headed by default; `zeron headless` runs the engine alone)
- `zeron.desktop` — XDG desktop entry template (`Exec=zeron` for packagers;
  the installers rewrite `Exec`, `TryExec`, and `Icon` to absolute paths under
  `~/.zeron/app/current`, since `~/.local/bin` is often not on a desktop
  session's `PATH`)
- `zeron.png` — 1024×1024 Zeron app icon
- `install.sh` — installs into `~/.zeron/app/<version>` behind a `current`
  symlink (the curl installer's layout, which the in-app updater manages),
  links `~/.local/bin/zeron` to it, and writes the desktop entry and icon under
  `$XDG_DATA_HOME` (default `~/.local/share`). The curl installer does the same
  from the extracted tarball; `scripts/test-linux-desktop-entry.sh` checks both

The release profile in the root `Cargo.toml` sets `lto = "thin"` and
`strip = "symbols"` for distribution builds.

## macOS

```sh
scripts/package-macos.sh    # → target/package/zeron-<version>-macos-<arch>.dmg
```

Builds the release binary, assembles `Zeron.app` (Info.plist + icns), ad-hoc
signs it (set `CODESIGN_IDENTITY` for a real Developer ID), and wraps it in a
dmg. The auto-update tarball retains an internal `Zeron.app` path so older
installed builds can update into Zeron. CI runs this on tags
(`.github/workflows/release.yml`). The manual steps it automates, for reference
(run on a macOS host — gpui needs Metal; no cross-build from Linux):

1. Build the universal (or per-arch) binary:
   ```sh
   cargo build --release -p zeron --target aarch64-apple-darwin
   cargo build --release -p zeron --target x86_64-apple-darwin
   lipo -create -output zeron \
     target/aarch64-apple-darwin/release/zeron \
     target/x86_64-apple-darwin/release/zeron
   ```
2. Assemble the bundle:
   ```sh
   mkdir -p Zeron.app/Contents/{MacOS,Resources}
   cp zeron Zeron.app/Contents/MacOS/zeron
   sed "s/__VERSION__/$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')/" \
     dist/macos/Info.plist > Zeron.app/Contents/Info.plist
   ```
3. Icon: generate `zeron.icns` from `dist/macos/icon-1024.png` (the macOS-shaped
   variant of the artwork — squircle mask, margins, and shadow pre-baked, since
   `sips` can't apply an alpha mask) and place it at
   `Zeron.app/Contents/Resources/zeron.icns`:
   ```sh
   mkdir zeron.iconset && sips -z 256 256 dist/macos/icon-1024.png --out zeron.iconset/icon_256x256.png
   iconutil -c icns zeron.iconset -o Zeron.app/Contents/Resources/zeron.icns
   ```
4. Sign + notarize (required for distribution):
   ```sh
   codesign --deep --force --options runtime --sign "Developer ID Application: …" Zeron.app
   xcrun notarytool submit Zeron.zip --keychain-profile … --wait
   xcrun stapler staple Zeron.app
   ```
5. Ship as a `.dmg` (`hdiutil create -volname Zeron -srcfolder Zeron.app -ov -format UDZO Zeron.dmg`).

## Windows

```powershell
./scripts/package-windows.ps1 -ReleasesUrl https://github.com/zeronsh/zeron/releases/latest/download
```

Produces, under `target/package/`:

- `zeron-<version>-windows-<arch>-setup.exe` — the per-user installer built
  from `dist/windows/zeron.iss` with Inno Setup 6
- `zeron-<version>-windows-<arch>.zip` — the portable package
- `zeron-<version>-windows-<arch>.exe` — the bare executable the in-app
  updater downloads

The installer and the zip both carry `zeron-update.json`, the marker that lets
the app update itself in place. CI runs `scripts/test-windows-installer.ps1`
against the setup on every Windows build.
