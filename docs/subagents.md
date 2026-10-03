# Subagent status and stopping

A subagent card tracks the child's assignment separately from the parent turn. A parent finishing or a spawn tool returning does not prove the child has finished. Conversely, keeping the parent's harness process alive does not prove every child is working.

Codex child turn ends, `thread/status/changed` (`idle`, `notLoaded`, `systemError`), and parent `subAgentActivity` completion/interruption items settle the original spawn card. These notifications never settle the parent turn. A confirmed new child turn or assignment reopens the same card. Replayed turn ends and late content must not settle or resurrect a later assignment.

Claude Code also restores task identity from structured Agent results. Explicit terminal results and failed spawns settle the child; an `async_launched` result leaves it running.

When a runtime exits, every locally owned running spawn card is failed, including cards from completed parent turns and cards without a live transcript sink. Engine-restart recovery retains its existing stale-card sweep.

Active Claude Code and Codex cards offer **Stop**, separately from the transcript-open affordance. The button supports pointer activation and Enter/Space, shows **Stopping…** during the request, and offers **Retry stop** with an error tooltip if it fails.

`StopSubagent { chatId, toolUseId, targetDeviceId? }` routes to the chat's host device. The host validates that `toolUseId` names a locally owned spawn before sending a child-specific control:

- Claude Code: resolve the original spawn to its native task ID, send `stop_task`, and await its control response.
- Codex: resolve the original spawn to its child thread, use its live turn ID (or read its active turn if the start was missed), and send `turn/interrupt` to that child turn. An already-idle child needs no interrupt.

Only a successful acknowledgement finalizes the target's open transcript as aborted. Failures and timeouts leave the child status intact; there is no fallback that cancels the parent. Other harnesses reject individual stop requests until they implement a native child control. The desktop button is currently exposed for Claude Code and Codex.

## Native screenshot fixture

```sh
DISPLAY="${DISPLAY:-:0}" cargo run --locked -p zeron-ui --features subagent-fixture \
  --example subagent-fixture -- /tmp/zeron-subagent-shots
```

The fixture renders the production shell and transcript against an isolated engine and deterministic harness, uses the production Stop request path, verifies that only its target settles, and captures dark/light screenshots. It makes no model calls. Captures are written outside the repository.
