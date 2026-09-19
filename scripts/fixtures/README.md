`resource-stream.jsonl` contains sanitized deltas from a successful Haiku
profiling response: an 80-section Rust ownership tutorial with code fences.
Only text/reasoning deltas and the successful completion marker are retained;
account, session, timestamp and usage metadata are omitted.

The assistant text is 51,769 bytes, SHA-256
`c1809f92c26c682c6f035478f7ca63980e25fb5e97ea7746dcb41ecd7dbb0a25`.
Reasoning contributes another 853 bytes. The production transcript's part
separators bring the combined short reply to 52,624 bytes.

Use `ZERON_REPLAY_REPEAT=10 ZERON_REPLAY_DELAY_MS=8` for the synthetic long
workload. See [the profiling report](../../docs/performance-resource-usage.md)
for build settings, commands and measured results.

`runway-short-stream.jsonl` is a synthetic short reply in 24-character chunks.
It keeps the own-send runway active through completion. Use a 400 ms replay
delay for the [runway performance comparison](../../docs/performance-runway-scroll.md).

`transcript-selection-stream.jsonl` is a synthetic short reply in six-character
chunks. A 1200 ms replay delay leaves time to expand/collapse a long prompt and
select live text. See the [native regression recordings](../../docs/transcript-selection-regressions.md).

### Composer capabilities and floating activity

`composer-questions.json` covers choice-only, options with custom text, multiple
selection, text-only, non-blocking and long/Unicode questions. The same payloads
feed the production Wizard regression tests and `composer-polish-fixture` native
renderer (`appshots-fixture` feature). The runner also advances a two-page request,
then returns to page one and captures its preserved typed answer.

`composer-activity.json` covers no activity, plans, all task states, goals,
authoritative empty task snapshots, overflowing plans and combined activity.
The native runner captures collapsed/expanded states; projection tests consume
the identical data. It also composes the combined activity payload with the
long question and three queued messages from one synthetic Codex harness. It
captures that densest supported glass stack at 440×520 and 840×960 in both light
and dark appearances. Existing rich Markdown/chip cases remain in that runner,
which now also opens the real compact picker in standard, fast and favorite-model
list states with a fixture catalog.

The fixture defaults to an explicitly frosted surface. Set
`ZERON_FIXTURE_SURFACE=opaque` for the opaque preference; unknown values fail
instead of silently producing mislabeled evidence. Its keyboard case dispatches
real Tab, Enter, Escape and Space keystrokes through the production focus and
activity handlers. The `activity-motion-*-000ms`, `090ms`, and `250ms` frames
show transition geometry at fixed checkpoints. In a reduced-motion run those
checkpoints should all show the settled state. Static frames do not establish
smooth frame pacing, which still requires a live preview or recording.

Relevant automated suites:
- `cargo test -p zeron-ui --lib composer` — rich input, mode commands, question
  constraints/paging, independent plans/todos/goals and interrupted motion.
- `cargo test -p zeron-ui --lib pickers` — favorites, supported options, keyboard
  traversal, haptic step counts, slider endpoints and reduced motion.
- `cargo test -p zeron-harness --lib` and adapter integration fixtures under
  `crates/harness/tests` — discovery, native mode routing and question responses.
- Engine `nonblocking_questions_and_goal_updates_survive_turn_completion` and
  document todo round-trip tests — persistence and post-turn activity.

Build the native fixture with `cargo check -p zeron-ui --features appshots-fixture
--example composer-polish-fixture`. Launch/capture it only on an explicit preview
request, with a dedicated output directory:

```sh
ZERON_FIXTURE_SURFACE=frosted cargo run -p zeron-ui --features appshots-fixture --example composer-polish-fixture -- /tmp/zeron-composer-polish-frosted
ZERON_FIXTURE_SURFACE=opaque cargo run -p zeron-ui --features appshots-fixture --example composer-polish-fixture -- /tmp/zeron-composer-polish-opaque
ZERON_FIXTURE_SURFACE=frosted ZERON_FIXTURE_REDUCE_MOTION=1 cargo run -p zeron-ui --features appshots-fixture --example composer-polish-fixture -- /tmp/zeron-composer-polish-reduced-motion
```

Rendered fixtures are visual evidence, not live provider compatibility evidence.
Account-specific native turns and frame pacing require separate verification.

Compaction regressions use the fake Codex peer's native item start/completion
sequence and the production transcript projection test (including interruption).
Typed-question and closed-channel scenarios assert the exact response received
by the native peer. Run `cargo test -p zeron-harness --test codex` and
`cargo test -p zeron-ui --lib question`; `--lib compaction` covers the divider
lifecycle and existing shimmer timing tests. These tests do not launch a preview.
