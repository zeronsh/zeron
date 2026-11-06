# Provider update rig

Proves that updating an agent CLI from Zeron updates it through whatever
installed it (npm, pnpm, bun, Homebrew, WinGet, Scoop, the CLI's own
updater…), on a throwaway engine that can't touch anything real.

Each run starts the real `zeron-engine` binary with:

- its own port (`ZERON_IPC_PORT`, never 27654) and data dir (`ZERON_DATA_DIR`),
- a fake `HOME` and XDG dirs inside the run's folder,
- a Finder-like `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin` plus the sandbox's
  node), and `ZERON_NO_LOGIN_SHELL=1`, so it can't borrow the shell's PATH,
- every provider disabled except the one under test,
- that provider pointed at a sandboxed install (`PI_EXECUTABLE=…` and so on).

It then runs `CheckHarnessUpdates`, then `ApplyHarnessUpdate` while watching
`WatchHarnessUpdates`, printing each phase with a timestamp, and finally
asks the launcher itself for its version.

**Never** point `RIG_DIR` at `~/.zeron` or at a real install. Don't use
port 27654. Don't restart Zeron.app or any engine the rig didn't start.

## Build

From the repository root, on the branch to test (`after`), and at the
commit before it (`before`) in a separate worktree:

```sh
git worktree add ../zeron-before ebd227f3          # the code before the fix
ZGX_CHECK=0 cargo build -p zeron --bin zeron-engine
(cd ../zeron-before && CARGO_TARGET_DIR=target-before ZGX_CHECK=0 cargo build -p zeron --bin zeron-engine)
cargo build --manifest-path scripts/provider-update-rig/rigclient/Cargo.toml
```

## macOS and Linux

