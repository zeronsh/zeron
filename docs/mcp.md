# Glitch Flow MCP server

`glitch-flow mcp` serves the Model Context Protocol on stdin/stdout and proxies every
tool into the running engine's localhost IPC (`ws://127.0.0.1:$GLITCH_FLOW_IPC_PORT`,
default 27654) — the same `zeron_rpc` surface the headed app and `glitch-flow sync`
dial. It is a subcommand of the one `glitch-flow` binary: no Node runtime, no extra
install, a few MB resident.

Crate: `crates/mcp` (`zeron-mcp`). The protocol layer is hand-rolled
(`initialize`, `ping`, `tools/list`, `tools/call`; newline-delimited JSON-RPC
2.0) — the repo already owns JSON-RPC framing for the Codex and ACP drivers and
the stdio tool-server subset is tiny, so no SDK dependency was taken.

## Identity

The engine injects this server into supported harness runs with the
originating chat in the environment:

| Variable                | Meaning                                   |
| ----------------------- | ----------------------------------------- |
| `GLITCH_FLOW_IPC_PORT`  | Engine to proxy (default 27654).          |
| `GLITCH_FLOW_CHAT_ID`   | The chat whose agent spawned this server. |
| `GLITCH_FLOW_DEVICE_ID` | That chat's host device.                  |

The corresponding `ZERON_*` names remain compatibility aliases. Per-run
injection sets both names to the same values so an inherited environment
cannot redirect the MCP server to another engine or chat.

When `GLITCH_FLOW_CHAT_ID` is set, every `send_message` is prefixed with a
`[Message from Zeron chat <title> (<id8>) …]` line so the receiving agent and the
human reading that transcript can tell an agent-to-agent message from a typed
one, and the server refuses to message its own chat.

### Parent links

A chat created through `create_chat` records the creating chat as its parent:
`Chat.parent_chat_id` (proto) ⇄ `parentChatId` on the registry/workspace chat
row (`Mutate createChat { parentChatId? }` → `WorkspaceHost::create_chat_with_parent`).
The default is the origin chat (`GLITCH_FLOW_CHAT_ID`); an explicit `parent` argument
(id, prefix, or title) overrides it. `list_chats { parent }` returns a chat's
children, and every chat summary carries `parentChatId`. The field is additive
and serde-defaulted: rows written by older engines read as parentless, and a
dangling id (parent deleted) is tolerated rather than cascaded.

The left sidebar renders a recursive chat tree, and `list_chats { parent }`
lets an agent query one chat's direct children. Ticket links are separate:
`create_chat { ticket }` attaches a new chat to a ticket, while its parent chat
still records the agent thread hierarchy.

Agent chat runs receive the app-owned MCP server as a per-run addition to
their usual provider configuration. Codex uses thread config overrides,
Claude Code receives a merging `--mcp-config`, ACP receives a stdio server in
`session/new` and `session/load`, Cursor SDK receives an inline `mcpServers`
entry, and locally launched OpenCode receives a process-scoped
`OPENCODE_CONFIG_CONTENT` merge. Each gets the current executable, chat ID,
device ID, and engine IPC port. Title generation and model discovery do not
receive the server. The mock harness does not need MCP access. An externally
attached OpenCode server needs its own MCP configuration.

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
| `read_chat`        | `WatchDocMessages` opening `reset` frame, rendered        |
| `send_message`     | `QueueCommand` Run / Steer, or `QueueMessage`             |
| `wait_for_turn`    | `WatchSessions` until the chat settles                    |
| `interrupt_chat`   | `QueueCommand` Interrupt                                  |
| `respond_to_input` | `QueueCommand` RespondInput                               |
| `archive_chat`     | `Mutate setChatArchived`                                  |
| `list_ticket_boards` | `WatchTickets` + `WatchSpaces` snapshots                 |
| `list_tickets`       | `WatchTickets` snapshot with board/status/search filters |
| `get_ticket`         | Ticket, children, comments, linked and descendant chats  |
| `mutate_ticket`      | `MutateTicket` validated board, ticket, link, comment ops |

The ticket tools give agents the same native records as the UI. A board is a
logical planning project and may link to several device/folder Spaces. Tickets
form an Epic to Issue to Subissue tree. Chats are linked execution threads, and
their child chats retain their own parent hierarchy. `mutate_ticket` supports
create/update/delete for boards, tickets, and comments; Space and chat links;
and reparenting a ticket. Create operations mint UUIDs when omitted and return
them in the result. The engine validates references, parent cycles, and text
size. Use `list_ticket_boards` and `list_tickets` to obtain IDs before changing
existing records.

Watch streams are the engine's only read surface (there is no one-shot "get
transcript" RPC); a snapshot is "subscribe, take the first item, drop" — drop
cancels server-side, exactly what the sidebar does on attach.

`send_message` mode `auto` mirrors the composer: idle → `run`; working → `steer`
when the harness steers mid-turn (claude, codex), else a queue row held for the
end of the turn; `awaitingInput` → refuses and points at `respond_to_input`.
A working row older than 45 s is treated as idle (the UI's staleness window).

`wait_for_turn` after a send is edge-triggered on the `Session` row captured
before the send: it returns on a new `last_completed_turn`, an
`awaitingInput`/`errored` stamp newer than the baseline, or a working→idle
edge. A brand-new chat has no session row until the host picks the run up, so
the wait keeps waiting in that case rather than reporting the unstarted run as
done (this was the one bug the first live run found).

## Smoke recipe

```sh
BIN=target/debug/glitch-flow
{ echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}'
  echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"list_chats","arguments":{"limit":5}}}'
  sleep 5; } | GLITCH_FLOW_IPC_PORT=27655 $BIN mcp
```

`create_chat` with `"prompt": "Reply with exactly the word pong", "wait": true`
against a live daemon returns the assistant's `pong` in a few seconds; archive
the chat afterwards with `archive_chat`. Unit tests (`cargo test -p zeron-mcp`)
drive the whole tool set against an in-memory stub `RpcService`.
