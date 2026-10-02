# Update support

Zeron checks and updates the installation selected by the owning device's launch
resolver. Home and Settings → Updates show the state reported by that device,
whether it is this Mac or a remote engine.

## Shared lifecycle

- Configuration overrides, PATH priority and provider fallbacks match launches.
  A newer duplicate does not take precedence. No duplicate is removed.
- Checks record the launcher, the resolved file and the owning package. Apply
  revalidates that evidence before and after waiting for idle; a changed
  installation is checked again instead of updated.
- Installations and updates share an installer lease, so two agents that share
  a Homebrew or npm installation never mutate it at the same time. An agent
  waits for its own runs first, so a busy agent does not hold up the others.
  Per-agent execution gates coordinate discovery, running work and new
  launches.
- Agent updates take an engine restart admission ticket. An engine restart
  never starts under an agent installer, and an agent installer never starts
  once a restart has closed admission.
- Verification resolves the next launcher and probes its version. A different
  launcher, an older version than expected or a downgrade are failures even
  when the installer exits successfully. A managed archive is expected to
  launch from the release directory it just installed. Discovery caches are invalidated after
  every mutation attempt.
- Failure messages keep the tail of the installer's output. Homebrew 7's
  warning about unrelated untrusted taps is removed from them.

These are per-engine guarantees. They do not lock out external package managers
or prove the hidden behavior of arbitrary wrapper scripts.

## Agent installers

| Installation | Action |
| --- | --- |
| Homebrew cask or formula (`Caskroom/<token>/<version>/…`, `Cellar/<token>/<version>/{bin,libexec}/…`) | `brew upgrade --cask` or `--formula` with the brew of that prefix. An upstream release Homebrew has not published yet is shown with the brew command, and automatic updates wait for Homebrew. |
| npm globals, including under Homebrew's Node | The agent's own updater. |
| OpenCode | `opencode upgrade --method <curl\|npm\|pnpm\|bun>` for the layout of the binary that runs, so a Homebrew keg beside a standalone copy is not upgraded instead. |
| Codex Unix standalone | Verified release archive, installer lock and atomic activation. Only a launcher that follows the `current` link qualifies; a direct release path is pinned and stays manual. |
| Antigravity managed archive | The archive pinned by the Antigravity ACP registry. |
| winget, nix, snap, system packages | Manual instructions. |

## Zeron itself

| Installation | Ownership and outcome |
| --- | --- |
| Desktop app and embedded engine | One installation and one update row. Explicit desktop restart reserves embedded-engine admission when idle; busy work prevents restart. |
| Homebrew-owned macOS desktop bundle | A matching cask receipt and bundle link establish ownership. Manual update through the owning Homebrew. |
| Managed Unix daemon, local or remote | Engine-owned operation survives client disconnects. Durable record precedes mutation; installed target is verified before restart and running version after recovery. Automatic (`ZERON_AUTO_UPDATE`) and requested updates are the same operation. An unusable cached download is replaced. |
| Supervised daemon | Restart through launchd/systemd after idle. Failed or unconfirmed restart reports restart required and reopens admission. |
| Hand-started managed daemon | Installs and reports restart required. No guessed process restart. |
| Supervised daemon whose binary someone else replaced (the desktop app swapping its bundle, `zeron update`) | Restarts into the installed binary when idle, without installing. Tried once per installed version. |
| Source build, copied binary, older engine | Manual guidance. Unsafe legacy mutation is not offered. |

## Remaining limits

- Engine retry deduplication retains the latest operation, not all historical
  request IDs. An operation interrupted before it installed anything is
  forgotten on restart, and an outcome is dropped once the engine runs another
  version than the one it describes.
- Desktop restart while busy is refused rather than queued. Separate daemons
  sharing a desktop bundle are not reserved by the embedded-engine gate. Desktop
  updates do not yet have a post-relaunch operation journal.
- Duplicate installations are not enumerated in the UI. Settings identifies
  the selected installation.
- Real launchd/systemd restart and relayed reconnect recovery need controlled
  runtime coverage. Linux and Windows have not been runtime-tested.

## Verification

```sh
cargo test -p zeron-engine --lib
cargo test -p zeron-harness --lib -- --test-threads=1
cargo test -p zeron-proto -p zeron-update -p zeron-rpc
cargo test -p zeron-ui --lib -- --test-threads=1
cargo fmt --all -- --check
```
