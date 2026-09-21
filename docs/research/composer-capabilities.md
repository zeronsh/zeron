# Composer capability mapping

Audited 2026-09-19. These are integration contracts, not promises that every
installed agent version or account offers the same modes. The UI requires the
host's `agent-modes-v1` capability and the selected model's adapter-advertised
options. Some catalogs are live and some are versioned static contracts, as
called out below. An advertised native mode keeps its exact ID and label; no
prompt simulates a mode.

| Harness | Plan / native modes | Persistent goal | User input above composer |
| --- | --- | --- | --- |
| Codex | Default requires `collaborationMode/list`; Plan also requires its advertised preset; Goal requires enabled `goals` | Only when `experimentalFeature/list` advertises enabled `goals`; `thread/goal/get,set,clear` plus update/clear notifications | `item/tool/requestUserInput` (blocking or asynchronous), plus completed assistant-message questions |
| Claude Code | `--permission-mode plan`; explicit approval of `ExitPlanMode` | Not exposed | `AskUserQuestion` control request and plan approval |
| Cursor | Pinned `@cursor/sdk@1.0.31`: `mode` on create and send, including resume | Not exposed | Not exposed: pinned public SDK has no answer channel; `askQuestion` remains disabled |
| OpenCode | V1 `/agent` must advertise visible primary `build` and `plan` agents; V2 plan is not exposed by this integration | Not exposed | `question.asked` with reply/reject; explicit permissions while planning |
| Devin | ACP live `configOptions` or legacy `modes` only | No dedicated goal lifecycle API; no fabricated Goal option | ACP question choices and native mode permissions |
| Grok | ACP live `configOptions` or legacy `modes` only | Same ACP restriction | Same ACP input bridge |
| Hermes | ACP live `configOptions` or legacy `modes` only | Same ACP restriction | Same ACP input bridge |
| Pi | `pi-acp` live `configOptions` or legacy `modes` only | Same ACP restriction | Same ACP input bridge |
| Antigravity | ACP live modes only; pinned fixture advertises Default/Auto Edit/YOLO, **not Plan** | Same ACP restriction | Same ACP input bridge |
| Mock | Test-only harness; no production mode selector | Test events only | Test callback |

The first nine rows are production harnesses. Mock exists only for fixtures and
is included because the Rust contract is exhaustive over `HarnessId`.

ACP mode IDs are opaque: `architect` is not rewritten as `plan`. Semantic mode
configurations use a shared UI key but are sent through their original config ID.
Grouped select choices are supported. Legacy modes use `session/set_mode`.
When legacy modes exactly duplicate an advertised `thought_level` option (as in
`pi-acp@0.0.33`), they remain reasoning choices and are not promoted to agent
modes. Independent opaque mode IDs remain intact.
Missing or rejected explicit modes stop before prompting; native defaults are
preserved instead of silently selecting a permissive mode. For agents advertising
modes, permission requests go through the question tray. `switch_mode` requests
always require an answer, even when every option has a standard allow/reject kind.
For agents without modes, existing unattended tool permissions remain unchanged;
question-shaped choices still use the tray.

If an explicit ACP mode's live session state changes, the runtime retires at the
turn boundary. Undelivered follow-ups resume through setup, which validates and
reapplies the exact mode against the current catalog before prompting. Mode-stable
sessions can continue using the warm runtime.

Codex fallback model lists intentionally contain no mode claims. Runtime discovery
must establish Default and Plan support; Goal is offered only when the separate
experimental feature catalog enables it. Selecting Goal is a one-shot creation intent, not a saved model preference.
It is consumed after the Run command is accepted and is never persisted into a
new chat's configuration or model defaults. Ordinary messages and switching
Build/Plan do not pause or resume a goal. Existing goals have explicit native
Pause/Resume, Edit and Delete controls; both the local and owning engine need
`goal-actions-v1`. Editing keeps the ordinary message draft intact. New goal
creation while a message would be queued is rejected without losing the draft;
queued messages do not yet carry per-message goal creation intent.
The adapter consumes native goal update and clear events,
but does not fabricate a goal when the feature is absent. Claude and Cursor add
their versioned native mode options to their adapter model catalogs rather than
discovering them per session. Claude's installed CLI supports the native plan
permission mode. Cursor's pinned published type definitions include
`AgentModeOption`, but no public question response operation. OpenCode exposes
Build/Plan only when the live V1 `/agent` catalog contains both visible primary
agents. OpenCode V2 plan is an integration gap, not a claim that the product
itself cannot plan.

