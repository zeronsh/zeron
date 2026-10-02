# Goal mode

`/goal <objective>` makes a chat keep working, turn after turn, until an
**independent verifier child chat** judges the objective met. The idea comes from
ZCode; the difference is that the verifier here is a full hidden chat that can
open files and run tests, not a tool-less model call over the transcript.

Screenshots (live, mock harness with `ZERON_MOCK_GOAL=1`): [`docs/screenshots/goal-mode/`](screenshots/goal-mode/) — `collapsed-running` (tray header, round marker, goal-set marker), `collapsed-complete`, `expanded-rounds-working`, `expanded-paused` (reason line, verifier cost, per-round verdicts and todos), `expanded-complete`, `stacked-trays` (goal + todo + queue together). The expanded shots use a build whose tray opens expanded by default (the tray itself defaults to collapsed); no interaction was automated.

Two pieces, built to be separable:

1. **The child ask** (`crates/engine/src/ask.rs`) — a reusable engine primitive:
   run a short-lived hidden child chat and get a *typed result* back. The goal
   verifier is its first caller; workflow agents are the next.
2. **The goal controller** (`crates/engine/src/goal.rs` pure, and
   `doc_host/goal.rs` effectful) — host-side, harness-agnostic.

## The child ask

```
AskBackend::ask(parent_chat, AskSpec { prompt, result_schema, … }, cancel)
    -> Result<AskOutcome { result, child_chat_id, usage, repairs, nudged }, AskFailure>
```

`AskService` is the engine implementation. For one ask it:

1. validates the schema (`boon`, JSON Schema 2020-12; only local `$ref`s — file and
   URL references are refused so a model-influenced schema cannot make the engine
   read or fetch anything);
2. creates a hidden child chat: `parent_chat_id` = the requester (or the
   requester's own parent when the requester is itself a child, so nesting stays one
   level), same project, cwd and — by default — harness/model as the requester,
   overridable per ask; titled up front so the auto-titler stays away; marked
   `meta.askChild` so boot recovery never revives it;
3. injects a **run-scoped MCP server** (`ZERON_ASK_ID`, see `docs/mcp.md`) that
   offers `read_chat`, `get_chat`, `whoami` and `submit_result`;
