# BB capability gap and Glitch Flow backlog

> Planning note, 2026-09-25. Source review only; no live parity test or implementation is implied.
> GF numbers below are draft ticket IDs in this document, not GitHub issues or native app tasks.

## Native Tasks implementation update

The capability map below records the **pre-implementation baseline**. The
current Glitch Flow source now includes native boards linked to device/folder
Spaces, epics, issues and nested subissues, comments, agent chat links, a Tasks
list/board/detail UI, and agent-facing MCP ticket operations. The Rust registry
is the authoritative local-first store; bb's Tasks service is a reference, not
a runtime dependency. The Windows demo seeds a small example board.

GF-001, GF-002, GF-004, and GF-006 now have first implementations. GF-003 has
ticket-to-chat linking and a start-chat flow in the native UI. GF-005 has
subissues and comments; labels, attachments, dependencies, assignments,
notifications, and richer activity history remain future work. The feature
still needs a headed UI pass and cross-device acceptance before parity claims.

## Scope and evidence

This note compares the current Glitch Flow GPUI prototype in this checkout with get-bb/bb main at 2f61160fe5c31c4f067fe72de125b2dffbb8c3c3. The separate SchizzY/glitch-flow repository currently contains only the small mock-agent demo, not this UI. This checkout has substantial uncommitted work, including the Glitch Flow app, so preserving a reviewable source baseline is a prerequisite to implementation.

"Present" means source exists, not that every path was run or matches bb. "Partial" means related code exists but a named bb behavior is missing or unverified. "Missing" means no corresponding product model or runtime was found in the inspected Rust app. bb plugins and experiments are identified as such; they are not assumed to be core features.

Local anchors: [prototype scope](../../PROTOTYPE.md), [Space and Chat models](../../crates/proto/src/entities.rs), [workspace registry](../../crates/doc/src/workspace.rs), [recursive chat sidebar](../../crates/ui/src/shell/spaces.rs), [terminal service](../../crates/engine/src/terminals.rs), [device RPC](../../crates/engine/src/rpc.rs).

## Capability map

| Area | bb capability | Glitch Flow source state | Backlog |
| --- | --- | --- | --- |
| Tasks and tickets | [Official Tasks plugin](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/plugins/tasks/README.md): tracker projects, keys, states, priority, board/list, comments, links to agent threads, delegation, CLI | **Missing.** No Task, board, or ticket row in the registry. Project Actions are saved terminal commands, not work items. | GF-001–GF-006 |
| Conversations | [Durable threads and event timelines](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/docs/system-overview.md), send, steer, queue, fork, handoff | **Partial parity.** Chats, run journals, durable queue, and provider events exist; bb's full lifecycle and event semantics need comparison. | GF-007 |
| Child work | bb parent/child threads, owner lifecycle, cross-project children, blocker/completion signals | **Present in part.** Chat has parent_chat_id, MCP creates children, and the current sidebar renders a recursive tree. Parent/owner lifecycle parity is unverified. | GF-007 |
| Terminal | [Host terminals](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/packages/sdk/src/areas/terminals.ts) with input/output/replay/resize and remote host routing | **Present in part.** Native PTY and device routing exist. Shells detach from the UI but end with the engine. | GF-009 |
| Projects and worktrees | [Existing checkout, managed worktree, setup/teardown, cleanup](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/docs/worktrees.md); machine-specific sources | **Partial.** A Space is a device/folder pair; Git branches and worktrees exist. Cross-device logical project identity and full lifecycle parity need design. | GF-008 |
| Files and review | File browsing/editor, diffs, optional file viewer/editor plugins, PR/issue surfaces | **Partial parity.** Native editor and diff viewer exist; bb's broader preview and review flows are not fully audited. | GF-017 |
| Providers and approvals | Codex, Claude, Pi, ACP and others; model/permission routing, approval and question UI | **Partial.** Several harness slots and input UI exist. Provider breadth and permission/approval contracts need direct parity tests. | GF-010 |
| Workflows | [Optional Workflows plugin](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/plugins/workflows/README.md): QuickJS orchestration, durable runs, hidden worker threads, replay and inspection | **Missing.** Project Actions do not provide an agent workflow runtime. | GF-011 |
| Automations | Scheduling, delayed sends, concurrency limits, script/agent automations | **Missing or unverified.** No equivalent scheduler/run model found in the Rust source review. | GF-012 |
| Multi-device execution | [Enrolled host daemons and per-machine project sources](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/docs/multiple-devices.md) | **Partial.** Device-addressed relay exists, but requires configured edge/auth; hosted enrollment and management differ. | GF-013 |
| Remote viewing and phone | bb Connect or private URL, remote browser client, mobile client and push | **Missing/partial.** This native app can drive peers through its relay, but no bb-style remote browser/mobile client was found. | GF-013, GF-018 |
| GitHub | [Official GitHub plugin](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/plugins/github/README.md): issues, PRs, mentions, review and dispatch | **Partial.** PR discovery/attached URLs exist through the host gh installation; issue tracking and broader review flow are not present. | GF-015 |
| Documents and memory | [Official Docs plugin](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/plugins/docs/README.md), document mentions and vaults; Memory plugin | **Missing or unverified.** File editing exists, but no comparable managed document/memory product was found. | GF-016 |
| Extensibility and scripting | [Plugin system, skills, CLI and SDK](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c/docs/repository-overview.md): pages, tools, jobs, commands, storage | **Partial.** Typed RPC and MCP tools exist; no equivalent user-installable plugin platform or general user CLI/SDK was identified. | GF-014 |
| Browser agents | [Optional Browser Automation plugin](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/plugins/browser-automation/README.md), live preview and annotation | **Unverified.** Browser UI code exists, but bb-level agent browser control was not established in this audit. | GF-019 |
| Navigation and polish | Thread search, splits, tabs, command palette, drafts, mentions, configurable shortcuts | **Partial.** Native panes/splits, sidebar, composer, and drafts exist; a behavior-by-behavior audit is needed. | GF-017 |
| Voice and notifications | Voice dictation, remote/push notifications, task activity | **Unverified/partial.** Audit before treating these as missing. bb Tasks itself lists a task inbox/notifications as future work. | GF-018 |