Native plans/checklists, Codex goals, and questions use the existing durable
transcript events and composer tray. Cursor `createPlan` and Claude plan-exit
content map to the same plan representation. Native mode choices apply to the
next message; they are not immediate commands to interrupt or change a running
turn. A plain-text question from an agent is not treated as a structured request.

## Evidence and verification

Read-only local version/help/catalog probes produced this inventory. Missing
means no executable was found in the integration's production search paths; it
does not say whether the user has an account or could install the adapter.

| Harness | Local production transport observed | Version / source boundary |
| --- | --- | --- |
| Codex | `codex` installed | `codex-cli 0.154.0`; generated app-server protocol plus read-only `collaborationMode/list` and `experimentalFeature/list` |
| Claude Code | `claude` installed | `2.1.273 (Claude Code)`; `--help` advertises `--permission-mode plan` |
| Cursor | `cursor-agent`/`cursor` absent; managed SDK package not installed | Integration pins `@cursor/sdk@1.0.31`; published typings were inspected during implementation |
| OpenCode | `opencode` installed | `1.18.31`; `agent list` advertised visible primary `build` and `plan` agents |
| Devin | `devin` installed | `3000.10.31`; ACP help inspected; live ACP session discovery remains authoritative |
| Grok | `grok` absent | Integration-managed package pin is `@xai-official/grok@1.0.4` |
| Hermes | `hermes` installed | `0.21.3`; `hermes acp --version` also reported `0.21.3` |
| Pi | `pi` and managed `pi-acp` installed | Pi `0.85.1`; integration adapter `pi-acp@0.0.33` |
| Antigravity | `agy_acp_server` absent | Integration archive pin is `1.1.1`, downloaded only on first use |

The managed `claude-agent-acp@0.66.0` and `codex-acp@1.1.14` packages also
exist locally, but they are not the production Claude/Codex transports in this
contract; those harnesses use their native integrations. The inventory probes
made no hosted model turns. No sign-in or credential-content read was performed.

A separate, bounded live Codex 0.154 app-server check used a read-only temporary
workspace and the installed default model (`gpt-6-astra`). Paused goal creation,
editing and Resume produced an automatic turn, the requested short reply and a
native `complete` goal update. A smaller-budget check reached `budgetLimited`;
the native budget includes input context, not only generated text. Synthetic
goals were cleared and their threads archived. These are direct protocol checks,
not end-to-end UI checks or evidence for other providers' live accounts.

