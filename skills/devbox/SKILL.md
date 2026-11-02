---
name: devbox
description: Set up an owner-named Boat devbox, or Namespace on request, with Crew, Chromium and private Ashler Tilt. Not Scaffold or production deployment.
---

# Personal devbox

Use for a personal persistent Linux amd64 Crew machine. **Default new machines to Boat `xlarge` (16 CPUs / 32 GB)**; keep an explicitly selected or recovered Namespace machine on its supported `l` shape. Verify the provider's current shape and actual `nproc` and `/proc/meminfo`. Reuse an owner-matched machine for the selected provider unless a new one was requested. A setup request authorizes one billable machine, not an account plan purchase, broader sharing, or replacement after an uncertain response.

Default to Crew production; use staging when requested or when testing against the owner's staging controller. Never replace or restart the Mac client hosting the active agent turn. Controller and host must both run Crew with neutral devbox bindings before claiming wake support.
Native WakeDevice in this port is restricted to an authenticated local synced **staging** controller in project/deployment `ashler-staging`; this is not a production wake rollout. Do not claim native Namespace wake or active-turn behavior is verified in either channel without the actual end-to-end evidence. Preserve existing Namespace production release/tmux setup independently.

## Provider and identity

Resolve the signed-in owner through `gh api user --jq '{login,name}'`; use a confirmed name if the profile is empty. Name the machine `<owner-login>-ashler-dev` and the Crew device `<Owner Name> · Devbox · <machine-name>`. Existing device names survive reboot; rename a reused device through Crew rather than assuming an environment change changes its row.

Read only the selected provisioning variant:

- [Namespace](references/namespace.md): `devbox`, persistent `/workspaces`, task-marker idle protection.
- [Boat](references/boat.md): `boat`, `xlarge`, persistent `/home/user`, finite TTL protection.

Preserve interactive provider sign-in, approval and MFA. Never manufacture authorization URLs, answer provider safety gates automatically, transfer broad local credentials, change account permissions, or enable public hosting to bypass setup failures. Read the target checkout's guidance and preserve existing worktrees.

## Crew from source and OMP

Devboxes run Crew built on the host from one commit of `Ashler-AI/comet` `main`, not a release archive. Pin `main` to its 40-hex commit once, then take that commit's workflow, provider reference, scripts and source together; never mix commits:

```bash
COMMIT="$(gh api repos/Ashler-AI/comet/commits/main --jq .sha)"
[[ "$COMMIT" =~ ^[0-9a-f]{40}$ ]] || exit 1
gh api -H 'Accept: application/vnd.github.raw+json' \
  "repos/Ashler-AI/comet/contents/skills/devbox/SKILL.md?ref=$COMMIT" > selected-devbox-skill.md
gh api "repos/Ashler-AI/comet/tarball/$COMMIT" > "comet-$COMMIT.tar.gz"
shasum -a 256 "comet-$COMMIT.tar.gz"
```

Read the selected workflow and its provider reference, and fetch `skills/devbox/scripts/{build-crew.sh,configure-host.sh,configure-host.py,autostart.sh,keepalive.py}` through the same `?ref=$COMMIT` route. Copy the scripts into one private directory on the host and the tarball into `~/.cache/crew-build/`, then build with the provider environment `configure-host.sh` uses:

```bash
COMET_DEVBOX_PROVIDER="$PROVIDER" COMET_DEVBOX_ID="$DEVBOX_ID" \
  bash build-crew.sh "$HOME/.cache/crew-build/comet-$COMMIT.tar.gz" "$TARBALL_SHA256" "$COMMIT"
```

`build-crew.sh` rechecks the tarball digest, installs the release workflow's Linux build dependencies, and builds the production-identity `comet` and `zeron-engine` with the stable toolchain under a renewable lease. It installs them into `~/.local/lib/crew/<version>-src-<commit12>-<sha256>/` and prints `CREW_BINARY` and `CREW_BINARY_SHA256` for `configure-host.sh`. Source, Cargo downloads and build output stay in `~/.cache/crew-build`, which Boat snapshots skip (see Disk budget). Run it with `systemd-run --user` so a snapshot can't freeze a build started from SSH.

To update a configured host, build the newer commit, then stop the idle service, move its env and unit aside, rerun `configure-host.sh` with `CREW_EXISTING_DEVICE_ID`, and start it again; the device ID is kept. Keep the previous build until the new one has served a real turn. Hosts installed from a release archive keep working until their next update.

