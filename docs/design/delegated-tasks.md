# Delegated tasks

Chats an agent creates through the Zeron MCP server become *delegated
tasks*: the caller can be notified automatically when the task finishes,
tasks can delegate further within a cap, and the whole subtree reports
back through the same path.

## Data model

`Chat.delegation: Option<{ by, depth }>` (proto) marks a chat as a
delegated task:

- `by` — the chat id that delegated the work; notices go there.
- `depth` — 1 for a task delegated by a top-level chat, +1 per hop,
  capped at `MAX_DELEGATION_DEPTH` (2). The cap and the create rules
  (`delegation_create_error`) are shared with the MCP layer via proto so
  previews and the tool's own pre-checks cannot drift; the engine
  enforces the same rules when the `createChat` mutation lands. A
  delegated task naming another chat's `parent` is rejected — that path
  would bypass `delegatedBy` and the caps.

`parentChatId` always points at the delegation tree's **root** chat so
tasks-of-tasks still list under the top-level conversation; `by` names
the actual delegator. A plain side chat has a parent but no `delegation`
and cannot delegate.

`createChat { delegatedBy }` is the only input; the engine derives
`delegation`, `parentChatId`, and enforces the rules: side chats cannot
delegate, depth caps, and a task cannot create a child with a higher
sandbox level than its own (the sandbox level is a request, not an
enforcement — the Codex and Claude Code adapters do not enforce it
today; the dispatch additionally clamps a run request to the row's
level).

## Arming and settlement

`QueueCommand { notify: { batch, seal } }` arms the command's message:
the engine's delegation ledger records that this turn owes `by` a
notice. Arming happens *before* the command lands in the doc, and is
undone if queueing fails, so a turn can never run unarmed.

A per-engine watcher evaluates armed tasks and settles them:

- `Working`/`AwaitingInput` — not done; awaiting-input tasks get an
  attention notice carrying the request and question ids.
- `Idle` — the doc decides: the armed message's own turn must have an
  assistant entry after it (no pending command, no registered run, not
  mid-revival). A `Complete` entry settles `completed`, `Aborted` or
  orphaned `Streaming` settles `interrupted`, a message that never
  landed settles `errored` after a grace window. With no live run, the
  journal's terminal `Done` status is authoritative — an errored turn's
  doc entry can read `Complete` after recovery.
- A task may keep working after its first settle — a harness can
  continue on its own (Claude Code finishing a `run_in_background`
  command re-invokes itself and streams a new turn). The outcome
  record retains the quoted result entries; a later `Complete`
  assistant entry after them, while the armed message is still the
  chat's last user turn, produces a later-result notice to the delegator
  (`notice-later-{chat}-{hex8(entry ids)}`, same durable-delivery path;
  every unreported later entry goes in ONE notice in transcript order.
  The envelope — id, entry ids, AND the quoted texts — is saved as
  `pendingLater` before delivery and retried as exactly that envelope
  (a retry never re-reads the transcript) until the delegator's doc
  durably holds it; only then does `reportedThrough`/`reportedThroughAt`
  advance — a restart neither drops nor repeats, and a missing cursor
  entry falls back to its timestamp, never to zero). The
  session-status watcher queues the check for the chat whose status or
  `lastCompletedTurn` moved; the boot pass checks every outcome record
  once. A later result never arrives before the first result's notice
  (`initialDelivered` gates it, set in the same save that retires the
  obligation). A cancelled outcome never produces a later-result
  notice — `task_cancel` also stops the member's parked runtime — and a
  new real user message ends eligibility for that outcome; engine
  notices (`notice-*` ids) are not user turns. A trailing `Streaming`
  entry is reconciled from the journal's terminal `Done` first, the
  same recovery the armed path applies.

At settle time the result's assistant-entry ids are captured into the
ledger; the notice quotes exactly those entries, so a later
self-continued turn can never leak into an earlier obligation's
notice.

## Delivery

The ledger is a host-local `delegations.json` (atomic write: temp file,
fsync, rename — the shared `write_file_atomic` helper):
`{ tasks: [chatId, delegator, batch, messageId, asked, armedAtMs,
settled { outcome, atMs, note, entries, recovered }, delivered],
seals: [delegator, batch, sealed, firstArmedAtMs, lastProgressMs],
outcomes: { chatId: { messageId, outcome, note, atMs, resultEntries,
reportedThrough, reportedThroughAt, recovered,
pendingLater { noticeId, entries, bodies, throughAt, recovered },
initialDelivered, cancelled } }, recoveredEntries: { entryId: chatId } }`.
`recovered` marks a reply the journal — not a live finish — produced:
the turn's final transcript write may have been lost, so its notice
renders a warning line in the header area outside the quoted output.
`recoveredEntries` is saved BEFORE the stamp so the mark survives a
second restart.
The ledger undergoes compaction toward 1,000 outcome records
host-wide (oldest dropped, latest outcome per chat kept); records
still owed a delivery — not cancelled, with an open pendingLater
envelope or an undelivered first result — are protected and may
exceed that target. It exists so a restart can finish
deliveries that were in flight — the engine never loses a result notice
because the ledger outlives the notice itself — except in the
documented crash windows below.

The `notice-*` id namespace is engine-reserved: a Run/Steer command
claiming one is rejected at every external boundary (queue RPC and the
relay/sync ingestion path into `execute`). Queue rows authored by synced
peers are trusted like their user turns — the row's own id carries no
trust flag.

