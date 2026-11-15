# Git History scroll investigation (2026-11-15)

The investigation sections record the behavior before optimization. The
implementation and repeated before/after measurements are documented below.

The Git plugin can exhaust its default 32 MiB budget with 1,000 commits.
That count is fixture-specific, not a threshold: a later synthetic graph
test also exhausted it with only 100 commits (see below).
This is reproducible with the current pinned zgx revision
`d391ea7276f0de5cb3d65c191bfd66c02a4c358d`. It is not yet proof of the
reported persistent slowdown of the entire native window.

History virtualizes its rows (`plugins-builtin/git/history.luau`, `ui.list`,
overscan 8). The framework removes rows outside the current window. Its
data is not bounded: `repo.luau` appends pages of 200 commits and republishes
the accumulated history. The `laid` computed holds the graph and commits;
`keys` and `index` hold additional derived collections. Each append rebuilds
these over the full history. `MAX_LANES = 8` bounds drawing, not graph layout
or storage.

The framework meters Rust state values together with the plugin's Luau
heap. Near its budget, `zgx-script/src/interrupt.rs` runs a full Luau
collection synchronously at an interrupt safepoint. This can cause UI pauses,
but collection time was not isolated in this experiment.

## Measurements

Ran the existing ignored `git_scroll_cost` release benchmark with real
commits from this checkout (`git log --topo-order`, newest first), 240 scroll
steps of 48 pixels. The fake Git service returned the full selected history
in one response; this measures loaded history, not incremental pagination.
GitHub decorations were disabled. These are headless CPU measurements, not
native presentation times or FPS.

| Commits | Plugin budget | Scroll p95 | Result |
| --- | --- | --- | --- |
| 200 | 32 MiB | 4.99 ms | Pass |
| 600 | 32 MiB | 2.67 ms | Pass |
| 800 | 32 MiB | 2.54 ms | Pass |
| 1,000 | 32 MiB | Invalid | Memory exhaustion while building graph keys |
| 1,000 | 128 MiB | 3.09 ms | Pass |
| 3,135 | 128 MiB | 3.19 ms | Pass |

The failure names `history.luau:1060`, inside the `keys` computed:
`git: memory error: not enough memory`. The failed run displays only three
texts and must not be interpreted as a fast scroll result.

Successful runs mounted 132 texts at rest and at most 150 during the longer
scrolls, despite thousands of commits. This supports working virtualization;
text count is not a count of every scene node. Hover p95 was about 1.1 ms
with 600 and 800 commits. Persistent global scroll degradation was not
reproduced by these workloads.

## Diagnostic changes and follow-up

`git_scroll_cost` now reports total script heap bytes before and after
scroll, and after collection. `GIT_PERF_MEMORY_MB` overrides only the test
plugin budget; the application still uses its existing limits. Example:

```sh
GIT_PERF_JSON=/path/to/git-world.json GIT_PERF=history,hover \
  GIT_PERF_MEMORY_MB=128 cargo test --manifest-path apps/zgx/Cargo.toml \
  --release --bin zeron-zgx git_scroll_cost -- --ignored --nocapture
```

The JSON fixture uses the shape documented by `git_world` in
`apps/zgx/src/plugins/examples_tests.rs`. Measurement logs from this session
are `/tmp/zgx-history-*.log`; generated fixtures are `/tmp/zgx-history-*.json`.

The next native trace should include repeated pagination, GitHub enabled,
and scrolling another panel before and after closing History. Inspect plugin
memory, GC pauses and GPU/compositor work separately. A production fix should
reduce full-history derived state and repeated conversion; increasing the
budget alone only moves the failure point.

## Follow-up: 100 commits and graph complexity

The first experiment did not establish a minimum failing commit count.
Follow-up workloads kept 100 commits and varied their parent topology.
The real fixture is the newest 100 commits of this checkout; the synthetic
variants keep their metadata but replace parents. All use 32 MiB unless
otherwise specified.

