# Packaging

Releases ship the **zgx** desktop app (`apps/zgx`, built with `--features beta`)
alongside `zeron-engine` built from the same commit (`.github/workflows/release.yml`
→ `.github/workflows/zgx-build.yml`). Each platform packages `ui/` and
`plugins-builtin/` beside the binary so templates inherit from Zeron's scripts.
The engine must be built with `LUAU_CXXFLAGS=-DNDEBUG` (release scripts refuse
an engine compiled with Luau asserts).

## Linux

```sh
LUAU_CXXFLAGS=-DNDEBUG cargo build --release --locked -p zeron --bin zeron-engine
ZGX_VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')" \
ZGX_ENGINE="$PWD/target/release/zeron-engine" \
  scripts/package-linux.sh
```

Produces `target/package/zeron-<version>-linux-<arch>.tar.gz` containing:

- `zeron` — the ZGX desktop app and CLI dispatcher (`zeron headless` and other
  engine subcommands hand off to `zeron-engine` beside it)
- `zeron-engine` — the background engine binary
- `ui/` and `plugins-builtin/` — Zeron's Luau interface and built-in plugins
- `zeron.desktop` — XDG desktop entry template (`Exec=zeron` for packagers;
  the installers rewrite `Exec`, `TryExec`, and `Icon` to absolute paths under
  `~/.zeron/app/current`, since `~/.local/bin` is often not on a desktop
  session's `PATH`)
- `zeron.png` — 1024×1024 Zeron app icon (`scripts/make-app-icons.py`)
- `install.sh` — installs into `~/.zeron/app/<version>` behind a `current`
  symlink (the curl installer's layout, which the in-app updater manages),
  links `~/.local/bin/zeron` to it, and writes the desktop entry and icon under
  `$XDG_DATA_HOME` (default `~/.local/share`). The curl installer does the same
  from the extracted tarball; `scripts/test-linux-desktop-entry.sh` checks both

Without `ZGX_VERSION` / `ZGX_ENGINE`, `scripts/package-linux.sh` packages the
standalone `apps/zeron` CLI/engine binaries (`PROFILE=debug` for a fast CI
smoke package).

## macOS

Requires a macOS host (Apple silicon, macOS 13+ minimum) and **Zig 0.16**
(`brew install zig@0.16`, for Ghostty terminals in `apps/zgx`):

```sh
LUAU_CXXFLAGS=-DNDEBUG cargo build --release --locked -p zeron --bin zeron-engine
ZGX_VERSION="$(grep -m1 '^version' Cargo.toml | sed 's/.*"\(.*\)".*/\1/')" \
ZGX_ENGINE="$PWD/target/release/zeron-engine" \
  scripts/package-zgx-macos.sh
```

Produces, under `target/package-zgx/`:

- `Zeron.app` — with `Contents/MacOS/zeron` (the ZGX app),
  `Contents/MacOS/Zeron Engine` (the engine helper), and
  `Contents/Resources/{ui,plugins-builtin,zeron.icns,Assets.car,licenses}`
- `zeron-<version>-macos-<arch>.dmg` — distributable disk image
- `zeron-<version>-macos-<arch>-app.tar.gz` — auto-updater archive

The script runs `scripts/macos-app-icon.sh` (`Assets.car` from
`dist/macos/Zeron.icon` on Xcode 26+, plus `zeron.icns` from
`dist/macos/icon-1024.png`), ad-hoc signs by default, and signs/notarizes when
`CODESIGN_IDENTITY` and `NOTARY_KEY_PATH` / `NOTARY_KEY_ID` / `NOTARY_ISSUER_ID`
are set.

## Windows

Requires Inno Setup 6 (`winget install JRSoftware.InnoSetup`):

```powershell
$env:LUAU_CXXFLAGS = '-DNDEBUG'
cargo build --release --locked -p zeron --bin zeron
$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
./scripts/package-windows.ps1 `
  -ReleasesUrl https://zeron.sh/releases `
  -ZgxVersion $version `
  -ZgxEngine "$PWD\target\release\zeron.exe"
```

Produces, under `target/package/`:

- `zeron-<version>-windows-<arch>-setup.exe` — the per-user installer built
  from `dist/windows/zeron.iss` with Inno Setup 6
- `zeron-<version>-windows-<arch>.zip` — the portable package containing
  `zeron.exe`, `zeron-update.json`, and `beta\<version>\{ui,plugins-builtin,engine\zeron.exe}`
- `zeron-<version>-windows-<arch>.exe` — the standalone executable for legacy
  in-place updaters (on first run it fetches the matching zip to populate its
  version folder; omitted on beta builds)

Omitting `-ZgxVersion` / `-ZgxEngine` packages the standalone `apps/zeron`
CLI/engine binary (exercised by Windows CI installer/CLI smoke tests via
`scripts/test-windows-installer.ps1`).