bb currently supports Windows execution through Ubuntu on WSL2, not native Windows PowerShell or drive-letter paths. That matters only if a later ticket chooses bb itself as the backend; it is not a requirement to replace the existing Rust engine. See [bb platform support](https://github.com/get-bb/bb/blob/2f61160fe5c31c4f067fe72de125b2dffbb8c3c3/docs/platform-support.md).

## First product milestone: native Tasks

A task is planned work. A chat is execution and discussion. A workflow is orchestration. They may link to one another without sharing identity or lifecycle. Native Tasks should be part of Glitch Flow's own product, not a pasted bb React panel.

The first data model should use a stable TaskBoard identity separate from Space. A Space is one device/folder pair; one board can explicitly link to one or more Spaces. Do not infer a shared project from matching folder names or Git remotes. Tasks belong to the board and carry no device path. Chat links supply execution context. Deleting or unlinking a Space must not delete its planning record.

Choose one authoritative task store before GF-001 implementation. If bb becomes the backend, the native GPUI Tasks screen can use bb Tasks through an adapter; do not duplicate those records in Loro. If the Rust engine remains authoritative, the existing registry offers local persistence, queued offline changes, and optional sync, but [registry rows are bounded index data](../../docs/registry-sync.md). The edge limits each serialized operation to 16 KiB ([registry core](../../edge/src/registry-core.ts)); descriptions then need a validated small limit, with comments, attachments, and long documents in an appropriate content store. Never silently truncate task text.

Suggested release cut:

1. GF-000 preserves the actual UI source in a reviewable baseline.
2. GF-001 and GF-002 deliver persistent native boards/tasks and a usable list/detail view.
3. GF-003 links tasks to chats and lets the user explicitly start work from a task.
4. GF-004–GF-006 add board polish, richer records, and agent-facing task operations.

## Draft tickets

### GF-000 — Preserve the actual Glitch Flow source baseline (P0, prerequisite)

Capture the current U: GPUI app and its uncommitted changes in a reviewable project source state without losing unrelated work. Record which executable and worktree reproduce the visible UI. The four-file SchizzY/glitch-flow demo must not be treated as the app source.

### GF-001 — Native TaskBoard and Task persistence (P0)

Choose and document the authoritative task backend after a small API/persistence spike (bb Tasks service or Glitch Flow registry). Add stable board/task IDs, explicit board-to-Space links, title, validated Markdown description, status, priority, timestamps, and a typed UI-facing read/write/watch interface. On first use, create or choose a board linked to the selected Space. Verify restart durability, multi-client visibility, concurrent update behavior, input bounds, and that unlinking/deleting a Space leaves board/tasks intact. If the registry is chosen, verify offline edits, configured-device sync, and the 16 KiB operation bound. If bb is chosen, verify its service contract through the native client without a second task store. No agent dispatch in this ticket.

### GF-002 — Native Tasks route and list/detail UI (P0; depends on GF-001)

Add Tasks to the GPUI shell for the selected board. Create, read, edit, and change status/priority from a list and detail view. Show empty, loading, conflict, and error states; retain selection across navigation. Use the existing visual system. No scheduler or workflow state in this view.

### GF-003 — Link tasks to chats and start work explicitly (P0; depends on GF-001–002)

Attach/detach multiple chats to a task, including existing parent/child chats. Show live chat status and worktree context from the linked Chat rather than copying it into Task. From a task, start a new chat in an explicitly chosen linked Space using the existing composer/worktree flow, then record the link. Creating a task alone must not launch an agent.

### GF-004 — Board view and finding tasks (P1; depends on GF-002)

Add status columns, status moves, task search, and basic filters. Reuse the same task queries and mutations as list/detail; verify the two views agree after restart and remote sync. Manual ranking can be a later refinement.

### GF-005 — Rich task records (P2; depends on GF-001)

Add subtasks, labels, comments/activity, and attachments with a storage design appropriate for content larger than registry index rows. Keep the task's history readable if a linked chat is archived or deleted.

### GF-006 — Agent-facing task operations (P1; depends on GF-001–003)

Expose bounded task read/search/update/comment/link operations through the app's existing agent control surface (MCP, and a CLI only if a CLI is adopted). Let agents report progress to a linked task. Add explicit delegation/presets only after user-controlled launching and task-to-chat linking work reliably.

### GF-007 — Thread and child lifecycle parity (P1)

Audit bb's fork/handoff, queue/steer, child ownership, blockers, completion notifications, archive/delete, and cross-project child behavior against current chats. Keep the recursive child sidebar and add only missing behavior; define recovery and parent/child cleanup tests for each selected gap.

### GF-008 — Project and worktree lifecycle (P1)

Define logical project identity across device-specific Spaces. Compare managed worktree creation, setup/teardown hooks, retention and cleanup, and source mapping to bb. Implement selected gaps without changing unrelated checkouts or deleting a workspace still used by a chat.

### GF-009 — Terminal lifetime and remote behavior (P1)

Compare current PTY detach/replay/reconnect/engine-exit behavior to bb host terminals. Specify desired lifetime across UI and engine restarts, remote routing, resource caps, and cleanup; implement and test the chosen contract.

### GF-010 — Providers, permission modes, and approvals (P1)

Audit real installed provider/model discovery, authentication, permission ceilings, command/file approvals, and question delivery across local and remote devices. Ticket only demonstrated gaps after the matrix is verified with live provider sessions.

### GF-011 — Durable agent workflows (P2; depends on GF-003, GF-007)

Choose a small workflow model with runs, workers, progress, cancellation, concurrency, and recovery. Keep workflow runs distinct from tasks and chats. The first workflow should create linked worker chats, survive app restart, and show progress in a native inspector; advanced replay can follow.

### GF-012 — Automations and scheduling (P2; depends on GF-011 where applicable)

Add scheduled/delayed sends and recurring task or script execution only with durable dispatch state, clear ownership, cancellation, retry bounds, and a visible run history. Project Actions remain user-launched terminal commands until deliberately integrated.

### GF-013 — Device enrollment and remote access (P1 discovery, then implementation)

Map the current peer relay and sync model against bb's central server, execution-machine enrollment, remote viewing, and mobile pairing. Decide which experiences Glitch Flow should own. Verify real cross-device execution under configured edge/auth before marking existing device code production-ready. Separate remote control from remote execution in UI and tickets.

### GF-014 — Scripting, skills, and extensions (P2)

Define which task/thread/project operations need a stable CLI or SDK and which extension points are worth supporting. Start with documented read/write contracts for the native Tasks and chats; defer a general plugin marketplace until a concrete use case requires it.

### GF-015 — GitHub issues and PR review (P2)

Extend current PR discovery/URL links into task-linked issue and PR records, status refresh, comments, and review actions only after task identity is stable. Preserve provenance between a manually linked PR, a task's implementation PR, and a reference PR.

### GF-016 — Documents and knowledge (P2 discovery)

Decide whether a native document vault or structured project knowledge is needed beyond the file editor. If yes, define storage, mentions, conflict-aware edits, and task/chat links before copying bb's Docs or Memory surfaces.

### GF-017 — Navigation, files, and review parity (P2 audit)

Check bb's search, split panes, panel tabs, drafts, mentions, shortcuts, previews, editor, and diffs against the current GPUI behavior. Record concrete missing journeys and create smaller implementation tickets; do not rebuild features already present.

### GF-018 — Mobile, voice, and notifications (P3 discovery)

Validate desired phone/remote viewer path, voice input, and notification delivery. Distinguish bb features that ship from experiments, and do not count bb Tasks' proposed inbox as shipped parity.

### GF-019 — Browser agent capabilities (P3 discovery)

Determine whether Glitch Flow needs live embedded browsing, agent-driven desktop/headless browser sessions, annotations, and replay. Record platform support and ownership before choosing an implementation.

## Evidence still needed

- A headed UI pass against the current U: build. Source presence alone is not a behavior check.
- A configured two-device edge/auth run, since the Windows mock demo does not exercise remote linking.
- Feature-level provider/approval and navigation checks before turning partial/unverified rows into implementation claims.
- A project-source baseline before any backlog ticket edits application code.
