# Fix opencode background subagent lifecycle: live chip + wake settle

Fixes: *(link the issue opened from `docs/issues/opencode-background-subagent-lifecycle.md`)*

## Problem

Two defects break `background: true` subagents on the opencode 2.x wire:

1. **The spawn chip settles at spawn.** opencode answers a background `task` immediately: the tool part completes with `metadata.status == "running"` ("The subagent is working in the background…") while the child session keeps working. The driver read that completed part as the child's end, latched `child.done = true`, emitted the chip's `Done`, and then dropped all later child frames — no working indicator, no live transcript, nothing in the Subagents section.
2. **The chat stays Working after the child finishes.** opencode then re-invokes the parent server-side to deliver the completion notice — an execution that starts with *no prompt from Zeron*. Its `busy` carried the home of the previous turn, so no turn was armed; the wake's terminal idle found `turn.active == false` at the settle gate and settled nothing. The engine had already resumed the parked transcript on the wake's output, and because this driver reports a deterministic turn end the engine keeps no quiesce watchdog — so the chat waited forever ("Scheming…" with a growing timer) until the reaper.

## Fix

`crates/harness/src/opencode/mod.rs`:

- Don't treat a backgrounded `task` completion as the child's end: `task_backgrounded()` detects `metadata.status == "running"` (fallback: the tool-owned `"The subagent is working in the background"` output prefix), and the part handler leaves the `ChildRun` unlatched.
- Settle the chip from the child session's own terminal frames (`session.idle` / `session.interrupted` / `session.error`, i.e. the 2.x `session.execution.*` terminals after normalization).
- Carry tool `metadata` through the v2 normalizer for `session.tool.success` / `session.tool.failed` so the `status: "running"` marker survives a real 2.x wire.
- Treat a `busy` on our session with no turn in flight as a wake execution: arm a turn (with the normal stall bound) so the wake's own terminal idle settles through the regular path (`AssistantMessageCompleted` + `Done`). A silent wake still errors out at the stall bound instead of hanging.

`crates/harness/src/opencode/tests.rs`:

- Unit tests: background detection via metadata and via the output fallback; foreground completion and still-running spawns are not backgrounding.
- v2 normalizer test: tool `metadata` survives `session.tool.success`.
- v2 run-loop regression: backgrounded spawn → `session.tool.success {metadata.status: "running"}` → child text streams tagged → child `session.execution.succeeded` → exactly one tagged `Done(Completed)` ordered after the child content.
- v2 run-loop regression: prompted turn settles → wake sequence with no prompt (`session.execution.started` → step/text → `session.execution.succeeded`) → a second parent `Done(Completed)` carrying the wake text.

## Verification

- `cargo test -p zeron-harness --lib opencode` — 69 passed.
- `cargo test -p zeron-harness --test opencode` — 17 passed.
- Regression-proofed: with the busy re-arm temporarily removed, the wake test times out (no second `Done`).
- Live run against opencode 2.0.26: background spawn → parent `Done #1` → child streams tagged tool/text events → child chip `Done` → wake text ("The background \"Wake probe\" subagent finished…") → **parent `Done #2`**, no third turn, clean exit.

Foreground spawns are unaffected: their `task` part completes only when the child finishes, so the existing settle path stays correct.
