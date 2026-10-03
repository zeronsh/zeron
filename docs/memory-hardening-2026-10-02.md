# Memory hardening — 2026-10-02

Branch `codex/memory-hardening` starts from upstream main
`1c83cda221bd18a3b55ea49e5b25ec2c148955bc` (0.2.102).
The [comprehensive baseline audit](memory-audit-2026-10-02.md) records source
evidence, reproducible growth mechanisms, and the archived before measurements.

## Changes

* Chat sync retains a 256 KiB / 32-batch window over its durable SQLite outbox.
  A single legal large row can exceed that target. Stable batch IDs, ordinal
  order, checkpoint-only rows, and failed-write recovery are preserved. Healthy
  ACKs refill the window; HTTP fallback drains successive windows. Transport
  copies share payload ownership. Offline history recovery imports bounded pages.
* Checkpoint catch-up stops reading after 256 KiB / 32 frames while its fetch is
  pending. Socket queues shrink from 64 to 2 frames to propagate backpressure. Checkpoint
  downloads reject bodies over the server's 32 MiB limit.
* PTY readers use a 256 KiB queue; output batches flush at 16 KiB. Viewers share
  a 32-event broadcast ring and 1 MiB replay. Slow viewers receive a visible gap
  and parser reset. Closing Windows ConPTY continues draining discarded output
  so bounded backpressure cannot deadlock native cleanup. Subscriber admission
  caps at 16 per terminal.
* Quiet, detached transcript mirrors are cleared on the idle sweep. The warm
  document navigation cache shrinks from 12 to 4; watched documents, writers,
  active sync, and failed persistence remain protected.
* Journal tail, last-event, and reopen scans retain one line instead of loading
  and decoding the entire historical file. Replay skips old payloads before
  deserializing them. Full requested replay is still proportional to its result.
* Dropped unary RPC futures and harness requests remove pending registrations.
  Production UI streams use scoped subscriptions. Reused server request IDs
  abort the previous task rather than orphaning it. Stream and transport queues
  shrink from 256 to 8 ordered frames; full transcript frames no longer multiply
  into hundreds of MiB behind a slow reader.
* File watcher notifications coalesce into one pending kick. Deleted chats
  release UI terminal tabs after registry hydration.
* User images are decoded with allocation/dimension limits, normalized into
  PNG previews up to 2048 pixels per side (SVG viewport up to 512), and charged
  for decoded CPU/GPU pixels. Only rendered transcript rows shield user images;
  shield removal immediately trims the cache. Animated read-back previews show
  the first frame; the durable original file is preserved.
* Font books own font bytes instead of leaking each registered font forever.
  A regression checks repeated owner destruction using weak references.

## Validation and measurements

Local verification uses Windows x86-64, Rust 1.99.0, release, locked dependencies.
The diagnostic allocator measures **requested live Rust bytes**, excluding RSS,
native/C allocations, GPU allocations, allocator overhead, and child processes.
The original baseline binary is retained outside the repository and its SHA-256
matches the archived JSON. [After measurements](performance/memory-hardening-2026-10-02.json)
record code revision `8204d3a8`, exact source/binary hashes, and all 33 samples.

| Offline fixture | Before | After | Metric |
| --- | ---: | ---: | --- |
| 32 MiB outgoing edits while disconnected | 32.03 MiB | 276.7 KiB | Live Rust bytes |
| 16 MiB terminal output, stalled viewer | 24.45 MiB | 0.99 MiB | Live Rust bytes |
| Quiet transcript watches dropped | 149.48 MiB | 17.02 MiB | Live Rust bytes |
| Checkpoint gated while 64 rows catch up | 17.09 MiB | 2.05 MiB | Live Rust bytes |
| Reopen a 16 MiB journal and append | 32.14 MiB | 192.3 KiB | Peak Rust bytes |
| 16 MiB outbox loaded plus transport copy | 32.01 MiB | 257.9 KiB | Live Rust bytes |

