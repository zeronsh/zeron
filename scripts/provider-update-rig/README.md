# Provider update rig

Proves that updating an agent CLI from Zeron updates it through whatever
installed it (npm, pnpm, bun, Homebrew, WinGet, Scoop, the CLI's own
updater…), on a throwaway engine that can't touch anything real.

Each run starts the real `zeron-engine` binary with:

- its own port (`ZERON_IPC_PORT`, never 27654) and data dir (`ZERON_DATA_DIR`),
- a fake `HOME` and XDG dirs inside the run's folder,
- a Finder-like `PATH` (`/usr/bin:/bin:/usr/sbin:/sbin` plus the sandbox's
  node), and `ZERON_NO_LOGIN_SHELL=1`, so it can't borrow the shell's PATH,
- every provider disabled except the one under test, and pinned to a path
  that doesn't exist (discovery also searches `/usr/local/bin`, and the engine
  checks every provider as it starts, before the rig can disable any),
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

Needs `node`, `npm` and `bun` on PATH (`node` and `bun` are copied, nothing is
modified) and network access to registry.npmjs.org and cursor.com. The rig's
own promises are tested by `python3 scripts/provider-update-rig/rig_test.py`.

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
| `npm_nvm` | Pi installed by an nvm Node's npm; no nvm on PATH | updated 1.4.0â†’1.5.0 in 4.2 s | updated in 5.1 s |
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
   says `canApply=True` and `failed`: `git` has no `update`, like a CLI whose
   distribution stripped its updater.
4. If Linuxbrew is installed and has any formula with a `bin/` (for
   example `jq`), check it read-only: start the `after` engine as in the
   rig, with `GROK_EXECUTABLE=/home/linuxbrew/.linuxbrew/bin/jq`, and run
   `CheckHarnessUpdates`. Expect no "manages …, not …" error (the brew
   owns its prefix). Don't apply.

On a shared machine, run every rig command inside a read-only root, so a
mistake can't reach anything real and `pkill` sees only the rig's processes:

```sh
bwrap --ro-bind / / --dev /dev --proc /proc --tmpfs /tmp --bind "$R" "$R" \
  --unshare-pid --die-with-parent --new-session <command>
```

### Results on Linux (personal-metal, Ubuntu, 2026-11-06)

`before` is `zeron/zgx` 48a0dcbc, `after` this branch. Both ran in that read-only
root, and `after` is the build with the system-package fix. Times are to the
APPLY RESULT.

| Scenario | before | after |
|---|---|---|
| Unit tests | n/a | `harness_updates` 83 pass, `cmd_shim` 6 pass. On the pushed branch 2 failed on Linux: one read this machine's real `/usr/local/bin/codex`, one expected `/usr/bin/pi` to update itself |
| `npm_nvm` | updated 1.4.0→1.5.0 in 4.6 s | updated in 4.8 s |
| `pnpm_global` (`~/.local/share/pnpm`) | **failed in 1.1 s**: "Run pnpm setup…" | updated in 2.7 s |
| `bun_global` | **failed in 1.2 s**: "verification returned 1.4.0", 1.5.0 written to `~/.cache/.bun` (as on macOS) | updated in 1.0 s, no second copy |
| `hermes_channel` | **check failed**: "no recognizable verdict" | updated to 0.20.1 in 0.01 s |
| `grok_fails` | failed, then "checking", "available": the error gone when a second view opened | failed; still "failed" with its reason in a second view |
| `grok_prompt` | **stuck "installing"** for the whole 45 s watch | failed in 10.0 s: grok stopped to ask "…Replace it? [y/N]" |
| `cursor_native` | "manual-action-required", no update offered | offered 2026.11.05-86047df, updated in 0.02 s |
| `system_git` | `canApply=True`, failed: "/usr/bin/git exited with exit status: 1: update-ref" | `canApply=False`, `sudo apt install --only-upgrade git`. On the pushed branch: failed, with no command |

Linuxbrew isn't installed there, so step 4 didn't run.

## Windows (WING-PC)

