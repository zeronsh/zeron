# Remote sends, cold hosts, and mobile suspension

Investigation base: upstream `9e1a1115` (2026-10-03).

## Report and probable cause

The report describes a mobile send that looks active immediately, a new chat
appearing on the desktop before execution starts, a delay of roughly 5–20+
seconds, and occasional failures when the iOS app is closed before the first
output. It was observed mainly with Codex, also with Cursor and Devin; Windows
was suspected as the host platform.

The inspected code contains a delivery failure mechanism matching these
symptoms. This is not proof that every reported delay has this cause: no logs
from the reporting devices were available, and provider startup can contribute
additional latency.

Before the fix, a remote send had three independently scheduled effects:

1. The workspace registry publishes the new chat and its host/configuration.
2. The session document publishes the Run/Steer command or queued message.
3. A POST to the host's device room durably tells the host to discover that
   document. The edge persists the nudge; the host persists its own discovery
   receipt before acknowledging it.

Seeing the chat in the registry proves only the first effect. A chat2 row ACK
proves that the edge stored the bytes, not that a cold host opened the room or
that a provider began executing.

Before this fix:

- `ClientInner::nudge_host` waited up to 50 × 100 ms for **all** pending registry
  writes before attempting the wake. Unrelated writes could delay an existing
  chat too. Hosts already support the wake-before-CreateChat race: missing rows
  default to chat2, and wake admission is durable.
- Only HTTP 503 received retries (three total attempts). Network errors, most
  server errors, and token retrieval failures silently ended the wake task.
- The phone's wake task had no persistent receipt. Suspension or termination
  before its request completed could strand a command whose row was already
  durably stored on the edge.
- Thin-client relaunch restored the workspace but started chat rooms only when
  opened or preloaded. An unopened chat outbox did not resume automatically.
- The session and workspace derived Working from an unadopted Sending command.
  Thus the UI could imply provider activity before any host acknowledgement.
- The desktop sender also posted a best-effort wake, without recovery after an
  error or restart. Its transcript already distinguished Sending from a turn.

There is also a distinct host-owned startup phase. `SessionsEngine::dispatch_inner`
records a run and publishes Working before `drive_run` waits for a harness
execution/update lease and calls `harness.run`. That initialization can delay the
first output, but the task belongs to the host and does not depend on an open
viewer. Working therefore means the host owns the turn, not that the provider
has already emitted its first token. The fix does not promise to remove provider
boot, model discovery, installation, or network latency.

These paths are outside provider-specific harnesses and host-platform branches.
The failure mechanism can affect iOS/iPadOS and other frontends using
`zeron-client`, with Linux, macOS, or Windows execution hosts, and desktop-to-
desktop remote sends using the engine. The tests vary host platform metadata
and RunRequest harness selection; they are not native Windows/macOS/iOS runs.

## Fix and delivery boundary

Thin-client session writes first persist the registry row needed to recover the
chat. A versioned `viewer-delivery` job records the outstanding host wake.
Discovery pages the union of that job set and the existing chat outbox, resumes
rooms without a UI open, and retries wake requests with fresh credentials and
bounded backoff. Opening an older persisted unadopted send also re-arms a wake.

Desktop remote sends record an independent `remote-wake` job. The engine resumes
those jobs after workspace restoration while its existing sync scheduler
continues to recover outgoing document rows.

Both senders may wake immediately, but retire a receipt only after:

1. outgoing document rows were acknowledged **before** the final wake request;
2. the edge accepted that wake with a successful HTTP response;
3. the outbox remains empty and the captured job version still matches.

A wake sent before publication can catch up to an empty room. Requiring a final
post-ACK wake closes that ordering race. Conditional completion prevents a slow
old request from erasing a newer send. No command is re-minted: existing command
IDs, Loro imports, and the host's processed-command ledger retain deduplication.
Deletion removes delivery receipts with the chat's local snapshot/outbox.