Run the source's `install.sh --install-omp` on the host and confirm `omp --version`. Never copy model-provider OAuth tokens or install a moving OMP version. Crew-owned turns use Crew Agent Auth; do not run OMP `/login` or set up a separate broker.

## Configure the host

Install/verify Node/npm, Python, gcloud, passwordless sudo, and the selected provider's prerequisites. Namespace needs tmux; Boat needs its official CLI and a sandbox-scoped credential with `sandbox.update`. Confirm Boat `extend` succeeds before lengthy setup; never substitute an account-wide token. Render only Infisical `platform/localdev` products for the Ashler stack.

Upload the selected workflow's complete script set into one private directory. For this native workflow, use the host's actual immutable extracted executable path and raw executable checksum, then run:

```bash
CREW_OWNER_NAME="$OWNER_NAME" \
CREW_DEVBOX_NAME="$DEVBOX_NAME" \
COMET_DEVBOX_PROVIDER="$PROVIDER" \
COMET_DEVBOX_ID="$DEVBOX_ID" \
CREW_BINARY="$IMMUTABLE_CREW_EXECUTABLE" \
CREW_BINARY_SHA256="$VERIFIED_EXECUTABLE_SHA256" \
CREW_CHANNEL="$CHANNEL" \
  bash /tmp/configure-host.sh
```

Boat's configured ID must match the native `BOAT_ID`. A copied template binding fails closed. Configuration protects its own execution: owned Namespace marker or a renewable Boat lease. It installs pinned Playwright Chromium outside `~/.cache`, channel-qualified Crew/gcloud helpers, and a bounded managed guidance block without replacing personal instructions. Existing engines are not restarted.

If Chromium cannot reach its CDN, use the exact pinned installer URL on an authorized local machine, compare SHA-256 before and after transfer, and extract into the reported cache directory. Do not substitute mirrors or change network/security policy. Prove a rendered page, not just `--version`.

Keep staging and production data directories separate. Do not copy `session.json`, device IDs, or credentials across environments. On a reused machine, inspect active turns before any restart; stop only the selected owned engine after work is idle. Never run competing channels or supervisors against one data directory.
This native runtime uses production `~/.crew` / IPC `27958`, or staging `~/.crew-staging` / IPC `27957`; callbacks remain `27654` / `27656`. The Linux build has a production identity; staging supplies explicit CLI/runtime coordinates, not a private Canary build. Owner labeling uses the actual native `ZERON_DEVICE_NAME`; callback configuration uses `ZERON_CALLBACK_PORT`. Staging Edge/control plane/project/deployment are `comet-staging.internal.ashler.com`, `scaffold-staging.internal.ashler.com`, `ashler-staging`, `ashler-staging`. Production retains its ordinary project and omits deployment unless supplied by its validated release contract.

The guard requires a physical, owner-owned, non-shared-writable executable outside either profile's moving `app` tree and refuses different existing configuration or helper files. Existing native profiles require explicit `CREW_EXISTING_DEVICE_ID` adoption with matching saved coordinates; no credentials or identities are copied. Legacy profiles are retained, never adopted by renaming their directory. Add the actual checkout path through Crew's project picker; there is no invented native default-folder flag.


## Private setup forwarding and sign-in

Before presenting login URLs, verify the required local ports are free. Credential callbacks cannot be remapped; fail closed rather than stop another listener. App/Tilt local ports can use an unused alternative, consistently in every link and health check.

| Purpose | Local | Remote |
| - | - | - |
| Crew login | production `27654`, staging `27656` | same |
| gcloud login | `8085` | `8085` |
| App | `13000` | `3000` |
| Tilt | `20350` | `10350` |

Use the selected provider's forwarding commands for all four mappings. Keep the setup-scoped forwards alive through authentication and endpoint checks, then stop them **before an ordinary Crew turn**. Never expose Tilt, callbacks, or browser CDP through public hosting. The coding session travels over Crew's relay, not these tunnels.

Run `~/.local/bin/crew-devbox-$CHANNEL login` on the host; open its actual printed authorization URL locally and let the fixed forwarded callback complete login. For gcloud, use `~/.local/bin/crew-devbox-gcloud-login` and the `8085` forward. Do not use remote bootstrap, `--no-browser`, or copied credentials. Inspect a pending login before retrying; duplicate flows can select `8086` and strand the callback. Infisical keeps its supported interactive login; GitHub uses the owner credential below instead of another browser approval.

