# Background processes

Agents start processes and move on: a dev server, a test watcher, a `tail -f`.
They keep running after the turn that started them, and until now the only
sign was a transcript chip saying "running in background". The composer now
carries a **Background** tray listing what the chat's agent still runs, for how
long, with each process's latest output and its own Stop.

Screenshots: [`docs/screenshots/background-processes/`](screenshots/background-processes/).

## Data

The runtime is the authority. Each driver publishes its agent's whole set on
`TurnControl` (`set_background_processes`), never a delta, and says where each
process's output can be read (`OutputSource`). The engine watches that set and
mirrors it on the chat's session row (`Session::background_tasks`), so every
device sees it, opened or not. The row's 15 s live heartbeat keeps it fresh for
as long as the runtime lives; `view::background_tasks` treats a row past
`SESSION_STALE_MS` as an engine that took its processes with it. When the run
ends, the list is cleared.

| Harness | Listed | Command | Output | Stop |
| --- | --- | --- | --- | --- |
| Claude | `background_tasks_changed` roster, `task_type: "local_bash"` (Bash `run_in_background`, a command moved to the background, `Monitor`) | the spawning tool_use's `input.command`, joined via `task_started.tool_use_id`; else the roster's description | the task output file the tool_result names ("Output is being written to: …"), or `<tasks dir>/<task id>.output` | `stop_task` control request (`perTaskStopAffordance` is already declared) |
| Codex | `commandExecution` items still open after their turn ended | `item.command`, without the login-shell wrapper (`/usr/bin/zsh -lc '…'`) | `item/commandExecution/outputDelta`, kept as the last 16 KB per open command (no longer opted out at `initialize`) | `thread/backgroundTerminals/terminate {threadId, processId}`, then the command's `item/completed` (or the existing prune) removes it |
| Mock | `ZERON_MOCK_BACKGROUND=1` | three scripted processes | each prints its script once, a line every 1.6 s | honored after 250 ms |

Other harnesses publish nothing, and the tray stays hidden for their chats.

Claude reports background agents in the same roster; they are subagents, shown
by their spawn chips, and stay out of this list (they still count toward
`TurnControl::background_live`, which keeps the idle reaper off). Wire shapes
were captured live from Claude Code 2.1.312; the roster arrives *before*
`task_started`, so a late start republishes the list with its command. Codex
commands of an interrupted turn are held back until the existing prune has told
the killed foreground command from the ones still running.

Output is plain text: `background::plain_text` drops terminal escapes,
collapses carriage-return rewrites (progress bars) to their final state, and
trims trailing blank lines. Files are read from their last 16 KB, starting at a
whole line.

### RPC

Both are relay-forwardable and addressed to the chat's host:

- `StopBackgroundTask {chatId, taskId}` → `{stopping}`. `false` when no live
  runtime lists the process (it ended meanwhile). A process leaves the list only
  when the agent reports it gone, never on the request alone.
- `ReadBackgroundTaskOutput {chatId, taskId}` → `BackgroundTaskOutput {text,
  truncated}`, or `null` when it is not listed.

Hosts and engines that answer them advertise `background-tasks-v1`. A viewport
offers Stop and output only when the chat's host *and* its own engine (which
forwards the request) do; otherwise the rows only say what runs. Older peers
never publish the field, so their chats show no tray at all.

## Behaviour

`crates/ui/src/background_tray.rs`, mounted by the composer at the top of its
tray stack: above the checklist and the queue, one step narrower than the tray
below it.

- **Collapsed** (the default): `Background · 3` and the commands, in the mono
  face, joined with `·`. It is ambient state, so it never opens by itself.
- **Expanded**: one row per process, oldest first: a still busy-tone dot, the
  command (mono, truncated), the agent's description (faint), how long it has
  run (`12m 34s`), and a Stop button.
- **Output**: clicking a row opens the tail of its output beneath it (up to 200
  lines, 8 visible, scrolling), following the end unless scrolled up. "Earlier
  output not shown" marks a cut; "No output yet" an empty one.
- **Stopping**: the row dims and reads `Stopping…` until the next session row
  drops it. A failed request shows "Couldn't stop that process" under the list.
- **State** (open flag, open row, stops in flight) is per chat, in memory for
  the app run, like the todo tray's.
- **Accessibility**: the header, rows and Stop are buttons with `aria_label`,
  tab stops and a focus ring; Stop has a tooltip.

## Performance

A collapsed tray costs no frames: there is no animation (the dot is still),
and its 1 s tick (clocks, plus re-reading the open output) runs only while it is
expanded and stops when it collapses, empties or the chat changes. Session-row
writes happen when the set changes, plus the heartbeat the live runtime already
sends.

## Testing

```sh
cargo test --locked -p zeron-harness background      # BackgroundWork, Claude roster joins
cargo test --locked -p zeron-harness --test claude background_shells
cargo test --locked -p zeron-harness --test codex background
cargo test --locked -p zeron-doc background_processes
cargo test --locked -p zeron-ui --lib background_tray -- --test-threads=1
```

Live: `ZERON_HARNESS=mock ZERON_MOCK_BACKGROUND=1` fills the tray.

## Not done

- Mobile (iOS/Android) tray.
- MCP `get_chat`/`list_chats` don't report background processes yet.
- Other harnesses (OpenCode, ACP agents, Cursor, Pi) don't report theirs.
