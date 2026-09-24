# Bounded chat synchronization

Historical chat count must not determine concurrent network resource use. Local
inspection, a document's lifetime, and its sync client's lifetime are separate.
The changes require no manual data cleanup. They do not change the OS file limit.

## Admission and lifetime

`DocHost::open_local` returns the one authoritative writable document and installs
its durable publication subscription. `open` additionally requests synchronization;
it does not spawn a connection immediately. Boot salvage first inspects snapshots
and rollback sources without registering handles. Legacy epochs are treated as
recovery sources, not evidence the adopted chat2 transcript is healthy. A real
candidate waits for catch-up and rechecks under the command/import locks. Failed
recoveries retry from durable metadata.

Before reserving a slot, the dispatcher selects an operation using the opened
document's effective generation and current registry ownership. Modern chats
join normally; only the owning host seeds a legacy chat. Wakes for an explicitly
foreign legacy chat retire by captured version without touching outgoing batches
or consuming a slot. A missing registry row still represents a possible new chat,
and a local chat2 epoch still overrides stale legacy registry metadata. A modern
registry row overtaking an old handle defers admission until cutover replaces it,
without retiring its wake.

A single dispatcher per host admits at most 12 clients, up to four per 100 ms tick.
It gives a background selection one in every four admissions, with oldest-service
ordering within each class. The disk backlog is paged by chat ID, rather than
materialized as thousands of documents or semaphore-waiting tasks. A served quiet
background client releases its slot; a warm interactive client has a 10 s reuse
grace after catch-up completes. Incomplete catch-up cannot be retired by that
idle grace. Closing clients retain their slots until teardown completes, while
the dispatcher continues admitting other work and handling durable jobs. The
bounded set of close tasks is tracked and joined before the host's final
snapshot; actor ownership is retained throughout shutdown so a cancelled close
cannot detach the actor.
All session waits, including handshake, backfill and backpressured sends, are
interruptible by shutdown. Under contention a caught-up client yields after 30 s. A client still
catching up has up to five minutes before rotation so slow checkpoint downloads
are not continually cancelled. These are initial policy constants, not measured
optimal values for all devices.

Process-wide chat data budgets allow 24 socket lifetimes, four simultaneous dials
and eight HTTP requests. At most six background HTTP requests hold slots, leaving
two for interactive requests. Socket permits live in the pump until teardown;
HTTP permits cover the response body. Dial starts are spaced by 50 ms. HTTP idle
pool retention is bounded separately. Registry/device control links are outside
the chat budget; Git, terminal processes, DNS, Happy Eyeballs connection attempts
and other features still consume descriptors outside it. These caps leave room
for them but cannot guarantee headroom against arbitrary unrelated resource use.

A typed EMFILE/ENFILE cause pauses chat admission for two seconds. Reconnects use
jitter and the same budgets, including the HTTP fallback. Admission has at most
128 queued permit requests. Cancelling client construction aborts its actor and
fallback; it cannot detach a socket-owning task.

Caller-owned handles protect the open-to-subscribe handoff without the former
30 s document eviction exemption. Watches, explicit agent writer leases, active
bounded clients, pending host commands and failed publication writes protect
resident state. Legacy document Arc ownership remains a conservative guard.
Periodic cache eviction runs even when no document changes. Network teardown does
not detach the durable local-update subscription. Failed outbox writes retain a
memory copy, pause new client admissions, and retry; already-running agents and
persistence continue. Memory containing unpersisted edits is never intentionally
reclaimed to satisfy the cache target.

## Durable progress and wake protocol

SQLite migrations add a durable work table, a monotonic version clock, and a
reconciliation cursor. A wake receipt is retired only after catch-up, command
acceptance and successful snapshot persistence, closing the crash-before-debounce
window. The dispatcher discovers unacknowledged/rejected outbox
batches after restart without a UI open. Checkpoint obligations use the existing
idempotent publication path. Snapshot and cursor persistence remain atomic.

New hosts advertise `nudgeAck=1`. The edge persists a wake before sending it and
keeps it until the host confirms local durable admission. An opaque token fences
old ACKs against newer wakes for the same chat. Delivery uses a 64-receipt window
and a five-second alarm for retry. A reconnect starts a fresh window.

The edge keeps at most 4096 distinct queued chat wakes. At capacity it returns 503
without discarding accepted work, and persists an overflow reconciliation marker.
The host accepts that marker durably, waits for registry synchronization, and
scans owned chat IDs in resumable pages of 32. The sender's command/outbox remains
the source of truth; a wake is only a notification to look for work.

Deploying the edge before or after the engine is supported: old edges omit tokens,
and old hosts receive legacy delivery without ACK requirements (in pages rather
than a full replay burst). Legacy hosts retain their former best-effort guarantee;
they cannot acknowledge or process the new reconciliation marker. No deployment
is performed by this change. Database changes are additive; downgrading loses
the new delivery/resource guarantees.

## Diagnostics and validation

`SyncStatus.resources` exposes process budget use/limits, pending admissions, open
documents, retention reasons, durable batch/job counts, Linux descriptor count
and the Unix soft descriptor limit. A per-chat `state` and additive
`ChatConnectivity.syncState` distinguish local, queued, connecting, synchronized,
offline and storage-error states. The selected chat's connection caption displays
queued sync or persistence trouble without exposing internal limits.

Relevant checks:

```
cargo test --locked -p zeron-engine --test sync_resources -- --nocapture
cargo test --locked -p zeron-engine --lib --test session_publication --test restart_resume --test local_profiles --test born_chat2_race --test codex_subagents --test message_queue --test transcript_salvage
cargo test --locked -p zeron-sync --lib
cargo test --locked -p zeron-rpc --test device_room
cargo check --locked -p zeron-ui
cd edge
npm run typecheck
npm test
```

The resource regression lowers RLIMIT_NOFILE to 256 only in an isolated child,
queues 1000 chats, serves real loopback WebSockets, drops connections together,
checks bounded documents/transports, repeatedly launches Git, waits for durable
work to drain, and checks socket release on shutdown. It measures a two-worker
headless test process, not the complete desktop application's platform baseline.
The workerd tests use real DO SQLite and sockets for old ACKs, retry, overflow and
more-than-256-wake delivery. Other coverage checks restart publication, lost ACKs,
checkpoint rejection, constructor/shutdown cancellation and the view-attachment
race. Loopback lifecycle tests hold twelve valid checkpoints beyond the idle
grace, verify durable snapshots after release, and admit an unrelated chat while
a renewed wake closes a peer stalled before ROWS_DONE.

Admission regressions also queue twelve foreign legacy wakes (including missing
room-generation metadata), assert prompt interactive admission and receipt
retirement, then repeat the wake burst. Controls cover a wake preceding its
registry row, modern non-host readers, a local epoch overriding stale registry
metadata, host reassignment while waiting, version-fenced retirement, and
preservation of pending outbox bytes.
