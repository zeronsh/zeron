# Cloud device and credential vault

Status: v1 implementation (2026-10-01). Supersedes the "restricted runners are deferred"
note in the encrypted-sync RFC for the Cloud device only.

## Product shape

**Cloud is a place a session runs, and every Cloud session gets its own machine.** It is a
row of the composer's device menu, after the devices holding the project: offered while Cloud
is on, for any project whose GitHub repository (the folder's `origin`) the Zeron GitHub App
reaches. Picked, the composer drops the checkout menu (the machine clones the picked branch
fresh), and the model menu offers only the agents Cloud has an active account for. A session started there runs in its own sandbox — which clones the repository, sleeps
when the session is idle and wakes when the chat is sent to — instead of on the computer the
project lives on, which may then be offline. Cloud has its own providers: a provider works there
only once an account has been connected *for Cloud* (Settings → Providers, with Cloud picked in
the device switcher). Cloud is listed there as one entry, never its session machines; no other
device list shows it.

- v1 providers on Cloud: **Codex** and **Claude Code** only.
- v1 repositories on Cloud: **GitHub repositories** the user installed the Zeron GitHub App on.
  Each session sandbox clones its project's repository from the branch picked in the composer;
  agents get a one-hour GitHub App installation token for `git push` and `gh pr create`, so
  pushes and pull requests act as the App (see "GitHub").
- Side chats an agent spawns (`parentChatId`) run in their parent session's sandbox: they work
  on the same files.
- Opt-in. Nothing is provisioned, and no credential leaves a device, until the user enables
  Cloud in Settings → Cloud. Local login stays the default everywhere.

## Topology

```
laptop engine ─┐                      ┌─ zeron-vault Worker (service binding only)
               │  WorkOS JWT          │    VaultAccount DO  per (user, provider)
               ├──────────────► edge ─┤    VaultDevices DO  per user (keys, parents, revokes)
session engine ┘  runner JWT   Worker │    secret KEK (rotatable), daily canary cron
 (one sandbox                         │
  per session)                        ├─ CloudAccount DO per (org, user): account,
                                      │    sessions (repo + sandbox + lifecycle each), metering
                                      ├─ CloudIndex DO: every Cloud user (cron, orphan scan)
                                      └─ Workflows per session: provision / wake / sleep / delete
```

Each **session device** is an ordinary Zeron engine (`zeron headless`) running as a systemd
service inside one sandbox ([Boat](https://docs.boat.dev) today). It hosts its own DeviceRoom
and its chat, so transcripts, diffs, terminals, file browsing and side chats work unchanged. We
do not use Boat's integrated agents (`POST /prompt`): they would bypass the session doc, the
command plane and every existing UI surface. Every sandbox is created `noEnv: true` (nothing of
Zeron's Boat account reaches it).

### Devices and ids

| Device | Id | Registry row | Engine |
| --- | --- | --- | --- |
| Logical Cloud device | `cloud-{uuid}`, minted at enable | none — it is the vault parent provider connections are authorized for, and the runner JWT's `cld` | none — never dialed or forwarded to |
| Session device | `cloud-{uuid}`, minted per session | written by its engine: `platform: "cloud"`, capability `cloud-session` | `zeron headless` in the session's sandbox |

Projects stay where they are: a space on the computer whose folder it is, whose owner stamps
`githubRepo` (`owner/name` of the folder's `origin`) next to `gitDetected`, so every device can
offer Cloud without asking that computer. A Cloud session's chat row is `{spaceId: <the
project>, deviceId: <session device>, cwd: /home/user/{repo}}`. Device lists hide session
devices. Catalog requests aimed at a Cloud device (`ListHarnesses`, `ListModels`; the picker
aims at the logical device while Cloud is picked) are answered by the asking engine from its own
catalog restricted to Codex + Claude Code, so opening a composer never wakes a sandbox.

### Session lifecycle

1. **Create.** When a chat is created with Cloud picked (`createChat {cloud: true}`), the
   creating engine looks the project's `githubRepo` up in the repositories Cloud reaches and
   calls `POST /cloud/{org}/sessions {chatId, spaceId, repo, branch?}` (idempotent per chat),
   which mints the session device id, claims its device room for the user and starts the
   ProvisionWorkflow; the chat row is written with that device as host and the machine's
   checkout as cwd. The first send queues in the chat doc as usual and is executed once the
   engine is up.
2. **Provision.** Sandbox created with the enrollment and the project
   (`ZERON_CLOUD_ACCOUNT`, `ZERON_CLOUD_CHAT`, `ZERON_CLOUD_REPO`, `ZERON_CLOUD_PATH`,
   `ZERON_CLOUD_BRANCH`) in its per-sandbox env; the engine installs, enrolls, clones the
   repository with its GitHub grant (public repositories need none) and only then drains the
   chat's commands. With `CLOUD_TEMPLATE` set, sandboxes start from that named snapshot (engine
   pre-installed, no identity) instead of installing.
3. **Sleep.** After `CLOUD_IDLE_MINUTES` without activity (no turn running, no one doing anything on it)
   the session's sandbox is stopped. Archiving a chat lets it sleep.
4. **Wake.** A send to the chat nudges its host device; a nudge to a sleeping session device
   wakes it, and the queued command runs once the engine reconnects. A send also recovers a
   session in `error` that still has a sandbox, unless the failure is one a retry can't fix
   ("Machine sizes"). Merely viewing a sleeping chat (or its sidebar row) does not wake it:
   client dials to a sleeping session device get the ordinary offline answer, and the UI offers
   an explicit Wake for files, terminals and diffs.
5. **Delete.** Deleting the chat (or Cloud) runs the DeleteWorkflow: revoke the
   session device, stop, capture final usage, delete the sandbox. The transcript stays.

At most `CLOUD_MAX_AWAKE` (default 5) sessions per user are awake at once; waking another puts
the least recently active idle session to sleep first, and fails with a clear error when every
awake session is busy.

## Edge: Cloud control plane (main Worker)

### CloudAccount DO — `cloud1/{orgId}/{userId}`

Single source of truth for one user's Cloud: account state and logical device id; sessions
(`chatId → {deviceId, spaceId, repo {fullName, cloneUrl, defaultBranch, path}, baseBranch?,
sandbox {provider, id}, state, enrollHash, runnerKey, lastTokenTs, activeRuns, lastActiveAt,
error, failedAction, generation}`); the lifecycle ledger and metering rows. Nothing is written
to the registry: Cloud is not a device.

### Runner identity — `POST /runner/enroll`, `POST /runner/token`

Each session machine's engine is its own device and never sees a WorkOS session (WorkOS
refresh tokens are single-use; sharing a laptop's `session.json` would race). Instead:

1. Provisioning passes `ZERON_RUNNER_ENROLL={orgId}.{userId}.{deviceId}.{code}` as per-sandbox
   env. On first boot the engine generates an Ed25519 key (`{data_dir}/runner-key`, 0600) and
   calls `POST /runner/enroll {orgId, userId, deviceId, code, publicKey}`. The DO checks the
   code hash, stores the key, and clears the code (single use). It also enrolls the same key
   with the vault as device kind `cloud`, with the account's logical device as `parentId`, so
   providers authorized for Cloud cover every session machine.
2. Every token refresh is `POST /runner/token {orgId, userId, deviceId, ts, sig}` with
   `sig = Ed25519("zeron-runner-token\n{orgId}\n{userId}\n{deviceId}\n{ts}")`, `ts` in Unix
   milliseconds within ±120 s and strictly greater than the last accepted `ts`. The Worker
   answers with a **runner JWT** (ES256, `RUNNER_JWT_PRIVATE_KEY` secret, 1 h lifetime):
   `{iss: "zeron-edge", sub: userId, org_id, dev: deviceId, cld: accountDeviceId, kind: "runner"}`.
3. `authenticate()` accepts runner JWTs alongside WorkOS JWTs. `Verified` gains
   `deviceId?` and `kind: "user" | "runner"`. Runner tokens are refused on `/auth/*`,
   `/cloud/*` and the vault upload/admin routes, and may host only their own DeviceRoom
   (`/device/{dev}/ws?role=host`).

Deleting a session's machine (or turning Cloud off, or a vault revoke) clears its key, so the
next token refresh fails and that engine goes dark within one token lifetime.

#### Trust boundary

The sandbox runs full-access agents on arbitrary repository code, so anything that can read
`runner-key` can mint runner tokens. A runner token therefore acts as **its session's device**
(the JWT's verified `dev` claim), never as the whole user — and never as a neighbouring
session. Enforced at the edge:

- **Paths** (`edge/src/runner-policy.ts`, 403): no `/auth/*` org routes, no `/cloud/*`, no
  `/vault/*` except `POST /vault/{org}/grant` for its own device. `/device/{id}/*` only for its
  own id; for any other device only `GET /device/{id}/status` (read-only liveness) — no
  client/host sockets, nudges or sidecars. No legacy loro rooms (`/session`, `/tail`,
  `/stats`, `/diff`, `/snapshot`, `/append`, `/workspace`). On `/registry/{org}` only row sync
  (`ws`, `GET rows`, `POST push`) — no `stats`, `push-target` (APNs titles) or `reset`.
- **Chat-scoped data** (`edge/src/runner-access.ts`): `/chat2/{chatId}/*` and
  `/blob/{chatId}/*` only while registry row `chats/{chatId}` is live and its
  `fields.deviceId` equals the runner's device; otherwise 403 `not_host`. Positive answers are
  cached per isolate for ≤5 min; refusals are never cached.
- **Registry** (`edge/src/registry-runner.ts`; the Worker stamps `x-zeron-runner-device`
  on runner forwards and strips any inbound value). Writes: `devices` only its own row;
  `chats`, `spaces`, `sessions` only when the existing live row is hosted by the runner AND
  the row after the op still is (no claiming, handing away or ownerless rows); deletes need a
  live owned row; tombstones are never revived; every other kind (`preferences`,
  `sidebarPins`, anything new) is refused. Refused ops are skipped individually (an
  `{t:"error", code:"not_owner"}` frame each, plus `rejected` in the ack); the rest of the
  batch applies. Reads (hello/backfill, live `rows` broadcasts, `GET /rows`): every `devices`
  row, the hosted rows the runner owns, the projects (spaces) of the chats it hosts, and
  tombstones of hosted kinds (they carry no fields); laptop chats, other projects and other
  sessions' chats and session rows — titles, previews, status — never reach it.
  Presence is unfiltered, but a runner socket's presence/attribution device is pinned to its
  verified id.

User bearers are unaffected by all of the above.

#### Delegation

The one widening: a session's machine may start and manage **delegates** of the chat it runs
(Zeron MCP `create_chat { cloud: true }`), one level deep. A delegate is a Cloud session of
its own, on its own machine, recorded with `parentChatId` = the parent's chat.

- **Start** (`POST /cloud/{org}/sessions {chatId, parentChatId, branch?, config?}` with a
  runner bearer → `CloudAccount.createChildSession`): `parentChatId` must be the chat the
  runner's device runs (`byDevice`), and that chat must not itself be a delegate (403
  `too_deep`). The delegate takes the parent's project and repository — a machine never names
  another repository — and `config.harness` must be Codex or Claude Code. The usual limits
  hold: credits, and `CLOUD_MAX_AWAKE` machines at once. A runner request without
  `parentChatId` is refused (a machine never acts as the user).
- **Its chat row** is written by the edge, as the user (`chats/{chatId}`: the new machine as
  host, `parentChatId`, the parent's `spaceId`, `roomGen: 2`, the asked-for `config`): a
  runner may write only rows it hosts, and only the edge knows which machine it just made.
- **Manage** (`DELETE /cloud/{org}/sessions/{chatId}`, `POST …/{wake|sleep}`): only sessions
  whose `parentChatId` is its chat. Retry, the session list, projects, billing and Cloud
  on/off stay the user's.
- **See and reach**: the registry shows the runner its delegates' chat and session rows
  (`RunnerScope.children`), and lets it rename, archive or delete them — never re-home them.
  `/chat2/{chatId}/*` and `/blob/{chatId}/*` admit it when it hosts the chat's parent
  (`/chat-host` answers `parentHost`). `/device/{id}/*` admits a client socket (never the host
  role) and the command nudge for a device hosting one of its delegates
  (`runnerDeviceGate`, via the registry's `/device-parents`). No other main chat — on the
  laptop or another session — becomes visible.

A stolen runner key therefore reaches at most: starting machines on the repository its
session already has (bounded by credits and the awake limit), and its own delegates.

### Workflows (Cloudflare Workflows)

Every provider call is a `step.do` with retries; provider configuration errors (e.g. an API
key lacking `sandbox.resume`) fail the step immediately with an operator-facing
`CloudStatus.error`. Secrets never appear in step outputs. Instance ids are deterministic per
(DO, generation), so a double click cannot run two provisions. Each state change is reported
back to the DO, which also re-checks state before and after every wait, so a lost workflow
event costs at most that wait's timeout.

All four run per session (one instance per session lifecycle, ids from the session device +
its generation).

**ProvisionWorkflow** — `create` (`Idempotency-Key: {sessionDeviceId}-g{generation}`,
`noEnv`, `ttlSeconds: CLOUD_TTL_SECONDS`, `machine: CLOUD_MACHINE`, `from: CLOUD_TEMPLATE` when
set, per-sandbox env `ZERON_RUNNER_ENROLL`, `ZERON_EDGE_URL`, `ZERON_DEVICE_NAME="Cloud
session"`, `ZERON_DEVICE_PLATFORM=cloud`, `ZERON_CLOUD_{ACCOUNT,CHAT,REPO,PATH,BRANCH}`) → poll
until ready → `exec` the install script → `waitForEvent("enrolled", 10 min)` (sent by `/runner/enroll`) → `ready`. The
install script (`edge/src/cloud/install-script.ts`) is idempotent POSIX `sh`: it downloads the
Linux release from `{edge}/releases/`, writes `~/.zeron/env` (0600) from the per-sandbox env
(systemd services don't see it), and enables a **system** unit `zeron-cloud.service` running
`zeron headless` as the exec user with `Restart=always`. Enabled units restart on resume
(verified on Boat: a resumed sandbox lands on a new machine in ~4 s with the service up).

**WakeWorkflow** — one step (`edge/src/cloud/wake.ts`, `fastWake`): `resume`, then at once
`systemctl start --no-block zeron-cloud` (Boat holds a command until the machine can run it,
so this races the machine's own start of its enabled units), polling the state every 250 ms,
then easing off (no Workflow sleep: those cost whole seconds). A resume during a stop is taken
(it keeps the machine); a refused one falls back to the slow poll. A wake onto another size stops
the machine first, so that never applies to it. A resume refused for good comes back typed, as
"A wake refused for want of disk" says. Then
`waitForEvent("online", 90 s)` (the runner's first token after resume); on timeout it runs one
idempotent `sudo systemctl restart zeron-cloud` and waits 5 more minutes before failing. None of
this gates the queued message: the engine drains it as soon as it joins its room, so the
workflow only decides when the session reads `ready`.

**SleepWorkflow** — `zeron quiesce` → `sudo systemctl stop zeron-cloud` → `stop` → poll until
stopped. A refused stop is retried (the provider bills nothing while a stop is refused); it is
never forced.
- *Quiesce* (headless IPC `QuiesceForSleep`): the engine starts no new work from here on, so a
  send arriving now waits in its chat for the woken machine instead of starting a turn the stop
  would cut off ("A killed turn"). A turn in flight or a message already waiting makes it exit
  75: the sleep is declined and the session stays `ready` (idle sleep retries a full idle window
  later). A quiesce the stop never follows lifts itself after 3 minutes. An engine that can't
  answer (gone, or older than the handshake) doesn't block the sleep. Accepted, the engine clears
  rebuildable caches before it answers ("Disk").
- *Stop the engine*: Boat reports a sandbox stopped once its disk snapshot is taken while the
  machine may run on a moment; an engine still connected then acknowledged a send's nudge and
  wrote changes the snapshot never saw. Stopped first, it has drained to disk and its device
  room has no host, so sends queue there until the woken engine joins.

**DeleteWorkflow** — revoke the runner and its vault enrollment → `stop` → wait until stopped →
read and persist final usage (it must report `running: false`; usage is unreadable after
deletion) → `delete` → `off`. A refused stop leaves `error` + `failedAction: "delete"`; a
second DELETE resumes — and so does the DO on its own: a failed delete retries after 1, 5 and 15
minutes, then hourly (`deleteRetryAt`), and one stuck mid-way (a lost workflow) resumes after 5
minutes. Every 10 minutes the DO also checks each machine against its chat (`sweepOrphans`, via the
registry's `/chat-host`): a machine whose chat row is gone — deleted on any device, the client's
follow-up DELETE lost, or a row that never landed — is deleted once its session is 15 minutes old.
A registry it can't read skips the pass; it never deletes on doubt. The finished delete tombstones the session's device row in its user's
registry: no device lists it, and the dial gate (`WorkspaceHost::peer_liveness`) reads a deleted
device as dark, so nothing dials it again.

Idle policy: the runner posts `POST /runner/heartbeat {activeRuns, clients, backgroundRuns,
waitingRuns, diskUsedBytes, diskTotalBytes}` every 60 s, where
`clients` counts interactive relay requests it served since the previous heartbeat — sends, terminal
input, file writes, actions — never requests that only look (lists, reads, searches, watches, status:
`runner::passive`), and never open links or streams, so an app merely displaying a Cloud chat
doesn't keep its machine awake.
The DO alarm puts the device to sleep after `CLOUD_IDLE_MINUTES` (default 10) without activity.
When the provider requires auto-stop (Boat trial: ≤2 h), the DO re-extends the TTL while runs
are active, and a provider-initiated stop is detected and recorded as sleep.

### Machine sizes

The sizes live in one place, `MACHINES` in `edge/src/cloud/policy.ts`, and clients read them from
`CloudStatus.machines`. `diskGiB` is what a size *holds*, the limit a resume is refused against. It
counts what the machine added, not the base image `df` also counts (about 20.5 GiB on Boat,
`SandboxCapabilities.diskOverheadGiB`): a `default` machine reading 62 GiB in `df` fits, and one
reading 72.4 doesn't.

| Size | vCPU | Memory | Holds | Credits | Source |
| --- | --- | --- | --- | --- | --- |
| `small` | 2 | 4 GB | 51.2 GiB | 0.5× | Refused at the same figure as `default` on a shrink |
| `default` | 4 | 8 GB | 51.2 GiB | 1× | Boat's refusal text (its table says 51) |
| `large` | 8 | 16 GB | 111.8 GiB | 2× | Boat's refusal text (its table says 111) |
| `xlarge` | 16 | 32 GB | 251 GiB | 5.56× (50/9) | Boat's table (rounded down) |

A session's size starts as the account's default (`PUT /cloud/{org}/machine`, or the operator's
`POST /admin/cloud/machine`, whichever came last, over `CLOUD_MACHINE`), and only a wake changes
it: a provider changes size only on a resume, so a sandbox already up is stopped first. A size is
always the user's call, and never a machine's: runner bearers can't pick one.

**A wake refused for want of disk.** The provider module turns its refusal into a typed
`SandboxFailure` (`disk_too_small`, with `neededGiB` when it gave a figure); nothing above
`providers/boat.ts` reads Boat's wording. It is never retried, and never upsized on its own, since
credits change. The session reads `error` with `wakeBlocked: {reason: "disk_too_small", neededGiB}`
and a send leaves it alone. A wake on a size smaller than `neededGiB` is refused up front
(`409 machine_too_small`, with `machine`, `neededGiB` and `capacityGiB`); a bigger one clears the
block. A refused *shrink* leaves the machine stopped and wakeable on its own size: the session reads
`sleeping`, with `wakeBlocked` saying what the smaller size couldn't take.

Boat refuses in two ways, both seen on real machines:

- **A resume on the size it's on** is accepted (`202`). The sandbox then reads `error`, with "needs
  51.8 GiB of disk but the largest 'default' machine holds 51.2 GiB" on it. Asking again on the same
  size right away answers `409 resume_failed`. A resume on a bigger type gets it out, so a wake
  resumes a sandbox in the provider's error state rather than giving up on it.
- **A resume to a smaller type** is refused outright: `400 type_too_small`, "uses 77.7 GB of disk,
  which does not fit a 'default' machine (76.0 GB)". That's decimal GB with the base image in, and
  the same machine read "needs 51.8 GiB" above.

**Metering across a resize.** Credits come from the provider's billable seconds, which carry the
multiplier of the size that was running, so nothing here splits a stretch by size. The sandbox row
records the size it is on, which is what the live estimate between meter reads spends at, and a
resize schedules a reconcile. A day's `sandboxType` shows its last size.

### Disk

The heartbeat reports the machine's disk as `df` reads it. The edge takes off the base image and
judges the rest against what the size holds, `warn` from 80% and `critical` from 90%, so a machine
warns before it can no longer fit its size. The heartbeat's reply carries `diskCapacityGiB` for the
engine's cleanup, in `df`'s terms (what the size holds, plus the base image).

**Lazy hydration.** A resumed disk copies its files in on demand, so `df` under-reads until that's
done (seen: 26 GiB right after a resume Boat said needed 118.9). The edge therefore keeps a
high-water mark: a higher reading always counts; a lower one counts only once the disk is all
there. From a wake on, `diskHydrating` holds it. A lower reading asks the provider at most every 5
minutes (`SandboxInfo.hydrated`, Boat's `hydrated`), and the flag clears on `true`, or on a provider
that doesn't say. A refused resume's `neededGiB` also raises the mark, since it's the provider's own
figure.

**Pre-sleep cleanup** (`crates/engine/src/cloud_disk.rs`), once quiesce has accepted the sleep,
budgeted 25 s of the 60 the edge gives it. It deletes from an allow-list: always
Codex's own downloads in the managed `CODEX_HOME` (`cache/`, about 60 MB of plugin catalog, and
`plugins/cache/`, about 30 MB of the account's plugin bundles), which every wake would otherwise copy
back in while the engine starts, and which Codex fetches again itself, in the background, once it
runs (never its `auth.json`, sessions, databases or settings); always `target/*/incremental`; then, only while the disk is still over the warning level and asking again
before each, `target/*/deps` and named package-manager download caches in the home directory. A
`target` counts only if Cargo marked it as its own (`.rustc_info.json` and a `CACHEDIR.TAG` with
Cargo's signature) and git tracks nothing in it, which also rules out anything outside a repository.
Symlinks are never followed. A cache is renamed before it is deleted, so running out of time leaves
nothing half-emptied for a build to trust, and the next sleep finishes it. It touches no login
(`~/.claude`, where a parked Claude child may hold `.oauth_refresh.lock`), and kills no process. A
failure is logged, and the sleep goes on. Right after a wake the disk reads low, so only the
always-safe first tier runs.

### Data expiry

A provider may delete a stopped sandbox's disk (Boat: 14 days after it stopped, reported as
`dataExpiredAt`; `SandboxCapabilities.stoppedRetentionMs`). `CloudSession.expiresAt` says when,
while it sleeps.

**A scheduled resume and stop keeps it.** A resume restarts the provider's clock, so a machine 12
days into its sleep comes up on its own size and goes straight back to sleep (`refreshing`,
ledger `refresh`). Its idle age doesn't change, a send meanwhile keeps it up, a failed attempt goes
back to sleep unseen, and it retries hourly. It never evicts anyone: it waits for a free slot. It's
skipped for a session blocked on disk (the user must pick a size, and `expiresAt` says by when) and
at zero credits.

Cost: machine seconds only, about a minute per machine per 12 days. That's $0.0006 on `default` and
$0.0033 on `xlarge`, billed to the user as usual (about 1 credit on `default`, 6 on `xlarge`). A
resume counts as a start, but starts are a rate limit (200 a day on Boat's $20 plan) before they are
a charge. The alternative, a saved snapshot, is $0.056 a day past the included count ($1.68 for 30
days, 10 included per $20 of plan), and restoring one means a new sandbox and re-enrollment.

**When it's gone anyway** (no credits, a user who didn't act, or the provider's refusal
`snapshot_data_expired`), the session reads `data_expired`, for a client that says it can read that
(`x-zeron-cloud-states: data_expired`). A shipped engine's `CloudState` is a closed enum, and one
state it doesn't know would fail its whole session list, so a client that doesn't say gets `error`
with `failedAction: "wake"` and `dataExpired: true` on every session route. A send or a wake starts
nothing: an empty machine must be asked for. `POST …/retry {machine?}` starts a fresh one: the same session
device, chat, repository and base branch, a new sandbox and a newly enrolled key. The clone checks
out the session's branch from `origin` if it was pushed, so only unpushed work is lost. The
transcript lives in the chat's room, regardless.

### A killed turn

The engine marks a command handled before running it, so the command plane never runs it twice.
What follows a machine dying mid-turn is a resume or a clear failure, never silence:

- **Consumed, never resolved** (killed between the ledger write and the outcome): the drain's dead
  command sweep rejects it, and the sender sees "Not delivered".
- **Running, journal open** (the usual case): boot recovery (`SessionsEngine::recover_stale`) stamps
  the streaming reply and re-dispatches the turn, resuming the provider's session, if it is under
  12 hours old and has revivals left. That happens during assembly, ahead of the credential broker,
  so the run waits for the broker (`HarnessRegistry::expect_run_environment`): without it the agent
  has no login and can't find its own conversation.
- **Running, but it can't resume** (stale, out of revivals, nothing to run it with): the note goes
  on the reply. A kill before the reply reached disk leaves no reply to stamp, so the note gets an
  entry of its own, as does a revival that fails to start.
- **A disk older than the room** (a machine that died can return to its last snapshot): recovery
  sees no open journal. Once the room has caught up, a reply still streaming from this device with
  no run behind it, or a `Run` marked applied that this disk's ledger has never seen, gets a note
  (`DocHost::settle_rolled_back_turn`, Cloud machines only). A message still pending just runs.
- **Nobody around to boot it**: a machine that goes silent with a turn in flight would stay stuck
  until the next send. Once its heartbeats are 3 minutes late and it last reported work, the edge
  stops it and wakes it straight back, so boot recovery runs. A machine the provider lost ends in
  `error`.

### Engine updates on Cloud

A Cloud machine runs a *system* unit, `zeron-cloud.service`, not the user unit installs elsewhere
use. `zeron_update::service_unit()` knows it from `ZERON_DEVICE_PLATFORM=cloud` (written by the
installer on every machine), and `ServiceScope::of(unit)` picks the manager: `sudo -n systemctl` to
act, plain `systemctl` to read.

That only helps an engine new enough to have it. One older than this logs "update available" and
stays where it is (seen on 0.2.114), so a machine provisioned earlier keeps its engine, and everything
that needs a newer one, until something installs it. `zeron update` works from there: it installs and
switches `current`, its restart fails (it looks for the user unit), and the next start runs the new
version.

**A wake updates before it answers** (`crates/engine/src/wake_update.rs`), so a machine that sleeps
most of the time still answers on the latest release. Before it starts any work, including turns a
dying machine left to resume, the engine looks for a release (`manifest.json`, within 5 s). The token
and the rooms go on meanwhile, so with nothing to update the check only costs what it outlasts them
by. With something newer it installs it (120 s), restarts, and the update trial below takes over,
which holds work until the new version is proven. A wake on the latest release, including a first
boot, goes straight on.

The message always gets answered. A check that hangs or fails, a failed or slow download, a refused
restart and a restart that never comes all lift the hold on the version already running, and the
background updater tries again once it's idle. It can't loop: a release it went back from or failed
to install isn't tried again from a boot, and one that merely failed to download waits 6 hours
(`update-boot-skip.json`). A new version that fails its trial gives way to the old one, which finds
the rollback, doesn't try again, and answers. One that hangs gives way after 5 minutes, not the 10 an
update nobody is waiting on gets.

Nothing running is cut off. Nothing starts a turn while work is held, and a terminal someone opened
ends the attempt. Past the wake, an update arriving mid-turn goes to the background path, which
waits until no run or terminal is live and, on Cloud, no message is waiting.

**Only the running version stays.** Each release is a folder of its own (`~/.zeron/app/<version>`,
about 270 MB), and a resume copies the whole disk back in, all at once, while the engine starts:
every old version kept was read in beside the one starting, and on staging a machine three updates
in answered in about 39 s instead of 18 (send to first output, n=8 each), its release check alone
taking 7 to 23 s. So once an update settles (committed, or given up), and on every start after,
`prune_installed_versions` removes the older versions and the one an update gave up on. A Cloud
machine (`ZERON_DEVICE_PLATFORM=cloud`) keeps none of them: going back there is a reinstall, which
downloads anyway. Other Linux installs keep the newest older one, to go back to by hand. Never
`current`, the running executable's folder, or a newer version (an update being staged).

A Cloud machine doesn't keep the release's `zeron-engine` either: it's the desktop app's background
engine (78 MB), and the unit runs `zeron headless`, which links the engine. The installer and the
updater leave it out, and the tidy and the pre-sleep cleanup remove it from versions installed before.
Measured on staging (send to first output, n=8 each), a machine on one version went from a median of
17.5 s to 14.2 s once neither it nor Codex's downloads came back with each wake; its wake downloaded
225 MB, 205 of them the engine it runs.

A longer wake reads as updating: the engine says so (`POST /runner/wake-stage`), `CloudSession.stage`
is `updating` while it lasts (a stage nobody clears stops showing after 10 minutes), the desktop's
waiting word follows it, and the answer's setup steps include "Updated Zeron to X".

### Waking

What a wake is made of, in order, and who owns each part. The target is a send to a running
agent in under 5 s; today's floor is below.

| Stage | Owner | What keeps it short |
| --- | --- | --- |
| Send → nudge → `autoWake` → workflow | us | sender polls its durable job every 100 ms; the rest is a Worker and a DO |
| Resume accepted, machine `ready` | Boat | "a few seconds"; every resume lands on a new machine and is a reboot (no memory, no processes) |
| Engine started | us | `zeron-cloud-boot.service` (active through the snapshot, so it starts on any resume) races the exec; the unit no longer waits on `network-online.target` |
| Binary loads | Boat's lazy disk | `~/.zeron/app` isn't in the base image, so it downloads on every resume; Boat's playbook (`~/.ascii/playbook.json`) fetches what we opened first, first; everything else on the disk downloads at the same time, so old engine versions are removed ("Engine updates on Cloud") |
| First token, room joined | us | connect timeout 3 s and 250 ms retries (a restored machine often loses its first connection) |
| Release check | us | one small request beside the rest of the boot; a newer release is installed first ("Engine updates on Cloud") |
| Command → agent running | us | grants and git identity primed at boot; for 2 minutes after boot no agent CLI check (its install would hold the run) |

The sleep's stop command brings machines provisioned before these units up to date, since
nothing re-runs the installer.

**Measuring.** Every wake writes a `wake_timing` ledger entry (detail: JSON, `WakeTiming` in
`wake.ts`) and a `cloud.wake_timing` log line: when the machine is up, and again once the engine
reports its side (`POST /runner/wake-timing`, once per wake, at the run's first output).
`GET /admin/cloud/wakes?orgId&userId` (`ADMIN_TOKEN`) lists one user's. `at` is each stage in ms
after the wake was requested (`sent` is negative and on the sender's clock); `spans` has the
totals; `asleepMs` says whether a longer idle window would have caught it. A wake with no `sent`
had no send behind it (an explicit wake, or a dark machine brought back to recover its turn); a
refresh writes no timeline. `updateResult` says how its look for a release ended (`current`, `updated`,
`wentBack`, `installFailed`...), `version` which version answered, and `spans.updateStartedToReleased`
what an update added.

### Portability

Boat is one implementation of a provider-neutral `SandboxProvider` interface
(`edge/src/cloud/sandbox-provider.ts`): create (idempotent, with per-sandbox env), get
(normalized state), stop, resume, delete, extend TTL, exec, usage, list. Only
`edge/src/cloud/providers/boat.ts` knows Boat's API; `SANDBOX_PROVIDER` selects the
implementation. Everything above it (workflows, the CloudAccount DO, metering) and everything
inside the sandbox (a POSIX install script, a systemd unit, a `zeron headless` runner configured
purely by `ZERON_*` env) is provider-neutral. Sandbox references are stored as
`{provider, sandboxId}`, so a user can hold sandboxes on two providers during a migration and
billing stays exact.

Optional for a provider: `resume` on another size, `failure` on a refusal a retry can't fix,
`stoppedRetentionMs`, and `SandboxInfo.{stoppedAt, hydrated}`. A provider must guarantee: idempotent create, a disk that persists across stop/resume,
enabled systemd units restarting on resume, per-sandbox env visible to exec, and usage readable
until deletion. Moving users needs no migration step: new sessions start on the new provider,
existing ones keep theirs until deleted; transcripts live in sync rooms and repositories on
GitHub, so only unpushed work in an old session's machine is tied to the old provider.

### Metering

Cloud machine time is billed to Zeron by Boat and will be charged on to users, so every
second is attributed to a user and reconciled against Boat's own meter rather than estimated
from our lifecycle events.

- **Authoritative numbers** come from Boat's `GET /sandboxes/{id}/usage?since&until`
  (billable seconds with the machine-type multiplier applied, stopped time excluded, period
  boundaries split pro rata).
- The CloudAccount DO keeps the user's **sandbox history** (one sandbox per session, each
  tagged with its chat), an append-only **lifecycle ledger** (create / ready / wake / sleep / stop /
  delete / TTL extensions / errors — the audit trail that explains a bill), and **monthly
  usage rows** per sandbox.
- Rows are reconciled hourly while anything is unclosed and on every lifecycle transition.
  After a UTC month ends, one last reconciliation bounded at the month end marks the row
  `closed`; closed rows never change. Explicit deletion captures final usage *before* the
  sandbox is deleted. Reconciliation failures never block lifecycle actions.
- A `CloudIndex` DO lists every Cloud user so an hourly cron can reconcile all of them, and a
  daily scan of Boat's sandbox list reports **orphans** (sandboxes no user owns, usually a
  crashed provision) — those are Zeron's cost, not a user's.
- Rows are per **UTC day** and sandbox (a month is the sum of its days), so users see credits
  per day: Settings → Cloud → Usage, a range (last 7/30/90 days, this or last month) and a bar
  per day (`GET /cloud/{orgId}/usage?from&to`). Operators bill from
  `GET /admin/cloud/usage?month=YYYY-MM[&format=csv]` (`ADMIN_TOKEN`), and each closed month
  is written to R2 at `usage/{YYYY-MM}.json`.
- **Credits.** One credit is one minute of a small machine: 30 of Boat's billable seconds
  (which already carry the machine-size multiplier, so a minute costs 2 credits on a standard
  machine, 4 on large and about 11 on xlarge). Balances are worked out from the raw seconds, so
  the rate goes by the UTC day the usage fell on: days before `CLOUD_CREDIT_RATE_FROM` stay at 60
  seconds a credit, and from that day on it's `CLOUD_CREDIT_SECONDS`. Without a real day in
  `CLOUD_CREDIT_RATE_FROM`, every day stays at 60. Set it to a whole UTC day after the deploy, and
  don't move it once that day has come: that would re-price the days in between.
  The balance is credits granted (a one-time `CLOUD_STARTING_CREDITS` on first enable; operator
  grants via `POST /admin/cloud/credits {orgId, userId, credits, reason}`) minus credits used,
  less awake machines' time since their meters were last read. At zero or below, new sessions
  and wakes are refused (`402 no_credits`), the composer holds Cloud sends, and the account's
  reconcile — every 5 minutes while a machine is awake (awake machines only; everything
  hourly) — puts awake machines to sleep. `CloudStatus.credits` carries the balance.

## Credential vault — `zeron-vault` Worker

A separate Worker (`edge/vault/`), with no routes, no `workers_dev`, and a minimal
dependency list (none at runtime; WebCrypto only). The only way in is the `VAULT` service
binding from the main Worker, which passes the **verified** identity. It has its own deploy job and CODEOWNERS entry.

### Durable Objects

- **VaultDevices** `dev1/{userId}` — enrolled device keys `{deviceId, kind: laptop|cloud,
  publicKey, enrolledAt, revokedAt?}`, an append-only audit log, and the per-user disable flag.
- **VaultAccount** `acct1/{userId}/{provider}` — the provider's accounts (up to 20 encrypted
  credentials, one row per slot), each with its `authorizedDevices`, `generation`, `status`
  (`connected` · `needs_reconnect`) and refresh bookkeeping, and which one is **active**.
  **Single-threaded per id, and refreshes are single-flighted per slot, so at most one refresh
  is ever in flight per account.** That is the property that fixes refresh-token races.

### Multiple accounts

A provider holds several accounts. Agent logins and API keys work this way (GitHub's are all in
use at once: see "GitHub"): grants always come from the active one, and every Cloud
machine uses it from its next turn (the engine re-checks a cached grant older than 30 s before
a run). Uploading the same login again replaces its own account instead of adding one —
identity is never secret material: Claude's profile `email|organization` (sent beside the
blob), Codex's `id_token` email + `account_id`, an API key's key id (a hash), GitHub's login. A
new upload becomes active. Forgetting the active account leaves none active (grants answer
`no {provider} account is active`); the generation counter is per provider, so AADs stay
unique across accounts. The daily canary refreshes every account. A pre-accounts single
`record` row migrates into one active account with the same generation and envelope.

Settings → Providers shows Cloud's Claude Code and Codex cards like any device's: each card's
accounts are its subscription logins plus its API keys (`anthropic-key` / `openai-key`), one of
them active. The engine answers account calls aimed at `targetDeviceId: "cloud"` from the vault
(`cloud_accounts`); activating a key clears the active subscription login, since machines
prefer a subscription. Usage meters come from the vault, which alone holds the tokens: it calls
the provider's usage endpoint (Claude `api.anthropic.com/api/oauth/usage`, Codex
`chatgpt.com/backend-api/wham/usage`) and returns the body for the engine to parse; a 401/403
there never marks the account.

### Encryption

Records are AES-256-GCM encrypted under a KEK held as a secret on the vault Worker
(`VAULT_KEK`, 32 random bytes). Worker secrets are stored apart from Durable Object storage, so
a leaked storage dump alone decrypts nothing; a compromise of the Cloudflare account itself
could reach both (the trade-off for not running a separate key service). AAD =
`{userId}|{provider}|{generation}` binds a ciphertext to its slot and generation.

Envelopes carry the id of the key that sealed them (`v2.{kid}.{iv}.{ct}`, `kid` = a SHA-256
prefix of the key). Rotation: set a new `VAULT_KEK` and move the old one to
`VAULT_KEK_PREVIOUS`; records sealed under the previous key are re-sealed under the new one the
next time they are opened. Removing `VAULT_KEK_PREVIOUS` makes any record not yet migrated
unreadable (that user reconnects), which doubles as the emergency response to a leaked key.
The key source is pluggable (`KekProvider`), so an external key manager can replace the
secret later without touching records' format.

### Providers (v1)

| Provider id | Material | Refresh |
| --- | --- | --- |
| `codex` | ChatGPT OAuth token set from `codex login` (`id_token`, `access_token`, `refresh_token`, `account_id`) | `POST https://auth.openai.com/oauth/token` (`grant_type=refresh_token`, Codex public client id) |
| `claude` | Claude Code credential blob from a sign-in through Claude's own OAuth flow (`claudeAiOauth`), or a long-lived `claude setup-token` token | `POST https://platform.claude.com/v1/oauth/token` (`grant_type=refresh_token`, Claude Code client id); setup tokens never refresh |
| `github` | GitHub App user token + refresh token (never granted; see "GitHub") | `POST https://github.com/login/oauth/access_token` with `GITHUB_APP_CLIENT_ID/SECRET` (vault secrets) |
| `anthropic-key` | User's own Anthropic API key | none (static) |
| `openai-key` | User's own OpenAI API key | none (static) |

**Claude subscriptions.** Anthropic's published Claude Code terms restrict third parties from
storing or intermediating Claude.ai credentials "unless we've mutually agreed otherwise".
Zeron holds Claude logins in the vault under its arrangement with Anthropic for running
Claude Code on users' Cloud devices (confirmed by Wing, 2026-10-01; keep the written
confirmation on file next to this doc). Scope it to that purpose: Claude grants are delivered
only to the unmodified Claude Code CLI on the user's own devices, never to another harness or
SDK, and sign-in always completes through Claude's own OAuth flow. Alternatives that need no
vault: the CLI's own `claude auth login` inside the Cloud device, or the user's own Anthropic
API key (`anthropic-key`).

### Sign-in stays on the device

- **Codex**: the laptop runs `codex login` into a throwaway `CODEX_HOME` (the existing account
  flow), reads `auth.json`, uploads it, and deletes the temp home. The laptop's own login is
  untouched, so there is exactly one refresher (the vault) for the uploaded grant.
- **Claude**: the laptop runs the existing Claude sign-in (Claude's own OAuth flow) in a
  capture-only mode that never activates the result locally, and uploads the credential blob.
  Again one refresher.
- **GitHub**: device flow. The laptop asks the edge to start it; the user approves on
  github.com; the *vault* polls GitHub and stores the result, so the token never transits a
  device. Separately, the user installs the App on the accounts / repositories Cloud may reach
  (`githubInstallUrl` in the vault status → `github.com/apps/{slug}/installations/new`).
- **API keys**: pasted on the laptop, uploaded once.

The vault only refreshes. It owns each refresh token from upload on.

### Grants

`POST /vault/{orgId}/grant {provider, deviceId, ts, sig}` (sig by the device key, same scheme
as runner tokens with prefix `zeron-vault-grant`) → `{provider, accessToken, expiresAt,
accountId?, generation}`. A grant requires: the device is enrolled and not revoked, the device
is in the credential's `authorizedDevices`, the user is not disabled, and the global kill switch
(`VAULT_DISABLED=1`) is off. Grants are the provider's own access token, refreshed by the vault
when within 10 minutes of expiry (Codex: when `last_refresh` is older than 7 days or the JWT has
<1 h left). The consumer caches a grant until expiry (offline fallback) and never sees a
refresh token.

Failure semantics: a refresh that returns `invalid_grant` marks the account `needs_reconnect`
and stops issuing grants. An ambiguous failure (timeout/5xx) keeps the current generation and
retries on the next grant; the DO persists a new generation before returning it.

### GitHub

The standard GitHub App model: sessions act as the App, never as the user.

- Each **user token** (device flow) stays in the vault. It lists the user's repositories for
  the device menu's Cloud row and proves which App installations the user can reach.
- A `github` grant names the session repository (`repo: "owner/name"`, required). The vault
  lists `/user/installations` with the user token, picks the installation on `owner` (not
  suspended), reads the repository with the user token, and mints `POST
  /app/installations/{id}/access_tokens` with an RS256 App JWT (`GITHUB_APP_PRIVATE_KEY`, `iss` =
  client id): a one-hour token for that one repository, read-only when the login can't push.
  Minted tokens are cached sealed per repository and access level and reused while more than 15
  minutes remain; both checks run on every grant, so a user who loses access stops getting tokens
  at once.
- Because installations are found through the user's own token, a caller can never name an
  installation it doesn't belong to (the GitHub-documented check; an `installation_id` from a
  setup redirect is never trusted).
- No installation on `owner` → `not_found` with a `github_app_not_installed:` message carrying
  the install link, aimed at `owner` when GitHub names its id. A 401 on the lookup →
  `needs_reconnect`. A 401/403 on minting is our App's credentials, never the user's:
  `unavailable`, the connection stays connected.
- Commits are the session's login's (its name and GitHub noreply address, in the run's
  environment), co-authored by `zeronsh[bot]`; pushes and pull requests show as the App. The
  daily canary also checks the App credentials (`GET /app`).

#### Several logins

A user connects up to 10 GitHub logins, each through its own device flow (a work and a personal
one, say). They are all in use at once; connecting never removes one. The same login again
replaces itself (`alreadyConnected` in the poll: GitHub approves as whoever the browser is signed
in as, so the app says to switch accounts there).

**Logins and installations are separate records.** A login is a person's identity: who a session
acts for, who authors its commits, what they may reach. An installation of the App is repository
access, and belongs to no login. The github `VaultAccount` keeps `accounts` (logins),
`github_installations`, and `github_login_installations` (which login reaches which), and each
session's `github_sessions` row records both its login and its installation. Reply shapes keep
them apart too: a repository names its `installationId` and the logins that reach it (`account`,
`accounts`), and `GET github/repos` lists `installations` with `reachedBy`. Nothing requires an
installation to be reached through one person, which is what leaves room for an organization to
hold them later. (Nothing shares them yet: all of this is per user.)

Rules:

- **Repositories** merge across logins, deduplicated by name; each names the login a new session
  would use. One login failing leaves the others' repositories and appears in `unavailable`
  (`needs_reconnect` or `upstream`); the listing fails only when no login answered.
- **A new session** takes the first login that can push, else the first that can read, in this
  order: the active one (the most recently connected, unless the user picked another with
  `POST accounts/github/active`), then the others, most recently connected first. A login that
  can push always beats one that can only read. A read-only pick isn't kept while another login
  couldn't answer, since that one might push.
- **A session keeps its login and installation** from its first grant, whatever is connected or
  chosen later. The vault holds this per device and repository, so machines need no change and a
  side chat on the same machine shares it. A delegate
  (`create_child_session`) takes its parent's: the main Worker names the parent's device
  (`inheritFrom`, runners only, never from a request's body) at its first grant.
- **A dead login** (`needs_reconnect`) refuses its sessions' grants by name
  (`github_account_needs_reconnect:`). They never move to another login that reaches the
  repository, since that would change who authors the commits. Refreshes are single-flighted per
  login, so one login's rotation can't touch another's.
- **Disconnecting one** (`DELETE accounts/github/{slot}`) deletes its token and revokes its
  tokens with GitHub at once: its user token (`DELETE /applications/{client_id}/token`, with the
  App's client id and secret) and every installation token minted for its sessions
  (`DELETE /installation/token`, as the token itself), so a token already on a machine stops
  working mid-turn. Installation tokens are recorded with the login they were minted for
  (`github_tokens`, kept until they expire) and never shared between logins, so this never
  touches another login's sessions. The disconnect succeeds whether or not GitHub answers: what
  it didn't revoke waits in `github_revocations` and is retried on the object's alarm (1, 2,
  4… minutes, at most an hour apart) until GitHub answers, or until the token would have expired
  anyway. A token GitHub no longer knows counts as revoked. One minted while its login was being
  disconnected is revoked and never handed out. The sessions stay on the login: the next grant
  refuses with `github_account_disconnected:` and how to reconnect, and the engine ends the run
  there saying so, removing the git credentials. The same login connected again resumes them, as
  sessions hold its identity rather than its row. New sessions use whichever logins are left.
  `DELETE credentials/github` still wipes every login, revoking all of theirs.
- **A session's login that lost the repository** refuses with `github_account_no_access:`, again
  without moving.
- **Installing the App**: `githubInstallUrl` is GitHub's chooser for the browser's account; a
  repository whose owner has no installation links straight to that owner
  (`/installations/new/permissions?suggested_target_id=`). `installations[].manageUrl` is where an
  installation's repositories are chosen.

A login stored before identities existed takes one from its name when the object starts, and rows
that doubled up that way merge, keeping the newest. A login GitHub wouldn't name at connect time
learns its name at first use, its sessions with it. Old clients read the same shapes with fields
added, and an old engine's "Disconnect" still means all of GitHub.

### Admin

| Route (WorkOS bearer only) | Effect |
| --- | --- |
| `GET /vault/{orgId}` | Providers `{provider, status, authorizedDevices, updatedAt, account?}` + devices |
| `PUT /vault/{orgId}/credentials/{provider}` | Upload `{material, authorizedDevices}` |
| `PATCH /vault/{orgId}/credentials/{provider}` | Change `authorizedDevices` |
| `DELETE /vault/{orgId}/credentials/{provider}` | Disconnect (wipe every account) |
| `GET /vault/{orgId}/accounts/{provider}` | `{accounts}`: slot, status, active, kind, identity fields, devices |
| `POST /vault/{orgId}/accounts/{provider}/active` | `{slot \| null}` → pick the active account (null: none) |
| `DELETE /vault/{orgId}/accounts/{provider}/{slot}` | Forget one account |
| `GET /vault/{orgId}/accounts/{provider}/{slot}/usage` | `{body, fetchedAt}`: the provider's usage reply |
| `POST /vault/{orgId}/devices/enroll` | Enroll the caller's device key (laptops) |
| `POST /vault/{orgId}/devices/{deviceId}/revoke` | Per-device revoke |
| `POST /vault/{orgId}/github/device` | Start GitHub device flow → `{userCode, verificationUri, interval, flowId}` |
| `POST /vault/{orgId}/github/device/{flowId}` | Poll; on success the vault stores the token (`alreadyConnected` when that login was connected) |
| `GET /vault/{orgId}/github/repos?q=` | `{repos, installations, unavailable?}`: every login's repositories, merged |
| `GET /vault/{orgId}/github/branches?repo=` | Branch names, through the first login that sees the repository |

Kill switches: per device (revoke), per user (`POST /vault/{orgId}/disable`), and global
(`VAULT_DISABLED` var; or drop a leaked key via rotation).

### Daily canary

A cron trigger (`0 6 * * *`) refreshes one canary credential per refreshable provider (codex,
claude, github — the connections of the dedicated Zeron account named by `CANARY_USER_ID`) and
checks the GitHub App's own credentials, records `{provider, ok, status, at}` in a `canary` table and logs a
structured `vault.canary` line (Workers observability alert on `ok=false`). A provider whose
refresh contract changes is caught within a day, before users' grants start failing.

## Engine changes

- **Runner mode** (`ZERON_RUNNER_ENROLL` or an existing `{data_dir}/runner.json`): `Auth`
  gains a runner token source (enroll once, then signed `/runner/token`); scope is `Synced`
  for the runner's `{orgId, userId}`. Platform comes from `ZERON_DEVICE_PLATFORM` (`cloud`).
  The runner posts heartbeats and restricts its harness catalog to Codex + Claude Code.
- **Session machines** (`ZERON_CLOUD_*`, persisted to `cloud-session.json`): the device row
  carries `cloud-session`; at boot the engine clones the project into `ZERON_CLOUD_PATH` (with
  the GitHub grant's credential store; retried with backoff) and checks out
  `zeron/cloud-{12 alphanumerics of the chat id}` — one branch per session, so neighbours on one
  repository never push the same branch. A run dispatched meanwhile starts its harness only
  once the checkout is ready. A wake or restart keeps the existing checkout. A Cloud machine
  writes no viewport rows (sidebar pins) and no legacy diff sidecar. Every boot also schedules
  a reconcile of the chats it hosts (`DocHost::enqueue_wakeup("*")`): a machine stopped without
  warning (provider auto-stop, crash) after accepting a send still finds the message in its
  chat's room.
- **Setup steps, live**: the boot's steps ("Started a machine" / "Woke the machine", "Cloning
  owner/repo" → "Cloned owner/repo", "Checking out zeron/cloud-…") open the first answer after
  it as tool chips (`ToolCall::Unknown` tagged `cloudSetup`; the start/wake chip says how long
  the message waited), each spinning until its step finishes, grouped as "Set up the Cloud
  machine" / "Woke the Cloud machine". Before the machine can write anything, the sender shows
  the first chip itself ("Starting a Cloud machine" / "Waking the Cloud machine", an optimistic
  echo); the machine's entry (`{messageId}.cloud-setup`, chip `{messageId}.cloud-machine`)
  replaces it in place. The send is not "Not delivered" while the session is provisioning,
  starting or asleep.
- **No keychain on Cloud**: a Cloud device never calls macOS `security` (Providers-page
  account code), and git's helper list for github.com starts with an empty entry, so no system
  helper (Apple git's `osxkeychain`, libsecret) also stores the App token.
- **No login, no run**: a Cloud run of Codex or Claude Code with no credential at all (no
  vault grant, no API key, no login of the sandbox's own) fails at once with where to sign in,
  instead of the CLI retrying unauthenticated.
- **Credential broker** (runner only): before a harness spawn, the host resolves credentials:
  Codex from a vault grant (managed `CODEX_HOME` with the access token, empty refresh token;
  re-granted before expiry), Claude from a vault grant (Claude Code's credential file with the
  access token and an empty refresh token, rewritten before expiry), else the native sandbox
  login, else `anthropic-key`. GitHub grants (App installation tokens for the session
  repository, requested with `repo`) are kept fresh in a git credential store file (`{data_dir}/cloud/git-credentials`, 0600,
  wired as git's `credential.https://github.com.helper`) and `gh`'s `hosts.yml`, so `git`
  keeps working in sessions that outlive the spawn-time `GH_TOKEN`. Secrets ride a
  `#[serde(skip)]` field on `RunRequest`, so they are never persisted in the session doc, the
  journal or logs. On platform `cloud` only Codex and Claude Code are registered: nothing else
  is probed, update-checked or installable (some providers' images install a harness the first
  time its binary is invoked).
- **Projects' GitHub repositories**: SpacesSync stamps `githubRepo` (the `origin` remote's
  `owner/name`) on the spaces this device owns, with `gitDetected`. `ListGithubRepos` lists the
  repositories Cloud reaches through the edge (the vault calls GitHub with the user token:
  installed repositories only; a session machine lists its installation token's
  repositories); the UI keeps that list and offers Cloud for a project whose `githubRepo` is in
  it. `CloneRepo` on a session machine is credentialed and idempotent for an existing clone of
  the same remote.
- **Chats run on Cloud**: `createChat {cloud: true}` asks the edge for a session (`POST
  /cloud/{org}/sessions {chatId, spaceId, repo {fullName, cloneUrl, defaultBranch}, branch?}`,
  idempotent per chat; `branch` = the base branch picked in the composer, else the repository's
  default) and writes the chat with the returned session device as host and its checkout as
  cwd (the reply carries both). It refuses a project without a GitHub origin or whose
  repository Cloud doesn't reach. The edge claims the session device's room for the user when it
  mints the device, so the first send's nudge queues while the machine boots instead of
  404ing. `ListCloudBranches {repo, defaultBranch?}` lists the branches a session can start from
  (GitHub's, default first; `GET /vault/{org}/github/branches?repo=`). A side chat
  (`parentChatId`) whose parent runs on a session machine is hosted on that same machine. `ListHarnesses` / `ListModels` aimed at any Cloud
  device are answered by the asking engine (Codex + Claude Code), and anything else aimed at the
  logical device fails fast — it has no engine.
- **Laptop side**: `CloudStatus` / `CloudEnable` / `CloudDelete` / `CloudUsage`,
  `CloudSessions` / `CloudSessionWake` / `CloudSessionSleep` / `CloudSessionDelete`,
  `ListCloudBranches`, `VaultStatus`, `VaultConnectCodex` / `VaultConnectClaude`, `VaultPutApiKey`,
  `VaultAuthorize`, `VaultDisconnect`, `VaultRevokeDevice`, `GithubConnectStart` /
  `GithubConnectPoll` — RPCs that call the edge with the user's bearer and are never routed by
  `targetDeviceId`. The two sign-ins reply `AgentLoginStart` immediately and are driven with
  the existing `PollAgentLogin` / `CompleteAgentLogin` / `CancelAgentLogin`. The account calls
  (`ListAgentAccounts`, `ActivateAgentAccount`, `ForgetAgentAccount`, `StartAgentLogin` and the
  login trio) aimed at `targetDeviceId: "cloud"` are answered from the vault the same way.

## HTTP contract (main Worker)

JSON bodies; errors are `{error: <code>, message}` with the HTTP status. `{orgId}` must equal
the bearer's `org_id`.

| Route | Bearer | Body → reply |
| --- | --- | --- |
| `GET /cloud/{orgId}` | user | → `CloudStatus` (account), with `machines: [{id, name, vcpu, memoryGB, diskGiB, creditMultiplier}]` and `machine` (the default) |
| `POST /cloud/{orgId}/enable` | user | `{}` → `CloudStatus` (instant; mints the logical device id) |
| `DELETE /cloud/{orgId}` | user | → `CloudStatus` (`deleting` → `off`; deletes every session sandbox after capturing usage) |
| `GET /cloud/{orgId}/sessions` | user | → `{sessions: CloudSession[]}`, each with `machine`, and when known `disk: {usedGiB, capacityGiB, level}`, `wakeBlocked: {reason, neededGiB}`, `expiresAt` (Unix ms) |
| `POST /cloud/{orgId}/sessions` | user | `{chatId, spaceId, repo {fullName, cloneUrl, defaultBranch}, branch?}` → `CloudSession` (idempotent per chat; starts provisioning) |
| `PUT /cloud/{orgId}/machine` | user | `{machine}` → `CloudStatus` (the size new sessions start on) |
| `POST /cloud/{orgId}/sessions/{chatId}/wake` | user | `{machine?, wake?}` → `CloudSession`. `wake: false` only records the size for its next wake. `409`: `machine_too_small` {`machine`, `neededGiB`, `capacityGiB`}, `machine_awake`, `data_expired` |
| `POST /cloud/{orgId}/sessions/{chatId}/sleep` | user | → `CloudSession` |
| `POST /cloud/{orgId}/sessions/{chatId}/retry` | user | `{machine?}` → `CloudSession`: a failed lifecycle again; a fresh machine once `data_expired` |
| any session route | | `x-zeron-cloud-states: data_expired` lets a reply say `state: "data_expired"`; without it, that session reads `state: "error", failedAction: "wake", dataExpired: true` |
| `DELETE /cloud/{orgId}/sessions/{chatId}` | user | → `CloudSession` (`deleting` → removed) |
| `GET /vault/{orgId}/github/repos?q=` | user | → `{repos: GithubRepo[]}` (the vault calls GitHub) |
| `POST /runner/enroll` | none | `{orgId, userId, deviceId, code, publicKey}` → `{ok: true}` |
| `POST /runner/token` | none | `{orgId, userId, deviceId, ts, sig}` → `{accessToken, expiresAt}` |
| `POST /runner/wake-timing` | runner | `{sent?, token?, firstOutput?, version?, updateResult?, …}` (`EngineTiming`, `wake.ts`) → `{ok: true}`: its side of the wake, counted once |
| `POST /runner/wake-stage` | runner | `{stage: "updating" \| null}` → `{ok: true}`: shown as `CloudSession.stage` |
| `POST /runner/heartbeat` | runner | `{activeRuns, clients, backgroundRuns?, waitingRuns?, diskUsedBytes?, diskTotalBytes?}` → `{ok: true, diskCapacityGiB?}` (per session device) |
| `GET /vault/{orgId}` | user | → `VaultStatusView` |
| `PUT /vault/{orgId}/credentials/{provider}` | user | `PutCredentialRequest` → `VaultStatusView` |
| `PATCH /vault/{orgId}/credentials/{provider}` | user | `{authorizedDevices}` → `VaultStatusView` |
| `DELETE /vault/{orgId}/credentials/{provider}` | user | → `VaultStatusView` |
| `POST /vault/{orgId}/devices/enroll` | user | `EnrollDeviceRequest` → `VaultStatusView` |
| `POST /vault/{orgId}/devices/{deviceId}/revoke` | user | → `VaultStatusView` |
| `POST /vault/{orgId}/disable` | user | `{disabled}` → `VaultStatusView` |
| `POST /vault/{orgId}/grant` | user or runner | `GrantRequest` → `Grant` |
| `POST /vault/{orgId}/github/device` | user | `{authorizedDevices}` → `GithubDeviceStart` |
| `POST /vault/{orgId}/github/device/{flowId}` | user | → `GithubDevicePoll` |

Types are in `edge/vault/src/api.ts` (TypeScript) and `crates/proto/src/cloud.rs` (Rust).
Signatures are base64url (no padding) Ed25519 over UTF-8:

- runner token: `zeron-runner-token\n{orgId}\n{userId}\n{deviceId}\n{ts}`
- vault grant: `zeron-vault-grant\n{userId}\n{deviceId}\n{provider}\n{ts}`

`ZERON_RUNNER_ENROLL` = `{orgId}.{userId}.{deviceId}.{code}` (`code` is 32 random bytes,
base64url). After enrollment the engine persists `{data_dir}/runner.json`
(`{orgId, userId, deviceId, edgeUrl}`) and `{data_dir}/runner-key` (PKCS#8 Ed25519, 0600);
`runner.json` wins over the env var from then on.

### Metering

| Route | Auth | Reply |
| --- | --- | --- |
| `GET /cloud/{orgId}/usage?from=YYYY-MM-DD&to=YYYY-MM-DD` | user (own usage only; runner refused); default the last 30 days, at most 366 | `CloudUsage`: `{from, to, days: [{day, credits}], credits, balance, available: true}` — every day of the range |
| `POST /admin/cloud/credits` | `Authorization: Bearer $ADMIN_TOKEN` | `{orgId, userId, credits, reason}` (negative corrects) → `{balance}` |
| `GET /admin/cloud/usage?month=YYYY-MM[&format=csv]` | `Authorization: Bearer $ADMIN_TOKEN` (header only; constant-time compare; the route is `404` when the secret is unset) | `{month, generatedAt, closed, totals: {seconds, dollars, users, orphans}, users: [{orgId, userId, deviceId, month, seconds, dollars, closed, errors, sandboxes}], orphans: [{provider, sandboxId, state, firstSeenAt, lastSeenAt}]}`; CSV: `kind,month,orgId,userId,deviceId,provider,sandboxId,sandboxType,seconds,dollars,running,reconciledAt,closed,note`, one line per (user, sandbox) and per orphan |

`seconds` are the provider's billable seconds (machine-size multiplier applied), `dollars` its
list price, `reconciledAt` Unix ms. A month is `closed` once it has ended (plus a one-hour
grace for the provider's meter) and every day of every sandbox that existed in it has a closed row; closed
figures never change and are the billable ones. Each closed month is also written once to R2
`BLOBS` at `usage/{YYYY-MM}.json` (same JSON as the admin export) by the hourly cron
(`17 * * * *`), which also pokes every user's reconcile and, at 00:17 UTC, scans each
provider for orphan sandboxes (logged as `{"event":"cloud.orphan", provider, sandboxId, state}`).

## Prerequisites before real users store tokens

1. **Localhost IPC (P1, local-process vector).** The engine's IPC port accepted any local
   process. It now requires a per-install bearer (`{data_dir}/ipc-token`, 0600, rotated on each
   engine start) on the WebSocket handshake. Viewports, the CLI and `zeron mcp` read it at
   dial time; the engine passes the file's path (`ZERON_IPC_TOKEN_FILE`, never the token —
   Claude gets MCP env on its argv) to the MCP servers it injects. See `docs/mcp.md`.
2. **Claude injected-token expiry probe.** The Cloud engine writes vault-granted Claude access
   tokens (never refresh tokens) into Claude Code's credential file and rewrites it before
   expiry. Confirm with `scripts/probe-claude-injected-token.sh` that a long-running Claude
   Code session picks up the rewritten file after its injected token expires (and the
   `CLAUDE_CODE_OAUTH_TOKEN` variant), before any real user's login is stored.
3. Vault infrastructure secrets set (`VAULT_KEK`, GitHub App), canary
   credentials stored, and the vault deploy job approved.

## Operations

| Secret / var | Where | Purpose |
| --- | --- | --- |
| `BOAT_API_KEY` | main Worker | Boat API. Scoped key with `sandbox.create`, `.read`, `.update`, `.stop`, `.resume`, `.delete` and `exec` |
| `SANDBOX_PROVIDER` | main Worker var | provider for new sandboxes (`boat`; `fake` for local dev) |
| `CLOUD_TTL_SECONDS` | main Worker var | unset on a paid plan (no auto-stop); `7200` on a Boat trial |
| `CLOUD_IDLE_MINUTES` | main Worker var | idle sleep threshold (default 10) |
| `RUNNER_JWT_PRIVATE_KEY` / `RUNNER_JWT_PUBLIC_KEY` | main Worker | ES256 runner tokens (JWK JSON; add a `kid` to rotate) |
| `ADMIN_TOKEN` | main Worker | operator usage export (`/admin/cloud/usage`); route 404s when unset |
| `VAULT_KEK` / `VAULT_KEK_PREVIOUS` | vault | record-encryption key (base64, 32 bytes) and the one being rotated out; keep an offline backup — secrets can't be read back |
| `GITHUB_APP_CLIENT_ID` / `GITHUB_APP_CLIENT_SECRET` | vault | GitHub App user tokens (device flow enabled on the app); the client id is also the App JWT's `iss` |
| `GITHUB_APP_PRIVATE_KEY` | vault | the App's private key (PEM as downloaded): signs App JWTs that mint installation tokens |
| `GITHUB_APP_SLUG` | vault | `github.com/apps/{slug}` — the install link in Settings and the project picker |
| `CANARY_USER_ID` | vault | the dedicated account whose connections the daily canary refreshes |
| `VAULT_DISABLED` | vault | `1` = global kill switch |

Deploy order: the vault Worker before the main Worker (service binding); `deploy.yml` gates the
vault job on the `vault-production` GitHub environment (required reviewers), and the Cloudflare
API token also needs Workflows edit permission.
