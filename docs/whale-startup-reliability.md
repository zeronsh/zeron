# Whale startup persistence and networking

This change includes PR #385 (`efa1a0752`) on top of main (`d6c7b5ea9`).
#385 addresses transport progress, backpressure, reconnect/outbox replay and
HTTP fallback. It does not address the SQLite/runtime starvation described here.

## Failure and correction

The desktop bridge's pinned implementation uses two Tokio workers. The old
chat2 sink exports and synchronously writes the entire snapshot for each row.
A second store caller waits on the same `std::sync::Mutex<Connection>`, so both
workers can be unavailable to socket I/O, timers and updater requests. A 21 MB
document exceeds the edge's old 16 MiB checkpoint limit, and unconditional
startup cursor amnesty repeatedly downloads the uncheckpointed rows.

- A per-chat bounded wake queue coalesces replay changes on a one-second timer.
  ACK/checkpoint cursor boundaries bypass the timer. The cursor is captured
  before export; snapshot, cursor and a verification marker commit atomically.
  Failed writes remain dirty and retry, including when no more edits arrive.
- Snapshot jobs await a shared asynchronous gate, then export/write on the
  blocking pool. Remaining synchronous store APIs use `block_in_place` around
  the entire mutex wait and operation on multithread runtimes, allowing Tokio
  to hand their worker cores to other tasks. This preserves existing synchronous
  commit-before-send APIs. Current-thread runtimes retain synchronous behavior;
  production desktop and headless runtimes are multithreaded.
- Desktop owns a default-sized runtime and gives its handle to `gpui_tokio`.
  Large transcript/tail materialization and checkpoint preparation also run on
  the blocking pool.
- Verified snapshot cursors skip legacy amnesty. Old snapshots still receive
  one repair; missing causal dependencies and actual server resets still invoke
  recovery. The verification flag is an append-only SQLite migration, defaults
  false for old data, and is invalidated by legacy snapshot/cursor writes.
- Checkpoints accept up to 32 MiB. Transfer deadlines allow 300 seconds, with
  connect/read inactivity bounds and existing Range resume retained. This fits
  the reported whale; it does not eliminate document growth indefinitely.
- Updater connections time out after 15 seconds and stalled reads after 30
  seconds. There is no total download deadline: ongoing downloads can finish
  on slow connections.
- Shutdown interrupts a pending dial, cancels and joins HTTP fallback, then
  flushes open documents. An abrupt kill may leave the snapshot behind RAM;
  its atomic cursor causes missing remote rows to be replayed. Local outgoing
  edits retain the pre-existing durable outbox.

## Evidence (Linux, debug builds, 2026-09-18)

The private snapshot was read from a copy into a temporary store. No transcript
content or snapshot is committed. This is not a new-build test on work-laptop.

| Check | Result |
| --- | --- |
| Real 21,628,288-byte snapshot, two workers, 32 changes at eight/second, second store user, 146 simulated 50 ms room heartbeat timers | 4 snapshot writes; 13,432 heartbeats; worst interval 52 ms |
| Both workers contend on a connection held for 600 ms | Timer and real localhost TCP connection progress in under 300 ms |
| Same contention test with runtime handoff temporarily disabled | Fails: `SQLite contenders monopolized both runtime workers` |
| 1,000-row coalescing test | Zero immediate writes, one debounced write; urgent and shutdown flushes persist cursor |
| Concurrent cursor advance during export | First write keeps the pre-export cursor; next flush advances it |
| Verified cursor 69,000, checkpoint 53,984 | Requests rows after 69,000, no checkpoint fetch/replay |
| Real workerd checkpoint route | Accepts 21 MiB, advances checkpoint and trims covered row; Range resume returns correct bytes; rejects >32 MiB |
| Local HTTP/TLS stalls | TLS handshake, headers and body time out; slowly progressing response succeeds |
| Shutdown with pending socket dial and HTTP pull | Completes within 200 ms and drops fallback before returning |

The whale stress test applies metadata changes at the observed replay cadence
and exports the real snapshot. Its 146 heartbeat timers model scheduler demand;
they are not 146 production room sockets. The separate contention test exercises
real TCP progress. #385's transport fault suite also passes; its previously
recorded Cloudflare results are in `docs/transport-reliability.md`.

Regression suites passed: 1,077 UI tests (including animation/history behavior),
199 engine unit tests, 19 engine integration tests (profiles, restart, publication,
subagents), 53 sync unit tests, 14 RPC unit tests, 24 RPC/sync integration tests,
8 updater tests, and 56 edge tests. Fixture-dependent tests remain ignored by
default. Edge typechecking passes. The full workerd run exits successfully but
emits its Vitest shutdown-state diagnostic; the focused checkpoint run is clean.

## Reproduction

```sh
cargo test -p zeron-sync -p zeron-rpc --features zeron-sync/mock-server
cargo test -p zeron-engine --lib --test session_publication --test restart_resume --test local_profiles --test codex_subagents
cargo test -p zeron-update
cargo test -p zeron-ui --lib -- --test-threads=1
npm --prefix edge run typecheck
npm --prefix edge test
ZERON_WHALE_SNAPSHOT=/path/to/private-copy.bin cargo test -p zeron-engine --lib real_whale_replay_keeps_146_heartbeats_running_on_two_workers -- --ignored --nocapture
```

## Rollout

The larger checkpoint cap requires the edge deployment, and persistence/runtime
changes require updated desktop/host binaries. Dev must run a version that can
upload the whale checkpoint. No production deployment or work-laptop launch is
claimed by these local results. Historical tool-animation suppression is retained;
the existing UI history/replay tests pass.
