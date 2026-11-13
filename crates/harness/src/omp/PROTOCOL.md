omp RPC compatibility contract

The driver targets omp >= 18.7.0 and is verified against omp 18.20.0 (the
installed `omp/18.20.0`, live) and the oh-my-pi sources at 5de4b03 (package
version 18.20.0). It speaks omp's RPC mode, the surface omp documents for a
host that owns the process ("RPC mode is the lowest-common-denominator way to
embed omp"; ACP is for clients that already speak ACP). Frames are NDJSON,
never JSON-RPC: commands `{id, type, ...}`, responses `{id, type:"response",
command, success, data? | error, code?}`, events `{type, ...}`. stdout is the
protocol only; stderr is omp's log and becomes the crash message's tail.

Launch: `omp --mode rpc-ui --approval-mode <mode> [--session-dir D]
[--resume <file>]`. `rpc-ui` is the same protocol as `rpc` for a host that
answers dialogs: it connects tool UI (the ask tool, approvals) to the RPC UI
bridge and turns PTY bash off. Discovery uses `--mode rpc --no-session`.

Startup
- Wait for `ready` (`supportedProtocolVersions`, `maxReassembledFrameBytes`).
  omp writes nothing before it. With no usable model, or a session it cannot
  open, omp exits before `ready` and says why on stderr ("No default model
  selected…", `Session "…" not found`): the first is reported as not signed
  in, the rest as a crash with the stderr tail.
- `negotiate_protocol {protocolVersion: 2}` when offered (its response is
  still v1). Afterwards an oversized logical frame arrives as an unbroken
  `rpc_chunk` sequence (`chunkId`, `index` from 0, `count`, `byteLength`,
  base64 `data`). Reassembly is strict, as rpc.md specifies; a sequence that
  breaks a rule is dropped and logged, the stream goes on.
- Session settings, none persisted by omp (`applyRpcQueueModeCommand` passes
  `persist: false`): `set_event_filter {events:null, messageUpdates:"delta"}`
  (deltas instead of the full message on every token), `set_ask_dialog`,
  `set_steering_mode all` (pending steers land together),
  `set_interrupt_mode wait` (a steer waits for a running tool instead of
  interrupting it). A refusal of any of these is logged, not fatal.
- `set_host_tools` with Zeron's MCP tools (below), then the model and the
  thinking level, `get_available_commands`, and `get_state` (`sessionId`,
  `sessionFile`, `model`, `contextUsage`). `set_model` does not persist
  (`ModelControls.setModel` without `persist`); thinking is set after the
  model because `setModel` re-applies the model's default level.

Turns
- `prompt {message, images?, streamingBehavior?}` acknowledges admission, not
  completion. Every accepted prompt not answered `data.agentInvoked: false`
  gets exactly one `prompt_result {id, agentInvoked, status:
  completed|aborted|error, error?, sessionSettled}`, written after the
  response for the same id and after the terminal `agent_end` of the run it
  started or joined (rpc-prompt-results.ts). Each prompt id is a ticket; a
  turn ends when its last ticket is in.
- `agentInvoked: false` on the ack is a local slash command: done, with its
  `command_output` (which arrives before the ack) as the reply. omp runs its
  own slash commands from a prompt, so they are sent as typed.
- A refused prompt is a failed response for its id; one that fails after
  being accepted reports a `prompt_result` with `status: "error"`.
- At a turn's end the driver sends `get_state` before `Done`: a slash command
  may have switched sessions (no frame announces it), and `Done` carries the
  session id the engine resumes. Bounded at 15 s.
- A run omp starts itself (an async job's result waking it, a goal
  continuation) begins with `agent_start` and no ticket; it is a turn of its
  own and ends at the `agent_end` that yields (`yielded`, else `isTerminal`
  not false). `prompt_result.sessionSettled: false` / `hasPendingAsyncWork`
  report background work that may wake the session; `session_settled`
  reports it gone. The runtime is held while it is pending.

Events
- `message_update.assistantMessageEvent` `text_delta` / `thinking_delta`
  stream text and reasoning (delta projection: `delta` only); `message_end`
  carries the whole message and supplies what the deltas missed, its
  `usage` and `stopReason`. Tool calls are `tool_execution_start` (final
  args) and `tool_execution_end` (`result.content`, `isError`; `edit`'s
  `details` carry `path`/`oldText`/`newText`, or `perFileResults`). The
  `toolcall_*` deltas are not needed. Unknown event types and fields are
  ignored, as rpc.md requires.
- A user `message_start` (attribution `user`) confirms the oldest accepted
  steer (`Steered`); the opening prompt's own is not a delivery.
- `extension_error` and `notice {level:"error"}` are shown, not fatal.

Steering, stop, teardown
- A later prompt is `prompt` with `streamingBehavior: "steer"`: queued at the
  next step while a run streams, a run of its own when idle — one command,
  so a prompt cannot fall between the two. `queue_update` reports the queue.
- A stop is `abort_and_restore_queue` (withdraw queued input, then abort;
  plain `abort` would leave queued steers to run afterwards). The turn ends
  at the tickets' `aborted` results; if omp confirmed the abort and reports
  none within 15 s, the turn ends Interrupted anyway. The process and its
  background jobs live on.
- Teardown stops the turn, waits up to the interrupt grace for it to end,
  then ends the process group.

Dialogs (`extension_ui_request`)
- `select`, `confirm`, `input`, `editor` and `ask` (after `set_ask_dialog`)
  are answered once with `extension_ui_response` (`value`, `confirmed`,
  `answers`, or `cancelled` with `timedOut` when its `timeout` passed). A
  request omp withdraws (`method:"cancel"`, `targetId`) closes the question
  unanswered. Presentation methods need no answer; warning and error
  notifications are shown.
- Tool approvals (verified live with `--approval-mode always-ask`) arrive
  after `tool_execution_start` as a `select` titled `Allow tool: <name>`
  plus detail rows (`Command: …`), options `["Approve", "Approve <name> for
  this session", "Deny"]` (`approvalChoiceLabels`; `Approve Once` beside
  grants). Zeron asks yes/no like every driver; yes answers the approve-once
  label, anything else `Deny`.
- The ask tool uses `select`/`input` (its form dialog is TUI-only); its
  "Other (type your own)" choice is followed by an `input` with the same
  title.

Access modes: supervised = `always-ask` (reads run, writes and commands ask),
auto-accept edits = `write`, full access = `yolo` (`--approval-mode` wins over
config; per-tool `tools.approval` rules the user set still apply).

Host tools: omp has no per-run MCP flag (its servers come from config files).
Zeron's own MCP server and the plugins' are started by the run and offered as
`set_host_tools` definitions (`loadMode: "essential"`); `host_tool_call` runs
MCP `tools/call` and answers `host_tool_result` (`isError` rejects);
`host_tool_cancel` stops waiting.

Sessions: omp writes `<timestamp>_<sessionId>.jsonl` under a per-project
folder of `~/.omp/agent/sessions` (or `PI_CODING_AGENT_SESSION_DIR`, a named
profile's store, or `--session-dir`), headed by a `session` record, and only
once the session has a message. Resume finds the file by id (name and header)
and passes `--resume <file>`; a session with no file starts fresh with a
visible notice. Zeron never writes omp's files.

Sources: https://omp.sh/docs/rpc, /docs/modes, /docs/approvals,
/docs/sessions, /docs/env, /docs/mcp (2026-11-13); oh-my-pi
packages/coding-agent/src/modes/rpc/{rpc-mode,rpc-types,rpc-prompt-results,
rpc-session-settle,host-tools}.ts, tools/approval.ts,
extensibility/extensions/wrapper.ts, session/model-controls.ts, main.ts.
Version floor from the published packages (unpkg): `prompt_result` with
`sessionSettled` from 18.4, `set_ask_dialog` by 18.4.12,
`abort_and_restore_queue` from 18.7.0.
