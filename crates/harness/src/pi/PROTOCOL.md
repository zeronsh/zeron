Pi RPC compatibility contract

The native driver targets Pi >= 0.85.1. Requests and events are JSONL, never
JSON-RPC. Responses and lifecycle events must retain stdout order.

`prompt` success acknowledges preflight. Normal runs finish at `agent_settled`,
not `agent_end`; a final error message determines the terminal status after
retries. Extension commands and input handlers may acknowledge without a run.

Legacy ACK barrier (verified against installed 0.85.1 and inspected 0.87.1):
after prompt acceptance request `get_state`. In these versions AgentSession's
`isStreaming` is `_isAgentRunActive`, covering retries/post-run continuations.
The synchronous path sets it immediately after the preflight callback, before
another stdin command can execute. A false isStreaming AND false isCompacting
at this ordered barrier closes a handled prompt (or a run already settled).
This is NOT polling for idle or a quiet timeout. Process all preceding events
before the response, and scope barriers to the submitted execution. A future
`data.disposition` is additive; the state barrier also catches extension-started
work, which can accompany a handled disposition.

Independent work started later by an extension is a new run (`agent_start`).
Discovery/startup timeouts diagnose startup only; no active model run is timed
out for lack of output. Interruption clears queues before aborting.

Steering uses `prompt` with `streamingBehavior: "steer"`. `set_steering_mode`
persists globally, so Pi's native `all` mode is selected only while neither
global nor project settings configure a mode.
Preflight/state barriers remain serialized, but consumption does not gate the
next submission after `queue_update` confirms an appended steering entry.
Keep one FIFO delivery record per original input, including duplicate text.
`message_start` for each consumed user input emits its own `Steered`; neither
the prompt ACK nor `queue_update` is a consumption receipt. Extension commands
wait for earlier deliveries, and unqueued/handled inputs block pipelining until
consumed or settled. An idle barrier must not confirm queued inputs that never
produced their consumption events.

Sources inspected: Pi dist/core/agent-session.js and dist/modes/rpc/rpc-mode.js
0.85.1, and upstream commit f07218c4d4bbc12bef056a7058c3dd49dfe41abe.

Session identity is refreshed from every accepted state barrier, including
extension-driven new/switch/fork operations. Pi defers file creation until an
assistant message. Only a host-owned empty-session record, checked with
get_entries (model/thinking changes only) and a second idle get_state, permits
recreation using --session-id in the same cwd. Any submitted input revokes that
proof before the write to stdin. Missing history/custom entries never authorize
recreating the same UUID; an unrestorable session starts a new one with a
visible "without the previous context" notice, like the other harnesses.
