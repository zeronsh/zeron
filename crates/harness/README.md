# Explicit agent installation

Settings → Providers → **Install** runs on the selected device, as its user. Merely
listing or detecting agents never invokes these CLI installers. The table below
was checked with `curl -fsSL` against the linked vendor documentation on
2026-09-21. Commands download the vendor's current release, except for the existing
pinned Antigravity archive.

| Agent / documentation | Unix command (Linux and macOS) | Native Windows command |
| --- | --- | --- |
| [Claude Code](https://code.claude.com/docs/en/setup) | `curl -fsSL https://claude.ai/install.sh \| bash` | `irm https://claude.ai/install.ps1 \| iex` |
| [Codex](https://github.com/openai/codex/blob/main/README.md) | `curl -fsSL https://chatgpt.com/codex/install.sh \| sh` | `irm https://chatgpt.com/codex/install.ps1 \| iex` |
| [Cursor](https://cursor.com/docs/cli/installation) | `curl https://cursor.com/install -fsS \| bash` | `irm 'https://cursor.com/install?win32=true' \| iex` |
| [OpenCode](https://opencode.ai/docs/) | `curl -fsSL https://opencode.ai/install \| bash` | `npm.cmd install -g @opencode/cli` |
| [Pi](https://pi.dev/docs/latest) | `curl -fsSL https://pi.dev/install.sh \| sh` | `npm.cmd install -g --ignore-scripts @earendil-works/pi-coding-agent` |
| [Grok](https://docs.x.ai/developers/release-notes) | `curl -fsSL https://x.ai/cli/install.sh \| bash` | `npm.cmd install -g @xai-official/grok` |
| [Hermes](https://hermes-agent.nousresearch.com/docs/getting-started/installation) | `curl -fsSL https://hermes-agent.nousresearch.com/install.sh \| bash` | `irm https://hermes-agent.nousresearch.com/install.ps1 \| iex` |
| [Devin](https://cli.devin.ai/) | macOS, when brew resolves: `brew install --cask devin-cli`; otherwise `curl -fsSL https://cli.devin.ai/install.sh \| bash` | `irm https://static.devin.ai/cli/setup.ps1 \| iex` |
| Antigravity | Existing verified 1.1.1 archive in `acp::antigravity_archive`; no shell command | Same archive path on supported architectures |

Unix shell commands run through `sh -c`, with PATH augmented from the user's login
shell and npm toolchain. Native Windows `irm` commands run through
`powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command`.
Windows npm commands use the resolved npm launcher (normally `npm.cmd`).

If shell prerequisites are absent, these documented npm alternatives are used
when npm resolves: `npm install -g @openai/codex`,
`npm install -g @opencode/cli`,
`npm install -g --ignore-scripts @earendil-works/pi-coding-agent`, and
`npm install -g @xai-official/grok`. Codex also falls back to npm on Windows when
PowerShell is unavailable. Grok's npm route is documented in its
[enterprise guide](https://docs.x.ai/build/enterprise); its
[published package metadata](https://registry.npmjs.org/@xai-official/grok/latest)
includes Windows binaries. No action is offered when none of the documented
methods' prerequisites resolves, or for Mock. The row then keeps a manual hint;
shell hints on Windows can be used in WSL.

Install processes receive `CI=1`, `NONINTERACTIVE=1`, `TERM=dumb`, no stdin/TTY,
and no inherited `ZERON_*` or nested Claude environment markers. Output is bounded
and credentials are redacted before errors reach Settings. Cancel, timeout
(15 minutes), or dropping the request kills the owned process tree. There is no
automatic elevation: an installer that needs sudo or an interactive prompt may
fail. Cancelling does not undo files an installer already wrote.

After installation, executable version caches for the agent are invalidated and
the normal installed probe runs again. A missing binary produces PATH guidance;
the running app's login-shell snapshot is not refreshed. Authentication remains a
separate step. Hermes may additionally require its documented ACP Python extra
for agent sessions; the Install action checks CLI detection only.

## Verification

The `installer-fixture` feature exposes an explicit command injection seam for
integration tests. The engine's command environment override is compiled only
for unit tests, never production. Tests exercise successful detection, failure,
missing binaries, cancellation, duplicate requests, process descendants, bounded
redaction, platform prerequisites, and version-cache invalidation.

The opt-in engine test `rpc::tests::installer_rpc_npm` runs the documented Codex
npm alternative in a disposable prefix and isolated home, through `InstallHarness`.
It asserts that the returned Settings descriptor changes from uninstalled/disabled
to installed/enabled. It never runs a vendor shell installer. Set `TMPDIR` to a
scratch directory before running it with `--ignored --nocapture`.

The iOS app adds no Install action. `HarnessDescriptor.can_install` already has
`#[serde(default)]` (`crates/engine/src/registry.rs`); iOS's `WireHarness: Decodable`
in `apps/ios/Zeron/Sync/WorkspaceStore.swift` ignores unrecognized keys, including
`canInstall`. `HarnessCatalog` continues consuming the existing mapped fields.
