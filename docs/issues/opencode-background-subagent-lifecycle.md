# opencode 2.x background subagents are broken end to end: the chip settles at spawn, then the chat stays Working when the child finishes

**Area:** harness / opencode driver + engine turn lifecycle
**Harness:** opencode (2.x wire)
**Version:** Zeron v0.2.105 (main @ 9e83ceea); opencode verified on v2.0.26

## Summary

Zeron visualizes subagents for the opencode harness — a spawn chip with a working indicator, a live subagent transcript tab, and a row in the explorer's "Subagents" section. All of that works for **foreground** `task` spawns. Subagents spawned with opencode's `background: true` option are broken in two ways, at opposite ends of their lifecycle:

1. **At spawn**, the chip resolves instantly: no working spinner, no live transcript tab, nothing in the Subagents section — while the opencode child session keeps working.
2. **At completion**, when the child finishes and opencode re-invokes the parent to deliver the completion notice, that notice streams into the transcript but the turn never ends: the chat keeps showing Working ("Scheming…" with an ever-growing timer) until the platform's stale-run reaper finally clears it.

## Steps to reproduce

One flow shows both defects:

1. Use the **opencode** harness with opencode 2.x and a configured provider.
2. Ask the agent to spawn a background subagent that finishes on its own, e.g.:

   > Use the task tool to launch one background subagent (`background: true`) that runs `sleep 5`, then reply with the word finished. Tell me you launched it.

3. While the child runs (~5s), watch the transcript chip and the explorer's "Subagents" section — defect 1.
4. Wait for the parent to report back — defect 2.

## Actual behavior

**Defect 1 — chip settled at spawn:**

- The `Agent: <description>` chip resolves to a quiet, done-looking state within the same event burst as the spawn.
- No working spinner, no "Open subagent" link, no live transcript tab.
- The "Subagents" section lists nothing.
- The child session (`ses_…`) keeps running inside `opencode serve`; none of its traffic reaches the UI.

**Defect 2 — chat stuck Working after completion:**

- The parent's post-completion report streams into the chat normally.
- The chat's Working indicator never stops. It stays "Scheming…" (or equivalent) with an ever-growing elapsed time.
- The stuck state survives until the platform's stale-run reaper finally ends it.

## Expected behavior

- The chip starts with the pulsing working indicator, offers a live subagent tab, and appears as running in the Subagents section; it settles (done/failed) only when the child session actually ends.
- The wake's output is one more completed turn: once the parent finishes reporting, the chat returns to idle and accepts the next message.

## Root cause

### Defect 1 — the spawn is read as the child's end

opencode 2.x answers a `background: true` spawn **immediately**. The `task` tool part completes at spawn with:

```jsonc
"state": {
  "status": "completed",
  "output": "The subagent is working in the background (sessionID: …). You will be notified automatically when it finishes.",
  "metadata": { "sessionID": "ses_child", "status": "running" }
}
```

while the child session keeps executing. The driver reads that completed part as the child's end:

1. `task_completion()` in `crates/harness/src/opencode/mod.rs` matches `state.status == "completed"`; the `message.part.updated` handler latches `child.done = true` and emits a tagged `AgentEvent::Done` for the spawn chip.
2. While latched, all later child frames are dropped (`if child.done { return BusOutcome::Continue }` in the `message.part.updated` / `message.part.delta` handling). Only a new *user* message un-latches, and the background child's prompt is delivered before the latch.
3. The 2.x normalizer (`normalize_v2_frame_with_session_models`) does not carry `metadata` through `session.tool.success` / `session.tool.failed`, so the `status: "running"` marker never reaches the driver on a real 2.x wire at all.
4. Child session terminals (`session.idle` / `session.interrupted` / `session.error`) are ignored for non-parent sessions in `handle_bus_event`, so nothing else can settle the chip.
5. Engine-side, a tagged `Done` with no previously seen content takes the "Done with no sink" path in `crates/engine/src/sessions.rs`: no subagent doc is created and no `subagent_ref` is stamped, which is why the Subagents section filters the row out.