The updated senders also carry the selected host's `hostDevice` routing hint on
both WebSocket joins and HTTPS pushes. The chat room commits the opaque row,
a versioned host-wake receipt, and the next alarm **in one SQLite storage
transaction before ACKing the row** (the [SQLite storage transaction API](https://developers.cloudflare.com/durable-objects/api/sqlite-storage-api/#transaction) includes SQL and alarm operations).
It immediately forwards the wake to the existing device-room queue. Failed forwards (including unclaimed/offline
hosts, queue saturation, server errors, and timeouts) retain the receipt and
retry from durable alarms with backoff capped at one minute. A newer row fences
completion of an older forward. Host-authored rows do not create wake loops.

Consequently, once all command bytes have reached the updated edge, host
discovery no longer requires the phone to stay alive, send a separate wake,
or reopen. The chat room owns the handoff until the device room durably accepts
it; the device room then owns delivery to the host. The original local receipts
remain as recovery and compatibility paths for older edge deployments.

The Worker supplies the canonical chat ID from the authenticated route, and
forwards the verified owner identity. A caller's query cannot redirect a wake
to another chat; another user's device still rejects it. Rooms keep their opaque
log discipline: no Loro parsing or provider execution moves to the server.
Wake retries and nightly backups share the existing durable alarm without
moving backups onto the wake cadence. No new deployment binding is needed.

Discovery and concurrent delivery are bounded. Stalled jobs yield after a
12-second service window, so eight dead destinations cannot permanently block
later healthy destinations. Jobs remain durable when that window ends.

On iOS, recent or pending sends request a [UIApplication background task](https://developer.apple.com/documentation/uikit/uiapplication/beginbackgroundtask%28withname%3Aexpirationhandler%3A%29) for at most 25 seconds
(or until iOS expires it), ending on foreground or sign-out. This gives normal
app switching time to finish delivery. Force-quitting or losing connectivity
before the edge accepts all command bytes still requires reopening the app; a
stopped process cannot transmit. Local receipts survive for that recovery.
Chat visibility on the desktop remains registry visibility, not proof that all
command bytes arrived. The UI keeps unadopted sends in Sending/Queued until the
host takes ownership.

Deploy the updated edge together with the updated senders (edge first is safe).
The server-owned handoff is not available on an older edge, where the local
wake/relaunch recovery remains the fallback. This PR does not deploy production.

The shared thin-client transcript and session list now reserve Working for host
status or actual streamed output. The iOS composer explicitly says “Sending to
host…” during unadopted delivery; the existing echo retains Sending/Queued/Failed.
This does not treat a pending first Run as permission to start duplicate turns.

## Regression coverage

- Real workerd/SQLite chat-room tests accept a prompt via HTTPS or WebSocket,
  keep the phone offline (no restart and no separate nudge), and deliver its
  wake when the desktop joins. The desktop can then retrieve the stored bytes.
- Real workerd tests cover device-room queue saturation with both devices
  offline, stale-forward fencing, transaction rollback of rows/receipts/alarms,
  batch replay deduplication, host-output loop prevention, legacy senders,
  invalid/unauthorized routes, and wake/backup alarm coexistence.
- The actual thin-client socket carries its selected host hint; desktop
  transport tests verify both WebSocket URLs and HTTPS push requests.
- A server error after row ACK must retry the host wake and recover it after
  relaunch, with no viewport open or preload.
- Immediate exit after a new chat's first send must preserve the registry row,
  resume publication and host discovery, and keep exactly one original command.
  The test covers Windows/Codex, macOS/Cursor, and Linux/Claude host metadata.
- A successful wake to an empty room must retain the receipt until a final wake
  after row publication.
- Desktop wake failure survives row ACK and a reopened DocsStore/WorkspaceHost.
- Eight failed wakes yield capacity to a later healthy chat without deleting
  the failed obligations.
- Metadata discovery pages over 64 jobs, does not duplicate jobs with outbox
  rows, retains wakes after ACK, fences stale completion, and respects deletion.
- Existing live client, demo, born-chat2, relay delivery, and sync admission tests
  exercise surrounding behavior.

The two central new live-client regressions were also run against the unmodified
upstream client (with new implementation-specific UI assertions excluded):
`acked_command_recovers_failed_host_wake_without_opening_the_chat` failed with
“host wake was not retried”; the immediate-exit regression failed with
“windows: first send was stranded without a UI open”. Both pass with the fix.

The new accepted-prompt/no-relaunch regression was also run with the previous
PR's unchanged chat-room server. With implementation-specific receipt assertions
excluded, the prompt was accepted but the desktop never received its wake
(`expected [] to include <chatId>`). It passes with the server-owned handoff.

All 166 executed tests passed on Linux:

| Check | Result |
| --- | --- |
| `cargo test -p zeron-client -- --test-threads=1` | 45 passed |
| `cargo test -p zeron-sync --lib sync_jobs::tests -- --test-threads=1` | 3 passed |
| `cargo test -p zeron-engine --lib remote_delivery -- --test-threads=1` | 2 passed |
| `cargo test -p zeron-engine --lib sync_lifecycle_tests -- --test-threads=1` | 19 passed |
| Engine `born_chat2_race`, `relay_delivery`, `rich_composer_delivery` suites | 7 passed |
| Edge unit + real workerd suites (`npm test`) | 58 + 32 passed |
| Edge TypeScript check and Worker bundle dry run | Passed |
| `cargo check -p zeron-mobile` | Passed on Linux |
| Changed Rust code's rustfmt checks and `git diff --check` | Passed |

The new server tests establish stored-byte and durable host-wake delivery, not
native provider execution. Native iOS background scheduling and visual rendering
require Xcode/device validation. Native Windows and macOS provider launches were not exercised on the
Linux validation host.