- Published Cursor SDK 1.0.31 package: `options.d.ts`, `agent.d.ts`, and
  `createPlan` tool delta types. [Official SDK documentation](https://cursor.com/docs/sdk/typescript).
- ACP [session modes](https://agentclientprotocol.com/protocol/v1/session-modes)
  and [configuration options](https://agentclientprotocol.com/protocol/v1/session-config-options).
- OpenCode [agents](https://opencode.ai/docs/agents/), plus the integration's
  V1/V2 HTTP/SSE fixtures.
- Regression coverage: native mode discovery, disabled/missing Codex goal support,
  opaque/grouped ACP mode IDs, stale intent rejection, plan-exit question round
  trip, Cursor plan/create and build/resume, external native-input resolution,
  request preflight, provider-scoped resume, and existing harness input tests.

Fixture coverage verifies the adapter contracts; it does not establish live
end-to-end behavior for every installed provider version, account, or optional
ACP extension. Missing executables were not installed to expand the matrix.

## Plan and task projection

Plan prose and task snapshots are independent. Failed or still-unconfirmed todo
writes do not replace the last successful snapshot. An authoritative empty list
clears the list. Native in-progress, blocked and cancelled states survive document
storage; old `done`-only documents remain readable. Cancellation never counts as
successful completion.

- Codex: streamed/completed plan items, `turn/plan/updated`, and legacy todo items.
- Claude: `TodoWrite`, plan-exit prose when supplied, and successful `TaskCreate`,
  `TaskUpdate`, `TaskList` results. Task-list JSON and the CLI's numbered text form
  are accepted; unrecognized results preserve the prior snapshot. Successful
  create/update/delete operations retain native task IDs as durable patches, so
  restarting or resuming the Claude process does not lose existing task state.
- Cursor: `createPlan` and `updateTodos`, including camel-case `inProgress`.
- OpenCode: `todowrite` and session-scoped `todo.updated`, including empty lists.
- ACP transport can normalize native `plan.entries` snapshots, but that is not
  proof that every adapter emits them. Installed Hermes maps its todo results to
  those entries; installed `pi-acp@0.0.33` does not emit them. Devin, Grok and
  Antigravity task emission remains unverified. No checklist is inferred from
  arbitrary assistant text or slash-command names.

The tray starts with a compact summary. Details expand without replacing the
composer; goal status, plan prose, and task states remain independently visible.
The expansion retargets from its current height and respects reduced motion.
Questions and queues reduce the context budget. Switching conversations resets
expansion and scroll state, and collapsed streaming plan text is not repeatedly
parsed. Automated checks cover state projection, storage and animation math;
live appearance and frame timing still require a focused preview.

## Question contracts and mode selection

Mode choices appear in the existing model-options picker. Only choices advertised
by the selected model are offered; remote hosts also need `agent-modes-v1`.
Opaque ACP IDs survive unchanged. Selecting a choice changes the selected mode
from the next message onward. Ordinary mode selections persist; Goal creation is
the one-shot exception described above.

Existing-chat selections and new-chat defaults use the same effective option
filter: a loaded model catalog removes retired choices from the outgoing request,
and a host without `agent-modes-v1` cannot receive a saved mode. Persisted choices
remain available if the catalog later offers them again. Outgoing requests use
the same capability gate.

Question constraints travel with each request, rather than being guessed from a
harness badge. The engine validates IDs, cardinality and allowed labels before
resolving a pending request. The question header exposes explicit cancellation,
which sends an empty answer list through the same durable outcome watch. A
rejected cancellation restores the typed answer and choices. Invalid responses leave it pending; an empty response
is cancellation. Impossible choice-only requests with no options cancel immediately.

| Integration | Options | Custom text | Multiple choices | Descriptions | Non-blocking |
| --- | --- | --- | --- | --- | --- |
| Claude AskUserQuestion | Yes | Yes | `multiSelect` | Preserved | No |
| Claude plan exit | Implement / keep planning | Feedback keeps planning | No | N/A | No |
| Codex requestUserInput | Yes | `isOther`, or text-only question; legacy missing flag remains permissive | Legacy `multiSelect` only | Preserved | `isBlocking: false` |
| Codex assistant questions | Yes | Yes, sent as a follow-up/steer | No | Wire carries labels only | Yes |
| Codex approvals | Yes / No | No | No | N/A | No |
| OpenCode questions | Yes | `custom` (defaults true) | `multiple` | Preserved | No |
| OpenCode permissions | Yes / No | No | No | N/A | No |
| Devin / Grok / Hermes / Pi / Antigravity | ACP permission options | No generic free-text reply in the integrated ACP transport | No | Wire carries option names | No |
| Cursor | No public answer channel in pinned SDK | Unsupported | Unsupported | Unsupported | Unsupported |

Codex `isSecret` requests are explicitly rejected with a protocol error; the
composer does not claim to be a credential input. ACP extension-specific custom
answers are not inferred or fabricated. [Codex app-server protocol](https://developers.openai.com/ja-JP/docs/app-server),
[OpenCode question schema](https://github.com/anomalyco/opencode/blob/dev/packages/schema/src/v1/question.ts),
and [ACP permission schema](https://docs.rs/agent-client-protocol-schema/latest/src/agent_client_protocol_schema/v2/client.rs.html)
are the source contracts; the installed generated Codex types were also inspected.

Question pages retain their typed answers when navigating back. A choice-only page
makes the text editor read-only and explains that an option is required. Questions
borrow and restore the current draft, including selection, rather than silently
discarding it. Empty pages cannot advance. Option descriptions wrap below labels.

Submitting an answer retains its complete draft until the durable command reaches
an outcome. Pending delivery has no arbitrary timeout. Rejected, expired,
superseded or cancelled answers restore the same question pages and typed answers,
including after navigation or displacement by a blocking question. Applied answers
stay hidden until the resolved transcript update arrives. Reconnecting watches
reuse the command ID and back off between attempts.

Claude's `control_cancel_request` retires the waiter for its exact native
request ID. CLI teardown retires all remaining plan-approval and question
waiters. Codex assistant-message question waiters likewise end with their run.

Codex can withdraw a native server request independently of the tray. The
app-server contract emits `serverRequest/resolved` after a client response or
when turn start/completion/interruption clears a pending request. The adapter
then aborts only the matching waiter; teardown aborts all remaining waiters. The
engine observes the closed response receiver, durably resolves that question,
and preserves any unrelated blocking request. It sends no invented answer and a
late tray response cannot fall through to a different request. This lifecycle is
dynamic and therefore is not represented by the static `QuestionTransport` enum.

OpenCode's event bus can likewise report that another native client settled an
input. Pending waiters are keyed by request kind, session ID and request ID.
`question.replied`, `question.rejected` and `permission.replied` abort only the
matching local waiter, which closes the engine response receiver and retires the
tray request without posting a second reply. Foreign-session events are ignored,
and adapter teardown aborts any waiters still open.

## Request preflight and resume safety

The engine performs static request validation before it writes a user turn,
routes a warm steer, or interrupts a run for a queued replacement. This catches
invalid mode value types and adapter-known mode IDs. ACP choices still need the
live session catalog, so static preflight does not claim to validate those names.

Harness-native session IDs are persisted with both their harness and cwd. Resume
is injected only when both match the next run, so switching from one of the nine
production harnesses to another cannot feed the former provider's local ID to
the latter. A legacy row without a harness tag is resumed only when the journal
independently proves the same harness, session ID and cwd; otherwise the run
starts fresh. Within the same ACP harness, `session/load` failure still falls
back to `session/new`, and the resulting `SessionStarted` replaces the stored ID.

## Transport contracts versus availability

`crates/harness/src/interaction_contract.rs` is the exhaustive adapter-level
contract for all nine production harnesses. The engine checks question shapes
against the adapter's response transport. ACP cannot accidentally advertise free
text/multiple replies through an option-ID response. Cursor cannot accidentally
present a question without an implemented response channel. Adding a HarnessId
requires updating this exhaustive mapping and its tests.

This contract intentionally does **not** advertise model tools as globally
available. Keep three layers separate:

1. Adapter transport: which native request/response operations Zeron implements.
2. Installed-session discovery: modes, enabled goal features, primary agents,
   model options and protocol-version limits.
3. Individual request: native IDs, allowed choices, custom text, multiple answers,
   blocking behavior and descriptions.

The observed Codex session rejected `request_user_input` in Default mode and
accepted it in Plan mode. Therefore “Codex question transport supported” must not
be read as “the tool is available in every execution mode.” Live protocol/session
behavior wins over a static product-level feature list.

Every new mapping should include a captured/synthetic native request, normalized
UI state, the exact native response payload, cancellation, invalid/stale IDs,
and turn-end/restart behavior. Pure capability booleans or mocked callbacks alone
cannot establish round-trip compatibility. The typed-answer fixture specifically
asserts what the fake Codex app-server receives on stdin; mismatched IDs now fail
explicitly rather than being turned into successful empty answers.

## Compaction lifecycle

Codex `contextCompaction` item start/completion becomes a typed `Compaction` part
with one stable native ID. The generated 0.154 protocol item itself has no
separate status field; lifecycle comes from `item/started` and `item/completed`.
The transcript renders a standalone divider with a shimmering center label while
active, then a static completion label. Reduced motion removes the shimmer. Any
terminal turn stops an unresolved compaction and renders it as stopped rather
than completed, even when the provider omits the completion item. Compaction does
not get hidden inside a generic “Called tools” fold and does not fabricate an
assistant message.

Other adapters currently have no mapped compaction lifecycle in this contract.
Do not synthesize an indeterminate duration from a `/compact` command or assume a
completion notification supplies a start event. Their native signals need explicit
adapter mappings and round-trip fixtures before claiming equivalent progress UI.