Start the host with `~/.local/bin/crew-devbox-autostart "$CHANNEL"`. Namespace retains its dedicated self-respawning tmux server and declared `.bashrc` boot hook. Boat uses one guarded, channel-qualified `crew-devbox-$CHANNEL.service` running the native `headless` command with explicit coordinates and linger; do not run native `daemon install` over another channel's service. The immutable binary digest is rechecked before every service start. An occupied IPC port is accepted only for the expected owned process: its kernel executable image must match pinned SHA-256, size, owner and mode, and that PID must own the LISTEN socket inode. Cold-restore relocation is permitted; a matching pathname is never authority. Boat imports current verified `BOAT_ID` into the service manager, never a snapshotted unit file. If native identity is unavailable at OS boot, startup fails closed until an explicit trusted Crew wake supplies it. Already-active owned engines are not restarted. Only an already-enabled existing `ashler-localdev.service` is resumed; this does not prove app/Tilt readiness.

## GitHub from Infisical

After signing into Infisical on the devbox, reuse Scaffold's existing owner PAT from project `platform`, environment `platform`, path `/services/scaffold/user-github-pats`. This canonical owner store applies to both Crew channels; do not substitute the staging service's credential store. Resolve the owner email from authenticated Crew/Infisical identity, not a machine name, and the expected `OWNER_GITHUB_LOGIN` from the owner identity established above.

Derive `GITHUB_PAT_KEY` with Scaffold's `githubPatEnvironmentName` convention: remove any `+tag` from the email local part, uppercase the email, replace runs of non-ASCII-alphanumeric characters with `_`, trim surrounding `_`, and prefix `GITHUB_PAT_`. Resolve `PLATFORM_PROJECT_ID` from the platform checkout's `.infisical.json` or authenticated project metadata. Fetch only that owner's key, never the PAT bundle, another owner's PAT, or a GitHub App private key. Missing, invalid, or mismatched credentials stop setup; do not silently select a shared fallback. The selected credential must have the GitHub CLI scopes plus `repo` and `workflow` for normal repository/workflow changes.

Run on the devbox, without tracing or printing the credential:

```bash
(
  set +x
  set -euo pipefail
  token="$(infisical secrets get "$GITHUB_PAT_KEY" \
    --projectId "$PLATFORM_PROJECT_ID" --env=platform \
    --path=/services/scaffold/user-github-pats \
    --plain --silent --secret-overriding=false)"
  [[ -n "$token" ]] || { echo 'Owner GitHub credential is missing from Infisical.' >&2; exit 1; }
  [[ "$(GH_TOKEN="$token" gh api --hostname github.com user --jq .login)" == "$OWNER_GITHUB_LOGIN" ]] || \
    { echo 'GitHub credential does not match the devbox owner.' >&2; exit 1; }
  printf '%s\n' "$token" | env -u GH_TOKEN -u GITHUB_TOKEN \
    gh auth login --hostname github.com --git-protocol https --with-token
  env -u GH_TOKEN -u GITHUB_TOKEN gh auth setup-git --hostname github.com
)
```

Verify `gh api --hostname github.com user` and an authenticated `git ls-remote` for the requested repository, and confirm any file-backed `gh` credential is owner-only (`0600`). Keep the owner's Git author identity. Re-run this setup after central token rotation; do not bake authenticated profiles into templates or add a second credential service. This uses existing Infisical/GitHub CLIs and does not replace the pinned Crew runtime.

## Disk budget

Every Boat resume restores the whole disk except `~/.cache`. `xlarge`, the largest type, holds 251.5 GiB, and a box whose data outgrows it can never resume: one devbox stopped at 259.1 GiB is unrecoverable. Restore time follows file count as much as bytes. A 180 GiB, two-million-file devbox answered a Crew wake in 144 seconds but took about 30 minutes to finish restoring. Trimmed to 87 GiB, three cold cycles answered in 111–223 seconds and finished restoring in 10–31 minutes, mostly in the Docker-volume phase; pauses took 1 to 22 minutes.

- Keep build output and package caches in `~/.cache`. `build-crew.sh` builds there; also run `npm config set cache ~/.cache/npm`, and point `CARGO_HOME` and `CARGO_TARGET_DIR` there for any other Rust work. One leftover Crew `target` directory in the home was 83 GiB.
- Check `df -h /` before long work and keep the box under about 150 GiB. The local cluster's unpacked images are most of a restore's files; prune images and volumes nothing uses.
- Boat deletes a stopped box's data 14 days after it stops. Resume it, or save a named snapshot, before then.
- While `boat info` still shows `hydration`, don't walk the whole disk (`du`, `find`): it forces every file to restore at once.

