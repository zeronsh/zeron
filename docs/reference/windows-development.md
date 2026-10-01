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

`scripts/package-windows.ps1` builds the portable ZIP, the updater payload
`.exe`, and `zeron-<version>-windows-<arch>-setup.exe` (Inno Setup 6 required:
`winget install JRSoftware.InnoSetup`). `scripts/test-windows-installer.ps1`
installs, inspects, and uninstalls the setup silently; it touches the current
user's registration, so it only runs in CI or with `-Force`.

## Build and run

Install stable MSVC Rust, Visual Studio C++ build tools, Windows SDK, CMake,
and Git for Windows. On ARM64 also install LLVM so `clang` is on PATH —
`ring` compiles its crypto with Clang on `aarch64-pc-windows-msvc`, and MSVC
alone is not enough. Then run:

```powershell
cargo run --locked -p zeron
```

Close the app before rebuilding. For release builds, use
`cargo build --release --locked -p zeron`. If shader compiler discovery fails,
set `GPUI_FXC_PATH` to the Windows SDK's `fxc.exe`.

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

Frosted window chrome uses native Acrylic and requires Windows **Settings >
Personalization > Colors > Transparency effects**. Content cards and popovers
remain opaque because in-app backdrop blur is not supported. The pinned
[Zui DirectX fix](https://github.com/zeronsh/zui/pull/7) supplies the renderer
layout and edge-fade corrections.

## Verification

[Windows CI](../../.github/workflows/windows.yml) builds the release application
and tests application startup, agent discovery/protocols, process cleanup,
ConPTY, locking, UI behavior, and shader layouts. Shared Rust regressions run
on Linux and macOS. To run the engine and harness checks locally:

```powershell
cargo test --locked -p zeron-engine -p zeron-harness --lib
cargo test --locked -p zeron-harness --features native-fixture --test codex_availability --test windows_native
cargo test --locked -p zeron-engine --test codex_catalog --test codex_login_resolution --test auth
```

Fixtures use synthetic agents, so these tests do not establish authenticated
provider compatibility. GUI probes are optional CI dispatch checks and can
also run on an interactive Windows desktop after building the release app:

```powershell
cargo build --release --locked -p zeron-ui --example windows-render-fixture --features windows-render-fixture
./scripts/test-windows-lifecycle.ps1 -Runs 5
./scripts/test-windows-rendering.ps1
```

The lifecycle probe uses isolated data and provider homes. The renderer probe
captures only its synthetic window. Results go to ignored
`target/windows-lifecycle-*` and `target/windows-render-*` directories.

Remaining acceptance work includes authenticated provider runs and shutdown,
broader GPU/DPI coverage, accessibility, native browser and cross-device parity,
concurrent-launch log rotation, and invalid-window teardown diagnostics.
