# Whale transcript UI responsiveness

PR #444 addressed runtime/SQLite starvation, but left a separate synchronous
presentation path on the UI thread. When the full transcript arrived, the GPUI
observer built every row, parsed markdown, constructed tool details and
fingerprinted serialized tool payloads. Navigation also serialized tool payloads
to estimate cache size and recaptured the history baseline. Background JSON
parsing alone did not remove this work.

## Changes

- Main and subagent subscription pumps prepare presentation on GPUI's background
  executor. Frozen subagent snapshots use the same preparation. The foreground
  receives prepared rows with the corresponding frame; stale selection guards
  and subscription cancellation still apply.
- Unchanged entries share their prepared rows across delta updates. Upserts,
  appends and removals invalidate affected entries. Same-length text corrections
  invalidate parsed trees too.
- The navigation cache retains prepared rows, a precomputed byte estimate and
  a captured history baseline. Reopening shares these objects, with no whole
  transcript fingerprinting, tool serialization or markdown parsing.
- Cancelling a watch disposes of its background mirror/caches off-thread. Reset
  replacement and navigation-cache eviction also defer large object destruction.
- Fully historical tool groups avoid constructing per-tool history lookup sets.
  History cleanup uses a set of active markdown row IDs instead of repeatedly
  searching the whole row list.
- Tool summaries are computed during preparation. Settled collapsed groups do
  not construct hidden chip bodies; closing animations retain their bodies until
  the fold finishes.

The list still performs foreground bookkeeping and visible-element layout and
painting. This change removes whole-transcript parsing/formatting from that path;
it does not claim rendering costs zero time.

## Verification

`prepared_whale_open_and_revisit_do_not_build_rows_on_ui` prepares a large
transcript on another thread, then installs it in a GPUI transcript entity and
navigates away/back. A thread-local guard makes row construction and tool-summary
formatting fail if invoked during that foreground work. The test checks shared
prepared-cache identity and no historical tool entrance timestamps.

The fixture defaults to 5,000 markdown parts. Setting `ZERON_WHALE_SNAPSHOT`
reads a private local snapshot copy, joins continuation entries exactly as the
normal transcript does, and exercises the same installation/revisit path. No
snapshot or transcript content is committed.

`background_preparation_reuses_unchanged_rows_and_replaces_same_length_text`
checks row sharing and invalidation. The existing UI suite also covers preview
replacement, history/live animation boundaries, cached navigation, streaming,
folds, selection and stale subscriptions.

```sh
cargo test -p zeron-ui --lib -- --test-threads=1
ZERON_WHALE_SNAPSHOT=/path/to/private-copy.bin \
  cargo --config 'profile.test.package.zeron-ui.opt-level=2' \
  test -p zeron-ui --lib prepared_whale_open_and_revisit -- --nocapture
```

Measured on Linux with the actual 21,628,288-byte snapshot and optimized UI
code: **806 ms background preparation, 12 ms foreground installation, 9 ms
cached navigation away/back**. All 1,079 UI tests pass in both the standard test
profile and the optimized UI profile. These are local measurements, not
measurements from work-laptop.

The timing probe measures foreground state installation and transcript `sync`,
not an end-to-end painted frame or input latency on macOS. Its optimized run
optimizes `zeron-ui` only; dependencies keep the test profile. A fresh laptop
build is still needed to confirm the reported 300–700 ms pause is gone there.

This is a desktop UI change. It requires updating the laptop application, with
no additional host or edge rollout beyond PR #444.