A *sealed* batch whose members all settled releases one notice per
result set: the text is a `[Zeron task notice …]` message, each task's
output quoted in a `<task_result_{nonce}>` block whose nonce is hashed
from the notice id and an increasing counter until the tag does not
appear in the (already-clipped) output. Delivery goes through
`DocHost::deliver_notice`:

- never interrupts a running turn — it steers where supported, else
  writes a durable command for the next turn;
- never revives an archived or deleted delegator (the result stays in
  the task's transcript);
- never thaws a queue the user stopped — a stopped or question-blocked
  delegator finds the notice at the queue's steer slot (the frozen flag,
  `queue_paused`, is in-memory only: after a restart a stopped delegator
  with an empty queue may get the notice as a fresh turn);
- holds the doc's `drain_lock`, so a Stop either lands first (the notice
  freezes) or interrupts the notice's own run;
- synchronously persists the delegator's doc before the ledger retires
  the obligation, so a crash after delivery cannot lose the notice.
  Failed deliveries back off exponentially (to 60 s) and retry.

Shutdown quiescence: an in-flight delivery holds a read guard on the
`delivery_gate` for its whole duration; `pause_all_queues` raises
`stopping` then takes the write side before freezing any queue, so a
delivery that passed the `stopping` check always finishes its persist
and one parked on `drain_lock` refuses instead of parking the notice
where a restart would read it as a queue row. Deferred recheck tasks
are tracked and aborted at shutdown so none outlives the timer wheel.

Notice ids are deterministic (`notice-{batch}-{hash(members)}`), so a
retry or restart dedups against the delegator's transcript and queue.
`ensure_notice_persisted` checks both (under `drain_lock`, so a queue
row mid-promotion is never persisted as a gap) before every retirement.

## Batching

Tasks armed in one MCP call share a `batch`. A batch releases only after
its seal (`seal: true` on the last arm, `SealDelegationBatch` for batch
tools, or a 60 s auto-seal from the first arm for crash-orphaned
batches). After `BATCH_PROGRESS_AFTER` (15 min) a sealed batch that has
fresh settled results and members still working delivers a *progress
notice* with the finished results and a "still working" list; those
members are marked `delivered` so the final notice carries only the
remainder. A member marked delivered whose notice never landed is
repaired by the next pass — the repair runs before both progress and
final release, independently of candidate selection.

## Nesting and cancellation

A delegated task can delegate further (depth ≤ 2); its own result is
held until its children settle and their notice is delivered — a lead
never sees a result assembled before its pieces. `task_cancel` walks
`delegation.by` to find the subtree; it first attempts to save the
cancellation — then runtimes are interrupted, still-Pending Run/Steer
commands rejected (control commands untouched) and queued messages
removed — `rejectedPending` counts each affected message id once — and
each queue frozen, so no notices are sent. The cleanup is bounded: each
chat gets up to three 5-second attempts at its command lock and queue
pausing is bounded too; if the save or any cleanup step fails,
`task_cancel` returns a retryable error and the cancellation may
already be saved — retrying is safe. Members
hosted on another device are reported back as `notStopped` rather than
claimed dead. `task_status` and `ListDelegations` expose the tree state
and the ledger rows.

## Restart durability

- Ledger rows survive restart; the boot pass re-evaluates every armed
  task, repairs `delivered` marks whose notice never landed, releases
  finished batches, and waits for the IPC port so notice-triggered runs
  carry the Zeron MCP server.
- Crash-revived runs are tracked in `reviving` so the boot pass treats
  them as live, not interrupted.
- A graceful quit can journal `Done{interrupted}` without stamping the
  doc's `Streaming` entry; `recover_stale` stamps those from the
  journal's last line so `task_status` and transcripts don't report a
  turn as still running (or `idle`).
- The journal's terminal `Done` status (last-line read) decides
  completed vs errored after a restart. `task_status` prefers live
  state (working, awaitingInput, an unsettled
  armed row), then the settled row's outcome, then the ledger's
  retained `outcomes` map for that chat, and only then the doc-entry
  fallback.

## Limitations

- `notify` works only for chats hosted on the same device as the
  delegator; batch `notify` requests on remote chats are rejected.
- Sandbox is a requested level, not an enforced boundary for the Codex
  and Claude Code adapters.
- `queue_paused` (the stopped-queue freeze) is in-memory.
- A re-arm whose command fails to queue is rolled back in memory and
  persisted on the next successful ledger write; a crash before that
  write leaves the new arm, which then settles as never-started.
- `task_cancel`'s stop fence is in-memory — it does not survive an
  engine restart. Boot-time crash revival (`recover_stale`) does not
  consult the ledger, so a chat whose task the ledger records as
  cancelled can still be revived if it had an open journal; its result
  is never delivered (its outcome is cancelled).
- A batch whose only member is still working gets no progress notice —
  progress needs a settled sibling to report.
- Another writer's unguarded commit can publish a row that is still
  being written to the sync outbox; a crash in that window can persist
  a notice without its text, which restart recovery treats as
  delivered.
- If the engine dies after a turn's journal Done but before its final
  transcript is durable, the recovered reply can be incomplete; its
  notice is marked as recovered.
- Task output inside a notice is quoted but remains prompt-injectable;
  the notice text tells the model not to follow instructions inside the
  block.
