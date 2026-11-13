# Orchestrators and the Control room

Long-lived manager agents that run crews of threads, a board that shows where every piece of work
stands, one queue for everything that needs the person, and a **Brain dump** that takes whatever the
person says, in any order, and routes each part to the right orchestrator or thread.

Wire types: `crates/proto/src/orchestration.rs`. RPCs: `WatchOrchestration`, `Orchestrate`,
`WatchDeviceLoads`, `WatchComms` (`crates/rpc/src/lib.rs`).

## Why (the problem today)

A chat made into an orchestrator by hand spawns threads with `create_chat`, and every child reply
arrives through `send_message` as a **user turn** in the orchestrator's transcript. Children that
report wake it, it answers, they answer back. The transcript turns into noise, and nothing says what
state each task is in, where the answer to your question went, or what's waiting on you.

## What we keep from Capy Captains, and what we do better

| Capy | Zeron |
| --- | --- |
| Captain = long-lived agent; crew = threads it starts | Same. An orchestrator **is a chat** (same id) plus an `orchestrators` row. Crew threads are `ChildKind::Crew` children: full threads that may delegate to their own subagents. Existing threads can be added to a crew. |
| AGENTS.md / CREW.md / NOTES.md in a Drive | `rules`, `crewRules`, `notes` fields on the row: synced, editable in the UI, and readable and writable by the agent through tools. |
| DIGEST.md rewritten by the LLM | **No LLM-written digest.** The board, asks and crew state are structured data, so the digest is *computed* and is always true. The orchestrator moves items; the engine derives activity (testing, merging…), todo progress, PR and check state. |
| Crew messages land in the Captain's chat | Crew → orchestrator traffic is a **report**, not a user turn. Reports are filed as a compact "crew updates" part. Only reports that matter wake the orchestrator, and they are **coalesced** into one wake. |
| "Needs you" section + mention | **Asks** are first-class: question, decision, approval or blocker, with options. One queue across every orchestrator, answered inline, and the answer is delivered to the asker. Agents' own `AskUserQuestion` prompts (chats awaiting input) join the same queue. |
| Slack `captain=<name>` routing | **Brain dump**: one place to dump anything. A routing agent splits it into fragments and sends each to the right orchestrator or thread, or a new one. A receipt shows where each part went, and a part can be moved with one click. |
| — | **Control room**: a live map of machines, orchestrators and threads (with what each is doing), the board, machine load, and agent comms. |

## Concepts

- **Orchestrator** (`Orchestrator`): name, goal, rules, crewRules, notes, hue, crew policy (devices,
  max running, default harness and model), optional wake schedule, paused, archived. Kind `standard` or
  `brainDump` (one per profile, created on first use and hidden from the session list).
- **Crew** (`CrewMember`, keyed by thread chat id): orchestrator, item, role, last report head,
  released.
- **Work item** (`WorkItem`): the board. States: inbox, planned ("Next"), active ("Now"), blocked,
  review, done, dropped. Priority urgent, high, normal or low. Progress is explicit, or derived from
  the threads' `Session.todo`. Links (PR, branch, chat, URL), plus an **outcome**: the answer or
  result, which is what Answers shows.
- **Report** (`Report`): crew → orchestrator: progress, done, blocked, failed or message, with a
  one-line summary and optional detail and links.
- **Ask** (`Ask`): anything for the person.
- **Dump / Route**: the person's text and where each fragment went.
- **Live**: `Session.activity` (`ChatActivity`, via `classify_command`) and `Session.todo`
  (`TodoProgress`) are written by the host on the synced session row. `DeviceLoad` and `CommEvent`
  are ephemeral streams.

## Engine

### Storage

These are synced registry kinds (per-field LWW like chats, any device may write):
- `orchestrators`
- `crew`
- `workItems`
- `asks`
- `dumps`

Follow the `repositoryLooks` submodule pattern (`crates/doc/src/registry/repository_looks.rs`): one
submodule `registry/orchestration.rs` with `upsert_*`/`read_*`. Row ids are the entity ids (UUID
v4 simple hex, inside the room alphabet). Nothing is ever deleted: done and dropped items, answered
asks, released crew and old dumps stay. `WatchOrchestration` sends at most 50 newest dumps. History
lives in the registry and in transcripts.