The Python rig is POSIX-only. On Windows each row runs through
`windows-row.ps1`, in Windows PowerShell 5.1, with everything under
`$rig = "$env:TEMP\zeron-rig"`. Per run it gives the engine its own port and
`ZERON_DATA_DIR`, and a `USERPROFILE`/`HOME`, `APPDATA`, `LOCALAPPDATA` and
`TEMP` inside `$rig\runs\<label>-<row>`. `PATH` is System32 plus what the row
adds. Every provider is pinned: the one under test to its sandboxed install
(`*_EXECUTABLE`), every other one to a path that doesn't exist. The script
prints the check, each phase with its time, the APPLY RESULT, and the
launcher's version afterwards, and stops only the engine it started (killing
it if `StopEngine` doesn't stop it within 20 s).

```powershell
$rig = "$env:TEMP\zeron-rig"; $row = "scripts\provider-update-rig\windows-row.ps1"
& $row -Label after -Row w1 -Target codex -Launcher "$rig\npm-codex\codex.cmd" -ExtraPath "$rig\node"
```

Notes on running by hand, if you don't use the script:

- Windows PowerShell 5.1 drops the `"` inside arguments to a native program,
  so `& $c call … "{`"harness`":…}"` reaches rigclient as `{harness:…}` and
  it panics ("key must be a string"). Escape them as `\"`.
- With `$ErrorActionPreference = "Stop"`, anything rigclient writes to stderr
  stops the script, so use `Continue`.
- No Node on the machine: unpack the official `node-v*-win-x64.zip` (check
  it against `SHASUMS256.txt`) to `$rig\node`. Run the row installs below with
  that on `PATH` and `npm_config_cache=$rig\npm-cache`, so the real npm
  cache isn't used.

### W0. Unit tests (they include Windows-only ones)

```powershell
$env:ZGX_CHECK = "0"
cargo test -p zeron-engine --lib harness_updates
cargo test -p zeron-harness --lib cmd_shim
cargo clippy -p zeron-engine -p zeron-harness --all-targets
```

