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
selection, text-only, non-blocking and long/Unicode questions as synthetic
contract data. The `native-interactions-fixture` renderer uses focused hardcoded
questions to exercise dense choices and two-page navigation, then returns to page
one and captures its preserved typed answer.

`composer-activity.json` covers no activity, plans, all task states, goals,
authoritative empty task snapshots, overflowing plans and combined activity.
Projection tests consume this data directly. The native runner uses an equivalent
hardcoded plan/todo/goal payload with a long question and three queued messages
from one synthetic Codex harness, and captures the dense dark stack at 440×520.

The runner dispatches real number, Tab, Enter and Space keystrokes through the
production question, focus and activity handlers. It captures the activity
disclosure after a keyboard toggle. Static frames do not establish smooth frame
pacing, which still requires a live preview or recording.

The native interaction sequence uses an in-memory host restricted to the
synthetic chat's `SetGoal` calls: production Enter/Space handlers pause, resume,
cancel an edit, save an edit and delete the goal. Assertions check the exact
action sequence, rejected-update recovery and preservation of the ordinary
message draft. This tests the UI/RPC boundary, not a hosted model turn; Codex
protocol and engine recovery have separate tests.

Relevant automated suites:
- `cargo test -p zeron-ui --lib` — model-option selection, question
  constraints/paging and independent plans/todos/goals.
- `cargo test -p zeron-harness --lib` and adapter integration fixtures under
  `crates/harness/tests` — discovery, native mode routing and question responses.
- Engine `nonblocking_questions_and_goal_updates_survive_turn_completion` and
  document todo round-trip tests — persistence and post-turn activity.

Build the native fixture with `cargo check -p zeron-ui --features appshots-fixture
--example native-interactions-fixture`. Launch/capture it only on an explicit preview
request, with a dedicated output directory:

```sh
cargo run -p zeron-ui --features appshots-fixture --example native-interactions-fixture -- /tmp/zeron-native-interactions
```

Rendered fixtures are visual evidence, not live provider compatibility evidence.
Account-specific native turns and frame pacing require separate verification.

Compaction regressions use the fake Codex peer's native item start/completion
sequence and the production transcript projection test (including interruption).
Typed-question and closed-channel scenarios assert the exact response received
by the native peer. Run `cargo test -p zeron-harness --test codex` and
`cargo test -p zeron-ui --lib question`; `--lib compaction` covers the divider
lifecycle and existing shimmer timing tests. These tests do not launch a preview.


`composer-harnesses.json` provides synthetic reference cases for the nine
production harnesses. Each uses only its supported input/activity shape: Codex
goals, Claude/Cursor plan and task projections, OpenCode tasks/questions, and ACP
permission choices. Pi has no task case because its installed adapter does not
emit plan entries. The native renderer does not load this catalog, and these
cases do not establish live account/provider round trips. Goal-only cases cover
active, paused, blocked, complete, usage-limited and budget-limited states.

Inspect rendered images for visual quality and control reachability; a successful
fixture exit alone is not design acceptance.