`WorkspaceHost` gets a `watch::Sender<OrchestrationState>` published on change (same pattern as
chats, coalesced ≤ 4/s).

### Child kind and delegation depth

- An orchestrator chat may create chats; children it creates default to `crew` (not `subagent`).
- A **crew** thread may create subagents (one more level). Subagents still create nothing.
- The Brain dump may create orchestrators and standalone threads.
- `agent_gate` ceilings follow the same rule as the voice orchestrator exemption that exists today.

### Reports: the anti-spam path (core of the fix)

- `Orchestrate{op: report}` from a crew thread:
  1. Updates the crew row's `lastReport` (and the item's progress and links when given).
  2. Appends the report to the orchestrator's transcript as a new message part `crewReports`
     (`MessagePart::CrewReports { reports: Vec<Report> }`). Consecutive reports since the
     orchestrator's last turn go into **one** message, extended in place.
  3. Emits a `CommEvent::Report`.
  4. If `state.wakes()` and the orchestrator is idle, schedules a wake **15 s** later, so more reports
     can join. If it's working, the wake waits until the turn ends. One wake turn carries every unread
     report.
  5. `progress` reports never wake.
- The wake turn's prompt to the harness is generated text. It lists the reports, compact, with chat ids
  and item ids. In the transcript, the wake is the `crewReports` message itself: clients render that
  message as a "Crew updates" card and **never** as a user bubble.
- A crew thread's `send_message` to its own orchestrator is converted to a `report` with state
  `message` (body = detail, first line = summary). This keeps older prompts and agents working.
- Orchestrator → crew `send_message` is unchanged. Crew rules are prepended once per run as a
  `[Crew rules from <orchestrator>]` block, only when they changed since the last send.

### Asks

- `ask` creates the row, emits a desktop notification (`Ping::Request`) and a `CommEvent::Ask`.
- `answerAsk` marks it answered and delivers `[Answer from you] <question excerpt> → <answer>` to
  `fromChat`: run if idle, steer if working.
- A chat in `AwaitingInput` is not an `Ask` row. Clients merge it into the same queue from session
  status, and answering goes through the existing `RespondInput`.

### Brain dump flow

1. `Orchestrate{op: dump, text}` creates a `Dump{state: routing}` and ensures the Brain dump exists. It
   runs on this device with a fast model by default (configurable like any chat), with goal and rules
   from the built-in prompt below. The dump is sent as a run:
   `[Dump <id>]\n<text>`.
2. The Brain dump agent calls `world` (a compact snapshot), then `route` once per fragment, then
   `dump_done`.
3. `route` records the `Route` (span found by exact search of `excerpt` in the text) and **delivers
   at once**:
   - to an orchestrator or chat, as a message from the Brain dump (normal agent message attribution);
   - `newOrchestrator`/`newThread` create it first;
   - `item` creates a loose work item;
   - `note` appends to the Brain dump's notes;
   - `unsure` delivers nothing and sets dump state `needsYou`.
4. Routes with intent `question`: when the target's next turn ends, the engine copies the reply's
   final text into `route.answer` and sets state `answered`, and the dump shows it. Implementation:
   remember `(target chat, turn after delivery)` and read the last assistant text on completion.
5. `reroute` (the person): sends the new target the message, sends the old one a one-line retraction
   (`[Brain dump] Ignore the earlier note "<excerpt…>": it was meant elsewhere.`), and updates the
   route.

### Activity, todo and machines