4. prompts the child (the task, then a fixed "answer through `submit_result`, there
   is no person here" epilogue) and waits for its turn;
5. validates each submission against the schema. A violating one is answered, as the
   tool result, with the path-level violations (`/items/2/name: …`); the model has
   **3 repair rounds** inside the same turn, then the ask fails with
   `InvalidResult`;
6. a turn that ends without a submission gets **one nudge**; a second miss is
   `NoResult`;
7. always cleans up: interrupts the child if it still runs, forgets the ask, and
   **archives** the child (archive is the repo's convention for chats an agent made —
   `archive_chat` — and the transcript is the evidence a verdict points at).

Failures are typed (`AskError`: `Setup`, `Cancelled`, `Timeout`, `NeedsInput`,
`TurnFailed`, `NoResult`, `InvalidResult`) and carry the child id and what was spent,
so a budget keeps counting a verifier that crashed. `AskUsage` reports input/output
tokens (summed from the run journal's `Usage` events, so it works for every harness
that reports usage and is lag-free), wall time and turns.

### Permissions of a headless child

* The child's `auto_approve` is the requester's last request's value and its
  sandbox is `min(requester's, wanted)` (`capped_sandbox`) — never wider. A
  read-only ask requests `ReadOnly`.
* **Honest limit:** no harness enforces the sandbox level for chat runs today.
  Codex forces full access for every non-title run, Claude Code has no read-only
  mode, and the others ignore it. Read-only intent for verifiers and analysis asks is
  therefore carried by (a) the prompt ("do not create, modify, delete…") and (b) the
  restricted MCP toolset (no `send_message`, `create_chat`, goals…), not by the
  sandbox. The sandbox cap is still applied so the day a harness honours it, asks get
  it.
* A child that parks on a question or an approval can never be answered — nobody
  watches it — so the ask **fails at once** with `NeedsInput(<question>)` instead of
  waiting out its timeout.

### Testing against it

`FakeAsk` is an `AskBackend` with scripted replies (`push_result`, `push(FakeReply)`,
`on_call`, `calls()`, `cancelled()`): no chats, no harness. It validates replies
against the spec's schema. `DocHost::set_ask_backend` swaps it in for every
engine-internal ask.

## Goal state

The goal lives in the chat's session doc: `meta.goal`, one atomic JSON value
(like `contextUsage`), **host-only writer**, additive and serde-defaulted, so old
builds never read it and unknown future fields are ignored. It rides the existing
transcript watch (`TranscriptUpdate.goal` / `goalCleared`, sent on reset frames and
when it changes — not on every streaming tick), so any client that syncs the doc sees
rounds and verdicts. The thin-doc rebuild preserves it.

| Field | Meaning |
| --- | --- |
| `id`, `objective` (≤4000 chars), `summaryTitle` | identity; title is the truncated first line (see Titling) |
| `status` | `active · verifying · paused · complete · budgetLimited` |
| `iteration` | rounds started (a round = one prompt + the turn it drives + its verdict) |
| `maxRounds` (default 25), `tokenBudget?`, `timeBudgetSeconds?`, `extensions` | caps; `extensions` counts resumes past a cap |
| `tokensUsed`, `verifierTokensUsed`, `timeUsedSeconds` | accounting; verifier cost is separate and counts toward the budget |
| `reason` | why it is not running: `user · interrupted · turnFailed · verifierFailed · noProgress · maxRounds · tokenBudget · timeBudget · readOnly · restarted · verified` + a sentence |
| `verdicts` (≤20) | `{iteration, outcome pass/notSatisfied/failed, reason, nextAction, at, verifierChatId, verifierTokens, verifierMs}` |
| `pending` | the controller's ledger: waiting on a round's turn, or on a verification |

Round prompts are **machine-origin messages**: the queue row and the user message
carry an additive `origin` (`MessageOrigin::Goal{goalId, round, title}`), so UIs
render a compact "Goal · round N: …" marker while the harness receives an ordinary
user turn. Lifecycle events (set, verdict, complete, paused, resumed, cleared, limit
reached) are system-role transcript entries with `MessageOrigin::GoalEvent`; their
text part says the same in words, which is what a client that predates them shows.

## Commands

Mutations travel the command plane as `SessionCommandPayload::Goal { command }`
(`QueueCommand` RPC), so desktop, MCP and later mobile all use one path and the chat's
host executes it wherever it runs:

* `set {objective, limits?, replace?}` — without `replace` an existing running or
  paused goal is refused (a stray `/goal text` must not discard an objective);
* `pause`, `resume` (past a cap it grants another allowance: every cap is scaled by
  `1 + extensions`), `clear`.

Older hosts decode the unknown payload tag as an unreadable command and skip it (the
ledger already tolerates that), so a goal command is simply inert there; clients gate
on the `goal-mode-v1` capability before sending.

**Setting while a turn runs is accepted**: the goal is `active` with `iteration 0`
and its first round begins when that turn ends (ZCode rejects instead). The turn that
was already running never counts toward verification — which is also how "the first
turn after set/resume skips verification" is satisfied: only turns the controller
itself started are verified.

## The controller

Runs only on the chat's host, exactly like `drain_queue`. Ticks after every doc
change (cheap: a flag skips chats that never had a goal, and a turn in flight returns
immediately), every session-status change (the queue-flush watcher), every goal
command and every verdict. `decide(goal, observation) -> Action` is pure
(`crate::goal`); the effectful side gathers the observation, acts, and persists.

```
              set / resume                 turn ends, verdict needed
   (none) ─────────────────► ACTIVE ─────────────────────────────► VERIFYING
                              │  ▲  not satisfied (next round queued)   │
                              │  └────────────────────────────────────┤
        interrupt / pause /   │                                          │ pass
        error / no progress / │                                          ▼
        verifier failure      ▼                                       COMPLETE
                           PAUSED ◄── user pause, Stop
                           BUDGET_LIMITED ◄── rounds / tokens / time reached
   PAUSED / BUDGET_LIMITED ── resume ──► ACTIVE        any state ── clear ──► (none)
```

Per tick, for an `active` goal:

* turn running → wait; **a queued user message → wait** (user input outranks the
  controller; verification happens after it); subagents the round spawned still
  running → wait;
* nothing pending → check caps, then queue round *n*+1's prompt through the **normal
  queue** (so ordering, steering and attachment rules are reused) with id
  `goal-<id>-r<n>`;
* the round's turn finished → `Completed` verifies; `Aborted` or an errored session
  **pauses** (never continues after Errored/Interrupted turns); budgets are folded and
  checked first, so an exhausted budget skips the (costly) verifier;
* a user Stop (an `Interrupt` command) **pauses** the goal (`interrupted`) and
  cancels a running verifier.

A verification is a child ask (read-only, default 10-minute timeout, the same
harness/model as the chat) returning `{passed, reason, nextAction}`. Pass → `complete`.
Not satisfied → the next tick queues a continuation whose prompt carries the verdict,
the next action and the budget. A **verifier failure of any kind (timeout, crash, bad
output, a question it cannot ask) pauses the goal with the reason shown — it never
retries on its own**, so infrastructure errors cannot loop. Resume is the retry.

**One verification per round.** A verification is keyed by `(goal id, round)` and its
marker (the controller's in-process run entry) is created under the chat's controller
lock *together with* the `verifying` ledger write, and removed under the same lock only
after the verdict has been applied. So there is no instant at which the ledger says
`verifying` while nothing is registered: a tick that lands while a returned verdict is
still waiting for the lock sees a verification in flight and waits, and asking to start
the same `(goal, round)` again is a no-op. (Before this, the entry was dropped when the
verifier returned and the verdict applied afterwards, and a tick in between judged the
round a second time.) Restart recovery is unchanged: the registry is per-process, so a
`verifying` goal found at boot has no entry and is judged again.

### Safeguards ZCode lacks

* **Round cap** (default 25, per goal) → `budgetLimited`.
* **Token / time budgets** (optional, per goal; MCP `set_goal` params) → `budgetLimited`.
  Agent and verifier tokens both count; verifier cost is shown separately.
* **Per-verifier timeout** (10 minutes).
* **No-progress guard:** two consecutive not-satisfied verdicts with the same next
  action (whitespace/case/punctuation-insensitive) and no state-changing tool call
  (anything but read/search/fetch/todo/subagent-spawn) in the later round → pause
  with "no progress".
* **Idempotence:** a round prompt is keyed `goal-<id>-r<n>`; re-sending is a no-op
  if it is queued or already in the transcript, so a restart, a lost row or a double
  tick never double-continues. Verifications are idempotent the same way, per
  `(goal, round)`.

### Restart and recovery

`pending` is persisted, and the host keeps a device-local `goals.json` (chats with a
running goal — the host cannot afford to open every chat). On boot, after a short head
start for crash recovery's auto-resume of interrupted turns, each listed chat is
opened and ticked:

* `verifying` with no live verifier (the engine restarted mid-verification) → the
  round is judged again (the dead verifier's chat is archived by `recover_stale`,
  which never auto-resumes ask children);
* a round whose turn was revived by crash recovery completes under its own message id
  and is then verified — one run, one user entry, no second round;
* a round whose turn died and was not revived → paused (`restarted`);
* the controller's own prompt alone in a queue that the "recovered queue stays frozen"
  rule would hold is thawed (a person's queued rows are never touched).

### Read-only chats instead of plan mode

Zeron has no host-visible plan mode, so the analogue is a read-only chat
(`config.sandbox == ReadOnly`): a goal set there is recorded and immediately
`paused (readOnly)`; nothing is queued or verified.

### Titling

`summaryTitle` is the truncated first line of the objective. Reusing the chat-title
pipeline for an LLM title is not a trivial reuse (it titles *chats* from a prompt and
writes the chat row), so it is skipped.

## Surfaces

**Desktop** (`crates/ui/src/goal_panel.rs`). A tray at the top of the composer dock
stack (goal, todo, queue, composer; each tray one step narrower than the one below):
header with goal icon, status chip (Active / Verifying / Paused / Complete / Budget
reached), the current round's title or the stop reason, `R<n> · elapsed`, and
pause/resume and clear icon buttons with tooltips. Expanded: the objective, a meta line
(`Round 3 of 25 · 13k of 50k tokens (verifier 1k) · 4m 12s`), the reason line, and a
rounds list. Round 1's title is the goal title, round *n*'s is the previous verdict's
next action; opening a round shows the verdict reason, the todo items that first
appeared during it (latest status) and "Open verifier chat". Markers in the transcript
replace the round prompts and show verdicts, completion and pauses.

Stacked trays never overlap their content: each expanded list keeps a clearance equal to the edge the next tray tucks over it, caps its height (the goal list takes 32% / 22% / 18% of the window as 0 / 1 / 2 trays stack below it; todo and queue keep 30%) and scrolls with the shared edge fade. The goal list follows the newest round (and verdict) as they land.

The elapsed ticker repaints once a second while a goal runs and not at all under
reduced motion (which covers "pause animations in background"); the spinner is the
shared mini glyph spinner.

Slash: `/goal` (show the tray), `/goal <objective>`, `/goal replace <objective>`,
`/goal pause|resume|clear`. Only a line that *starts* with `/goal` is a command
(indent to send literally); control words must be the whole argument, so `/goal pause
the deploy` is an objective. Choosing `/goal` from the popup inserts `/goal ` for the
objective. There are no `--rounds` flags: slash commands here take no flags, so caps
are the defaults in the UI and `max_rounds` / `token_budget` /
`time_budget_seconds` on the MCP `set_goal`.

**MCP**: `get_goal`, `set_goal`, `pause_goal`, `resume_goal`, `clear_goal`
(`docs/mcp.md`). No tool completes a goal, and an agent cannot pause, resume, clear or
replace the goal verifying its own chat.

**Mobile**: the iOS transcript shows a goal strip and compact markers, `/goal` works from the composer
(`docs/workflows-mobile.md`). The pure parts (`/goal` parsing, chip, header text, markers) are
`zeron_proto::goal_view`, shared with the desktop tray.

## Verifier prompt

Adapted from ZCode's (see `THIRD_PARTY_NOTICES.md`): verification only; do not
modify anything; the transcript (readable with `read_chat` on the parent chat) is the
agent's account, not proof; verify against the actual workspace (read files, run tests
that leave nothing behind); inspect the todo list; fail on missing, weak, plan-only or
effort-only evidence; an objective needing no work passes; reason and next action in
the objective's language. The objective is wrapped in `<untrusted_objective>` with
`& < >` escaped, so it can neither close the tag nor open another.

## Failure modes

| What happens | Result |
| --- | --- |
| Verifier times out / errors / returns invalid output / asks a question | goal pauses (`verifierFailed`), reason shown, no retry |
| The agent's turn errors | pauses (`turnFailed`) |
| User presses Stop | pauses (`interrupted`) |
| Engine restarts mid-turn / mid-verification | revived turn is verified; dead verification re-run; unrevived turn pauses |
| Chat's host is another device | nothing runs here; the host runs it |
| Host is an older build | the command is skipped; no goal appears; `set_goal` reports `queued` |
| Chat is read-only | goal recorded, paused (`readOnly`) |
| Caps reached | `budgetLimited`; resume extends every cap by its allowance |
| Harness has no usage reporting | tokens read 0; rounds and time caps still apply |

## Testing

```sh
cargo test -p zeron-proto goal
cargo test -p zeron-doc goal
cargo test -p zeron-engine --lib -- goal:: ask::     # state machine, prompts, schema
cargo test -p zeron-engine --test ask_child          # the primitive against a real engine
cargo test -p zeron-engine --test goal_mode          # the loop, interrupts, caps, restart, non-host
cargo test -p zeron-mcp
cargo test -p zeron-ui --lib goal_panel
```

Live: `ZERON_HARNESS=mock ZERON_MOCK_GOAL=1 ZERON_MOCK_DELAY_MS=400`, then
`/goal Make the build green` in a chat. The mock turns add checklist items and run a
command; the engine's scripted verifier says "not satisfied" twice and then passes.

## Limits and follow-ups

* No harness enforces a read-only sandbox for chats (above).
* "Background work" detection covers subagent chips, not arbitrary background shells.
* A goal set from a client whose host is older is inert, silently; the UI gates on the
  capability, MCP reports `queued`.
* Per-goal verifier harness/model selection is possible in `AskSpec` but not exposed in
  the UI yet.
