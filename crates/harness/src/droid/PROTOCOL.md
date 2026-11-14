# Factory Droid native protocol

Observed on 2026-11-13 with `droid 1.10.0` (title sessions on 2026-11-14)
against the `@factory/droid-sdk 0.11.0` declarations.
`tests/fixtures/droid_rpc.rs` encodes these frame shapes.

## Transport

`droid exec --input-format stream-jsonrpc --output-format stream-jsonrpc` in
the project's cwd: newline-delimited JSON-RPC 2.0 with string ids,
`type: request|response|notification`, and `factoryApiVersion: "1.0.0"` on
every outgoing frame (the SDK's legacy API version, not the CLI's
`factoryProtocolVersion`). `FACTORY_DROID_AUTO_UPDATE_ENABLED=false` keeps
the CLI from updating itself under a session.

## Sessions and settings

`droid.list_models {}` needs no session. `droid.initialize_session
{machineId: "default", cwd, toolExecutionMode: "direct_only"}` returns
`sessionId` and the effective `settings`. `droid.load_session {sessionId,
toolExecutionMode}` restores cwd and settings and returns no `sessionId`: keep
the saved id and check the returned cwd. A nonexistent id returns exactly
`{"code":-32004,"message":"Session not found"}`, the only load failure that
starts a fresh session.

`direct_only` exposes tools directly instead of only through Script. It is a
startup setting: `update_session_settings` silently ignores it, so it is
checked when the session opens.

`droid.update_session_settings` accepts `modelId`, `reasoningEffort`,
`specModeModelId`, `specModeReasoningEffort`, `interactionMode` and
`autonomyLevel`. Its `{}` response is only an ACK, and several uncorrelated,
sometimes partial `settings_updated` notifications precede the final
`settings_updated {requestId, settings}`. Only that one is authoritative, and
every requested value must match it before a prompt is sent.

Plan is `spec`, otherwise `auto`. Supervised is `off`, Auto-accept edits
`medium`, Full access (and legacy auto_approve) `high`. Medium is broader than
Zeron's label: Droid also runs medium-risk reversible local commands
unprompted. The `auto` model requires effort `none`.

## Turns

`droid.add_user_message {text, imagePaths?}` ACKs with `{}` before the turn
runs. Events arrive as `droid.session_notification {sessionId?,
notification}`:

- `assistant_text_delta` and `thinking_text_delta {textDelta}`.
- `tool_call {toolUse: {id, name, input}}`, first with empty input and again
  once it is known; `tool_result {toolUseId, content, isError}`.
- `session_token_usage_changed {tokenUsage}`, cumulative for the session.
- `create_message {requestId, message: {id, role: "user"}}`: the receipt for
  a consumed message. The turn's first message id is its `turnId`; a message
  sent mid-turn is consumed at a step boundary, has its own id, and completes
  under the original turn.
- `queued_messages_discarded`: messages that were not delivered, named by
  `drainedRequestIds`, else `messages[].requestId`, else `requestId`.
- `agent_turn_completed {turnId, reason}`: the only end of a turn
  (`completed`, `cancelled`, or an error). Neither the ACK nor
  `droid_working_state_changed` is one.

## Server requests

`droid.request_permission {toolUses, options}`: each `toolUses` element holds
`toolUse`, `confirmationType` and `details` (a Spec plan is in `details`).
ExitSpecMode offers `proceed_once`, `proceed_auto_run_low|medium|high`,
new-session variants and `cancel`. Reply `{selectedOption}` with an offered
option; `cancel` ends the turn as cancelled.

`droid.ask_user {toolCallId, questions: [{index, topic, question, options,
multiSelect?}]}` is answered `{answers: [{index, question, answer}]}`, with
multi-select labels joined by commas, or `{cancelled: true, answers: []}`,
which also cancels the turn.

## Subagents

A Task call's result acknowledges launch, not completion. The parent session
then reports `child_session_available {childSessionId, toolUseId,
description}` and `child_session_settled {childSessionId, status}`; either
may repeat, and a status other than `completed` or `cancelled` is a failure.
A child may outlive the parent's turn.

No child event stream arrives over exec. Droid writes each child's history to
`<home>/.factory/sessions/<cwd encoding>/<child UUID>.jsonl`, `<home>` being
`FACTORY_HOME_OVERRIDE` or the user's home, and the cwd possibly a child
worktree's. Its first record, `session_start {id, callingSessionId,
callingToolUseId}`, binds it to the parent and Task call; `message` records
`{id, message: {role, content}}` follow and can repeat. The file can gain its
last lines after settlement.

`tool_progress_update {toolName: "TaskOutput", update: {type,
subagentSessionId, ...}}` arrives only while the parent reads the child's
output: cumulative `message` snapshots, and `tool_call`/`tool_result` updates
with no tool-use id. It is the fallback transcript when no history is found.

## Title sessions

A title is a fresh session initialized with:

```json
{
  "systemPrompt": "<TITLE_INSTRUCTIONS>",
  "disableBuiltinSkills": true,
  "autoRejectPermissionRequests": true,
  "privacyLevel": "private",
  "restrictToolIds": ["__no-tools__"],
  "autonomyLevel": "off",
  "interactionMode": "auto",
  "blockOnStartupLoad": true,
  "blockOnMcpLoad": true
}
```

A string `systemPrompt` replaces the coding prompt (the `{append}` preset form
extends it) and is echoed in `settings`. `__no-tools__` is the CLI's own
empty-intersection selector, not a documented guarantee, and initialization
does not report the resulting tool set: after the settings barrier,
`droid.list_tools {}` must list every tool with `currentlyAllowed: false`.
Empty `restrictToolIds`, `enabledToolIds` or `disabledToolIds`, and
`disabledToolIds: ["*"]`, each left tools allowed, and globally configured MCP
servers still load, so that check is the isolation boundary.

`privacyLevel: "private"` is accepted but not echoed. Title sessions persist
under `~/.factory/sessions` and may sync to Factory history.
