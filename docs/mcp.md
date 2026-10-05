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

`create_chat` and each request in `create_chats` accept `kind`:

| Input | Placement |
| --- | --- |
| `kind: "chat"` | Standalone session, visible in the left **Sessions** sidebar; no `parentChatId` is written. |
| `kind: "side"` | Child of the explicit `parent` or origin chat. Requires one of those. |
| Kind omitted, origin or parent supplied | Child, preserving the previous default. |
| Kind omitted, no origin or parent | Standalone, preserving terminal usage. |

`kind: "chat"` with a nonempty `parent`, `kind: "side"` without an origin/parent,
and unknown kinds fail before any writes. Empty `parent` uses the default.
The creation result includes the effective `kind`, `chatId`, `deviceId`,
`project` and `parentChatId`. Kind is derived from the parent; it is not persisted.

Children record `Chat.parent_chat_id` (proto) ⇄ `parentChatId` on the registry
row (`Mutate createChat { parentChatId? }`). Chats with a parent cannot create
chats through MCP, including standalone and batch creation or an explicit parent
override. A side chat cannot be selected as a parent; only one level is supported.

For children, `parent` (id, prefix, or title) overrides the origin (`ZERON_CHAT_ID`).
`list_chats { parent }` returns children, and chat summaries carry `parentChatId`.
Rows from older engines read as parentless; a dangling parent id is tolerated.

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
`kind: "side"` spawns) and opens either in the right pane.

## Tools

Chats are referenced by full id, a unique id prefix, or an exact title.
Projects by id, path, display name, or unique path suffix. Devices by id or
name (default: the local engine's device).

| Tool               | Engine calls                                              |
| ------------------ | --------------------------------------------------------- |
| `whoami`           | `LocalDevice`, `EngineInfo`, origin chat summary          |
| `list_devices`     | `WatchDevices` snapshot                                   |
| `list_projects`    | `WatchSpaces` snapshot                                    |
| `list_harnesses`   | `ListHarnesses {targetDeviceId?}`                          |
| `list_models`      | `ListModels {harness, targetDeviceId?}`                                    |
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

## Selecting a host and project

`list_projects {device?}` filters by device id or exact name; without arguments
it lists all projects. `list_harnesses {device?}` and
`list_models {harness, device?}` query the selected host with `targetDeviceId`;
without `device` they retain the local engine catalogs.

For creation, project alone determines the host. Device alone creates a session
without a project on that device. With both, the project is resolved **within**
the selected device, and a project id belonging to another device is rejected.
Neither argument means a projectless session on the local engine. Repeated names
or paths are errors with candidate ids and devices; use ids from discovery.

Before writing, creation checks the chosen engine's harness catalog, chooses
its available default (Claude Code when available, otherwise the first available
non-mock harness), and validates any explicit model against that host's catalog.
An explicit harness must be offered, installed and enabled. Catalog failures,
including model lookup failures, are returned with the device id; there is no
fallback to the local catalog. A successful remote query checks reachability;
`lastSeenAt` alone does not establish that an engine is online.

Discover and launch a standalone session:

```text
list_devices {}
list_projects { device: "<device-id>" }
list_harnesses { device: "<device-id>" }
list_models { device: "<device-id>", harness: "codex" }
create_chat {
  kind: "chat",
  device: "<device-id>",
  project: "<project-id>",
  harness: "codex",
  model: "<model-id from that host's catalog>",
  title: "Implement feature",
  prompt: "Implement the feature...",
  wait: false
}
```

Without a project (the host uses its home directory unless `cwd` is given):

```json
{"kind":"chat", "device":"<device-id>", "harness":"codex", "prompt":"Reply with pong", "wait":true}
```

A mixed `create_chats` batch, with an origin chat or explicit top-level parent:

```json
{
  "requests": [
    {"kind":"chat", "device":"<device-id>", "project":"<project-id>", "title":"Feature", "prompt":"Implement the feature"},
    {"kind":"side", "parent":"<parent-chat-id>", "project":"<project-id>", "prompt":"Review the API"}
  ]
}
```

The first prompt and later sends use durable chat commands drained by the host.
Transcript reads target the chat's host; session status comes from the shared
registry. Sends remember the session and existing message ids on the MCP connection before
sending, so a subsequent `wait_for_turn` after `wait:false` waits for that send,
even before a session row appears. A separate MCP server has no send baseline
and reports the chat's current posture. Creation records the send time before dispatching its first
prompt. If completion arrives before the transcript, the wait allows the new
assistant response to arrive within the **same timeout**. A previous response
is never substituted for a missing response to a new send. An unfinished
transcript or missing response at the deadline returns `timedOut`.

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
child chat can. Goal commands from these tools carry the calling chat as their
issuer, and the chat's host applies narrower rules to them than to a person's
(`agent_goal_permission`), so relaying a request through another chat changes
nothing:

- an agent cannot give **its own chat** a goal, and sets at most the default
  number of rounds;
- a goal still in play can be paused, resumed, replaced or cleared only by the
  agent that set it, so **no agent can change a goal a person set** (a finished
  goal may be cleared or followed by a new one);
- only a person extends a goal **past its limit**.

The tools check the same rules before queuing, so a refusal returns at once with
its reason. A process that bypasses the tools and writes commands to the engine's
local port directly is outside this model (see the ask profile below).

## The ask profile

The engine injects a restricted server into the hidden child chat of a *child
ask* (goal verifiers today, workflow agents next) by adding `ZERON_ASK_ID`. That
server lists only `whoami`, `get_chat` and `read_chat` plus the run-scoped
`submit_result`, whose input schema is the ask's JSON Schema (fetched with
`GetAskSpec`; a non-object schema travels under a `result` key). `submit_result`
posts to `SubmitAskResult`: an accepted result completes the ask, a rejected one
returns the path-level violations as an error result so the model repairs and calls
again (three repair rounds, then the ask fails).

Only the child's own server can answer. The engine also puts a per-ask secret in
that server's environment (`ZERON_ASK_TOKEN`), keeps it in memory (never in the
synced doc, where `meta.askChild` is a bare marker), and refuses any
`SubmitAskResult` without it. Both RPCs are machine-local: the device relay refuses
them, so another device on the account cannot answer for a verifier. The token
guards against a stray or curious process that learned the ids; a process running
as the same OS user can still read another process's environment, so it is not a
sandbox boundary.

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

An isolated stdio instance is available with
`cargo run -p zeron-engine --example mcp_standalone_smoke`. It provides a temporary
project, an origin coordinator and a scripted `codex` adapter (`smoke-1`) returning
`pong`. Discover its ids with the list tools, create `kind: "chat"` with a prompt
and `wait: true`, then check `read_chat`, `send_message` and a mixed batch. The
profile is removed on exit. This checks MCP dispatch and engine execution without
changing your running app's sessions or requiring provider credentials.

The `mcp_standalone_session_executes_on_the_selected_device` test in
`device_routing` additionally runs two engines with a shared registry and device
relay, verifies execution on the selected host (and absence of execution on the
caller), and retrieves responses through MCP.
