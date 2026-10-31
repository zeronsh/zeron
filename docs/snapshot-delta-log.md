# Local chat snapshot delta log

Chat2 engine documents keep the existing one-second persistence cadence. A save
samples the room cursor, then the document version vector, and exports Loro
updates since the last successful save. It advances the saved version only after
SQLite commits. Each row also stores a shallow registry of root container IDs:
accessing an empty Loro root creates no operation, but a full snapshot preserves
that root in its deep value. Replaying the registry preserves this case without
walking the transcript. The registry has its own fixed FNV-1a integrity checksum,
in addition to Loro's update checksum. Its bytes count toward the chain threshold and measured
write volume. Operations included by a racing export can appear again in the
next update; Loro imports are idempotent. A cursor reset is stored in row order,
so recovery uses the last valid row's cursor, rather than the largest cursor.

The base remains an ordinary `snapshots` row. Migration 9 adds
`snapshot_deltas(doc_id, seq, bytes, cursor, roots)` and base replacement/deletion
triggers. Each appended row and its cursor commit together. Base replacement,
cursor/epoch/verification metadata, and delta deletion commit in one transaction.
The first save of a new document writes a base. Reopening seeds the persisted
version from the imported disk state **before** replaying any publication outbox.

Compaction runs after 30 seconds without save activity, on shutdown/eviction, or
when the next delta would exceed 256 rows or
`max(512 KiB, base_snapshot_bytes / 2)` total delta bytes. A clean chain still
compacts on shutdown. Legacy s2 documents and the thin client's writer retain
full saves; their readers, local import, salvage and inspection examples understand
the new log. All full-save APIs invalidate existing deltas atomically.

A reader fetches the base and ordered deltas in one SQLite read transaction,
then releases the connection before importing. It checks sequence continuity,
Loro checksums, root registry decoding and causal import completion. A damaged row ends the usable
prefix, including its cursor; later rows cannot advance past it. If importing
that row partially changed Loro, recovery reconstructs the known-good prefix.
The engine replaces a damaged chain before appending again. SQLite WAL
transactions already make interrupted inserts/compactions atomic; checksum
checks additionally contain truncated/copied payloads. An unreadable base
remains an error, as before.

The debug equivalence oracle restores the committed chain and compares its
version vector and deep value with a full export at that exact frontier. It also
requires the chain to include the version sampled before export. Release builds
run this every 32 delta saves when `ZERON_PERF_VERIFY=1`. A mismatch logs an error,
writes a full snapshot, and resets the chain; a failed fallback remains dirty
and retries on the existing cadence.

## Migration and downgrade

Existing snapshot-only databases load with the same state, cursor and epoch;
there is no snapshot rewrite during migration. Older engines ignore the new
table and read at least the last full snapshot, with that snapshot's own cursor.
After a clean new-engine shutdown they see the compacted latest saved state.
After a crash or forced stop, they cannot read newer delta-only changes: the
base can lag by the bounded chain (including a long, slowly changing turn).
An acknowledged remote update can be fetched again from the base's older room
cursor. Unsynced delta-only state is unavailable to a downgraded binary.

If an older engine saves or deletes the base, the database triggers remove the
stale delta chain in the same transaction, even though that engine does not know
about it. A subsequent upgrade cannot merge that stale chain onto a replacement
lineage. A save after a forced downgrade discards that delta-only chain;
recovery of those changes then depends on the independent outbox/remote history.
Running two engines against the same data directory is unsupported.

## Publication outbox

`chat_outbox` remains the independent durable publication obligation. Appending
or compacting the snapshot log does not acknowledge, delete or send its rows.
Delta and full saves use the existing outbox-writer barrier and failed-publication
marker handling. The outbox's insert-before-send ordering is unchanged. Delta inserts/updates/deletes
invalidate salvage receipts in the same transaction. A new receipt is issued
only for an exact healthy base/epoch with no outbox or delta rows; the conditional
insert rechecks both logs atomically, so a concurrent append cannot certify stale
content. Receipt reads also ignore receipts while deltas exist, including receipts
written by an older binary that inspected only the base during a downgrade.

## Validation and write-volume measurement

`delta_log_seeded_crash_reopen_equals_full_snapshot` runs 300 seeded xorshift
sequences of 150 map/text/list/peer operations, randomized persistence/crash
points and compactions. Reopening the log must match a separately saved full
snapshot in both version vector and deep value. `PERF_SEEDS=N` increases the run.
Other tests cover interrupted insert/compaction transactions, truncated delta
rows, migration, old-engine writes, threshold and quiet compaction, failed saves,
outbox replay, and a transcript present only in a delta during open/salvage.

`streaming_five_mb_store_write_volume` starts with 2,500,000 seeded ASCII bytes
in a LoroText, producing a measured 5 MB serialized snapshot (Loro includes state
and operation history). It streams eight 108-byte commits per simulated second, with one
save per second for 12 seconds. It compares the actual `ChatPersistence` delta
writer with the prior full-export/full-upsert path. Both use the same edited doc.
`DocsStore::snapshot_payload_bytes_written()` counts committed payload bytes
submitted to SQLite (base snapshots plus deltas); it excludes initial seeding,
SQLite pages/indexes/WAL amplification and the separate outbox. Shutdown
compaction is reported separately. This measures write volume, not throughput
or elapsed latency; debug oracle exports are enabled during the test.

Measured on Linux with the debug equivalence oracle enabled:

| Store payload measurement | Full saves | Delta log |
| --- | ---: | ---: |
| Initial base snapshot (excluded from stream totals) | 5,000,477 B | 5,000,477 B |
| 12 simulated streaming seconds | 60,009,011 B | 13,475 B |
| Mean streaming bytes per second | 5,000,750.9 B/s | 1,122.9 B/s |
| Shutdown compaction (separate) | — | 5,000,784 B |

The streaming window contains 12 deltas and does not cross a compaction
threshold. Full compaction still costs O(document size), paid separately at the
listed lifecycle/size/count boundaries. These are committed store payload bytes,
not filesystem/WAL bytes or measured wall-clock throughput.

Final validation (worker-rules jobserver and isolated XDG/git/temp/data environment):

| Command | Result |
| --- | --- |
| `cargo test -p zeron-sync --lib` | 83 passed, 0 failed; 300 seeds × 150 ops |
| `cargo test -p zeron-engine --lib chat_persistence -- --nocapture` | 12 passed, 0 failed, 1 ignored |
| `cargo test -p zeron-engine --lib doc_host::` | 66 passed, 0 failed |
| `cargo test -p zeron-engine --lib chat2_host::` | 6 passed, 0 failed |
| `cargo test -p zeron-client --lib live::room::tests` | 1 passed, 0 failed |
| `cargo build -p zeron-sync --example doc_surgery --example transport_live` | passed |
| `cargo build -p zeron-engine --example doc_inspect` | passed |

The ignored real-whale heartbeat test requires a privately supplied
`ZERON_WHALE_SNAPSHOT`; the seeded 5 MB fixture needs no external data. Crash
coverage uses transaction rollback, injected compaction failure and truncated
payloads, rather than a physical power-cut test. The salvage review fixes through
`60d1a4e2f` are included in the base of this change.
