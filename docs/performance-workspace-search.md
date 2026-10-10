# Workspace search on fff

One engine-side index, [`fff-search`](https://crates.io/crates/fff-search)
`=0.11.0`, now serves every workspace search surface:

| Surface | RPC | Before | Now |
| --- | --- | --- | --- |
| Composer `@` | `SearchFiles` | `nucleo-matcher` over a walk cached for 10 s | fff names (files and folders) |
| File tree search | `SearchWorkspaceFiles` | a full disk walk per query | fff names; "show ignored" still walks |
| cmd+K Files | `SearchWorkspaceFiles` | — | fff names, focused chat's folder |
| cmd+K In Files | `SearchWorkspaceContent` (new) | — | fff plain-text grep, smart case |

`WarmWorkspaceSearch` (new) lets the UI start an index before the first query.
Both new methods are forwardable, so remote chats search on their host. Older
hosts answer `UnknownMethod`: warm-ups are skipped silently and the palette's
In Files section explains that the device needs an update.

The code lives in `crates/engine/src/workspace_search.rs` (`WorkspaceSearch`,
owned by `Repos`). `SearchFiles` and `SearchWorkspaceFiles` keep their wire
shapes; iOS's `@` uses `SearchFiles` and only sees the new ranking.

## Index lifecycle

One fff file picker per working folder (repository, worktree or plain
folder), keyed by the canonical root that `WorkspaceFiles::resolve_target`
returns:

- Created by the first warm-up or search of that root. It scans in the
  background and keeps itself current with its own file watcher.
- A search waits up to 300 ms (`SCAN_WAIT`) for the initial scan, then answers
  from whatever is indexed; content answers carry `indexing: true` then.
- Dropped after 15 minutes without a search or warm-up (`IDLE_EVICT`; a
  reaper thread checks every 60 s, so up to 16 minutes in practice).
- At most 7 live indexes (`MAX_LIVE`); creating an eighth evicts the least
  recently used. Each index runs its own file watcher, so the cap also bounds
  inotify instances.
- The UI warms the focused chat's index once, when the chat gains focus. There
  is no heartbeat or pinning: a focused chat left without searches for 15
  minutes loses its index and the next search (or refocus) rebuilds it.
- An index a chat warmed survives `release_unless_warmed`, which the
  new-project folder picker calls on the home index when it closes.
- Each index caps its cached file contents at 16 MB (`CACHE_BUDGET_BYTES`);
  fff would otherwise size it up to 512 MB.
- Eviction stops the scan and watcher, drops the index, and asks glibc to
  return the freed memory (`malloc_trim`) on Linux.

Fixed options: no content (bigram) index — see "Paths-only index" below —,
mmap warm-up off, no frecency database (`SharedDb::noop`),
git recency on, symlinks not followed. fff's tracing and panic hooks are not
installed.

## Measurements

`crates/engine/examples/workspace_search_bench.rs`, release build, Linux
(Fedora 44, x86_64), 10 timed runs per query after a warm-up; medians. "Before"
is the same benchmark at commit `0c6b94ed`, before the port. comet is this
repository (≈2.7k tracked files, 3.3k entries walked, 139k with ignored ones);
t3code is a ≈27.7k-file TypeScript monorepo.

| Median per query | comet before | comet after | t3code before | t3code after |
| --- | ---: | ---: | ---: | ---: |
| `SearchFiles` (`@`, 8 results) | 0.13–0.58 ms¹ | 0.70–1.57 ms | 1.0–31 ms¹ | 1.0–3.6 ms |
| `SearchWorkspaceFiles` (tree, ≤200) | 27.7–30.1 ms | 0.70–2.6 ms | 209–409 ms | 1.8–4.4 ms |
| …including ignored (walk) | 310–781 ms | 237–248 ms | 105–320 ms | 91–109 ms |
| `SearchWorkspaceContent` (100, 3 per file) | — | 0.17–0.43 ms | — | 0.72–1.99 ms |
| Index build (scan + content index) | 8–11 ms walk² | 51–61 ms | 29 ms walk² | 625–685 ms |

¹ The old `@` index was rebuilt by a full walk whenever it was older than 10 s;
the benchmark's runs all hit the cache. ² The old walk had no content index.

Queries: `composer`, `repos`, `cmdpal`, `Cargo.toml`, `workspace_files`.

| Process RSS | comet | t3code |
| --- | ---: | ---: |
| Baseline (no index) | 6.1 MB | 6.0 MB |
| Index built | 25.5 MB (+19.4) | 90.5 MB (+84.5) |
| After every query above | 39.3 MB | 123.0 MB |
| 2 s after evicting the index | 16.6 MB (+10.5) | 56.7 MB (+50.7) |

The residual after eviction is allocator state, not a live index: repeated
create/evict cycles of comet's index plateau at about +20 MB over the first
baseline after four cycles and stop growing (glibc keeps per-thread arenas
for fff's search and scan threads). Before, the old `@` cache cost 3.6 MB for
comet and 12 MB for t3code, but every tree query re-walked the disk.

Against the plan's targets: tree and `@` p50 ≤ 2 ms in comet — met (0.70–2.6
ms; the tree's `Cargo.toml` query, 200 results, is the slowest). Content p50
≤ 50 ms — met by two orders of magnitude. ≤ 20 MB per index in comet — met for
the index itself (+19.4 MB); query-time allocations add more until eviction.
Back within ±10 MB of baseline after eviction — not met on Linux (+10.5 MB
after one cycle, plateauing near +20 MB).

## Paths-only index

The tables above were taken with fff's content index on. It is now off:
indexes hold paths only and `SearchWorkspaceContent` greps the indexed files
without a bigram prefilter. Re-measured the same way:

| | comet, content index | comet, paths only | t3code, content index | t3code, paths only |
| --- | ---: | ---: | ---: | ---: |
| Index ready | 51–61 ms | 41 ms | 625–685 ms | 303 ms |
| RSS with the index | 25.5 MB | 19.5 MB | 90.5 MB | 76.7 MB |
| RSS after every query | 39.3 MB | 43.8 MB | 123.0 MB | 91.7 MB |
| `SearchWorkspaceContent` median | 0.17–0.43 ms | 0.48–2.3 ms | 0.72–1.99 ms | 1.1–21 ms |

Name searches are unchanged. Content queries with many hits stay near 1 ms;
rare or absent strings scan every candidate (~20 ms on t3code's 27.7k files,
page cache warm). A cold disk makes the first such query slower; that was not
measured. A full rescan of t3code drops from ~1.7 s to ~1.05 s of CPU, and
build CPU from ~2.0 s to ~0.9 s.

## Behavior changes

- Ranking follows fff: typo-tolerant fuzzy matching with filename, git-recency
  and modification-time boosts. The top results are better; a long tail of
  weak matches fills the tree's 200 slots where the old substring ranking
  returned fewer rows.
- An empty `@` query lists the chat's featured paths first, then recently
  changed files (it used to list the shallowest paths).
- Dotfiles are indexed inside git repositories only; in a plain folder fff
  skips them. `.git` is never indexed.
- Symbolic links are not indexed (fff does not follow them), so name and
  content searches skip them; the "show ignored" walk still lists them.
- The tree's "show ignored" mode keeps a direct walk (fff has no
  include-ignored mode), now ranked by plain case-insensitive substring: name
  prefix, then name, then path matches, shorter paths first.
- Binary files and files over 10 MB are never content-searched.

## Cost

- Dependencies: 53 new lockfile entries (most are Windows target shims),
  including C builds of `libgit2-sys`, `libz-sys` and `lmdb-master-sys` (a C
  compiler is required on every platform) and `notify 9.0.0-rc` alongside our
  `notify 7`. `nucleo-matcher` is gone.
- Threads: fff's global search and background pools, plus one watcher per
  live index (at most seven).

## Reproduce

```text
BENCH_ROOT=/path/to/checkout cargo run --release -p zeron-engine \
    --example workspace_search_bench
```

`BENCH_QUERIES=a,b,c` changes the queries and `BENCH_RUNS` the timed runs.
Index creation and eviction are logged at `info` (`workspace search index
created` / `scanned` / `evicted`, with root, file count, scan time and arena
bytes).