Net effect: the only lifecycle signal Zeron ever receives for a background subagent is "done at spawn".

### Defect 2 — the completion wake never settles

When the child finishes, opencode re-invokes the parent session on its own to deliver the completion notice. That is a *wake* execution — one started with no prompt from Zeron:

1. **Driver:** the wake is announced as `session.status {type: "busy"}` on the parent session. The driver only arms a turn when *it* posted a prompt; a busy with no turn in flight is ignored (`handle_bus_event`).
   The wake's text still streams (the engine forwards content regardless), but its terminal frame — `session.execution.succeeded` → `session.idle` — finds `turn.active == false` at the settle gate, settles nothing, and no `AssistantMessageCompleted` + `Done` is ever emitted for the wake.
2. **Engine:** the wake output resumes the parked transcript ("parked session resumed by self-continued agent output"), flipping the chat to Working — and for harnesses that declare a deterministic turn end (opencode reads terminal frames directly and reports `deterministic_turn_end() == true`), the quiesce watchdog is retired, so nothing ever rescues the swallowed Done.

Evidence from an affected chat's journal: the run ends with text deltas only — no `assistantMessageCompleted`, no `done` event — while the engine shows the session parked and Working.

## Proposed fix

1. Don't settle a `task` part that only *backgrounded* its child: treat `state.metadata.status == "running"` (fallback: the tool-owned `"The subagent is working in the background"` output prefix) as "child still live" and keep the `ChildRun` unlatched.
2. Carry `metadata` through the v2 normalizer for `session.tool.success` / `session.tool.failed` (the existing `session.tool.progress` pass-through shows the shape).
3. Settle the chip from the child session's own terminal frames (`session.idle` / `session.interrupted` / `session.error`; the 2.x normalizer produces these from `session.execution.{succeeded,interrupted,failed}`).
4. Treat a `busy` on our session with **no turn in flight** as a wake execution: arm a turn (with the normal stall bound) so the wake's own terminal idle settles through the regular path (`AssistantMessageCompleted` + `Done`). A silent wake then still errors out at the stall bound instead of hanging.

Foreground spawns are unaffected: their `task` part completes only when the child finishes, so the existing settle path stays correct.

## Tests

- Unit test for the background detection: the metadata marker and the output fallback match; a foreground completion and a still-running spawn do not.
- v2 normalizer test asserting tool `metadata` survives normalization.
- v2 run-loop regression test (defect 1): background spawn → `session.tool.success` with `metadata.status = "running"` → child text streams tagged → child `session.execution.succeeded` → exactly one tagged `Done(Completed)`, ordered after the child content.
- v2 run-loop regression test (defect 2): settle a prompted turn, then emit a wake sequence (`session.execution.started` → step/text frames → `session.execution.succeeded`) with no prompt and assert a second `Done(Completed)` carrying the wake text.

Both defects are reproducible deterministically without a provider using the scripted v2 wire in `crates/harness/src/opencode/tests.rs`.

## Workaround

Avoid `background: true` spawns (foreground subagents stream live and do not wake the parent); or interrupt the stuck chat and continue in a new turn.

## Impact

- Background subagent work is invisible for the entire run: no progress, no transcript, no steer affordance.
- The parent agent's own claim that a subagent "is still running" cannot be checked in the UI — the chip already looks finished.
- After the child finishes, the chat wedges in Working with no turn end.
- Background subagents are durable: when the server hosting a child goes away, opencode suspends it and later resumes it (`time_suspended` / `resume_attempts`), so a child can keep working long after the spawning run has ended. All of that work stays invisible in Zeron.

## Environment

- Zeron v0.2.105 (main @ 9e83ceea), macOS; opencode 2.x (verified on v2.0.26).