The benchmark now supports `GIT_PERF_BOUNCE=1` (reverse every 64 steps),
`GIT_PERF_STEPS=1200`, and a top-level `github` mock response in the input
JSON. It reports each VM heap before History, at rest, after scrolling,
after collection, and sampled every 40 steps outside the timed frames.
An additional real-history run traversed the list for 12,000 steps.

| 100-commit fixture | Graph segments | Sampled peak Git VM heap | CPU scroll p95 |
| --- | --- | --- | --- |
| Synthetic linear chain | 198 | 12.5 MiB | 2.05 ms |
| Real current history | 640 | 13.4 MiB | 2.06 ms |
| Real `git log --all --topo-order` | Not counted | 13.2 MiB | 2.25 ms |
| Real current history + mocked GitHub | 640 | 13.3 MiB | 3.15 ms |
| Synthetic chain + one distinct pending parent per commit | 5,248 | 18.4 MiB | 2.05 ms |
| Synthetic chain + four distinct pending parents per commit | 20,398 | 19.7 MiB | 1.99 ms |
| Synthetic chain + eight distinct pending parents per commit | 40,598 | Memory error at 32 MiB budget | Invalid |
| Same eight-parent graph, 128 MiB budget | 40,598 | 50.1 MiB | 2.13 ms |

Segment counts come from a reference calculation of the layout algorithm
over the fixture topology. The synthetic graphs intentionally leave many
parents beyond the loaded page; the eight-parent case is an extreme stress
case, not an observed shape of the user's repository. These cases demonstrate
why commit count alone cannot predict graph memory.

For the real history, 12,000 scroll steps peaked at 14.7 MiB of Git VM heap,
with p95 2.09 ms and max 5.29 ms, without an error. The GitHub fixture's
separate VM peaked at 9.1 MiB during 1,200 steps. That is a separate plugin
budget and must not be added to Git's consumption to claim Git exceeded
32 MiB. GitHub API replies were mocked; native image downloading, decoding
and GPU presentation were not measured.

### Memory accounting limits

The reported VM heap is **not the plugin's complete metered footprint**.
The budget also counts Rust-backed state, including anonymous computed
state holding the graph. `host.memory()` reports all named states but does
not expose every anonymous state or the exact per-plugin meter total.
The failed eight-parent run reported only about 20.5 MiB of Git VM heap
after the error; that is compatible with a 32 MiB combined limit and is not
evidence that the limit was ignored. Its failures name `history.luau:1030`
and `history.luau:1060`.

The Git plugin budget is shared by its session models and caches, not
renewed for each History tab. `repo.luau` retains models in its `models`
table. These measurements start from a fresh host and one session; they
do not measure accumulated histories/diffs from other sessions. The app
also requests 200 commits per page: visible commits and loaded commits
are different counts.

The native global slowdown with a small ordinary history remains
unreproduced. These results justify investigating graph storage and shared
plugin pressure, but do not establish GC as the cause of that native symptom.
Follow-up logs and fixtures are `/tmp/zgx-100-*.log` and
`/tmp/zgx-100-*.json`; the 32 MiB stress failure is intentional.

## Implementation, step 1: retain only visible graph segments

`graph.layout(commits, max_lanes)` now keeps every pending parent, lane and
color, but emits a segment only when both endpoints fit the requested lane
limit. History passes its existing eight-lane drawing limit. Omitting the
argument preserves the uncapped API. Width is tracked independently of
retained segments, including incoming lanes that end on a row. This preserves
compact-mode decisions, dot colors, hidden-lane joins and later page layout.

History's row keys also include the commit's color, previous row color and
whether another row follows. Compact rails depend on these even when the
commit's ordinary segments are all hidden; appending a page must update the
previous tail's continuation.

Regression checks compare the new layout against a frozen original
implementation. Across page prefixes and 32 deterministic DAGs, 15,306 row
comparisons check column, color, full width and the exact visible segment
sequence for limits 0, 1, 2, 8, 16 and no limit. Existing History tests cover
responsive geometry and path highlighting. A functional wide-graph test
also covers a hidden-lane commit's compact hover, page append and return to
full-width geometry.