Expect every test to pass, among them
`a_winget_package_is_known_by_its_folder`,
`a_winget_package_id_is_read_off_its_folder`,
`a_scoop_receipt_is_read_under_either_name` and
`scoop_is_asked_to_update_the_app_not_a_quoted_word`. The branch adds no
clippy warnings in `harness_updates*`, `process.rs` or `cmd_shim.rs`. Five
warnings there come from `zeron/zgx` (unused test-only items on Windows:
`InstallFileLock::file`, `wait_for_phase`, imports in `harness_updates.rs`'s
and `ownership.rs`'s tests).

### W1. Codex from npm (it was manual)

```powershell
& npm install -g "@openai/codex@0.165.0" --prefix "$rig\npm-codex" --no-audit --no-fund
& $row -Label after -Row w1 -Target codex -Launcher "$rig\npm-codex\codex.cmd" -ExtraPath "$rig\node" -Seconds 600
```

Expect `before`: `canApply=false`, manual "Update Codex with its original
installer or package manager". Expect `after`: `available`, `canApply=true`,
then updated to the newest release. The engine doesn't log the command, but
npm does: `$rig\runs\after-w1\home\AppData\Local\npm-cache\_logs\*-debug-0.log`
shows `argv "install" "--global" "--prefix" "<$rig\npm-codex>" …` and the
`node`/`npm-cli.js` it ran. Codex is large: on a slow line the install takes
minutes, hence `-Seconds 600`.

### W2. Pi from npm (it ran its own updater)

```powershell
& npm install -g "@earendil-works/pi-coding-agent@1.4.0" --prefix "$rig\npm-pi" --no-audit --no-fund
& $row -Label after -Row w2 -Target pi -Launcher "$rig\npm-pi\pi.cmd" -ExtraPath "$rig\node"
```

Expect `after`: updated to 1.5.0 at that path. For `before`, list every
`pi-coding-agent` folder under `$rig` before and after, to see which copy moved.

### W3. A prompting updater, and the process tree

Zeron quotes every argument it passes to a batch file (`"version"`), so the
fake compares `%~1`, not `%1`. Create `$rig\grok\grok.cmd`:

```bat
@echo off
if "%~1"=="version" (echo grok 1.0.0& exit /b 0)
if "%~1 %~2"=="update --check" (echo update available: 1.0.0 -^> 1.1.0& exit /b 0)
if "%~1"=="update" (echo Downloading grok 1.1.0...& <nul set /p "=An existing install was found. Replace it? [y/N] "& ping -n 3600 127.0.0.1 >nul)
```

Look for `ping.exe` while the engine is still up, not after it exits (the
engine's Job Object kills the tree when the engine goes, whichever engine it is):

```powershell
$look = 'Start-Sleep 4; "PINGS@4s: " + ((Get-Process ping -EA 0).Id -join ","); Start-Sleep 12; "PINGS@16s: " + ((Get-Process ping -EA 0).Id -join ",")'
& $row -Label after -Row w3 -Target grok -Launcher "$rig\grok\grok.cmd" -Seconds 25 -AfterApply $look
```

Expect `after`: Failed at about 10 s with "grok stopped to ask “An existing
install was found. Replace it? [y/N]”, and nobody can answer it here", with a
`ping` at 4 s and none at 16 s. Expect `before`: Installing until you stop it,
the `ping` still running, and an engine that doesn't stop on `StopEngine`.

### W4. WinGet (read-only)

```powershell
winget show --id SST.opencode --exact --accept-source-agreements --disable-interactivity
```

Expect a `Version:` line, which is what the `after` engine offers. A real
WinGet install isn't needed. The engine knows a WinGet package by its folder,
so copy a CLI into a folder named like WinGet's under `$rig` and check it.
Only the alias in the real `WindowsApps` folder can run `winget.exe`, so that
folder goes on PATH (it holds no provider CLIs; check):

```powershell
$wg = "$rig\winget-fake\Microsoft\WinGet\Packages"
New-Item -ItemType Directory -Force "$wg\SST.opencode_Microsoft.Winget.Source_8wekyb3d8bbwe", "$wg\Anthropic.ClaudeCode_Microsoft.Winget.Source_8wekyb3d8bbwe" | Out-Null
Copy-Item <an older opencode.exe> "$wg\SST.opencode_Microsoft.Winget.Source_8wekyb3d8bbwe\opencode.exe"
Copy-Item <a claude.exe> "$wg\Anthropic.ClaudeCode_Microsoft.Winget.Source_8wekyb3d8bbwe\claude.exe"
& $row -Label after -Row w4-claude -Target claude-code -Launcher "$wg\Anthropic.ClaudeCode_Microsoft.Winget.Source_8wekyb3d8bbwe\claude.exe" -ExtraPath "$env:LOCALAPPDATA\Microsoft\WindowsApps" -CheckOnly
```

Expect `canApply=true` and `latest` = WinGet's version, not npm's. Claude
through WinGet: `before` is manual (`winget upgrade Anthropic.ClaudeCode`),
`after` is `canApply=true`. Never apply W4: it would upgrade the machine's
real WinGet package. The upgrade passes `--exact`. Without it `--id
SST.opencode` also matches `SST.OpenCodeDesktop` (`winget search --id
SST.opencode`).

If Windows isn't in English, the `Version:` label is localized. Expect the
`after` engine to say "WinGet's description of SST.opencode names no
version it can install" as a check error. Note it.

### W5. Scoop, in a sandbox root

Scoop's installer, and `scoop install`/`scoop update` with a root whose
shims aren't on the user's PATH, add `<root>\shims` to the **user PATH in
the registry** (`HKCU\Environment`). Save that value first and put it back
after each Scoop command. A copy of the installer whose `Write-Env` does
nothing covers the installer, but not Scoop itself. Run Scoop with
`USERPROFILE` in the sandbox too, so its `~\.config\scoop` goes there.

```powershell
$env:SCOOP = "$rig\scoop"; $env:USERPROFILE = "$rig\scoop-home"   # in a child PowerShell
& <installer> -ScoopDir "$rig\scoop"
```

Install from the bucket, not as `opencode@1.19.0`. A pinned install records
a manifest `url` instead of a bucket, and `scoop update` keeps it where it
is. For an older release, take the bucket's manifest from an older commit
for the install, then put it back:

```powershell
$b = "$rig\scoop\buckets\main"
git -C $b log --format="%h %s" -- bucket/opencode.json      # e.g. 3e6c7baf67 "Update to version 1.19.0"
git -C $b checkout 3e6c7baf67 -- bucket/opencode.json
& "$rig\scoop\shims\scoop.cmd" install opencode
git -C $b checkout HEAD -- bucket/opencode.json
# back in the rig's shell (no SCOOP: the engine must find the root itself)
& $row -Label after -Row w5 -Target opencode -Launcher "$rig\scoop\shims\opencode.exe" -ExtraPath "C:\Program Files\Git\cmd"
```

Scoop 0.6 calls the receipt `apps\opencode\current\scoop-install.json`
(it was `install.json`). It should say `"bucket": "main"`. Expect `after`:
`latest` = the version in `$rig\scoop\buckets\main\bucket\opencode.json`,
then updated by Scoop with `$rig\scoop\apps\opencode\current` on the new
version. `before`: OpenCode 1.19's own `upgrade` now finds Scoop and
reinstalls from the bucket's file (its log, `~\.local\share\opencode\log`,
says `method=scoop`). It updates too, but leaves a receipt with a `url`
rather than a bucket. Older OpenCode stopped at "may be managed by a package
manager".

### W6. A failure stays visible

Use W3's `grok.cmd`, with `update` changed to:

```bat
if "%~1"=="update" (echo.>>"%~f0"& echo npm error syscall rename 1>&2& exit /b 1)
```

```powershell
$second = 'Start-Sleep -Milliseconds 800; Rc watch $Port $data WatchHarnessUpdates "{}" 6 grok | % { "SECOND $_" }'
& $row -Label after -Row w6 -Target grok -Launcher "$rig\grok-fails\grok.cmd" -Seconds 10 -AfterApply $second
```

Expect `after`: still `failed` with "grok failed (exit code 1): npm
error syscall rename". Expect `before`: "checking", then "available", and
the error gone.

### Results on Windows (WING-PC, Windows 11 26200, 2026-11-06)

`before` is `zeron/zgx` c6891da0, `after` this branch. W4 and W5 `after`
ran with the Scoop and WinGet fixes, W1–W3 and W6 before them (those paths
didn't change).

| Row | before | after |
|---|---|---|
| W0 | n/a | 40 + 6 pass. On the pushed branch one failed on a machine without npm on PATH (fixed: the test brings its own npm) |
| W1 Codex, npm | `canApply=false`, "Update Codex with its original installer…"; still 0.165.0 | 0.165.0→0.167.0 by `npm install --global --prefix <$rig\npm-codex>` with the rig's own npm, in 378 s |
| W2 Pi, npm | `pi update --self` failed in 2.8 s; no copy moved (1.4.0) | 1.4.0â†’1.5.0 at that prefix, in 233 s |
| W3 prompt | stuck "installing" past 60 s, `ping` alive at 56 s, `StopEngine` ignored | failed at 10.2 s with grok's question; `ping` gone by 16 s with the engine still up |
| W4 Claude, WinGet | `canApply=false`, `winget upgrade Anthropic.ClaudeCode`, latest 2.1.319 (vendor) | `canApply=true`, latest 2.1.317 (WinGet's) |
| W4 OpenCode, WinGet | `canApply=true`, latest 1.19.5 | `canApply=true`, latest 1.19.5 (WinGet's and npm's agree) |
| W5 Scoop | updated in 13.7 s by OpenCode's own updater; receipt left with a `url`, not a bucket | 1.19.0â†’1.19.5 by Scoop in 17.3 s, receipt still names `main`. On the pushed branch: check failed (receipt name), then Scoop was asked for `'update'` |
| W6 failure | failed at 0.16 s, error gone at 2.8 s | failed at 0.21 s; still failed with its reason in a second view |