## Work and sleep

Crew protects executing turns and releases protection on completion, cancellation, failed starts and shutdown. Parked harnesses and idle Tilt processes do not keep a host running.

- Namespace uses owned `/.namespace/tasks` markers. Crashes can leave stale files; remove only confirmed abandoned markers, never all tasks.
- Boat uses a one-hour finite TTL, renewed every five minutes while work is active. Its default TTL is **not idle detection**. Initial protection failure rejects work; renewal failure interrupts protected work and leaves it recoverable. Last completion resets the idle deadline once. A crash leaves the last finite deadline, not indefinite billing. Provider/control-plane outages can still interrupt work.
- For independent work outside a turn, load the selected config with `set -a; source "$HOME/.config/crew-devbox/$CHANNEL.env"; set +a`, then use `~/.local/bin/crew-devbox-keepalive <command> [arguments...]`. `--timeout SECONDS -- command` optionally bounds work; it never replaces finite Boat TTL renewal. Do not launch background work and assume the finished turn still protects it. Cleanup retains the process-group leader until descendants receive the final signal; cancellation cannot target a reused group ID.

The matching native controller's explicit Wake or pending send must boot only the selected bound machine, confirm the exact Crew peer, reload selectors and submit the unchanged draft once. Passive viewing must not wake a stopped device; provider login failure must not create a replacement. Automatic private app/Tilt/gcloud forwarding is release-dependent: use the selected source's documented support, retain manual setup forwards when needed, and do not describe unimplemented native forwarding as verified.

## Ashler runtime and proof

Read the checkout's `docs/agents/localdev.md` and `infra/localdev/README.md`; use maintained Linux installation paths for Docker, kubectl, kind, ctlptl, Tilt, just, gcloud and Infisical. Follow `just bootstrap`, `just env`, and `just dev`, not a competing stack. Retain the maintained Next.js binding (`0.0.0.0` by default): Docker/kind must reach `http://host.docker.internal:3000` for callbacks. Deny public inbound app traffic through provider private access or the host firewall while allowing required Docker bridge traffic. Keep Tilt/CDP and credential callbacks loopback-only; never publish them through provider hosting. Keep `ASHLER_INCREMENTAL_TSC_CHECKS=false`; never run local typechecks.

Before starting the Linux kind stack, check `fs.inotify.max_user_instances`. Kind containers share host-UID quotas: a full stack can exhaust the default 128 instances. If below 1024, persist `fs.inotify.max_user_instances = 1024` in `/etc/sysctl.d/60-ashler-localdev-inotify.conf` and apply that file with `sudo sysctl -p`. Retain higher existing values; do not raise watch-count or heap limits without evidence.

Seed only the confirmed host-local database. After stop/resume, verify app and Tilt separately from Crew readiness. Use maintained local recovery instructions for stale kind networking; never reset, migrate or reseed data merely to make wake pass, and never apply local recovery to shared databases. This skill does not select or promise datastore persistence.

Record actual owner, provider ID, CPU/memory/disk, Crew version/channel, and these observations:

1. Both Mac and host visible, owner-qualified Devbox label, same stopped device retained in Devices/folder picker.
2. Mac-initiated real selected-model turn returns `hostname`/`pwd` through Crew Agent Auth.
3. Active turn remains protected, including a turn exceeding one hour; overlapping completion/cancellation preserves other work; idle sleep follows final completion.
4. Headless Chromium renders a page; private app/Tilt endpoints and pod-to-web callback reachability work before and after resume. Forwarded health alone does not prove document generation/search/research callbacks.
5. Explicit stop/wake returns the same device ID, reloads refs/models, submits the draft once, and passive viewing leaves a stopped machine stopped.
6. Measure 32 GB peak memory during cold bootstrap, rebuild, browser and concurrent turns. Container limits are not measured use; leave optional data/observability stacks opt-in.

State authentication, release, UI or runtime proof gaps explicitly. Installation, tests and a ready relay alone do not prove the complete setup.

## Maintainer regression commands

After integration, run `python3 -m unittest discover -s skills/devbox/scripts -p 'test_*.py'` on Linux, plus `bash -n skills/devbox/scripts/{build-crew.sh,configure-host.sh,autostart.sh}`. The native OS regression checks relocated executable images, same-size wrong digests and PID-owned listener sockets. Follow with an explicitly authorized isolated staging host smoke; no test or installation alone satisfies the completion evidence above.
