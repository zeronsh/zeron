# Zeron MCP server

`zeron mcp` serves the Model Context Protocol on stdin/stdout and proxies every
tool into the running engine's localhost IPC (`ws://127.0.0.1:$ZERON_IPC_PORT`,
default 27654) — the same `zeron_rpc` surface the headed app and `zeron sync`
dial. It is a subcommand of the one `zeron` binary: no Node runtime, no extra
install, a few MB resident.

Crate: `crates/mcp` (`zeron-mcp`). The protocol layer is hand-rolled
(`initialize`, `ping`, `tools/list`, `tools/call`; newline-delimited JSON-RPC
2.0) — the repo already owns JSON-RPC framing for the Codex and ACP drivers and
the stdio tool-server subset is tiny, so no SDK dependency was taken.

## Identity

The engine can inject this server into a harness's MCP config with the
originating chat in the environment:

| Variable          | Meaning                                                       |
| ----------------- | ------------------------------------------------------------- |
| `ZERON_IPC_PORT`  | Engine to proxy (default 27654).                              |
| `ZERON_CHAT_ID`   | The chat whose agent spawned this server.                     |
| `ZERON_DEVICE_ID` | That chat's host device.                                      |

When `ZERON_CHAT_ID` is set, every `send_message` is prefixed with a
`[Message from Zeron chat <title> (<id8>) …]` line so the receiving agent and the
human reading that transcript can tell an agent-to-agent message from a typed
one, and the server refuses to message its own chat. The transcript renders
this routing header as “Message from **chat name**”, keeping the full routing
instructions in the stored prompt for agents.

### Parent links

A chat created through `create_chat` records the creating chat as its parent:
`Chat.parent_chat_id` (proto) ⇄ `parentChatId` on the registry/workspace chat
row (`Mutate createChat { parentChatId? }` → `WorkspaceHost::create_chat_with_parent`).
Chats with a parent cannot create chats through MCP, including batch creation
or an explicit parent override. A side chat cannot be selected as a parent;
only one level of side chats is supported.

The default is the origin chat (`ZERON_CHAT_ID`); an explicit `parent` argument
(id, prefix, or title) overrides it. `list_chats { parent }` returns a chat's
children, and every chat summary carries `parentChatId`. The field is additive
and serde-defaulted: rows written by older engines read as parentless, and a
dangling id (parent deleted) is tolerated rather than cascaded.

The left sidebar hides chats that have a parent: `AppState::visible_chats`
(the Sessions list, project tabs, jump slots) and the Archived section both
require `parent_chat_id == None`. Children remain addressable by id, deep
link, and every MCP tool; `list_chats { parent }` is how an orchestrator
finds them.

### Injection

The host engine stamps this server onto every run it drives
(`RunRequest.mcp`, additive): the same `zeron` binary with `args: ["mcp"]`
and the three variables above, pointed at the port the engine itself serves
(never a port it lost the bind race for). Each driver spells it in its own
dialect and leaves the user's configured servers alone:

| Harness | Where |
| ------- | ----- |
| Claude  | `--mcp-config <inline json>` (no `--strict-mcp-config`)             |
| ACP (Devin, Grok, Hermes, Antigravity) | `session/new` and `session/load` → `mcpServers: [{name, command, args, env}]` |
| Pi | Per-run `--extension` bridges stdio MCP into Pi tools in the native RPC process |
| OpenCode | Child-only `OPENCODE_CONFIG_CONTENT`: `mcp.zeron` on 1.x, `mcp.servers.zeron` on 2.x |
| Codex   | `thread/start` config overrides `mcp_servers.zeron.{command,args,env}` |
| Cursor  | SDK `Agent.create` / `Agent.resume` → inline `mcpServers.zeron`, plus `local.settingSources: ["user", "team", "mdm", "plugins"]` so `~/.cursor/mcp.json` and plugin servers load (not `project`: the SDK skips MCP approvals, so repo-defined servers would run unprompted) |