### Repeated measurement protocol

Three fresh processes per case and implementation, run sequentially on Linux
with the same release test executable and pinned framework. Tables report
the median of the three results. No production memory budget was increased.
Baseline runs use the frozen original graph with the same UI/benchmark
harness, including the explicit compact-row key dependencies.

The isolated layout benchmark performs 24 layouts, discards the first three,
and reports p50/p95 of the remaining 21. Its script-call budget is raised to
30 seconds so the old algorithm can complete the repeated layouts; this does
not change the plugin's production time/memory limits. These timings include
Luau allocation/collection, but exclude UI construction and Rust state
conversion. Luau assertions are enabled in this build.

| 100-commit topology | Original segments | Step 1 segments | Original layout p50 | Step 1 layout p50 |
| --- | ---: | ---: | ---: | ---: |
| Linear | 198 | 198 | 0.071 ms | 0.062 ms |
| One extra pending parent per commit | 5,248 | 878 | 1.915 ms | 0.538 ms |
| Four extra pending parents | 20,398 | 896 | 12.358 ms | 3.210 ms |
| Eight extra pending parents | 40,598 | 899 | 39.302 ms | 11.032 ms |

The extreme case stores 97.8% fewer segments, with no loss of the full
800-lane topology. Lane/parent searches are still over the full topology;
this change does not make the entire algorithm independent of graph width.

UI runs perform 1,200 scroll steps of 48 pixels, reversing every 64 steps.
Heap figures are sampled Git VM peaks, not the complete metered plugin
footprint (Rust state is additional). Opening includes 30 settling frames
and service/UI work, rather than only graph layout.

| Fixture | Original peak Git VM heap | Step 1 peak | Original open | Step 1 open | Original scroll p95 | Step 1 scroll p95 |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Real 100 commits | 13.27 MiB | 13.36 MiB | 27.57 ms | 27.27 ms | 2.10 ms | 2.07 ms |
| Real 100 + mocked GitHub | 13.35 MiB | 13.30 MiB | 44.41 ms | 43.74 ms | 3.11 ms | 3.12 ms |
| Synthetic 100, one extra parent | 18.45 MiB | 13.67 MiB | 35.07 ms | 28.01 ms | 2.04 ms | 2.06 ms |
| Synthetic 100, four extra parents | 19.74 MiB | 13.74 MiB | 68.26 ms | 30.20 ms | 1.98 ms | 2.07 ms |
| Synthetic 100, eight extra parents | Memory error | 14.16 MiB | Invalid | 38.92 ms | Invalid | 2.05 ms |
| Real 1,000 commits | Memory error | 18.71 MiB | Invalid | 48.21 ms | Invalid | 2.18 ms |

All three runs of each formerly failing fixture pass at 32 MiB after step 1.
Ordinary-history scroll times are effectively unchanged. This is evidence
of reduced graph allocation and resolved tested memory failures, not proof
that the reported global native-window FPS problem is fixed.

### Reproduction

```sh
python3 scripts/zgx-git-history-fixture.py /tmp/graph-wide.json \
  --rev 68e460af --count 100 --shape pending --extra-parents 8
GIT_PERF_JSON=/tmp/graph-wide.json GIT_PERF=history \
  GIT_PERF_BOUNCE=1 GIT_PERF_STEPS=1200 \
  cargo test --manifest-path apps/zgx/Cargo.toml --release --bin zeron-zgx \
  git_scroll_cost -- --ignored --nocapture --test-threads=1
GIT_GRAPH_PARENTS=8 cargo test --manifest-path apps/zgx/Cargo.toml \
  --release --bin zeron-zgx git_graph_layout_cost -- --ignored --nocapture
# Add GIT_GRAPH_REFERENCE=1 for the frozen original layout.
```

Use `--shape real`, `--shape linear`, `--all` or `--github` to generate the
other fixture types. Keep the generated JSON fixed across implementations.
Per-run measurements are in `zgx-git-history-graph-results.csv`; detailed
logs for this session are `/tmp/zgx-graph-{baseline,step1}-*.log`.
