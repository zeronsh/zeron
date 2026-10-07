# Zeron memory audit — 2026-10-02

**This document records the baseline investigation.** The production fixes,
after measurements, validation, and remaining limitations are tracked in
[the hardening follow-up](memory-hardening-2026-10-02.md). The baseline JSON
retains the original diagnostic source/binary hashes; the example has since
been adapted to exercise the bounded production paths.

Audited upstream `main` at **`1c83cda221bd18a3b55ea49e5b25ec2c148955bc`**,
application version **0.2.102**. The initial investigation used branch
`codex/memory-leak-audit`; fixes are on `codex/memory-hardening`. The original checkout's uncommitted picker work was
preserved; this audit uses a separate worktree.

Experiments and library tests were executed at `69e64ef5`. Upstream advanced
during the audit; the branch was then fast-forwarded to `1c83cda2` and its
inline-renaming UI delta reviewed. Engine/document/RPC/sync sources and locked
dependencies are identical between these revisions. The new UI rename keeps
one editor, clears explorer references on finish, and owns its subscriptions;
no additional large-buffer or task-retention mechanism was found in that delta.

## Conclusions

There are still credible paths to gigabytes of memory in both the headless
engine and the desktop application. They have different owners and require
different fixes. A single RSS reading does not identify which path occurred.

1. **Sync is a credible explanation for the dev/release difference.** Healthy
   acknowledged traffic drained in the isolated experiment; stalled/offline
   clients retained their complete backlog, and slow checkpoint downloads
   retained incoming rows. Sync also keeps documents resident while connected.
2. **Offline sync is durable, but the active client's entire outbox is also
   resident.** Loading and sending it make additional full payload copies.
   Connection-count limits do not limit these bytes.
3. **The 80 MiB document cache target is not an 80 MiB heap ceiling.** It uses
   compressed snapshot size times six, exempts pinned documents, and does not
   directly account for transcript mirrors, queues, or Loro's live allocation.
4. **Journal tail operations deserialize the whole historical journal.** Even a
   replay returning zero events can have a peak proportional to all past events.
   These peaks also feed allocator retention.
5. **Quiet legacy RPC subscriptions and cancelled unary calls do not clean up
   promptly.** Several production UI paths still use the legacy subscription API.
6. **Terminal subscribers and raw PTY queues remain unbounded.** A slow viewer
   can retain arbitrarily much output in the engine, independently of the 1 MiB
   terminal replay limit. This is a reproducible headless growth mechanism.
7. **User-image retention bypasses the apparent 64 MiB cache target.** The
   selected transcript protects all of its user images, including offscreen
   history; non-PNG decoded pixels are not included in its size estimate.

