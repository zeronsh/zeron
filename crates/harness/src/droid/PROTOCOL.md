# Factory Droid native protocol

Verified on 2026-11-13 with `droid 1.10.0`, against the
`@factory/droid-sdk 0.11.0` declarations. The synthetic peer at
`tests/fixtures/droid_rpc.rs` encodes the verified frame shapes.

## Transport and process

`droid exec --input-format stream-jsonrpc --output-format stream-jsonrpc`,
with the project's cwd. Newline-delimited JSON-RPC 2.0, string ids, and
`type: request|response|notification`. Every outgoing frame includes
`factoryApiVersion: "1.0.0"` (the SDK's **legacy** API version, not its protocol
version). The CLI reports `factoryProtocolVersion: "1.332.0"`; this optional
field is not needed by the client.

Zeron owns the process tree, captures bounded stderr, disables self-update
with `DROID_DISABLE_AUTO_UPDATE=true` and
`FACTORY_DROID_AUTO_UPDATE_ENABLED=false`, and retains its execution lease
until shutdown has reaped the tree. `DROID_EXECUTABLE` overrides discovery.

## Sessions and settings

Sessionless `droid.list_models {}` works. `droid.initialize_session` accepts
`machineId: "default"`, `cwd`, and `toolExecutionMode: "direct_only"`.
Initialization returns `sessionId` and effective `settings`.
`direct_only` exposes tools directly instead of only via Script. This is a
startup setting: **update_session_settings silently ignores
toolExecutionMode**, and its correlated notification omits that field.
Zeron therefore checks it on initialize/load, not at the update barrier.
`load_session` accepts `toolExecutionMode: "direct_only"`, restores cwd and
settings, and returns **no sessionId**. Keep the requested saved id and check
the returned cwd before prompting. A fresh CLI process successfully loaded a
session created by a previous process with the same cwd and direct-tool mode.

`droid.update_session_settings` accepts `modelId`, `reasoningEffort`,
`specModeModelId`, `specModeReasoningEffort`, `interactionMode` and
`autonomyLevel`. Its `{}` response is only an ACK. It emits several
uncorrelated, sometimes partial `settings_updated` notifications before the
final `settings_updated {requestId, settings}`. Only the matching request id
is authoritative; every requested value must match before a prompt is sent.

Mapping: Plan -> `spec`, otherwise `auto`; Supervised -> `off`, Auto-accept
edits -> `medium`, Full access (including legacy auto_approve) -> `high`.
Medium is broader than Zeron's edit-only label: Droid also auto-runs
medium-risk reversible local commands. This mapping is intentional.
Auto model requires `none` for both reasoning efforts.

`droid.load_session {sessionId}` with a nonexistent UUID returns exactly
`{"code":-32004,"message":"Session not found"}`. Other load failures must not
discard context.

## Streaming

`droid.add_user_message {text, imagePaths?}` ACKs with `{}`, before completion.
`droid.session_notification` carries `params.notification`; sessionId is
sometimes absent on parent events. Text and thinking notifications use
**textDelta**, not the high-level SDK stream event's `text` field.
`tool_call {toolUse:{type:"tool_use",id,name,input}}` arrives first with empty
input, then again with populated input. Upsert by id.
`tool_result {toolUseId,content,isError}` settles that tool.
`session_token_usage_changed.tokenUsage` carries cumulative session usage.

Only `agent_turn_completed {turnId,reason}` ends a prompted turn.
Observed reasons: `completed`, `cancelled`; SDK also defines errors.
Neither ACK nor `droid_working_state_changed` is completion.
`create_message {requestId,message:{id,role:"user",...}}` is the prompt
consumption receipt. The first message id is the terminal turnId. A steered
message has its own id but completes under the original turnId. Live
mid-turn injection was consumed, so steering is `StepBoundary`. Zeron emits
`Steered` only on the matching requestId receipt, never on the ACK.
`queued_messages_discarded` reports undelivered messages, not successful
steering. Their request ids are read as the SDK reads them:
`drainedRequestIds` when present, else each `messages[].requestId`, else the
notification's own `requestId`.
Each consumed follow-up, including idle prompts and steers without a caller
message id, rotates the assistant segment id through `Steered`. The initial
prompt uses `SessionStarted` instead.
EOF during active work is an error.

## Models and child sessions

