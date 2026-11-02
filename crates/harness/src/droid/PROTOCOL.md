# Factory Droid native protocol

The native driver uses Droid's `stream-jsonrpc` interface without an SDK
runtime dependency.
Launch `droid exec --input-format stream-jsonrpc --output-format stream-jsonrpc`.
Both auto-update environment switches are disabled at spawn. Credentials are
owned by Droid; Zeron never invokes authentication RPCs or `droid update`.

## Contract

Frames are newline-delimited JSON. Every outgoing frame includes
`jsonrpc:"2.0"`, `factoryApiVersion:"1.0.0"` and `type` (`request`,
`notification`, or `response`). IDs are strings, including `"1"`. The shared
reader accepts incoming frames without `jsonrpc:"2.0"`.
Numeric IDs or missing envelope fields receive `-32700`, with
`id:null`. Plain Codex/ACP framing is unchanged.

`list_models {}` is sessionless. IDs are preserved exactly, including `auto`
and fast variants. Disabled rows are excluded; only minimal/low/medium/high/
xhigh/max become Zeron reasoning levels.
The normal catalog cache hashes the executable, Factory/Droid environment,
and only `.factory/{settings.json,settings.local.json,auth-deployment.json}`
under `FACTORY_HOME_OVERRIDE` or the home directory. Auth and organization
cache rewrites must not invalidate it.

Initialization and load must finish before a prompt is submitted. Always set
`autonomyLevel` (off/medium/high for Supervised/Auto-accept edits/Full access),
`interactionMode` (spec/auto), and `toolExecutionMode:"direct_only"`.
`auto_approve` means Full access for tool permissions, not plan approval.
Explicit models and efforts apply equally to the spec model.
Omitted/default selections use the effective normal model and
effort, then mirror those to spec settings before verification.

Settings can be coerced or organization-capped despite a successful RPC.
Check effective model, effort, spec model/effort, autonomy, mode and tool mode,
including `availableAutonomyLevels`, before prompting. Updates produce an
effective `settings_updated` with **`requestId` equal to the update RPC ID**
before the empty acknowledgement. This is the verification barrier, not an
uncorrelated earlier notification. The response's `availableModels` supplies
supported efforts for errors. Configured steers wait for turn completion.
Each later idle prompt reapplies the requested interaction mode.
Native settings updates apply immediately, so never send them during a running
turn.

Ordinary tool permissions use `proceed_once` and `cancel`. Every batched action
must fit the run's access mode; otherwise one shared approval question decides
the whole batch. A batch containing `exit_spec_mode` always asks for plan
approval, including with Full access or legacy `auto_approve`. Only after an
explicit Yes, reply with the offered `proceed_once` for Supervised (autonomy
Off), `proceed_auto_run_medium` for Auto-accept edits, or `proceed_auto_run_high`
for Full access. `proceed_once` exits spec mode with autonomy Off;
level-specific outcomes preserve it during implementation.
If the matching outcome is not offered, cancel visibly rather than silently
downgrade or escalate. No, missing answers, and stop still send `cancel`.
Unknown client methods, including `pick_options`, receive `-32601`.
Questions are cancellable; stopping closes each pending input with empty
answers. AskUser multi-select answers use one string of comma-separated labels.

`tool_call` is an argument upsert, not a new invocation. Emit one tool chip at
the first queued phase, permission request, or result. Results are capped to
16 KiB; inline edit diffs to 64 KiB. Native `ExitSpecMode` calls wait for
permission details before emission, so `details.plan` wins over preliminary
arguments. Only the proposal's title/body survive input sanitization, at most
64 KiB serialized; malformed/oversized proposals cancel visibly, not truncate.
The desktop renders this content as Markdown outside compact work folds;
the small approval question remains separate from the tool-access policy.
MCP names use `server___tool`.
Initialization and load attach MCP through `mcpServers`. Image attachments use
`imagePaths`.
Final `create_message` text is never replayed. Steering is receipted only by
the matching user `create_message.requestId`; discarded input is not receipted.
Text retractions leave already streamed text intact and emit a visible warning.

Only `agent_turn_completed.reason` determines terminal status: completed,
cancelled, authentication failure, or an error with the last error message.
EOF during a turn is an error, not success. A stop interrupts the turn but
keeps the process reusable; teardown closes stdin and reaps the owned process
group, escalating against stubborn children. The prompt end is authoritative.
`child_session_available` and `child_session_settled` update background task
counts and keep runtimes alive while children run. The parent RPC stream does
not forward child messages. Correlate `childSessionId` with `toolUseId` and
read the child's native `sessions/<cwd>/<id>.jsonl` under
`FACTORY_HOME_OVERRIDE/.factory` or `~/.factory`, using the shared incremental
transcript reader. Verify the header's child, parent, and tool IDs before
rendering; omit context injections and encrypted thinking. Updates arrive at
message/tool boundaries, not token granularity.
Child permissions stay on the parent's approval path but their
tool chips belong to the child transcript. Settlement is authoritative by child
session ID, independent of optional tool IDs. Drain until the transcript has
been quiet for two seconds, including partial writes, with a ten-second maximum,
then emit a tagged terminal event. Native background counts own runtime liveness;
only bounded drains add an exit barrier, and teardown closes orphan histories.
Missing or changed private-format histories degrade to chip-only.
Later parent work is a new parent turn.
The driver reports `deterministic_turn_end = false`.

`model_authentication_failed` maps to `Done(Errored)` with terminal `/login`
guidance, not the engine Cloud-path `HarnessError::NotSignedIn`.

Load reattaches MCP and ignores `session.messages`. `-32004` starts a fresh
session with a visible “without the previous context” notice. No other load
error silently discards context.