- The host engine sets `Session.activity` while working, from the running tool. Bash or command calls
  go through `classify_command`. Edit/Write → editing, Read/Grep/Glob → reading, web tools → browsing,
  create_chat/send_message → delegating, none → thinking. `Session.todo` comes from the newest
  `ToolCall::Todo`. Write only on change (it's a synced row).
- `WatchDeviceLoads`: while subscribed, every 5 s, local CPU (sysinfo, already a dep), memory and
  load1. For other online devices, `DeviceStats` over the relay plus a CPU figure (add `cpu.usage`
  to `DeviceStats`).
- `WatchComms`: a bounded ring (100) of `CommEvent`s this engine produced or delivered. Messages
  between agents (the existing agent-message path), reports, asks, answers, routes and spawns.
  Every frame is an array: the backlog first, then arrays of the events since (the backlog again
  after a lag). Clients merge frames and dedupe by `id`; a frame never replaces the list.

### Wake schedule

A small timer in the engine. For each non-paused orchestrator **hosted here** with `wake`, it sends
`wake.prompt` as a run at the scheduled times (skipped if already working; `lastWokeAt` updated).

### MCP tools (Zeron MCP server, filtered by the caller's role)

Role is resolved at server start: orchestrator, Brain dump, crew or plain.

| Tool | Who | Op |
| --- | --- | --- |
| `report(state, summary, detail?, item?, links?, progress?)` | crew | report |
| `ask(question, kind?, options?, urgent?, item?)` | everyone | ask |
| `board(filter?)` → items, crew with status/activity/todo, open asks | orchestrators, Brain dump | read |
| `item_upsert(…)` | orchestrators, Brain dump | upsertItem |
| `crew_add(chat, role?, item?)`, `crew_release(chat)` | orchestrators | addToCrew / releaseFromCrew |
| `notes_set(text)` | orchestrators | updateOrchestrator(notes) |
| `world()` → orchestrators (goal, crew, items), recent top-level chats (title, status, project, device), devices | Brain dump | read |
| `route(dump, excerpt, target, intent, message, candidates?)` | Brain dump | route |
| `dump_done(dump, summary)` | Brain dump | dumpDone |
| `orchestrator_create(name, goal, rules?, project?, device?, prompt?)` | Brain dump | createOrchestrator |

`create_chat`/`create_chats` from an orchestrator default to kind `crew` and accept `item` and
`role`. They apply the crew policy: allowed devices, `maxRunning` (refuse with a clear message when
full), and default harness and model.

### Built-in prompts (appended to the harness's system prompt by role)

- **Orchestrator**: "You are an orchestrator in Zeron: <name>. Goal: <goal>. Your rules: <rules>.
  - Split work into board items (`item_upsert`) and run each through crew threads (`create_chat`,
    kind crew, one item each; stack work that touches the same files).
  - Keep item states true; put results in `outcome`.
  - Crew reports arrive as 'Crew updates': act on them, don't echo them.
  - Ask the person only via `ask`, only for real decisions.
  - When the person asks you something, answer in your reply, briefly, and record the answer as
    the item's outcome.
  - Keep `notes` as your plan."
- **Crew**: "You are on <orchestrator>'s crew, working on <item>.
  - Crew rules: <crewRules>.
  - Report with `report`: done, blocked or failed when it happens, progress at milestones. Never
    `send_message` your orchestrator.
  - Ask the person (`ask`) only when your orchestrator can't decide."
- **Brain dump**: "You route the person's brain dumps.
  - For each dump, call `world` once.
  - Split the dump into fragments by meaning (one per destination and intent); quote each fragment
    verbatim as `excerpt`.
  - Route each to the best existing orchestrator or thread. Create an orchestrator only for a new,
    lasting area of work, and a thread for a one-off task.
  - Rewrite `message` so the target can act without the rest of the dump.
  - Use `unsure` with candidates instead of guessing when two places fit.
  - Finish with `dump_done` and a one-line summary.
  - Never do the work yourself."

## Client (zgx), core feature

### Why core, not a plugin

Orchestration changes how messages flow (engine), needs data plugins can't see (parents, crews,
activity, todo), and becomes primary navigation. It also needs to work headless and on mobile against
the same engine. So the engine owns it, and zgx draws it as core screens.

Plugins can extend it through slots:
- `slot:orchestration.widgets`: extra Control room widgets.
- `slot:orchestration.item`: decorations on a board card (the github plugin can show PR checks there).

### Bridge

`apps/zgx/src/orchestration.rs` `install(cx, host)`:
- Watches `WatchOrchestration`, `WatchDeviceLoads` (only while a screen that shows loads is
  mounted: a `orchestration.loads_wanted(bool)` action) and `WatchComms`.
- Publishes `ui.shared("orchestration")`, `ui.shared("orchestration.loads")` and
  `ui.shared("orchestration.comms")`.
- Action `orchestration.op(op_table)` → promise, which forwards to `Orchestrate`.