OpenCode preserves inherited inline configuration and other servers. Its config
shape follows the installed binary's major version. Cursor uses the SDK's
[inline MCP configuration](https://cursor.com/docs/sdk/typescript); OpenCode's
[1.x config layer](https://opencode.ai/docs/config/) and
[2.x MCP format](https://opencode.ai/v2/docs/mcp-servers) differ.
Pi's temporary extension lives only for the native RPC process lifetime;
user settings, extensions, and session arguments remain intact. The bridge
registers `zeron_<tool>` tools, propagates cancellation and errors, and closes
the MCP child when the Pi session shuts down.

Title runs never carry it. A run with no served port (embedded engine that
lost the bind) gets no Zeron tools rather than a dead server.

### Forks and the explorer

`ForkSideChat { chatId, sourceChatId, parentChatId? }` copies a chat's
history through its latest completed response into a new chat and appends
a `fork` part (a system entry: `sourceChatId`, `sourceTitle`) as the seam;
the transcript draws it as "This chat was forked from <title>". `parentChatId`
defaults to the source; a side chat's own fork button passes its parent so
the copy lists as a sibling. The file explorer's footer lists a chat's
**Subagents** (its spawn chips) and **Chats** (its children: forks and
`create_chat` spawns) and opens either in the right pane.

## Tools

Chats are referenced by full id, a unique id prefix, or an exact title.
Projects by id, path, display name, or unique path suffix. Devices by id or
name (default: the local engine's device).

| Tool               | Engine calls                                              |
| ------------------ | --------------------------------------------------------- |
| `whoami`           | `LocalDevice`, `EngineInfo`, origin chat summary          |
| `list_devices`     | `WatchDevices` snapshot                                   |
| `list_projects`    | `WatchSpaces` snapshot                                    |
| `list_harnesses`   | `ListHarnesses`                                           |
| `list_models`      | `ListModels {harness}`                                    |
| `list_chats`       | `WatchChats` + `WatchSessions` snapshots (status merged)  |
| `get_chat`         | above + `WatchDocMessages` opening frame (pending input)  |
| `create_chat`      | `Mutate createChat` (+ `renameChat`; optional first send) |
| `create_chats`     | Concurrent `create_chat` requests with per-request results |
| `send_messages`    | Concurrent `send_message` requests with per-request results |
| `read_chat`        | `WatchDocMessages` opening `reset` frame, rendered        |
| `send_message`     | `QueueCommand` Run / Steer, or `QueueMessage`             |
| `wait_for_turn`    | `WatchSessions` until the chat settles                    |
| `interrupt_chat`   | `QueueCommand` Interrupt                                  |
| `respond_to_input` | `QueueCommand` RespondInput                               |
| `archive_chat`     | `Mutate setChatArchived`                                  |
| `get_goal`         | `WatchDocMessages` opening frame (`goal`)                 |
| `set_goal`         | `QueueCommand` Goal `set`, then waits for the host        |
| `pause_goal`       | `QueueCommand` Goal `pause`                               |
| `resume_goal`      | `QueueCommand` Goal `resume`                              |
| `clear_goal`       | `QueueCommand` Goal `clear`                               |
| `workflow_guide`   | (embedded authoring guide, `docs/workflow-guide.md`)      |
| `start_workflow`   | `WorkflowStart` (returns after the user approves)         |
| `get_workflow_run` | `WorkflowGet {runId, include}`                            |
| `list_workflow_runs` | `WorkflowList {chatId?}`                                |
| `stop_workflow_run` | `WorkflowStop`                                           |
| `resume_workflow_run` | `WorkflowResume`                                       |
| `resolve_workflow_question` | `WorkflowAnswer`                                 |

Watch streams are the engine's only read surface (there is no one-shot "get
transcript" RPC); a snapshot is "subscribe, take the first item, drop" — drop
cancels server-side, exactly what the sidebar does on attach.

`send_message` mode `auto` starts idle chats and steers working chats through
their live mailbox without interrupting tools or child processes. Providers
consume it at their next supported input boundary. Only explicit `queue` mode
creates a held queue row. `awaitingInput` refuses and points at
`respond_to_input`. Quiet, long-running turns still receive steering; the host
falls back to starting a turn if the live runtime has already exited.

Claude uses `priority: "next"` and confirms consumption through replayed user
messages. Cursor uses the SDK's native `Run.steer`, waits for active tools to
finish, and retains input until its delivery acknowledgment. New injections do
not wait for earlier delivery acknowledgments; that wait serializes responses
even when the SDK reports a single active turn. These are mid-turn
paths; they do not terminate the harness process. Adapters that only accept input
between turns are labeled **Send next** in the composer.

The opt-in `steering_live` engine test checks a rapid burst, foreground and
background process survival, retained context, exactly-once effects, and ordered
normal queue delivery. For Claude, Cursor and Codex it additionally requires the
burst to finish within the original turn. Set `ZERON_TEST_BURST=6` to reproduce a
six-message burst; select an inexpensive model with `ZERON_TEST_MODEL` and the
harness with `ZERON_TEST_HARNESS`. Codex is checked for child survival during
the active tool: its runtime cleans up background jobs on normal tool completion
even without steering. Other providers also check a job that outlives that tool.

For conversational redirection, run `cargo run -p zeron-harness --example
cursor_steering_probe -- gemini-3-flash`. It sends six bare digits during streamed
prose and requires only the latest requested answer. Counting turn completions
alone cannot distinguish real steering from serial responses inside one run.

`wait_for_turn` after a send is edge-triggered on the `Session` row captured
before the send: it returns on a new `last_completed_turn`, an
`awaitingInput`/`errored` stamp newer than the baseline, or a working→idle
edge. A brand-new chat has no session row until the host picks the run up, so
the wait keeps waiting in that case rather than reporting the unstarted run as
done (this was the one bug the first live run found).

## Goals

`set_goal {objective, chat?, max_rounds?, token_budget?, time_budget_seconds?,
replace?}` gives a chat a goal: it keeps working turn after turn until an
independent verifier chat judges the objective met (`docs/goal-mode.md`). `chat`
defaults to the chat this server speaks for (`ZERON_CHAT_ID`); another chat is
named like in `send_message`. The tool queues the command and waits a few seconds
for the host to apply it, then returns the goal; a host that is offline applies it
when it returns (the result says `queued`). `get_goal` returns the goal with its
verdict history, budgets and stop reason, or `null`.

Trust rules: there is **no tool that completes a goal** — only the verifier
child chat can. An agent also cannot `pause_goal`, `resume_goal` or `clear_goal`
the goal that is verifying *its own* chat, nor replace it with `set_goal
replace=true`: ending that loop is the user's decision, not a way out of
verification. It can read it, set a goal on its own chat when it has none, and
manage goals of other chats it supervises.

## Workflows

`start_workflow` runs a **Starlark script** that orchestrates many agent chats in the
background (`docs/workflows.md`; the authoring guide is `workflow_guide`). The tool
descriptions steer the model: start one only when the user asks for a workflow or the
work truly needs many independent agents, read `workflow_guide` first, and **do not
poll** — the result is delivered to the chat as a machine-origin message when the run
settles.

| Tool | Arguments | Notes |
| --- | --- | --- |
| `workflow_guide` | — | The embedded guide: API, worked example, patterns, limits. |
| `start_workflow` | `name?`, `script` \| `path`, `args?`, `max_concurrency?`, `harness?`, `model?`, `reasoning?`, `max_asks?`, `max_tokens?`, `max_runtime_seconds?` | The calling chat's own workflow. A script with problems fails with `path:line:col message` lines and creates no run. Otherwise the user is asked (an ordinary input question on the chat's live turn: phases, agents, literal commands, limits, a script excerpt) and the call returns `{runId, phases, agents, commands, …}` once they approve, or an error if they deny. The script is drafted to `.zeron/workflow-drafts/` in the project (add it to `.gitignore`). |
| `get_workflow_run` | `run_id`, `include?` (`nodes`, `reports`, `result`) | State: status, phase, usage, agents, artifacts, pending questions, stop reason; the full result and every reported item on request. |
| `list_workflow_runs` | `chat?` (default: own chat; `"all"`) | |
| `stop_workflow_run` | `run_id`, `reason?` | Own chat's runs only. Resumable. |
| `resume_workflow_run` | `run_id` | Own chat's runs only. Replays the journal; needs approval like a start. |
| `resolve_workflow_question` | `run_id`, `qid`, `answer` | Own chat's runs only. Answers an actor's escalation. |

Trust rules: an agent starts workflows for **its own chat** only (`ZERON_CHAT_ID`
required) and may stop, resume or answer only runs of its own chat — a user's own MCP
client (no chat origin) may act on any run on the device. Approval cannot be skipped by
the agent: it is waived only when the chat itself is auto-approve. The run executes on
the device that hosts the chat.

Behind the tools, `WorkflowStart`, `WorkflowGet`, `WorkflowList`, `WorkflowStop`,
`WorkflowResume`, `WorkflowAnswer`, `WorkflowArtifactData` and `WorkflowArtifactRead`
are ordinary engine RPCs (clients use the same ones); stop / resume / answer also travel
the command plane (`SessionCommandPayload::Workflow`) so a remote client can send them.

## The ask profile

The engine injects a restricted server into the hidden child chat of a *child
ask* (goal verifiers, workflow agents) by adding `ZERON_ASK_ID`. That
server lists only `whoami`, `get_chat` and `read_chat` plus the run-scoped
`submit_result`, whose input schema is the ask's JSON Schema (fetched with
`GetAskSpec`; a non-object schema travels under a `result` key). `submit_result`
posts to `SubmitAskResult`: an accepted result completes the ask, a rejected one
returns the path-level violations as an error result so the model repairs and calls
again (three repair rounds, then the ask fails). Both RPCs are IPC-only.

A workflow actor's ask also offers **`escalate {question?, context?, question_id?}`**
(`AskEscalate`, IPC-only; the spec carries `escalation: true`): a last-resort question
for the agent that started the run, at most three per ask. It parks only that ask — the
time spent waiting does not count against the ask's timeout — and returns the parent's
answer (`resolve_workflow_question`) as the tool result. A call waits at most ~45 s
(under the tool timeouts of the strictest harnesses); if unanswered it returns `pending`
with a `question_id`, and calling again with that id keeps waiting without spending
another escalation.

## Parallel side chats

Use `create_chats` with a `requests` array to launch independent workers in
one tool call, including when the harness executes its tool calls sequentially:

```json
{
  "requests": [
    {"project": "/repo", "title": "Review tests", "prompt": "Review test coverage"},
    {"project": "/repo", "title": "Review API", "prompt": "Review API compatibility"}
  ]
}
```

Use `send_messages` with the same envelope for existing chats; each item uses
`send_message` arguments. Both accept 1–32 requests, run them concurrently,
and return `results` in input order with `index`, `isError`, and either `result`
or `error`. A failed request does not cancel or roll back successful requests.
Each request defaults to `wait: false`; explicit waits also run concurrently.
Only batch independent work, not ordered messages to the same chat.

When using individual tools, launch **all** chats/messages with `wait: false`
first, then collect replies with `wait_for_turn`. Waiting on each individual
launch before issuing the next serializes work at the caller, even though
both the MCP transport and engine support concurrent chat runs.

## Smoke recipe

```sh
BIN=target/debug/zeron
{ echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}'
  echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_chats","arguments":{"limit":5}}}'
  sleep 5; } | ZERON_IPC_PORT=27655 $BIN mcp
```

`create_chat` with `"prompt": "Reply with exactly the word pong", "wait": true`
against a live daemon returns the assistant's `pong` in a few seconds; archive
the chat afterwards with `archive_chat`. Unit tests (`cargo test -p zeron-mcp`)
drive the whole tool set against an in-memory stub `RpcService`.
