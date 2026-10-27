# Version compatibility

Status: SHIPPED with chat3 (protocol 1) · Code: `crates/sync/src/compat.rs`, `edge/src/compat.ts`

Devices update at different times: a phone lags the App Store, a desktop sits on an old
build, a Cloud machine boots whatever image it has. Every client now says which **sync
protocol** it speaks, and the edge uses that to keep older builds out of what they can't
handle and to tell newer-but-not-new-enough builds to update.

## The protocol number

`zeron_sync::compat::PROTOCOL`. Every edge request carries it: `proto=` on socket URLs
(added in `dial::connect_ws`, so every dial has it) and the `x-zeron-proto` header on HTTP
(the default headers of the client's and the engine's edge HTTP clients). Builds from
before this existed send nothing; the edge reads that as **protocol 0**.

Bump it when a change needs every device that touches it to understand it, and decide
there what older protocols may still do.

| Protocol | Release | What changed |
|---|---|---|
| 0 | before chat3 | (announces nothing) |
| 1 | chat3 | gen-3 chats (docs/chat3-sync.md) |

## What protocol 0 builds get (the chat3 cut)

Protocol 0 builds read every chat generation ≥ 2 as chat2 and have no "update required"
handling, so they can't be told anything. They are kept away from what would break:

- **Updated devices read offline.** The registry room hides the presence of every device
  that announced a protocol from sockets and pulls that announced none
  (`RegistryRoom.presenceView`/`beat`). A device room tells a protocol-0 caller its host
  isn't connected (`/status`, which old engines probe when presence goes quiet) and
  refuses protocol-0 peer links to a protocol-aware host. Old builds treat an updated
  machine exactly like a switched-off one: sends to its chat2 chats queue in the room and
  are delivered (the room is shared), but nothing dials it.
- **A chat3 chat's old room refuses them.** Old builds dial `chat2/{id}` for a gen-3 chat.
  Its host marks that room moved (`POST /chat2/{id}/moved`, once per chat per process,
  retried until it lands), after which the room answers everything with a refusal and
  closes the sockets it had. Writes from an old build stay queued on that device instead
  of landing in a room no host reads.
- **Old hosts never get gen-3 chats.** A chat is born gen 3 only when the labs flag is on
  and its host advertises the `chat3-v1` capability (`RegistryDoc::new_chat_room_gen`).

## What protocol ≥ 1 builds do with a refusal

A refusal is HTTP 426 with `x-zeron-update-required`:

- `app`: the edge's minimum protocol is above ours, on every sync route (`registry`,
  `chat2`, `chat3`).
- `room`: this room needs a newer build (a moved room; a device room whose host is newer,
  for protocol 0 only).

Clients turn it into `SyncError::UpdateRequired`, stop redialing at their reconnect
cadence and retry every 30 minutes (`REFUSED_RETRY`; an operator can lower the minimum
again). `app` refusals latch `compat::app_update_required()` until a registry session
joins again.

What the user sees:

| | Desktop | iOS |
|---|---|---|
| A room refused this build, or the chat is on a generation this build doesn't know | Sidebar pill "Update Zeron to open this chat" (`ChatSyncState::UpdateRequired`) | First transcript row "This chat needs a newer version of Zeron…" |
| The edge refused this build | Pill "Update Zeron to keep syncing" (`Connectivity::update_required`) | The same first row in every chat |

Device protocols reach clients too (`RegistryClient::presence_protocols`), so a later
release can label a device that is online but newer than this build. No UI reads them
yet.

## Operator switch

`MIN_CLIENT_PROTOCOL` (Worker var, unset = 0) refuses older clients on the sync routes
with an `app` refusal. Only builds from protocol 1 on can show it; protocol-0 builds just
fail to connect. Raise it once a release that bumped `PROTOCOL` has spread, when the old
protocol must stop writing.
