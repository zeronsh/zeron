# Windows development

Windows supports native x64 and ARM64 source builds, a per-user installer,
and portable release ZIPs. The installer (`dist/windows/zeron.iss`, Inno
Setup 6) installs into `%LOCALAPPDATA%\Programs\Zeron` without elevation,
registers the Start menu entry, the `zeron://` link handler, and the Settings →
Apps uninstall entry. Both release packages carry `zeron-update.json` beside
`zeron.exe`, which lets the app replace its executable in place from GitHub
releases; keep it there for portable copies. Artifact names use Rust's
architecture token (`x86_64` or `aarch64`) so the updater can find the matching
executable; published releases currently include x64 only. Background services
are not supported yet.

`scripts/package-windows.ps1` builds the portable ZIP, the standalone updater
payload `.exe`, and `zeron-<version>-windows-<arch>-setup.exe` (Inno Setup 6
required: `winget install JRSoftware.InnoSetup`). Pass `-ZgxVersion <version>`
and `-ZgxEngine <path-to-engine-zeron.exe>` (as `.github/workflows/zgx-build.yml`
does) to package the shipping **zgx** app (`apps/zgx` with `--features beta`,
plus `beta\<version>\{ui,plugins-builtin,engine\zeron.exe}`); omitting them
packages the standalone `apps/zeron` CLI/engine binary (exercised by Windows
CI). `scripts/test-windows-installer.ps1` installs, inspects, and uninstalls
the setup silently; it touches the current user's registration, so it only runs
in CI or with `-Force`.

## Build and run

Install stable MSVC Rust, Visual Studio C++ build tools, Windows SDK, CMake,
and Git for Windows. On ARM64 also install LLVM so `clang` is on PATH —
`ring` compiles its crypto with Clang on `aarch64-pc-windows-msvc`, and MSVC
alone is not enough.

### Desktop app (`apps/zgx`)

Build the engine from the repository root, then run `apps/zgx`:

```powershell
cargo build -p zeron --bin zeron-engine
cd apps/zgx
cargo run --release              # intro / onboarding / main window (staging by default)
cargo run --release -- gallery   # design-system workbench
```

For a release-style build of the ZGX binary (production identity, no staging
endpoints): `cargo build --release --locked --features beta` inside `apps/zgx`.
Close the app before rebuilding.

### Engine / headless CLI (`apps/zeron`)

```powershell
cargo run --locked -p zeron -- headless
```

For release engine builds (required by `scripts/package-windows.ps1`), set
`$env:LUAU_CXXFLAGS = '-DNDEBUG'` and run `cargo build --release --locked -p zeron`.

## Configuration and agent support

| Setting | Behavior |
| --- | --- |
| Application data | `%LOCALAPPDATA%\Zeron`, falling back to `%USERPROFILE%\AppData\Local\Zeron`. Override with `ZERON_DATA_DIR`. |
| Managed adapters | `ZERON_ADAPTERS_DIR`, then `ZERON_DATA_DIR/adapters`, then the default application's `adapters` directory. |
| Provider credentials | Keep their provider-owned locations; changing Zeron's data root does not migrate them. |
| `CODEX_EXECUTABLE` | Executable override. `.exe` (and `.com`) launch directly; `.cmd`/`.bat` shims launch through a wrapped `cmd.exe` with literal, individually escaped arguments. The override must exist on disk. |

ACP, Claude, Codex, and opencode search PATH and known native installation
directories. Discovery is PATHEXT-aware: npm's `.cmd` shims (and any `.bat`)
resolve like `cmd.exe` would — per directory, extensions in PATHEXT order —
and spawn through `cmd.exe /e:ON /v:OFF /d /c` inside the same Job Object, so npm-
installed agents (`codex`, `opencode`, `pi`, a bare `npm i -g grok`)
work without following `node_modules` payloads. Batch arguments containing
CR/LF and batch executable paths containing percent expansion syntax are rejected.
GUI launches additionally
backfill `%APPDATA%\npm`, `%LOCALAPPDATA%\{pnpm,Programs\nodejs,Volta\bin}`,
scoop shims, and `%USERPROFILE%\{.local\bin,.bun\bin}` from PATH. Managed
JavaScript adapters run through Node; installation requires `node.exe` beside
`node_modules/npm/bin/npm-cli.js`. Volta's selected tool-image npm location is
unsupported.

An OS file lock prevents engines from sharing a profile; `engine.lock.pid` is
only diagnostic. Terminals use ConPTY. Windows agents, login commands, adapter
installs, and terminals own their child process trees through Job Objects.
Terminal close waits up to five seconds for cleanup and reports failure;
shutdown/drop log failures. Processes started through external services or
brokers are outside this ownership. Closing a terminal or exiting its shell also
terminates processes detached inside that job (including `start` children).

## Embedded browser & glass

Browser tabs in `apps/zgx` use `zgx-desktop`'s `webview2` feature (Microsoft
Edge WebView2 in a DirectComposition tree owned by the zgx window; ships with
Windows 11 and Edge on Windows 10, falling back to a placeholder if absent).
Window translucency respects Windows **Settings > Personalization > Colors >
Transparency effects**, watched from the registry by
`apps/zgx/src/transparency.rs`.

When Windows disables **Animation effects**, Zeron's **Reduce motion: System**
setting skips transitions and softens activity indicators.

## Verification

[Windows CI](../../.github/workflows/windows.yml) runs the Windows harness,
CLI, updater, engine, and voice-media suites, builds the release `zeron`
binary, verifies portable ZIP and Inno Setup installer packaging
(`scripts/package-windows.ps1` and `scripts/test-windows-installer.ps1`), and
checks isolated CLI startup without `HOME`. Shared Rust regressions run on
Linux and macOS, and Windows ZGX desktop builds run in
[`.github/workflows/zgx-build.yml`](../../.github/workflows/zgx-build.yml).

To run the engine and harness checks locally:

```powershell
cargo test --locked -p zeron-engine -p zeron-harness --lib
cargo test --locked -p zeron-harness --features native-fixture --test codex_availability --test windows_native
cargo test --locked -p zeron-engine --test codex_catalog --test codex_login_resolution --test auth
```

Fixtures use synthetic agents, so these tests do not establish authenticated
provider compatibility. Remaining acceptance work includes authenticated
provider runs and shutdown, broader GPU/DPI coverage, accessibility,
cross-device parity, concurrent-launch log rotation, and invalid-window
teardown diagnostics.