Luau shape, snake_case with Unix seconds like `sessions.now`. It is wrapped by
`ui/account/orchestration.luau`. `apps/zgx/ui/account/orchestration_sample.luau` is the canonical
example, and every fixture uses it:

```
orchestration.now:get() = {
  state = "loading" | "ready",
  orchestrators = { { id, kind = "standard"|"brain_dump", name, goal, rules, crew_rules, notes, hue,
                      crew = { devices, max_running?, harness?, model? }, wake?, paused, archived,
                      created, updated } },
  crew  = { { chat_id, orchestrator_id, item_id?, role, last_report? = { id, state, summary, at },
              released, joined } },
  items = { { id, orchestrator_id?, title, detail, state, priority, progress?, threads, links =
              { { kind, label, target } }, outcome?, source?, order, created, updated, done? } },
  asks  = { { id, from_chat, orchestrator_id?, item_id?, kind, question, options = { { label,
              detail } }, urgent, state, answer?, created, answered? } },
  dumps = { { id, text, state, routes = { { id, excerpt, span? = { from, to }, target = { kind, id? },
              intent, message, state, candidates, answer?, item_id? } }, summary?, device, created } },
}
orchestration.loads:get() = { [device_id] = { cpu?, mem_used?, mem_total?, load1?, cores?, online, at } }
orchestration.comms:get() = { { id, kind, from, to, summary, at } }   -- newest last, ≤ 100
```

Enum strings are snake_case (`brain_dump`, `needs_you`, `new_orchestrator`…). Chats come from
`sessions.now` as before. Session rows add `activity = { kind, label, since }` and
`todo = { done, total, current? }` to each chat.

### Screens

Routes: `control "/control"` and `orchestrator "/orchestrator/:id"`.

**Sidebar**:
- A "Control room" entry at the top. It carries a count badge of open asks plus chats awaiting
  input.
- An **Orchestrators** section: one row per orchestrator, with
  - its hue dot, pulsing while any crew thread works;
  - a working/total crew count;
  - its needs-you count.
- Orchestrator chats and the Brain dump are not listed as sessions. Crew threads stay hidden, as
  children already are.

**Control room** (`/control`): the Brain dump's home. Default widgets, top to bottom:
1. **Capture**: a large multi-line composer ("Dump anything"), Enter to send, Shift+Enter for a
   newline, voice when available. Under it is the **receipt** of the newest dump:
   - the dump text with each routed excerpt underlined in its destination's hue;
   - a chip per route: destination, intent glyph, state (pending, delivered, answered with the
     answer expandable);
   - `unsure` routes show candidate buttons inline;
   - any chip can be moved to another target from a menu.
   Older dumps collapse into a history list.
2. **Needs you**: open asks plus chats awaiting input, urgent first. Each is answered inline (option
   buttons, or a one-line field). Answered ones slide out.
3. **Map**: the memorable piece. One lane per online device, labelled with name and a load bar.
   - Orchestrator nodes sit in their hue, on their host's lane.
   - Crew threads are pills on the lane of the machine they run on, joined to their orchestrator by a
     hue line. Each pill shows status, an activity glyph (test, build, merge, push, edit…) and todo
     progress as a thin bar.
   - Comms animate as a short pulse travelling along the edge (respecting reduced motion).
   - Click opens the thread or orchestrator.
4. **Board**: columns Now, Next, Blocked, Review, Done (today). Cards sorted by priority, then order.
   - Each card shows the hue stripe, title, progress bar, thread avatars, PR chip and a priority
     mark.
   - Drag between columns or reorder sets state/order; a menu sets priority.
   - Filter by orchestrator.
5. **Machines**: per device CPU and memory sparkline (kept client-side from the stream), running
   threads, and tests or builds running now (from `activity`).
6. **Comms**: a live feed of reports, messages, asks, answers and routes, as compact rows with from →
   to in hues.

Widgets can be shown or hidden and reordered (device-local `ui-settings`). Layout: two columns at
≥ 1100 px (left: Capture, Map, Board; right: Needs you, Machines, Comms), one column below that.

**Orchestrator** (`/orchestrator/:id`):
- A header with name, goal, hue, crew count, model, paused toggle and a settings menu (rules, crew
  rules, notes, crew policy, wake schedule), editable in a dialog.