Needs `node` and `bun` on PATH (they're copied, never modified) and network
access to registry.npmjs.org and cursor.com.

```sh
export RIG_DIR=$(mktemp -d)/rig
export RIGCLIENT=$PWD/scripts/provider-update-rig/rigclient/target/debug/rigclient
python3 scripts/provider-update-rig/rig.py setup
for label in before after; do
  engine=target/debug/zeron-engine
  [ $label = before ] && engine=../zeron-before/target-before/debug/zeron-engine
  python3 scripts/provider-update-rig/rig.py run $engine $label \
    npm_nvm pnpm_global bun_global hermes_channel grok_fails grok_prompt cursor_native
done 2>&1 | tee rig-results.txt
```

Each real-install scenario watches for 240 s, so a full pass takes about 15
minutes per engine. Results on macOS (Apple silicon, 2026-11-06):

| Scenario | What it is | before | after |
|---|---|---|---|
| `npm_nvm` | Pi installed by an nvm Node's npm; no nvm on PATH | updated 1.4.0→1.5.0 in 4.2 s | updated in 5.1 s |
| `pnpm_global` | Pi from `pnpm add -g`; no `PNPM_HOME` in the engine's env | **failed in 0.5 s**: "Run pnpm setup…" | updated in 3.1 s |
| `bun_global` | Pi from `bun add -g` | **failed**: bun wrote a second copy to `~/.cache/.bun` (it follows `XDG_CACHE_HOME`), the launcher stayed on 1.4.0 | updated in 1.5 s |
| `hermes_channel` | Hermes on a release channel (fake, real `--check` wording) | **check failed**: "no recognizable verdict" | updated in 0.06 s |
| `grok_fails` | updater touches its install, then fails (fake) | failed, then a second view opened and **the error was gone in 0.12 s** (back to "available") | failed; still "failed" with its reason after a second view opened |
| `grok_prompt` | updater asks `[y/N]` and waits (fake) | **stuck "installing"** (it would have waited 30 minutes) | failed in 10.0 s: grok stopped to ask "…Replace it? [y/N]" |
| `cursor_native` | cursor-agent two releases behind (fake updater, real release feed) | "manual-action-required", no update offered | offered 2026.11.05, updated in 0.25 s |

### Linux only (personal-metal)

1. Unit tests, including the Linux-only one:
   ```sh
   ZGX_CHECK=0 cargo test -p zeron-engine --lib harness_updates
   ZGX_CHECK=0 cargo test -p zeron-harness --lib cmd_shim
   ```
   Expect every test to pass, including
   `a_system_package_is_left_to_the_system_package_manager`.
2. The rig as above, both engines. Expect the same columns as the macOS
   table. `bun_global` is the most likely to differ from macOS. Note where
   the `before` engine's bun wrote its copy:
   `find $RIG_DIR/runs/before-bun_global/home -path '*node_modules/@earendil-works/pi-coding-agent' -maxdepth 8`.
3. A CLI the system's package manager owns: `git` stands in for it, because
   its `version` command answers the way Grok's does. Only a check runs;
   nothing gets applied.
   ```sh
   python3 scripts/provider-update-rig/rig.py run target/debug/zeron-engine after system_git
   ```
   Expect `canApply=False` and `manual=sudo apt install --only-upgrade git`
   (or `sudo dnf upgrade git` / `sudo pacman -S git`). The `before` engine
   says `canApply=True` (it would have run the CLI's own updater).
4. If Linuxbrew is installed and has any formula with a `bin/` (for
   example `jq`), check it read-only: start the `after` engine as in the
   rig, with `GROK_EXECUTABLE=/home/linuxbrew/.linuxbrew/bin/jq`, and run
   `CheckHarnessUpdates`. Expect no "manages …, not …" error (the brew
   owns its prefix). Don't apply.

## Windows (WING-PC)

The Python rig is POSIX-only; on Windows run these by hand in PowerShell.
Everything lives under `$rig`. Only W4 touches a real per-user install,
and only if you choose to run it.

### W0. Unit tests (they include Windows-only ones)

```powershell
$env:ZGX_CHECK = "0"
cargo test -p zeron-engine --lib harness_updates
cargo test -p zeron-harness --lib cmd_shim
cargo clippy -p zeron-engine -p zeron-harness --all-targets
```

Expect every test to pass, among them
`a_windows_npm_shim_belongs_to_its_prefixs_npm`,
`a_winget_package_is_known_by_its_folder` and
`a_winget_package_id_is_read_off_its_folder`. Expect no clippy warnings in
`harness_updates*`, `process.rs` or `cmd_shim.rs`.

### Starting a throwaway engine

Build as in **Build** above, then run this in a new PowerShell window for
each row. Set `$exe` to the `before` or `after` engine:

```powershell
$rig = "$env:TEMP\zeron-rig"; New-Item -ItemType Directory -Force $rig | Out-Null
$label = "after"; $row = "w1"
$data = "$rig\$label-$row\data"; New-Item -ItemType Directory -Force $data | Out-Null
$port = 27911
$env:ZERON_DATA_DIR = $data; $env:ZERON_IPC_PORT = "$port"
$env:ZERON_EDGE_URL = "http://127.0.0.1:1"; $env:ZERON_DISABLE_NOTIFICATIONS = "1"
$env:ZERON_AUTO_UPDATE = "0"
# + the row's *_EXECUTABLE variable (below), then:
$engine = Start-Process -PassThru -NoNewWindow $exe
$c = "scripts\provider-update-rig\rigclient\target\debug\rigclient.exe"
foreach ($h in "claude-code","codex","cursor","devin","grok","hermes","pi","opencode","antigravity","muse","fx") {
  & $c call $port $data SetHarnessEnabled "{`"harness`":`"$h`",`"enabled`":$(if ($h -eq $target) {'true'} else {'false'})}"
}
& $c call $port $data CheckHarnessUpdates "{`"harness`":`"$target`"}"
& $c applywatch $port $data $target 240
& $c call $port $data StopEngine "{}"
```

Record the check line, every phase line with its time, the APPLY RESULT,
and what the launcher reports afterwards (`& <launcher> --version`).

### W1. Codex from npm (it was manual)

```powershell
$target = "codex"
& npm install -g "@openai/codex@0.165.0" --prefix "$rig\npm-codex" --no-audit --no-fund
$env:CODEX_EXECUTABLE = "$rig\npm-codex\codex.cmd"
```

Expect `before`: `canApply=false`, manual "Update Codex with its original
installer…". Expect `after`: `available`, `canApply=true`, then updated to
the newest release. Afterwards `"$rig\npm-codex\codex.cmd" --version`
should show it, and the engine log should show `npm install -g --prefix
<that dir>`.

### W2. Pi from npm (it ran its own updater)

```powershell
$target = "pi"
& npm install -g "@earendil-works/pi-coding-agent@1.4.0" --prefix "$rig\npm-pi" --no-audit --no-fund
$env:PI_EXECUTABLE = "$rig\npm-pi\pi.cmd"
```

Expect `after`: updated to 1.5.0 at that path. For `before`, record what
`pi update --self` did: which copy moved, if any.

### W3. A prompting updater, and the process tree on a stop

Create `$rig\grok\grok.cmd`:

```bat
@echo off
if "%1"=="version" (echo grok 1.0.0& exit /b 0)
if "%1 %2"=="update --check" (echo update available: 1.0.0 -^> 1.1.0& exit /b 0)
if "%1"=="update" (echo Downloading grok 1.1.0...& <nul set /p "=An existing install was found. Replace it? [y/N] "& ping -n 3600 127.0.0.1 >nul)
```

```powershell
$target = "grok"; $env:GROK_EXECUTABLE = "$rig\grok\grok.cmd"
```

Expect `after`: Failed at about 10 s with "grok stopped to ask “An existing
install was found. Replace it? [y/N]”, and nobody can answer it here".
Then `Get-Process ping -ErrorAction SilentlyContinue` lists nothing: the
whole tree was killed. Expect `before`: Installing until you stop it. Then
kill any leftover `ping.exe` by hand.

### W4. WinGet (real per-user install; only if that's OK on WING-PC)

Check only:

```powershell
winget show --id SST.opencode --exact --accept-source-agreements --disable-interactivity
```

Expect a `Version:` line, which is what the `after` engine offers. If
OpenCode (or Claude Code, `Anthropic.ClaudeCode`) is already installed
through WinGet, point the engine at
`$env:LOCALAPPDATA\Microsoft\WinGet\Links\opencode.exe` with
`OPENCODE_EXECUTABLE` and run only the check. Expect `canApply=true` and
`latest` = WinGet's version, not npm's. Claude through WinGet: `before` is
manual, `after` is `canApply=true`. Apply only if you're happy for WinGet
to upgrade that real install.

If Windows isn't in English, the `Version:` label is localized. Expect the
`after` engine to say "WinGet's description of SST.opencode names no
version it can install" as a check error. Note it.

### W5. Scoop, in a sandbox root

```powershell
$env:SCOOP = "$rig\scoop"
irm get.scoop.sh -OutFile "$rig\install-scoop.ps1"
& "$rig\install-scoop.ps1" -ScoopDir "$rig\scoop"
& "$rig\scoop\shims\scoop.cmd" install opencode@1.19.0     # an older release (current is newer)
Remove-Item Env:SCOOP                                       # the engine must find the root itself
$target = "opencode"; $env:OPENCODE_EXECUTABLE = "$rig\scoop\shims\opencode.exe"
```

Expect `before`: `opencode upgrade` fails at once ("may be managed by a
package manager"). Expect `after`: `latest` = the version in
`$rig\scoop\buckets\main\bucket\opencode.json`, then updated by `scoop
update opencode`, with `$rig\scoop\apps\opencode\current` on the new
version.

### W6. A failure stays visible

Use W3's `grok.cmd`, with `update` changed to:

```bat
if "%1"=="update" (echo.>>"%~f0"& echo npm error syscall rename 1>&2& exit /b 1)
```

Apply, then within a second run `& $c watch $port $data WatchHarnessUpdates "{}" 6 grok`.
Expect `after`: still `failed` with "grok failed (exit code 1): npm
error syscall rename". Expect `before`: "checking", then "available", and
the error gone.
