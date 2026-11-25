# Delegating in the background

A chat hands work to child chats, on any provider and any of the person's devices, and ends its turn. Zeron's engine on
the parent's device watches the children, the parent reads **Waiting**, and it is woken once with what settled. Nothing
in it needs a live runtime or a blocking tool call, so it works the same for every harness.

Code: `crates/proto/src/delegation.rs` (types, the wake message), `crates/engine/src/delegation.rs` (store and
supervisor), `crates/engine/src/sessions.rs` (the holdback), `crates/mcp/src/tools/delegation.rs` (tools).

## Who has it

Settings → General → **Delegate in the background**, per device (`async-delegation.json` in the data dir,
`GetAsyncDelegation` / `SetAsyncDelegation`).

- An installation from before it: off until the person turns it on.
- A new installation: on. If it joins an account with other computers, it follows them: on only where one of them has it
  on (devices advertise `async-delegation-v1`), since a new computer isn't a new user.
- The person's choice always wins.

A run is told at launch (`ZERON_ASYNC_DELEGATION`: `parent`, or `child` for a delegated chat); a parked session whose
setting changed is replaced before its next turn. Off, the MCP tools and instructions are what they were.

## Tasks

`create_chat` with a prompt (kind subagent), or `send_message` to a subagent child, registers a `DelegatedTask` on the
parent's engine **before** the message goes out, with the child's row as it stands then as the baseline. Tasks live in
`delegated-tasks.json` in the profile's store and survive restarts. A cohort is what one parent turn started
(`parentTurnId`, that turn's opening message).

A task settles on the first end of the child that moves past its baseline: completed, failed or stopped, or the child
archived or deleted. The child is watched by its synced session row, local or remote alike. A row nobody refreshes says
nothing: a child whose device sleeps is still running (`unreachable` in `task_status`), never failed. A child whose own
turn ended into a wait isn't done. The reply is read from the child's transcript, which may sync after its row (retried
with backoff, two minutes for another device); unread in time, the wake says so and points at `read_chat`.

A later message to the same child supersedes its open task.

A turn cut off by its own engine stopping isn't over: its row's outcome says `resumes`, and the next start picks it up.

With no project, device or folder named, a child works in its parent's folder. The message a parent sends a child tells
it that its final reply goes back by itself; messaging the parent to report is refused unless the child says it must
(`peer: true`).

## Waiting, and the end held back

`Inner::set_status_with_completion` is where every turn's end is published. While the chat has a task whose result it
hasn't been told, an end there keeps its `last_completed_turn` and outcome back, and the row reads `waiting` with the
runtime's own waits and the open tasks together (`WaitingKind::SubagentChat`, with the child, device and harness). A
turn starting drops the held end (it marks its own); nothing left to wait for releases it. So "finished" (the banner,
the chime, a push, `wait_for_turn`) is said once.

A turn whose only open tool calls block on another chat (`wait_for_turn`, or `mode: "wait"`) sets
`Session::blocked_on_wait`: it reads Waiting too, with `status` still `Working`.

## The wake

When the parent has no turn in flight, everything settled within 800 ms of the last goes in one message
(`wake: "all"`: once the whole cohort has). It is marked `Delivered` before it's sent and `Acknowledged` once its turn
starts; a stop between the two finds it missing from the transcript on boot and sends it again. It isn't a queue row, so
a paused queue never holds it. Transcripts show it as a line and a row a child, each with a line of its reply
(`delegation::wake_excerpts`); the replies are quoted so nothing in them can end their block or pass for another chat's
message.

A result a tool call already returned (`mode: "wait"`, `wait_for_turn`) is acknowledged and not sent again. A blocking
call that times out goes on in the background.

## In the transcript

The call that hands work over is a line on the transcript's activity tree and a row a chat (`ds/delegation`): how each
stands now (it follows the chat), its title, agent and device; a click opens it. Which chat each request became is said by Zeron's MCP server as the call goes
(`NoteHanded` → `AgentEvent::Handed`), since harnesses drop MCP results before the engine sees them. The transcript keeps
titles, agents, recipients and chat ids of these calls, never the prompt or message (`delegation::badge`).

A parent waiting is under way: it stays on the sidebar's Working shelf, its row as it was, with Waiting's glyph. What it
waits on, and for how long, is on its hover card.

## Questions and approvals

A child's question wakes a waiting parent after 5 s, with its request id for `respond_to_input`; answered or dismissed
first, the wake is dropped. An approval is never an agent's to give here. Both show on the child's row in the
parent's Subagents tray (an approval with Allow once and Deny, a short question with its options to press), whose rows come first and whose tab and pill wear the ask, and on the parent's own
row in the sidebar. Nothing opens itself.

## Stop

Stop on a parent (working or waiting), or archiving it, interrupts its open children wherever they run, disposes their
results, and closes the cohort: anything that turn still tries to start is refused. A waiting parent stopped this way
reads stopped, not finished. Each child also has its own Stop in the Subagents tray.

## Reaching other chats

With it on, an agent acts on its own children. `send_message`, `interrupt_chat`,
`respond_to_input` and `archive_chat` on any other chat need `peer: true`, for when the person asked, it's answering a
message that chat sent, or there is no other way. Standalone chats (`kind: "chat"`, on any device) are still there for
the same cases. Reading chats is open.

## Access

A child asked for no access mode gets the broadest of its harness's modes no wider than its parent's. A mode asked for
and not supported is refused.
