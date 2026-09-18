# Whale session blank-on-revisit regression

Base: `c24e0e26` (main, v0.2.76). Verified on Linux with synthetic local data;
`work-laptop` was not reachable from this environment, so this is not a claim
that its particular session or installed application was tested.

Two independently reproducible defects were found:

- `AppState::select_chat` discarded the transcript on every navigation. Even
  with a warm local engine, revisiting required a new full RPC reset before
  anything could render.
- `publish_messages_if_watched` checked the receiver count and cleared the
  mirror outside the lock used by `watch_messages`. A worker that observed
  zero receivers could mark dirty, let attach rebuild and clear dirty, then
  overwrite that freshly published transcript with an empty snapshot. A quiet
  document could remain empty until the next change.

The UI now moves recently viewed transcripts into a cache (12 inactive chats,
64 MiB estimated content budget). Navigation restores the destination in the
same state update, without waiting for RPC; fresh resets still replace it,
including authoritative empty resets. Restored content is classified as
historical so it does not replay entrance animations. Deleted chats and
account/runtime replacement clear cached data. Watches still end on navigation.
The engine serializes receiver checks, clearing, attach, and materialization
under the same mutex.

## Before/after proof

The new regression tests were retained while the affected production methods
were temporarily restored from the base commit (`select_chat`, `watch_messages`,
and the publication methods). Both failed:

```text
whale_transcript_revisit_is_synchronous_and_fresh_reset_wins ... FAILED
assertion `left == right` failed: revisit must render before any RPC frame
  left: 0
 right: 2000

transcript_attach_and_unwatched_clear_share_a_critical_section ... FAILED
unwatched clear escaped attach's critical section
```

With the fixes restored, both pass. The UI test switches away and back ten
times with 2,000 messages / 4,096,000 text bytes and no engine connection.
It checks synchronous availability, preservation of the original allocation,
historical classification, replacement by fresh data, and an empty reset.
The existing tool-group revisit view test additionally checks that rendered
rows exist before a new watch frame arrives, while preserving its historical
and live animation assertions.

Additional tests cover count/byte eviction, unloaded sessions, chat deletion,
account replacement, and a persisted 2,000-message snapshot opened with no edge
configured. One debug run measured 36.47 ms for offline cold open and 35.06 ms
for rebuilding the cleared mirror; these are observations, not timing gates or
laptop performance claims.

## Reproduce

```sh
cargo test -p zeron-engine --lib -- --nocapture
cargo test -p zeron-ui --lib --no-default-features -- --test-threads=1
```

Initial revisit validation: 196 engine tests and 1,075 UI tests passed. The focused transcript view
suite also passed all 107 tests. Changed Rust files pass rustfmt; `git diff
--check` passes.

A cache miss (first visit, process restart, or budget eviction) still reads the
local engine snapshot. The cache is a bounded presentation optimization, not a
replacement for durable local storage or authoritative synchronization.

## First-open follow-up: actual whale snapshot

The revisit fixes above did not address the first-open cost. A read-only copy of
the locally available **Zlang Performance And Safety** snapshot was profiled
without connecting to the edge. No transcript content or snapshot is committed.
It contains 37,630 parts across just 16 joined messages (21,628,288 snapshot bytes).
The original 2,000-message fixture did not represent this shape.

Linux debug measurements before first paint (not a laptop or rendered-frame benchmark):

| Stage | Original full opening | New 128-part opening |
| --- | ---: | ---: |
| Snapshot file read | ~16 ms | ~16 ms |
| CRDT import | ~24 ms | ~24 ms |
| Transcript materialization | ~604 ms | ~2.2 ms |
| JSON encoding + decoding | ~1,007 ms | ~2.6 ms |
| Opening response | 16,414,959 bytes | 44,127 bytes |

The bottleneck is whole-history materialization and transfer before anything can
be displayed, not disk I/O. In addition, the viewport builds presentation rows
from the opening response; that rendering cost is not included in this probe.

The desktop now opts into `WatchDocMessages { openingTail: true }`. The engine
reads the last 128 parts directly from local CRDT containers, preserving part
contents and continuation root IDs, and yields that provisional frame before
building the full mirror. The next frame is the ordinary complete reset, then
normal live deltas. Full snapshot opening/materialization runs on the blocking
pool and UI typed-frame decoding runs on the background executor. Nothing is
truncated in storage, and no remote response gates this local opening.

The provisional frame carries `historyPending: true`. The UI never uses it to
replace a complete cached/reconnecting view, never caches it as complete, and
keeps saved-scroll fallback gated until the full reset arrives. Clients that do
not opt in retain their existing full-reset protocol.

New tests prove the first opening arrives while full publication is held
blocked; every original part and a write between preview/attach appear in the
full reset; subsequent live deltas still arrive; previews preserve continuation
IDs and leave the snapshot unchanged; and incomplete history cannot overwrite
or contaminate the full UI cache.

Reproduce the read-only timing comparison with an exported snapshot:

```sh
cargo run -p zeron-engine --example transcript_load_probe -- /path/to/session.bin
```

The new path removes the full-history barrier to initial content. It does not
claim that reconstructing/rendering all 37,630 parts is instantaneous, nor that
work-laptop's installed build was tested.

Final follow-up validation: 108 document, 197 engine, and 1,076 UI library tests pass.

## Navigation animation preservation

`opening_tail_full_history_and_cached_revisit_never_replay_tool_entrances`
exercises the production opening-update reducer and transcript presentation:
preview → full history → three cached revisits → a new live tool. It prepends
an older tool group so the preview's tail tool changes group index when full
history arrives. At every historical stage, all header and tool entrance epochs
remain absent; after the live delta, only the new tool has an entrance epoch.
This new test and all 11 existing `tool_group` tests pass. No animation behavior
change was needed; both opening frames carry historical part baselines, and
cache restoration captures its own baseline before presentation.
