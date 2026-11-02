# Pruned workspace watches

Diff-sync and workspace-files register their source trees with `FsWatchHub::watch_tree`.
The shared `ignore::WalkBuilder` walk excludes gitignored directories and does not
follow symlinks. Both consumers use that same walk for their initial 8,000-directory
budget. An ignored `target/` or `node_modules/` no longer disables live source edits.
`ignore` does not skip `.git`, so an inline checkout's git directory is part of the
root walk (and its budget); diff-sync still registers the git directory explicitly,
which is a deduplicated no-op inline and the only coverage for a linked worktree.

On Linux and kqueue, the hub installs NonRecursive watches and shares one native
watcher across every accepted directory and consumer. The legacy recursive API
retains its per-root isolation: `SHARED_WATCHER` remains false on Linux. A directory
path needed by multiple registrations is installed once, and events fan out to all
registrations covering it. Different tree filters retain their own delivery scope.

FSEvents and Windows keep native recursive root watches and filter event delivery
using the same pruned directory set. They do not allocate a watch per source
directory: per-directory FSEvents paths can exceed the stream's path limit, while
Windows relies on native recursion for reliable subtree delivery.

Directory create, move, delete and ignore-rule changes schedule a batched full
reconciliation of the directory set. Native events, including both rename paths,
are delivered unchanged. A rescan after reconciliation covers files populated
before a new watch attached. Overflow broadcasts notify's rescan signal to all
consumers, recomputes directory sets and rearms existing paths as well: a lost
remove/recreate may leave the same pathname referring to a new inode. Watch failures
leave the tree unhealthy, and the existing repair ticks remain enabled. Hitting the
inotify limit (`fs.inotify.max_user_watches`) logs a warning; the tree is not live,
diff-sync keeps kicking on every repair tick, and workspace-files keeps its 120 s
resync. A directory that cannot be read or watched (permissions) behaves the same
way, and recovers on the next re-walk.

A directory created after its tree was walked but before its parent's watch attached
raises no event and is not in the walked set. Each pass that installed watches
therefore walks the tree a second time and repeats the pass if the sets differ
(bounded per `apply`, then handed to the batch worker). A re-walk that finds the same
directory set sends no rescan.
Workspace-files converts notify's rescan flag into `resync_required` on its stream.
Its watch setup runs on the blocking pool and completes before the subscription's
initial resync baseline.

The ancestor-based watch-cover calculation retains `minimal_cover_full` as an
oracle. Debug builds compare every calculation; release builds with
`ZERON_PERF_VERIFY=1` sample 1/32. A mismatch logs an error and reconciles from the
full cover. The seeded test runs 300 seeds x 150 registration/mode/failure/drop
operations; `PERF_SEEDS=N` extends it.

## Search watcher boundary

`fff-search` is pinned to 0.11.0. It exposes watch subscriptions but no public event
injection API. The index updater (`handle_debounced_events`), watcher constructor,
and directory subscription methods are crate-private. Turning off `watch` would
leave file names and content search stale. Its independent watcher is retained.
On Linux fff already uses NonRecursive directory watches, rather than a blind
recursive notify walk. Consolidating that last instance requires upstream ingestion
support or a separately reviewed dependency change.

## Kernel measurement

The Linux-only test hook `inotify_counts()` reads `/proc/self/fdinfo`, identifies
inotify fds through `/proc/self/fd`, counts their watch-descriptor records, and
counts `notify-rs inoti` threads through `/proc/self/task`. These are native counts,
not registration counts.

Run the ignored fixture test alone because the counters cover its whole process:

```sh
cargo test -p zeron-engine --lib fs_watch::tests::fixture_inotify_counts_before_and_after -- --ignored --nocapture --test-threads=1
```

Use the unit's isolated jobserver, target, XDG, Git and TMPDIR environment for this
command and all other tests. The fixture contains `src/deep`, `.git/refs/heads`,
`target/debug/deps`, and `node_modules/pkg/lib`, with the last two trees ignored.
It measures the former two recursive notify consumers plus a real fff index, then
replaces only those two consumers with shared pruned registrations and an explicit
git metadata registration. The fff index stays alive throughout both measurements.

Measured on the Linux fixture (idle process baseline: zero watches/instances/threads):

| Native resource | Before | After |
| --- | ---: | ---: |
| Inotify watches, including fff | 27 | 9 |
| Inotify instances, including fff | 3 | 2 |
| `notify-rs inoti` threads, including fff | 3 | 2 |
| Watches for diff-sync + workspace-files | 24 | 6 |
| Instances / notify threads for those two consumers | 2 / 2 | 1 / 1 |

The retained fff instance contributes three watches and one notify thread in both
measurements. The hub's six watches cover only the source and explicit git metadata
directories; neither ignored dependency/build subtree is watched by the hub.

Linux event tests cover create, modify, file/directory rename, delete, populated new
subdirectories, changing ignore rules, symlink isolation, different consumer
filters, shared flat/tree paths, and injected overflow with a missed directory.
One test fills the real kernel queue (a handler stalls notify's reader while
`max_queued_events` entries are created, then a directory is made whose event is
dropped) and checks the overflow signal repairs the directory set; the others inject
notify's exact rescan event. macOS and Windows require separate native validation.

Validation on Linux, 2026-10-30 (all commands used the unit's isolated environment):

| Command | Result |
| --- | --- |
| `cargo test -p zeron-engine --lib fs_watch` | 26 passed, 0 failed, 2 measurement tests ignored |
| `cargo test -p zeron-engine --lib diff_sync` | 46 passed, 0 failed |
| `cargo test -p zeron-engine --lib workspace_files` | 58 passed, 0 failed |
| `cargo test -p zeron-engine --lib spaces` | 3 passed, 0 failed |
| `cargo test -p zeron-engine --lib fs_watch::tests::fixture_inotify_counts_before_and_after -- --ignored --nocapture --test-threads=1` | 1 passed, 0 failed |
| `cargo test -p zeron-engine --lib fs_watch::tests::register_unregister_cycles_leave_no_inotify_fds_or_threads -- --ignored --nocapture --test-threads=1` | 1 passed, 0 failed (200 cycles, instances/watches/threads back to baseline) |
| `cargo test -p zeron-engine --test diff_sync_churn` | 7 passed, 0 failed |
| `cargo test -p zeron-engine --test workspace_files` | 8 passed, 0 failed |

Ignore inputs inside a registered tree (`.gitignore`, `.ignore`, `.git/info/exclude`)
raise events and reconcile at once. Inputs outside every registered root (global
excludes, an ancestor `.gitignore`, a linked worktree's shared `info/exclude`) raise
none. Both consumers call `FsWatch::refresh` from their 120 s repair tick, which
re-walks the tree; a changed directory set attaches the new directories and rescans
the consumers, an unchanged one is silent. A change outside the roots is therefore
picked up within one repair interval (about 120 s plus the batch quiet time), not
whenever the next topology event happens. Without it diff-sync's gate would have left
a newly un-ignored directory unseen until the forced capture on every 10th tick
(about 20 min). Ignored subtree contents intentionally have no native watch and
retain repair-based resyncs.

Reading a directory raises Access events, including `Access` on `.git` itself for
the walker. Every consumer must drop them; the spaces watcher now does, since it
would otherwise re-probe git on every walk.

Known limits, not changed here: macOS and Windows are unvalidated; a pattern in an
ancestor `.gitignore` that happens to match a directory inside `.git` (`logs/`)
prunes that directory from the git-dir walk; force-added tracked files under an
ignored directory are only seen by the repair tick.