- A body split in two:
  - **Left: the conversation**: the orchestrator's chat (existing transcript and composer), where
    `crewReports` messages render as a "Crew updates" card. Each card has one row per report
    (thread title, state chip, summary) and collapses after the first three.
  - **Right: the live state**: Needs you (this orchestrator's), the board filtered to it, and Crew,
    a grid of thread cards with status, activity, todo progress, machine, model and last report.

"New orchestrator" lives in the sidebar section header and the Ctrl+K palette. It opens a dialog with
name, goal, project, device and model; nothing else is required. "Make orchestrator" is on a chat's
context menu (`promoteChat`).

### Visual language

- Stay inside zgx tokens and `@ds`; there is no new palette.
- The only added colour is each orchestrator's **hue**, a token-derived accent at fixed
  lightness/chroma for light and dark. It is the thread that ties map, board, receipt and sidebar
  together.
- One orchestrated moment: when a dump is routed, its excerpts tint in sequence, about 60 ms apart,
  and their chips appear. Everything else moves only in response to the person or to real state
  changes (pulses on comms, progress tweening).
- Minimal copy: no helper text, empty states that invite action ("Nothing needs you").

## Decisions with Jev (optional, bring your own key)

[Jev](https://openrouter.ai/blog/insights/what-is-jev/) is TypeSafe's decision model. It returns typed verdicts (yes/no, a choice, a score) in well under a second and costs fractions of a cent per million input tokens. Orchestration makes many small decisions of this kind. A `Decider` in the engine makes each of them:
- **Rules** (default, today's behaviour);
- **Jev**, when the person adds a key in Settings → Orchestration. The model is `jev-latest` (the newest stable release, following TypeSafe's alias; `jev-preview` is opt-in). The key is stored like other provider keys, device-local, never synced.

Every Jev call has a confidence floor. Below it, the decision falls back to the rule or the agent, so a weak verdict can't misroute anything. Each decision is logged with its inputs, verdict and score, and shown on hover where it's used. Text sent to Jev is the fragment or report at hand, never whole transcripts.

| Where | Question | Effect |
| --- | --- | --- |
| Brain dump pre-pass | per clause: which target (orchestrators, recent threads, inbox, note) + intent | Receipt in < 1 s; the agent only handles low-confidence fragments (→ `unsure` chips), creation and message rewriting |
| Report wake gate | "does this report need the orchestrator to act?" | Fewer wakes than the fixed `ReportState::wakes` rule |
| Ask triage | "needs the person, or can the orchestrator decide under its rules?" + urgency score | Asks the orchestrator can settle go back to it; Needs you is ordered by urgency; notifications only above a threshold |
| Board state | the thread's last reply: done / blocked / review / still going | Cards move without an orchestrator turn |
| Route answered? | "does this reply answer the routed question?" | `answered` only when it does |
| Duplicate item | "is this fragment the same as item X?" | Merge instead of a new card |
| Stall | "is this thread looping?" on its recent output | Raises a blocker ask |
| Model tier | "which tier fits this task?" when spawning crew | The "model that fits each thread" choice, within the crew policy |

Phase 1 (after this build lands): the `Decider` seam, the key setting, the Brain dump pre-pass and ask triage. The rest follows once those are measured.

## Build streams

| Stream | Owns |
| --- | --- |
| engine-state | registry kinds, `WorkspaceHost` state and watch, `Orchestrate` handler (all ops except report delivery), `WatchOrchestration`, crew child kind and depth rules, Brain dump creation and dump dispatch, route delivery and reroute, asks and answer delivery, wake schedule, `WatchDeviceLoads` |
| engine-flow | `MessagePart::CrewReports` and report delivery with coalesced wake, crew `send_message` → report, `Session.activity`/`todo` projection, `WatchComms`, MCP tools and role filtering, built-in prompts, crew-policy enforcement in `create_chat`, question-answer capture for routes |
| ui-control | bridge (`orchestration.rs`, account wrapper, sample), `/control` route and all its widgets, sidebar Control room entry |
| ui-orchestrator | sidebar Orchestrators section, `/orchestrator/:id`, Crew updates card in the transcript, create/promote/settings dialogs, map and board components shared with ui-control (ui-control owns `ds/orch_*` map and board; ui-orchestrator consumes them) |