The Linux allocator issue already has a fix on this revision. Commit
[`4a7ef2cc`](https://github.com/zeronsh/zeron/commit/4a7ef2ccaa1b8d96f1e4106b58acc12fdc0dee13),
merged September 29, calls `malloc_trim(0)` every minute in headed and headless
Linux/glibc modes. The older investigation recorded a 7.2 GB daemon with 6.6 GB
in anonymous arena mappings. That is historical evidence, **not a measurement
made during this audit**. A report from a build without this commit may be that
same problem. A report from current main can still be any of the application
retention paths below.

The initial investigation added an offline diagnostic example and this report,
without changing production memory policies. The linked follow-up now records
production fixes and Linux native profiling. Native macOS profiling and an
actual reporter's workload remain necessary to attribute the reported 4 GB case.

## Scope and evidence

The source audit covers the application entry point, engine assembly/shutdown,
document cache, CRDT persistence and history, journal, sessions, RPC client and
server, chat/registry sync, terminal transport and emulators, checkout diffs and
watchers, attachment/media caches, transcript caches, preview/browser lifetimes,
harness stdio, and the shared mobile client/text layer. The edge chat-room limits
were checked where they affect desktop sync. This is not a separate complete
audit of the cloud edge, WebKit, graphics drivers, or ONNX runtime internals.

Local execution is **Windows x86-64, Rust 1.99.0, release, locked dependencies**.
No Linux runtime or macOS host is available here. The diagnostic example uses a
counting wrapper around Rust's system allocator and counts requested allocation
sizes. Its measurements exclude allocator overhead, native/C allocations, OS
mapping residency, GPU allocations, and subprocess memory. They establish
ownership and amplification, not cross-platform RSS predictions.

Evidence labels:

- **Measured:** reproduced with the current code by the provided example.
- **Confirmed in source:** ownership or absence of a limit is directly visible;
  the magnitude/frequency in user workloads is unmeasured.
- **Historical:** earlier repo measurements, with their original scope.
- **Hypothesis:** a workload-dependent explanation requiring native profiling.

## Does sync explain the smaller development footprint?

The configuration supports this distinction. In
[engine/lib.rs](../crates/engine/src/lib.rs), lines 745–767, a local profile has
no edge; a development profile enables it only with a nonempty `edge_token`;
a synced profile enables it. The Cargo optimization profile alone does not
switch sync off. Compare the same data/profile and executable configuration,
not just binaries labelled dev and release.

The controlled `sync` scenario uses the real `ChatClient`, SQLite outbox and a
loopback WebSocket fixture. With 16 MiB of acknowledged updates, it settled at
59 KiB of live Rust heap above its no-client baseline. With acknowledgements
withheld, 16 MiB stayed resident; after disconnection and another 16 MiB, it
held 32 MiB. Client shutdown released that heap while the SQLite store and
local fixture remained alive. The separate checkpoint scenario retained about
17.1 MiB while 16 MiB of rows waited for the checkpoint; completing catch-up
reduced that to 1.05 MiB with the connection still alive.

Thus a healthy connection does not inherently leak every transmitted byte in
this short experiment. Backlog, catch-up, and document pinning can produce the
observed difference. A disconnected active client differs from an edge-less
profile: the former still holds pending payloads in RAM. A nominally online
client with missing acknowledgements can accumulate them too.

The fixture deliberately discards remote row bodies and accepts a dummy
checkpoint to isolate transport ownership. It is not an authenticated engine
convergence test, a production-room measurement, or a long reconnect soak.
The local-only document experiment separately establishes substantial CRDT
and mirror costs. Sync retaining those documents can compound the problem.

## Priorities

| ID | Priority | Path | Applies to | Evidence | Why it matters |
| --- | --- | --- | --- | --- | --- |
| T1 | P0 | PTY and subscriber queues | Headless + desktop engine | Source + experiment | Byte growth has no bound when a consumer falls behind |
| S1 | P1 | Whole outbox residency and send copies | Synced engine | Source + loader/real-client experiments | Durable backlog also becomes live heap, sometimes twice |
| S2 | P1 | Checkpoint rows buffered in an uncapped vector | Synced engine | Source + real-client experiment | The actor drains bounded transport queues into an unbounded catch-up buffer |
| D1 | P1 | Compressed-size document budget and pin exemptions | Engine | Source + experiment | A small snapshot can represent a very large live document |
| J1 | P1 | Whole-journal tail/replay scans | Engine | Source + experiment | Historical output determines transient peaks and replay retention |
| R1 | P1 | Quiet RPC streams and cancelled calls | Engine + clients | Source + experiment | Cancelled views/requests can retain server work and leases |
| I1 | P1 | Protected/full-resolution user images | Desktop | Confirmed in source | Pixel residency can exceed encoded-byte targets by orders of magnitude |
| T2 | P1 | Inaccessible terminal tabs and scrollback | Desktop | Confirmed in source | Per-chat emulators survive deletion; exited history has no UI aggregate cap |
| H1 | P1 | Full CRDT history and large single active documents | Engine + mobile client | Source + synthetic history experiment | Evicting other chats cannot shrink one pinned history |
| S3 | P2 | Registry pending operations and persistence copies | Engine registry, all profiles | Confirmed in source | Unacknowledged metadata writes accumulate without a total byte cap |
| R2 | P2 | RPC item-count limits and request concurrency | Engine + clients | Confirmed in source | 256 items is not a byte ceiling; blocked requests retain state |
| C1 | P2 | Per-checkout patches and filesystem kicks | Engine | Confirmed in source | Per-checkout caps accumulate; event kicks have no queue cap |
| E1 | P2 | Completed-session metadata/request retention | Engine | Confirmed in source | Last prompts and metadata survive chat deletion |
| A1 | P2 | Harness writer queues and unlimited line reads | Engine/harnesses | Confirmed in source | A stalled child or huge line can grow buffers despite bounded event channels |
| M1 | P2 | Leaked font bytes per FontBook | Mobile/shared text | Confirmed in source | Actual unreclaimed allocations; unrelated to desktop/headless report |

P0 here means the first containment fix to implement, not proof that every
reported machine hit that path. P1 work addresses substantial resident bytes or
retained ownership; P2 addresses additional growth modes and hardening.

## Measurements on the audit revision

Raw counters, fixture details and executable/source SHA-256 hashes are recorded
in [the JSON results](performance/memory-audit-2026-10-02.json).
The example's small, finite workloads deliberately expose mechanisms without
trying to allocate 4 GB. See the raw file for exact counters and scenario
details. Native platform numbers must be collected separately.

| Scenario | Controlled input | Result above warm baseline |
| --- | --- | --- |
| Healthy sync | 64 acknowledged updates, 16 MiB total | 59 KiB live after drain; zero pending batches |
| Sync without ACKs, then offline | 128 pending updates, 32 MiB total | 32.0 MiB live; 3.5 KiB after client shutdown |
| Checkpoint download stalled | 64 incoming rows, 16 MiB total | 17.1 MiB live while waiting; 1.05 MiB after catch-up |
| Durable outbox load and send copy | 64 updates, 16 MiB on disk | Disk-only 651 B; loader 16.0 MiB; with copy 32.0 MiB live |
| Quiet documents | 12 chats, 24 MiB compressible text total | 125.5 MiB live despite a 6 MiB estimate / 205 KiB snapshots |
| Quiet transcript watches dropped | Same chats; zero remaining viewers | 149.5 MiB live; mirrors retain another 24 MiB |
| Count eviction and shutdown | Open eight new empty chats | 50.2 MiB live after eviction; returned to baseline on shutdown |
| Journal replay returning no events | 16 MiB historical event text | 16.1 MiB peak; reopen/append raises peak to 32.1 MiB |
| Stalled terminal receiver | 16 MiB shell output | 24.4 MiB live; 0.98 MiB after drain, primarily replay |
| Cancelled calls / dropped quiet streams | 64 timeouts + 32 legacy + 32 scoped | 96 abandoned server requests; scoped streams add none; connection drop clears all |
| Synthetic CRDT replacements | 8 MiB written; 64 KiB currently visible | 8.06 MiB full snapshot versus 128.4 KiB shallow snapshot |

Peak counters are cumulative within each scenario process. Small negative live
deltas after teardown mean earlier baseline allocations were also released;
they do not indicate negative allocation sizes. Measurements include fixture
allocations, so they are not exact per-type or per-component accounting. The
history live counter also includes exported snapshot buffers; its snapshot-size
comparison is the useful evidence for preserved history.

## Detailed findings

### T1 — Terminal backpressure stops before the engine's unbounded queues

Sources: [terminals.rs](../crates/engine/src/terminals.rs), lines 52, 64–86,
326, 388–411, 535–625; [rpc.rs](../crates/engine/src/rpc.rs), terminal subscription
handler near line 3252; [server.rs](../crates/rpc/src/server.rs), stream sender.

There are two unbounded stages:

- The blocking PTY reader sends `Vec<u8>` into an unbounded raw queue.
- Each terminal subscriber gets an unbounded `TerminalEvent` queue. `emit`
  clones the base64 output for every subscriber and uses synchronous `send`.

The RPC server awaits its bounded outbound queue, so a slow client stops the
RPC stream from consuming its terminal receiver. PTY production continues.
The replay ring's 1 MiB cap applies only to `LiveTerminal.replay`, not to that
receiver. If output stops, the queued bytes can remain indefinitely until the
receiver drains or drops. No GUI needs to be present on the hosting device.

The batcher also has no maximum batch-byte threshold. A 12 ms timer is a time
target, not a guaranteed memory limit while its executor is starved. The replay
ring deliberately retains at least one event even if that event alone exceeds
its nominal cap. Its accounting uses encoded base64 bytes, so the normal raw
replay window is smaller than 1 MiB.

At 10 MiB/s raw output, base64 alone adds approximately 13.3 MiB/s per stalled
subscriber: roughly 4 GiB in five minutes, before object overhead. This is a
capacity calculation, not a reproduced user throughput.

**Fix:** bound raw and subscriber bytes, cap each batch, and define an explicit
lag policy. Unix PTY reads can use backpressure; Windows cleanup must preserve
the ConPTY drain requirement documented in this module. Terminal bytes are an
ordered stream: silently discarding arbitrary chunks corrupts emulation. Use a
bounded replay/spool and explicit gap reporting or terminal-state recovery.
Track queued bytes and lagging subscriber count in diagnostics.

### S1 — Durable outboxes are still completely resident in active clients

Sources: [store.rs](../crates/sync/src/store.rs), lines 138–165;
[chat2_host.rs](../crates/engine/src/chat2_host.rs), lines 124–147;
[chat_client.rs](../crates/sync/src/chat_client.rs), lines 265–290, 587–599,
664–695, 1481, 1719–1750; [doc_host.rs](../crates/engine/src/doc_host.rs),
local publication near line 1579 and cold-open recovery near line 1456.

`pending_chat_updates` selects and materializes every payload for one chat.
`ChatClient` initializes its `VecDeque<PendingPush>` from that whole list, and
new updates continue to enter it until acknowledgement/rejection. The ~1 MiB
limit is per update; there is no total pending-byte limit.

Both socket `push_pending` and the HTTP fallback clone vectors of pending
payloads before sending. Those copies remain alive across backpressured sends.
The socket in-flight ID set avoids repeated retransmission within a session,
which helps traffic; it does not eliminate payload ownership or the initial
full backlog copy. Rejected batches can also be loaded alongside pending
batches for filtering. Cold open imports the entire durable outbox as another
materialized list.

Healthy storage does improve the situation when there is **no active client**:
the document publication subscription writes to SQLite, rather than always
keeping a second offline vector. The handle's `chat2_pending_local` now keeps
copies only after disk-write failure. This earlier fix is real. However, an
already-running client on an offline network still owns its complete backlog.
Storage failures intentionally preserve unsaved bytes and pin the document;
that is a correctness obligation, requiring visible limits and failure handling.

The 28-client host cap and 32-socket process cap bound connections, not payload
bytes. A few large backlogs suffice for a large heap.

The real-client experiment confirms payload retention after network loss and
release on explicit client shutdown. The separate loader experiment confirms
the full-copy amplification. Neither attributes the reported multi-gigabyte
case without measuring that machine's actual pending bytes.

**Fix:** page the durable outbox by ordinal and bytes, retain only a bounded
in-flight window, and acknowledge durable IDs. Use shared immutable payloads to
avoid deep copies. Keep unsaved edits recoverable and report a storage failure
instead of dropping them to meet a budget.

### D1 — Document budgeting understates heap and cannot constrain pinned state

Sources: [doc_host.rs](../crates/engine/src/doc_host.rs), lines 60–69, 839–887,
2912–3016; [constants.rs](../crates/doc/src/constants.rs), document byte budget.

The resident estimate is `max(compressed_snapshot_bytes * 6, 512 KiB)`.
Compression ratio depends on content, history, and encoding; it is not a stable
measure of live heap. Repeated text is a deliberate worst case for this proxy.
The 12-document cap bounds quiet document count, not total document size.

Documents with views, extra document/handle ownership, active sync, failed
publication, or pending host commands can prevent eviction. Keeping a live
writer safe is appropriate, but means the 80 MiB target is a **soft target**.
One enormous selected/active document can exceed it on its own. The network
scheduler does rotate/retire clients, and its periodic eviction runs while
idle; these newer fixes are present.

There is also delayed mirror release: `publish_messages_if_watched` clears the
mirror when it handles a subsequent change with no receivers. Dropping the
last receiver of a quiet chat does not itself clear the sender's retained
snapshot. Warm documents can therefore retain their materialized transcript
after the viewer leaves, until another commit or eviction. The current byte
estimate does not add those mirror bytes.

**Fix:** account for visible text/parts, history and retained mirrors separately;
expose estimate versus actual heap in profiling. Release quiet mirrors on the
last lease drop or a periodic sweep, serialized with attach/import. Preserve
writer safety and durable publication. Treat large pinned documents explicitly
(tail materialization, history replacement, or bounded execution policy) instead
of promising a hard global cap that eviction cannot enforce.

### J1 — Tail queries allocate in proportion to the whole journal

Sources: [run_journal.rs](../crates/engine/src/run_journal.rs), lines 103–111,
149–172, 208–244; [sessions.rs](../crates/engine/src/sessions.rs), subscription
and crash recovery near lines 359 and 836.

`read_lines` deserializes the entire journal into a vector. `replay(after_seq)`
filters that vector afterward, and `last_event` takes its last element afterward.
`scan_tail` simultaneously reads the raw file and calls `read_lines` to discover
the next sequence. The journal has no compaction window; its header explicitly
defers one. Raw tool inputs/outputs live here even when chat2 sidecars keep the
CRDT transcript thin.

The 16-file descriptor cap clears the whole open-file map. More than 16
interleaved chat journals can repeatedly force those expensive reopen scans.
That cap bounds descriptors, not scan allocations.

A slow replay subscriber also owns the returned replay vector until it finishes
streaming it. Small tail requests and boot's last-event recovery scans create
avoidable peaks even without such a subscriber. Released peaks can leave
allocator pages resident, explaining a high RSS with much less live data.

**Fix:** scan a bounded tail backward for sequence/last-event metadata; filter
while reading rather than afterward; stream/page replay instead of returning
all history. Keep torn-line handling and era fallback correct. Add journal
compaction/archive policy separately, preserving crash recovery and raw tool
sidecars as needed.

### R1 — Cancelled calls and quiet legacy subscriptions retain ownership

Sources: [client.rs](../crates/rpc/src/client.rs), lines 61–91, 142–193,
198–261; production callers in
[files/client.rs](../crates/ui/src/files/client.rs), lines 100–105 and 278–286,
and [terminal/panel.rs](../crates/ui/src/terminal/panel.rs), near line 773.

`RpcClient::call` inserts a pending oneshot sender and removes it for an error
response, a successful response, send failure, or connection teardown. Dropping
the call future (including a timeout) has no cleanup guard and sends no server
cancel. A request which never replies retains the pending entry and server task
for the connection lifetime. Harness stdio JSON-RPC has the analogous pending
request lifetime problem.

Legacy `subscribe` returns a bare receiver. Dropping it is noticed only when
the next server item is routed. A quiet stream can retain its server task and
watcher lease indefinitely on a still-open connection. `subscribe_scoped` and
`subscribe_checked` have owned drop cancellation, and the main document/queue
watchers use it. File watchers and terminal UI subscriptions still use legacy
`subscribe`; the generic workspace watches and import progress path do too.
Workspace file subscription drop normally tears down the checkout watcher, but
that drop cannot happen if an abandoned quiet RPC task still owns it.

This is logical retention of tasks/resources. A small pending-map entry alone
does not explain 4 GB; the retained stream/document/parameter graph or repeated
large requests can make it material.

**Fix:** give unary requests an owned cancellation guard; migrate remaining
production legacy subscriptions to scoped ownership. Ensure cancellation also
works during initial send/backpressure and remote proxy teardown. Verify with
quiet streams and unanswered requests, not only continuously active streams.

### R2 — Bounded item counts do not bound connection memory

Sources: [client.rs](../crates/rpc/src/client.rs), line 17 and frame routing;
[server.rs](../crates/rpc/src/server.rs), lines 23–58 and 185–214.

RPC queues have 256 slots. String/JSON payloads vary substantially in size;
watch resets, checkout-diff lists, and terminal batches can be large. A 3 MiB
item in every slot represents 768 MiB of payload in one queue, before JSON
object expansion. Real transcript deltas are usually far smaller, so this is a
capacity example, not typical usage.

The server has no aggregate byte or request-task admission limit. Unique
long-lived requests create tasks outside the queue cap. Reusing an ID replaces
its stored abort handle without explicitly aborting the old request. The
socket pump also awaits writes without its own application send deadline; a
blocked write pauses reading close/cancel frames in the same loop. The more
robust split-direction sync socket pump is not automatically used by local IPC.

**Fix:** per-connection queued-byte and request admission budgets, finite large
frame limits appropriate to real resets, cancellation-safe independent I/O,
and duplicate-ID handling. Latest-value watches can conflate; dependent
transcript deltas require a reset/resync policy rather than arbitrary dropping.

### H1 — Full snapshots preserve history, not just current transcript state

Sources: [schema.rs](../crates/doc/src/schema.rs), lines 789–794 and 1300 onward;
[doc_host.rs](../crates/engine/src/doc_host.rs), checkpoint export near line 2848;
[rebuild.rs](../crates/doc/src/rebuild.rs); [constants.rs](../crates/doc/src/constants.rs).

Normal snapshot persistence and checkpoint upload use `ExportMode::Snapshot`.
Replacing a map field can leave its old operations/strings in the CRDT history.
The synthetic history experiment demonstrates the mechanism with one repeatedly
replaced field; it is not a replay of a production conversation.

The chat2 migration rebuild strips large tool output/diffs into sidecars and
removes old legacy history. Current render output also preserves that stripping.
Those protections reduce the old whale problem. However, a one-time rebuild is
not ongoing resident-history reclamation. Uploading a full checkpoint and
pruning edge log rows does not discard history inside the live client's doc.
The Rust `RETAIN_DAYS` and `SOFT_CEILING_BYTES` constants do not enforce a local
history or heap ceiling. Message-part splitting also does not cap total history
or the joined transcript presented to viewers.

**Fix:** measure state bytes and history bytes separately. Replacing a live doc
with a shallow snapshot or rebuilt lineage needs the stale-peer/outbox/cursor
protocol, not a local truncation shortcut. Preserve all pending commands and
unsaved edits. Tail-first views can reduce materialization independently of
that correctness-sensitive protocol work.

### S2 — Checkpoint catch-up bypasses transport backpressure

Sources: [chat_client.rs](../crates/sync/src/chat_client.rs), lines 1132–1224;
[chat2_host.rs](../crates/engine/src/chat2_host.rs), checkpoint fetch near lines
269–345; [chat-room.ts](../edge/src/chat-room.ts), checkpoint cap and upload.

Rows are requested before the checkpoint finishes downloading. During that
download the actor continuously drains socket frames into `Vec<WireFrame>`.
There is no byte cap on this vector. Its 300 s deadline limits time, not bytes.
The parallel transfer helps latency, but a large backfill/active room and slow
checkpoint can accumulate substantial row payloads. This is transient catch-up
retention rather than proof of permanently leaked frames.

The current edge limits checkpoints to 32 MiB. The engine HTTP fetcher itself
extends its `got` vector without validating total bytes; it trusts the server
cap. The edge's upload path also reads the body before enforcing its cap. These
are distinct from local document heap size and do not make a 32 MiB checkpoint
an in-memory document limit.

In the loopback experiment, 64 rows containing 16 MiB of payload retained
17.1 MiB of live Rust heap while checkpoint completion was gated. Completing
the checkpoint and replay reduced it to 1.05 MiB. This confirms the temporary
ownership mechanism in the current actor; the fixture does not validate CRDT
contents or native allocator reclamation.

**Fix:** enforce checkpoint size while streaming; byte-budget buffered rows;
pause reads, request pages after import, or spool rows durably before replay.
Avoid dropping causally necessary rows. Preserve Range resume and checkpoint
version validation.

### S3 — Registry metadata has its own uncapped pending-operation list

Sources: [registry.rs](../crates/doc/src/registry.rs), lines 394–410,
557–565 and 593–623; [sync/registry.rs](../crates/sync/src/registry.rs),
lines 825–941 and presence access near line 408;
[workspace_host.rs](../crates/engine/src/workspace_host.rs), lines 329–332
and 621–628.

The workspace registry is separate from chat outboxes. Its `pending` vector
retains local metadata operations until acknowledgement. Batches are limited
to 400 operations for the edge protocol, with no total pending-byte cap.
Repeated offline status/metadata writes can accumulate historical operations,
even when they update the same visible row. `take_pushable` clones pending
batches for transport; `to_bytes` clones both authoritative rows and pending
batches before serialization. This produces additional full-list peaks.

This also applies to an edge-less local/development profile: local mutations
still append operations, but `join_room` returns without creating a client,
and `mutate` does not locally acknowledge/fold those pending operations.
The persisted overlay can therefore accumulate across restarts even without
chat sync. It is a separate mechanism from S1's active chat-client outbox.

The HTTP fallback is single-flight, which prevents unlimited simultaneous
pull tasks. Its spawned task is not part of the registry actor's joined task;
it can retain the document/batch list through an outstanding HTTP operation.
Do not interpret the chat scheduler's joined-worker teardown as proving this
separate lifetime. Presence TTL filters the public result but does not remove
expired entries from the underlying map; device-ID churn can accumulate small
inert entries. These are source findings without a magnitude experiment.
Registry rows are normally much smaller than chat/terminal output, making this
a secondary suspect for the 4 GB report.

**Fix:** expose registry pending bytes; bound transport/persistence copies;
consider safe coalescing of unsent same-row metadata operations while preserving
ordering and tombstones; compact durable local-only state explicitly.
Track/cancel/join fallback HTTP work during teardown
and prune expired presence entries. Preserve durable local metadata.

### I1 — User-image protection overrides cache size and pixel accounting

Sources: [attachments.rs](../crates/ui/src/attachments.rs), lines 236–253,
495–544, 618–714, 723–739, 816–825;
[transcript.rs](../crates/ui/src/transcript.rs), lines 5427–5458.

The cache has **separate** 64 MiB budgets for legacy user attachments and
normalized generated images. The newest image is excluded from eviction;
protected legacy images are also excluded. The protected set is built by
walking every user-attachment row in the selected transcript, not the viewport.
After scrolling through an image-heavy conversation, all of those loaded
images can remain protected. Switching the chat changes protection but does
not run the eviction loop immediately; eviction runs on a subsequent insertion.

Pixel accounting adds `width * height * 8` only when `png_dimensions` succeeds.
JPEG/WebP/GIF/etc. can therefore be charged essentially only their encoded
bytes even though GPUI retains decoded pixels and GPU copies. The 24 MiB intake
limit is a file-size limit. Legacy attachment decode does not use the normalized
generated-image path's explicit decode/preview limits.

One 3840×2160 RGBA image is 31.6 MiB for one pixel copy. Twenty such images can
account for roughly 1.24 GiB with a CPU and a GPU pixel copy, plus encoded bytes,
atlas overhead and temporary decodes. This is dimensional arithmetic, not a
native Metal measurement. Animated images can have additional frame costs.

Generated images and workspace preview media have stronger pixel/byte budgets;
those paths should not be conflated with legacy user attachments.

**Fix:** protect viewport plus overdraw; keep bounded thumbnails in transcript
rows; load originals on explicit preview; account for decoded dimensions and
frames for every supported format; enforce decode budgets; trim immediately
when protection changes. Budget the actual retained CPU/GPU representation.

**Earlier note corrected:** the pinned zui commit `667d0aa` implements
`ImageSource::Image(...).evict` by removing the decoded asset and dropping its
atlas image in all windows (including the active window). Zeron calls it in
`flush_evicted`. The older `memory-plan.md` follow-up claiming raw image atlas
tiles cannot be released is stale on this revision. Eviction eligibility and
pixel accounting remain the problems here.

### T2 / E1 — Deletion leaves terminal UI history and session metadata

Sources: [terminal/panel.rs](../crates/ui/src/terminal/panel.rs), lines 260–280,
322–328, 575–602; [terminal/emulator.rs](../crates/ui/src/terminal/emulator.rs),
line 44; [sessions.rs](../crates/engine/src/sessions.rs), lines 154–167,
545, 1318; [rpc.rs](../crates/engine/src/rpc.rs), lines 1135–1154 and 1195–1200.

`TerminalPanel.chats` retains per-chat tabs, emulators and reconnect tasks.
Its state observer changes selection but does not prune deleted chats. Once a
chat disappears from navigation, its old tabs are no longer conveniently
closeable. Each emulator has up to 10,000 history lines. Exited emulators also
stay in the UI until explicitly closed; the engine's 32-terminal cap and
30-minute exited-terminal reaper do not cap the number of these UI histories
accumulated over a long desktop session.

The engine session maps retain hubs, statuses, harness session refs, and the
last `RunRequest` for chats touched during this engine lifetime. Requests include
prompts. Completion removes the active run, not these maps. `DeleteChat` purges
the document/snapshot but does not clear session metadata, discard the journal,
or interrupt a live run; `DeleteSpace` does interrupt its affected runs before
purging their docs. A racing/live writer deliberately keeps its document alive.
Thus chat deletion cannot be assumed to reclaim all related resident state.

**Fix:** explicit, coordinated session deletion; interrupt/settle writers first;
prune UI tabs for deleted chats and retire their subscriptions; release inert
emulator histories by a documented aggregate budget. Preserve intentional
live-terminal survival when merely navigating away. Keep only request config
needed for fallback, rather than every completed chat's full last prompt.

### C1 — Checkout features have per-object caps, not process caps

Sources: [diff_sync.rs](../crates/engine/src/diff_sync.rs), lines 58–69,
564–669, 687–780; [spaces.rs](../crates/engine/src/spaces.rs), lines 129–178;
[repos.rs](../crates/engine/src/repos.rs), file-index cache near lines 1816–1841.

Each active checkout may retain up to 3 MiB of patch in the engine watch value.
Watchers serialize lists of all checkout diffs. Multiple checkouts and multiple
slow subscribers multiply retention; UI and temporary JSON copies add to it.
Entries are correctly pruned after their orphan grace. There is no aggregate
patch-byte limit, nor on-demand-only residency for unused patches.

Filesystem callbacks send `()` to unbounded kick queues for checkout diffs and
space probes. During an expensive capture or slow sidecar publish, callbacks
continue to enqueue redundant wakeups. Tokio's unbounded channel stores queue
blocks even for zero-sized messages. The receiver eventually debounces/drains
them, but a build storm needlessly allocates in proportion to event count.
The workspace-files watcher is a better model: bounded events and an overflow
resync signal are already implemented there.

The file-search index is capped at 250,000 entries per root and expires for
reuse after ten seconds. Expired entries are removed on a subsequent cache
insertion, not by an idle timer; the last large index can remain resident.
This is bounded workload-dependent cache retention, not evidence of an
ever-growing per-file leak.

**Fix:** conflating kick/Notify channels; aggregate byte accounting for checkout
patches; summaries plus on-demand patches; idle index expiry when material to
measurements. Preserve watcher overflow reconciliation and checksum semantics.

### A1 — Harness channels and line decoders need byte bounds

Sources: [jsonrpc.rs](../crates/harness/src/jsonrpc.rs), lines 49–71,
101–158 and 232 onward; adapter `BufReader::lines` paths in Claude, Codex,
Cursor and ACP; [lib.rs](../crates/harness/src/lib.rs), stderr tail near line 240.

The shared harness JSON-RPC writer is unbounded. Request futures can be dropped
without removing their pending entry; queued request strings remain if a child
stops consuming stdin. Notifications also enqueue without flow control.
Adapters often read a complete newline-delimited line before parsing/truncating
it. An exceptionally large tool result or never-terminated line can allocate a
large string independently of bounded event counts. Event queues in the main
adapters are commonly bounded at 256 items, which is helpful but not a byte cap.

The rolling retained stderr tail is already small (six lines, truncated content).
The pre-truncation line read is the remaining risk. The engine's unbounded
question/control event queue is another count-less queue, but its normal payload
rate is low; it is a lower-priority suspect than PTY output.

**Fix:** bounded writes with cancellation/control priority; request drop guards;
explicit maximum stdio line bytes and finite oversized-result policy; preserve
approval/question delivery. Account for CLI subprocess RSS separately from the
engine: a Node-based harness's heap is not a leak in Zeron's Rust heap.

### M1 — FontBook really leaks font data on each successful registration

Sources: [font.rs](../crates/text/src/font.rs), lines 203–215;
[style.rs](../crates/mobile/src/layout/style.rs), lines 154–163;
[layout/mod.rs](../crates/mobile/src/layout/mod.rs), typography construction near
lines 302 and 504.

`FontBook::add_face` uses `Box::leak` to obtain static bytes. Dropping a FontBook
does not free them. Typography clones and registers supplied font bytes for
each new layout worker; the debug line-layout helper also constructs typography.
Repeated view creation therefore leaks another copy of the fonts. The source
comment assumes process-long face lifetimes, but callers create/drop books.
Invalid fonts are validated before leaking, so the normal successful path is
the relevant one.

This is an actual allocation leak, scoped to the shared text/mobile layout
implementation. The desktop GPUI/headless engine paths do not use this FontBook
for their rendering, so it is not an explanation for the Linux/macOS reports.

**Fix:** share a process-owned font-data pool keyed by font identity, or make
the face representation own its bytes with safe lifetime handling. Repeated
layout-view creation/destruction should return to its warm font baseline.

## Protections verified in current source

| Surface | Current behavior | Remaining qualification |
| --- | --- | --- |
| Linux/glibc allocator | Minute trim in long-running app modes | Reclaims free pages; cannot free live queues/docs; absent on musl and custom embedding entry points |
| macOS allocator | App uses mimalloc with explicit `v2` feature | Historical allocator comparison is not a fresh native result; cannot solve live-object retention |
| Doc mirror | Shared `Arc` transcript snapshots; lazy while unwatched | Last quiet watch drop does not eagerly clear the mirror |
| RPC doc/queue watches | Owned checked subscriptions and delta frames | Some unrelated production subscriptions still use legacy receivers |
| Chat sync scheduler | 28 local clients; 32 process sockets; paged durable jobs; bounded dials/HTTP; joined host workers | Limits transports/tasks, not aggregate payload bytes; registry HTTP fallback has separate ownership |
| Failed publication | Keeps edits and retries disk persistence | Intentionally can grow/pin memory until storage recovers |
| Chat2 migration | Thin rebuild; tool sidecars and stripped transcript output | Ordinary local full snapshots retain later history |
| Engine replacement | Explicit worker cancellation, client shutdown, severed sessions/doc-host back-edges | Requires shutdown lifecycle; arbitrary external Arc holders can still retain state |
| Terminal engine registry | 32 sessions; 1 MiB replay; exited TTL; shutdown disposal | Subscriber/raw queues and accumulated desktop histories remain outside those caps |
| Transcript UI warm cache | 12 transcripts / 64 MiB estimated bytes; ownership moved on navigation | Current selected transcript/derived structures are separate; this is not whole-UI RSS |
| Markdown render caches | Viewport/overdraw row retention for flattened text/code; style invalidation | Entire current transcript and parsed trees can still be large |
| Syntax | Source/span limits; shared/lazy grammar queries | Finite grammar initialization and active documents still cost memory |
| Generated/workspace media | Decode/pixel/byte budgets; asset release on retirement | Legacy user attachments use a different path |
| Preview mux | 64 streams, 64 KiB receive windows, cancellation and response-body guards | Limits are per peer; native WebRTC state needs its own profiling |
| Native browser | Explicit close/stop loading, observer/delegate cleanup; Linux route weak ownership | WebKit/Chromium child/cache footprint is not tested here |
| Voice | Desktop-only lazy model, 60 s audio cap; downloads streamed | Recognition model/ONNX working memory is a legitimate headed-only cost |
| Mobile client | Six quiet warm sessions, view/pending/streaming pinning, encoded attachment LRU | No aggregate document-byte ceiling; platform decoded image caches need separate checks |

These checks rule out several stale descriptions in the August memory plan.
They do not establish day-long memory stability on either reported platform.

## Reproduction

### Offline mechanism experiments

Each scenario runs in its own process with fresh temporary data and no model API
call. The terminal scenario emits only 16 MiB into a temporary shell and shuts
that terminal down. The diagnostic allocator applies only to this example.

```sh
cargo build --release --locked -p zeron-engine --example memory-audit
for scenario in docs journal outbox rpc terminal history sync sync-catchup; do
  target/release/examples/memory-audit "$scenario"
done
```

On Windows use `target/release/examples/memory-audit.exe` and a PowerShell loop.
These workloads demonstrate ownership/peaks. The history scenario replaces a
synthetic Loro metadata field; the outbox scenario measures the actual durable
loader and deep-copy cost, not a complete authenticated offline room.
The two sync scenarios use the actual chat client and durable outbox against a
local WebSocket fixture, with an accepting/discarding document sink. They need
no credentials or external network and test memory ownership, not convergence.

### Diagnose a Linux report before changing allocator settings

Record exact version/commit, executable path, architecture, libc, uptime,
number of live runs/terminals, recent operations, and whether the figure is for
one PID, the process tree, or a container/cgroup. Record the same process after
two quiet minutes, covering two normal trim intervals on this revision.

```sh
zeron --version
zeron sync > sync-before.json
# Set PID to the actual headless process, not its GUI or agent child.
ps -p "$PID" -o pid,ppid,etime,nlwp,rss,vsz,comm
cat "/proc/$PID/smaps_rollup"
cat "/proc/$PID/status"
cp "/proc/$PID/smaps" smaps-before.txt
# After the relevant workload and two quiet minutes, capture again:
zeron sync > sync-after.json
cat "/proc/$PID/smaps_rollup"
cp "/proc/$PID/smaps" smaps-after.txt
```

Interpretation:

- RSS dropping on minute boundaries, large arena mappings, and low live heap
  after work suggest allocator retention. Reserved virtual size alone does not
  establish resident arena usage. Attribute RSS/PSS in the mappings.
- Growing live heap dominated by terminal `String`/`Vec`/channel allocations
  suggests T1. Drain/drop the subscriber in an isolated reproduction.
- Large `PendingPush`/wire vectors and offline backlog suggest S1/S2.
- Loro/state/history plus retained transcript snapshots suggest D1/H1.
- Journal deserialization peaks correlated with opens/recovery suggest J1.
- Increasing tasks/watchers with no traffic suggests R1 or inaccessible tabs.

For allocation attribution, build a release binary with debug information in a
separate target directory and run an **isolated** headless reproduction under
heaptrack. Capture both retained allocations and peak allocations. The GUI
binary brings graphics/native initialization into profiling; `headless` should
be the first controlled run. Do not infer a leak from heaptrack's cumulative
allocated-byte total; freed churn counts there too. The diagnostic example's
System allocator is likewise not a test of Linux trimming or macOS mimalloc.

`zeron sync` currently exposes document/client counts and retention reasons,
not the byte counters needed to settle this report. Add pending outbox bytes,
terminal raw/subscriber/replay bytes, mirror bytes, per-doc state/history size,
checkout patch bytes, RPC queued bytes and task counts before a long soak.

Do not recommend `MALLOC_ARENA_MAX=2` as the universal fix. The historical repo
comparison measured a substantial CPU penalty from arena-lock contention. The
existing periodic trim was chosen after that comparison. Verify the executable
actually includes the trim commit before drawing conclusions about its effect.

### Diagnose a macOS report

Separate an external daemon, the headed process, agent children, and WebKit
children. A headed app embedding the engine includes both ownership domains.
Capture RSS and physical footprint consistently; compare the same display
scale/window size. Use the existing resource helper where available:

```sh
ps -p "$PID" -o pid,ppid,etime,rss,vsz,comm
footprint -p "$PID" > footprint-before.txt
vmmap -summary "$PID" > vmmap-before.txt
```

Repeat after streaming stops, after switching away from the image-heavy chat,
and after closing relevant terminals/browser views. For GUI runs use
`ZERON_GPU_STATS=1` to separate renderer allocations, and the native helper in
`scripts/macos-resource-stat.c` for the same physical-footprint metric as the
existing profile. Rust allocation counters alone cannot account for Metal,
IOSurface, WebKit, or compressed memory charges.

The checked-in native foreground measurements in `performance-macos.md` ended
around 323–332 MiB UI physical footprint on their short workloads. They were
not eight-hour soak tests, and are not a universal desktop memory floor or
evidence that a new report is invalid.

### Workload matrix for the follow-up native soak

| Scenario | Controls | What must be checked |
| --- | --- | --- |
| Empty offline headless idle | Fresh profile; no agent; ten minutes | No linear growth in live allocations or tasks |
| Many quiet chats | Fixed content bytes; 20 then 100 opens; detach watches | Resident count eviction; mirrors release; bounded warm slope |
| One long chat | Fixed emitted bytes; repeated turns/tool updates | State versus history; peak publication/persistence copies |
| Offline streaming | Healthy disk; unavailable sync; fixed bytes | Durable outbox increases; in-memory in-flight bytes stay bounded after a fix |
| Disk persistence failure | Dedicated disposable store; controlled fault | Visible failure; unsaved edits retained; recovery drains the backlog |
| Slow terminal viewer | Fixed firehose; stalled/quiet/disconnected subscriber | Raw and subscriber byte limits; no silent terminal corruption |
| Quiet watcher churn | Open/close file views; no filesystem events | Server requests and OS watcher counts return to warm baseline |
| Large journal tail request | Same fixed file; small `afterSeq` tail | Peak should depend on tail/window, not whole file after J1 is fixed |
| Slow checkpoint and active room | Fixed checkpoint/row bytes | Bounded catch-up bytes; successful replay/resume |
| Image browsing | Known image dimensions/formats; scroll then switch | Decoded/GPU memory budget, viewport protection, immediate trim |
| Terminal create/exit/delete | Fixed width/history; many chats | UI emulator histories and tasks reclaim after deletion/retirement |
| Multiple checkouts/build storm | Known patches; identical event rate | Aggregate diff bytes; conflating kicks; no event-proportional queue growth |
| Sleep/wake/reconnect | Same network and workload before/after | Clients, tasks, buffers return to their bounded warm set |
| Mobile layout view churn | Same fonts; repeated view destruction | Leaked font byte slope becomes zero after M1 is fixed |

Run baseline and candidate sequentially, with immutable executable hashes,
same fixture digests, no simultaneous builds, and separate child-process
samples. Include at least an eight-hour mixed-use run after targeted bounds
are fixed. Test limits with slow consumers; a fast mock consumer cannot exercise
the main unbounded-queue modes.

## Suggested implementation order

1. Contain T1; add byte counters and a meaningful stalled-consumer stress check.
2. For the reported sync difference, prioritize S1 outbox paging and S2
   catch-up byte bounds, with online/stalled/offline comparisons.
3. Fix R1 owned cancellation and remaining legacy production subscriptions;
   prune deleted terminal/session ownership in T2/E1.
4. Stream J1 tail/replay; correct D1 accounting and quiet mirror release.
5. Normalize/budget I1 user media; use viewport retention and explicit originals.
6. Address S3 registry and aggregate RPC/diff budgets and stdio input bounds;
   design safe ongoing history reclamation; fix mobile font ownership separately.

Each change should carry an actual ownership/byte-bound regression scenario,
matching transcript/terminal output checks, and before/after native profiling.
Hard process RSS thresholds need platform/workload context; bounded queue bytes,
zero abandoned leases, and flat post-warm growth are more portable acceptance
criteria. Reclaimed objects may leave allocator pages resident temporarily.

## Validation and limits

The engine/document/RPC/sync library suites were run in release mode with
locked dependencies at `69e64ef5`: **553 passed, one ignored, zero failed**
(document 129; engine 344 passed/one ignored; RPC 16; sync 64).
Those suites validate existing lifecycle/protocol behavior, not absence of the
remaining growth paths. The ignored test is
`chat_persistence::tests::real_whale_replay_keeps_146_heartbeats_running_on_two_workers`;
it requires a privately supplied `ZERON_WHALE_SNAPSHOT`, unavailable here.
All eight diagnostic scenarios built and completed successfully. The example
passed `rustfmt --check`; report links and raw JSON were checked locally.
These components are unchanged on final audited main `1c83cda2`. UI tests for
the intervening upstream rename change were not executed during this audit.

No native Linux RSS/heaptrack run, native macOS/Metal run, multi-hour soak,
authenticated live model turn, or user data inspection was performed. No
production fix is claimed. The source audit and finite experiments establish
several reproducible mechanisms; they do not identify the exact cause on a
reporter's machine without version/workload/profile evidence.

## References

- [Existing memory plan](memory-plan.md): historical allocator and document
  measurements; several earlier findings have since been fixed.
- [Resource profiling](performance-resource-usage.md) and
  [native macOS follow-up](performance-macos.md): workload-limited checked-in
  CPU/footprint measurements and reproduction fixtures.
- [Sync resource resilience](sync-resource-resilience.md): current admission,
  document leases, persistence failures, and teardown contracts.
- [Linux `malloc_trim` manual](https://www.man7.org/linux/man-pages/man3/malloc_trim.3.html):
  attempts to return free pages across arenas; it does not free live objects.
- [Mimalloc environment options](https://microsoft.github.io/mimalloc/environment.html):
  purging options are allocator/version-specific, not object lifetime fixes.
- [Loro shallow snapshots](https://www.loro.dev/docs/concepts/shallow_snapshots)
  and [encoding modes](https://loro.dev/docs/tutorial/encoding): full snapshots
  preserve history; shallow snapshots discard history before a frontier.