Model discovery uses the shared coalescing catalog cache. Native ids and
supported effort ladders are retained; disabled rows and none/off efforts
are not selectable. The fallback is the provider default. Context hashes
include executable identity, the FACTORY_/DROID_ environment, and
`<home>/.factory/settings.json` and `settings.local.json`, where `<home>` is
FACTORY_HOME_OVERRIDE or the user's home.

Live Task probing delivered these parent-session notifications:

```json
{"type":"child_session_available","childSessionId":"<child UUID>","toolUseId":"<Task id>","subagentType":"worker","description":"Read probe text","timestamp":1}
{"type":"tool_progress_update","toolUseId":"<TaskOutput id>","toolName":"TaskOutput","update":{"type":"tool_call","toolName":"Read","status":"running","parameters":{"file_path":"probe.txt"},"subagentSessionId":"<child UUID>","timestamp":2}}
{"type":"tool_progress_update","toolUseId":"<TaskOutput id>","toolName":"TaskOutput","update":{"type":"tool_result","toolName":"Read","status":"completed","parameters":{"file_path":"probe.txt"},"details":"summary","text":"summary","fullOutput":"whole output","subagentSessionId":"<child UUID>","timestamp":3}}
{"type":"child_session_settled","childSessionId":"<child UUID>","toolUseId":"<Task id>","status":"completed","timestamp":4}
```

The original childSessionId/toolUseId binding owns the spawn chip; repeated
availability and settlement are idempotent. The Task result acknowledges
launch, not child completion. Settlement is keyed by child id, even when
toolUseId is absent. Status `completed` succeeds, `cancelled` interrupts,
and other terminal statuses fail. Children may outlive the parent turn.

TaskOutput also forwards `status` and cumulative `message` snapshots. Its
tool updates have **no child tool-use id**, and arrive only while output is
being read. No complete child notification stream was observed over exec.
For a full live transcript, Zeron uses the same incremental JSONL reader as
Claude's typed forks. Droid writes
`<home>/.factory/sessions/<cwd encoding>/<child UUID>.jsonl`, where `<home>`
is FACTORY_HOME_OVERRIDE or the user's home. The cwd directory may differ
from the parent's (a child worktree), so only one directory level is searched.
Only UUID child ids become filenames.

The first `session_start` must match `id`, `callingSessionId`, and
`callingToolUseId` before any content is read. Message records have outer
`type:"message"`, `id`, and `message:{role,content}`. Assistant `text`,
plaintext `thinking`, and `tool_use` blocks plus user `tool_result` blocks
normalize through the ordinary Droid tool mapper. Message ids deduplicate
replays. User text/context injections and encrypted thinking are omitted;
the original Task prompt supplies the child tab's user entry.

Histories are polled every 250 ms: updates are live at message/tool
boundaries, **not token-level**. Settlement drains late appends until 500 ms
of byte-level quiet (including partial lines), bounded by five seconds.
Background counts hold the runtime through this drain, independently of
parent turn completion and settings barriers. Teardown settles remaining
children as interrupted or failed.

If a history is unavailable when TaskOutput progress arrives, that child
uses wire progress instead: message snapshots become deltas; tool calls get
local ids matched to results by name/parameters. It never switches back and
replays the same content. This fallback is partial, not a full transcript.
Unknown child bindings never enter the parent transcript.

## Server requests

`droid.request_permission` has `toolUses` and `options` arrays.
Each toolUses element wraps `toolUse`, `confirmationType`, and **details**.
Spec plan details are on the element, not the top-level params.
ExitSpecMode offers `proceed_once`, `proceed_auto_run_low`,
`proceed_auto_run_medium`, `proceed_auto_run_high`, new-session variants,
and `cancel`. Reply `{"selectedOption":"..."}` using only an offered option.
Rejection produces a cancelled turn, not a successful turn.

`droid.ask_user` carries `toolCallId` and questions with `index` (observed 1),
`topic`, `question`, `options`, and optional `multiSelect`. Cancellation is
`{"cancelled":true,"answers":[]}` and also ends the turn as cancelled.
Answers are `{answers:[{index,question,answer}]}`; multiselect labels join
with commas. Unknown server methods receive JSON-RPC -32601.

## Boundaries and non-goals

No ACP, in-app login, account switching, usage meters, or cloud registration.
Only bound child histories are read; parent history is never replayed.
Authentication errors advise `droid` then `/login`, or `FACTORY_API_KEY`.
Autonomous Loop/cron quiescence is unproven:
`deterministic_turn_end` remains false.
