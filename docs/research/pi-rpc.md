**Research: moving Pi from ACP to native RPC in Zeron — 2026-09-29**

Recommendation: implement `PiHarness` in Rust and connect directly to `pi --mode rpc`. The `Harness` interface and Zeron events allow much of the engine and UI to remain in place. The main work concerns lifecycle management, event normalization, sessions, and extensions. The migration is implemented; this research preserves the earlier assessment. The definitive contract is in [PROTOCOL.md](../../crates/harness/src/pi/PROTOCOL.md), and the operational guide is in [Pi](../pi.md).

The previous path was `Zeron → AcpHarness → pi-acp → pi --mode rpc`. It is now `Zeron → PiHarness → pi --mode rpc`. This removes an intermediary process and the need to install the adapter; Pi and its dependencies are still required. No latency improvement was measured.

**Evidence reviewed**

- Code in this checkout: `crates/harness/src/acp/`, `jsonrpc.rs`, `process.rs`, `skills.rs`, the `Harness` interface, `crates/proto/src/agent.rs`, the engine registry, Settings, and Pi tests.
- The exact package `pi-acp@0.0.33`, downloaded without installing or running its dependencies. Its `gitHead` is `1bfcb394088ed879db8fd936b570bb626017f878`.
- Installed Pi version: `0.85.1`. Its RPC implementation was reviewed, and isolated tests were run with a local mock provider.
- npm listed Pi `0.87.1` at the time of this research. Its published reference code was inspected at commit `f07218c4d4bbc12bef056a7058c3dd49dfe41abe`; that version was neither installed nor run. [Package metadata](https://registry.npmjs.org/@earendil-works/pi-coding-agent/latest).
- Current official documentation. There are differences between that documentation and the published versions reviewed; see compatibility below.

**Specific limitations of the integration at the time of research**

| Area | Evidence and consequence | Proposed change |
| --- | --- | --- |
| Steering | `pi_spec()` declares `StepBoundary`, but the engine's static registry advertises `TurnBoundary`. Without an ACP steering extension, the harness cancels generation and sends another prompt, waiting for outstanding tools. | Use native `steer` while work is active and `prompt` while idle. Insert instructions without cancelling the model. Align the descriptor and driver. |
| Questions | The adapter cancels `input` and `editor`. It converts all `select` options to `allow_once`; Zeron's ACP classifier treats them as permissions and automatically accepts them. | Translate the `extension_ui_request/response` subprotocol into the question bridge, preserving the user's actual selection. |
| Errors | The adapter closes `agent_settled` as `end_turn` unless cancelled, without deriving the outcome from the assistant's final error. Zeron detects failures through a `notify` added to its MCP extension. | Read `message_end.stopReason/errorMessage` directly, preserve the final outcome after retries, and emit `Done::Errored` when appropriate. |
| Extension commands | The adapter's `get_commands` query uses `includeExtensionCommands: false`. | Expose native commands per workspace, alongside skills and templates. |
| Thinking | Zeron advertises levels up to `Max`; the adapter fixes its list at `off…xhigh`. | Query `get_available_thinking_levels` per model without advertising nonexistent levels. Decide how to represent `off`, which does not exist in `ReasoningLevel`. |
| Usage and context | The adapter provides statistics through `/session`, but its event translator does not emit the `usage_update` Zeron expects. | Consume usage and model information directly. Distinguish context occupancy, cumulative usage, and cost. |
| MCP | `mcpServers` is stored but not connected. Zeron already adds an extension and an executable wrapper for each run. | Keep the tool extension and inject it directly through `--extension`; remove the wrapper required by `pi-acp`. |

The adapter observations apply to the pinned package, not to all ACP servers. Its [session implementation](https://github.com/svkozak/pi-acp/blob/1bfcb394088ed879db8fd936b570bb626017f878/src/acp/session.ts) does forward `thinking_delta` and some dialogs; its README is outdated on that point. Model catalog discovery also already works: `default` is Zeron's fallback, not its only possible model.

**Driver design**

Create `crates/harness/src/pi/{mod.rs,rpc.rs,normalize.rs}` and move the MCP extension into that module. Implement `Harness` while retaining `HarnessId::Pi`, so existing chats and preferences continue to identify the same provider.

The transport needs a continuous stdout reader, correlation by `id`, pending responses, and an ordered event stream. Pi uses JSONL with `type`, `command`, `success`, and `data` fields, rather than the JSON-RPC 2.0 envelopes in `jsonrpc.rs`. The concurrency pattern can be reused, but that client requires changes. Split on LF; support CRLF and UTF-8 sequences split across reads. Retain stderr for diagnostics. [Official protocol](https://pi.dev/docs/latest/rpc).

Reuse executable/PATH resolution, `process::Command`, harness queues, `StderrTail`, the execution lease, and process cleanup. Keep the process alive between turns while its mailbox exists, and retire it when the session closes, the consumer is dropped, or execution is permanently interrupted. Preserve Unix process groups and Windows Job Objects; killing only the parent process can leave tools running.

| Pi input | Proposed Zeron output |
| --- | --- |
| `get_state` | `SessionStarted`, native identity, model, and session file |
| `message_update` with `text_delta` / `thinking_delta` | `TextDelta` / `ReasoningDelta` |
| `tool_execution_start/end` | `ToolCall` / `ToolResult`, correlated by `toolCallId` |
| `message_end` | Reconcile the final message, usage, and error/abort status |
| `agent_settled` | Close the active run with exactly one `Done` |
| Dialog `extension_ui_request` | Pending question followed by `extension_ui_response` |
| `get_commands` | `AvailableCommands` and skill discovery |

The current stream carries deltas, not all partial SDK snapshots. Reconstruct content by `contentIndex` and reconcile it with final messages without duplicating text or tools. Preserve Zeron's types for known tools; extension tools retain their names and arguments. Pi diffs require translation: they do not automatically match ACP's `ToolDiff`. Results containing images require an additional storage/rendering decision. [Official events](https://pi.dev/docs/latest/json).

**Lifecycle and compatibility: the most delicate part**

A successful `prompt` response confirms acceptance, not completion. `turn_end` closes a response and its tools. `agent_end` may precede a retry, compaction, or continuation. The correct terminal event for agent work is `agent_settled`, combined with the final error/abort status. Do not infer completion from silence or a tool result. Register the reader before sending commands, and tolerate different event/response orderings. [RPC lifecycle](https://pi.dev/docs/latest/rpc#run-lifecycle).

There is another terminal path: extension commands or input handlers that consume the prompt without starting a run. In the test, `/probe-noop` produced only a successful response; waiting for `agent_settled` in that case would block the chat.

The current documentation includes `data.disposition = started | queued | handled`. However, the tested Pi 0.85.1 returns success without that field, and the [code for 0.87.1](https://github.com/earendil-works/pi/blob/f07218c4d4bbc12bef056a7058c3dd49dfe41abe/packages/coding-agent/src/modes/rpc/rpc-mode.ts) also uses a boolean preflight acknowledgement without a disposition. An implementation should not assume that the documented `latest` matches the installed npm version.

Compatibility was resolved during implementation: after the ACK, send `get_state` through the same channel, processing all preceding events. In 0.85.1 and the inspected 0.87.1 code, `isStreaming` reflects `_isAgentRunActive`, including retries and continuations, and is set synchronously before the next stdin command. At this ordered barrier, `isStreaming: false` and `isCompacting: false` allow completion of a prompt consumed without a run. Each barrier belongs to a submission epoch; it is not a silence timer. The test using real Pi covers `/probe-noop` and session continuity.

Steering is submitted through `prompt` with `streamingBehavior: "steer"`: Pi queues it while active and starts a run while idle, in one atomic operation. This avoids the race between reading state and sending a separate `steer`.

For steering, the ACK means the message was queued, not that it has entered the context. Rotate the transcript segment when incorporation is observed, preserve Zeron's `message_id` values, and cover the race between queuing and becoming idle. `follow_up` exists, but `RunControls` currently has only one steering input: offering both behaviors to the user requires extending the contract/UI. To interrupt, empty the queue with `clear_queue`, send `abort`, and retain termination escalation if the process does not respond. [RPC commands](https://pi.dev/docs/latest/rpc-commands).

**Existing sessions**

The adapter already uses native Pi JSONL files and usually publishes their native UUID. It also stores the UUID → file mapping in `~/.pi/pi-acp/session-map.json`. This enables migration without converting history, provided the correct file can be resolved.

Keep the UUID stored by Zeron; obtain and cache the path on the execution host. For old sessions, consult the adapter's map and, if it is missing, search native sessions while respecting `PI_CODING_AGENT_DIR` and configured directories. Open with `--session <absolute-path>`, verify the returned UUID, and avoid replaying messages Zeron already has into the transcript.

In Pi 0.85.1, `--session <uuid>` works for a local session, but finding a session from another project may trigger an interactive fork confirmation on stdout. An absolute path avoids that branch. Recovery failures must be explicit; do not present a new session as though it retained the previous context.

The test reopened a native file after terminating Pi and preserved its UUID and all 10 messages. Recovery of a real ACP chat remained a pending migration test at this stage; the format and map were checked through code inspection.

**Extensions and Zeron's interface**

`select`, `confirm`, and `input` largely fit `RunControls.request_input`. `editor` needs to preserve prefilled text and multiline editing; the existing `UserInputQuestion` contract has no prefill field. `notify`, `setStatus`, `setWidget`, `setTitle`, and `set_editor_text` need dedicated paths if they are to be displayed; they are not questions. Dialogs must not block the event reader and must be resolved or cancelled when the run closes. [RPC UI](https://pi.dev/docs/latest/rpc-extension-ui).

RPC does not reproduce the entire TUI: `custom()` and several component APIs are unavailable. The migration does not automatically introduce sandboxing or native MCP either. Keeping the MCP extension preserves Zeron's chat tools; its error notification workaround can be removed once the driver reads errors directly. [UI limitations](https://pi.dev/docs/latest/rpc-extension-ui), [Pi execution model](https://pi.dev/docs/latest/security).

TUI commands should not be sent indiscriminately as prompts. The adapter offers `/compact`, `/session`, `/name`, `/export`, `/autocompact`, `/steering`, and `/follow-up`; preserving their functionality requires translating them to RPC. Discover commands in the actual cwd and reuse skill linking from `skills.rs`. Forking, the session tree, and detailed statistics are later product extensions: the transport supports them, but the current `Harness` contract does not expose all of them.

**Change map and suggested order**

1. Transport and normalization: create `PiHarness`, a JSONL reader, a persistent session, streaming, tools, image attachments, error handling, terminal handling, and interruption. Add a deterministic RPC fixture and lifecycle tests.
2. Parity and integration: old sessions, discovery by cwd, models/thinking, existing commands, MCP, questions, and steering. Register `PiHarness` in `crates/engine/src/registry.rs`. Introduce `PI_EXECUTABLE`; `PI_ACP_EXECUTABLE` points to a different binary and cannot be reinterpreted as Pi.
3. Adapter retirement: remove `pi_spec`, prewarming, and managed Pi ACP installation; update Settings, the `HarnessId::Pi` comment, parity/MCP documentation, and installation/resolution tests. Preserve the ACP infrastructure used by other agents.
4. Optional extensions: extension status/notifications, an editor with prefill, tree/fork, compaction controls, and costs. Some require new events and desktop/mobile consumers.

Priority tests before activating the driver: exactly one completion; errors before and after acceptance; recovery after retry; compaction; prompts consumed without a run; multiple steering messages and races with idle; cancellation during generation/tools/dialogs; EOF and crashes; reaping descendants; resuming a legacy UUID; MCP and its cancellation; images; large streams and unknown events; executable paths containing spaces and `.cmd` on Windows. The existing `pi_resume.rs`, `pi_live.rs`, `pi_mcp.rs`, and `real_acp_lifecycle.rs` tests provide scenarios worth porting.

**Validation performed during this research**

Pi 0.85.1 was run in a temporary directory, with an isolated agent directory, resource discovery disabled, and an explicit extension containing a mock provider. No external provider was invoked, and the user's Pi configuration was not modified.

The following passed: state/model/thinking/command queries; a streaming prompt with `agent_settled`; provider failure in `message_end`; steering with two turns under a single completion; `clear_queue` + `abort` with an `aborted` message; an `input` dialog answered through RPC; a command without an LLM call that returns only an ACK; and closing and resuming the file with its UUID and history preserved. Initial tests revealed a bug in the diagnostic script's formatter; after it was fixed, the full run completed successfully.

During this research phase, no Rust tests were run because no product code was changed. Real models, Windows, live retry/compaction, and end-to-end migration of an ACP chat were not validated either. This scope supports the architecture recommendation and identifies risks; it does not certify a driver that had yet to be implemented at that stage.