The sync backlog remains durably queued (128 batches); catch-up reaches cursor
64 after releasing its checkpoint gate. The outbox comparison deliberately
uses the old full loader/copy versus the new production window/shared payloads.
Terminal output outside the bounded ring/replay becomes an explicit gap rather
than retained backlog. Older UIs do not understand the new gap notice; engine
and UI should be upgraded together for that notice and parser recovery.
Full CRDT replacement history remains unchanged (8.06 MiB snapshot for a visible
64 KiB field), illustrating a remaining protocol-level cost.

Windows regressions pass: engine 346 (one private-fixture test ignored), RPC 18,
sync 66, harness 299, text 16, and UI 1,461 (five existing tests ignored).
The unchanged document library's 129 tests also passed during initial validation.
New tests cover quiet cancellation, ordered/backpressured RPCs, bounded durable
windows and HTTP drain, rejected-row recovery, lag/exit ordering, large/torn
journal tails, font destruction, image normalization, and ANSI gap recovery.

The PR's mobile-core integration check exposed a timing assumption: transcript
publication can precede the separately derived composer/workspace status. The
round-trip test now waits for the adopted echo, reply, connected room, and idle
composer together before asserting them. The full local text/markdown/client/
mobile suite passed (185 tests, one existing test ignored), and the affected
round-trip test passed 20 consecutive runs.

[Memory checks](../.github/workflows/memory-checks.yml) runs Linux core regressions,
Valgrind Memcheck on nine finite offline scenarios plus font ownership, and
Massif on document, sync, catch-up, and complete backend replacement workloads.
Its artifacts retain the exact commit, binary hash, versions, XML errors,
allocation samples, and Massif stacks. Definite/indirect leaks and invalid memory
access fail the check; still-reachable allocations require the retention analysis
and cannot be dismissed solely because Memcheck passes.

The [final Linux run](https://github.com/katulevskiy/zeron/actions/runs/37089996172)
passed on `8b347ef0`: **zero Memcheck errors** in all nine backend scenarios
and the font lifetime test, with all four Massif profiles completed. Linux
library regressions passed: engine 382 (two existing tests ignored), harness
315, RPC 18, sync 67, text 16. The
[native evidence summary](performance/memory-valgrind-2026-10-02.json)
records versions, binary/XML/profile hashes, allocator samples, and test counts.
The CI artifact contains the unstripped binary and complete native traces.

Earlier document runs reported ten uninitialized conditions at
`DocHost::open_local`. Replaying the exact binary with precise definedness checks
reproduced them. Disassembly showed a read of the absent chat row's `room_gen`
stack storage. Extracting just the routing scalar inside an explicit workspace
branch removes that read and releases the row's strings/config before document
loading. The final run uses ordinary Memcheck definedness checks; no error
suppression was added. Changes after its measured revision only format Rust,
finish documentation, replace temporary fork CI wiring with main/PR wiring,
and harden that client integration test's wait.

Massif's useful-heap peaks (including native SQLite allocations, excluding
stacks) are 56.48 MiB for the document fixture, 3.95 MiB for disconnected sync,
2.73 MiB for checkpoint catch-up, and 383.3 KiB for empty backend replacement.
Document peaks remain dominated by Loro storage; the disconnected sync peak
includes SQLite's cache. These fixture peaks are not process RSS or a guarantee
for a production account.

## Remaining limits

These changes address reproduced mechanisms, not an attribution of any specific
reporter's 4 GB RSS. A connected production account and reporter workload are
still needed for that attribution. This environment cannot run native macOS
Instruments; browser/GPU/ONNX/subprocess memory is outside these allocator samples.

Full CRDT operation history and pinned active documents can still dominate RAM.
Uncoordinated history compaction could fork sync lineage or discard offline edits,
so it is intentionally deferred to a protocol-aware migration. The registry
metadata outbox, aggregate checkout diff caches, full requested RPC/journal
results, and completed session configuration also remain candidates for further
budgeting. Unsaved edits are retained on disk failure rather than discarded to
meet a RAM target. Cache limits are targets, not a global process RSS ceiling.
