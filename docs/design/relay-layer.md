# Relay layer (chat generation 3)

Status: design, with a first implementation behind flags that default off. See
[What's built](#whats-built) for exactly which parts exist.

## The goal

Today every update to a chat is stored in that chat's Durable Object (DO). The
owner has decided:

- The relay just relays. Chats hosted on a user's own device aren't stored on
  the server.
- Cloud-hosted chats do store, so a chat can be loaded while its machine sleeps.
- No backups for non-Cloud chats. With a laptop off, other devices see only what
  they already synced.
- The sidebar stays stored, with less churn.
- Devices connect peer to peer over WebRTC where they can, for latency. The DO
  stays as the fallback and the offline queue.
- Nothing ships unless old clients keep working.

## The idea in one paragraph

**Delivery stops being a guarantee and becomes a hint.** A chat's ops reach other
devices however they can: a direct link, the DO's relay, or the stored queue.
None of those has to be reliable, because Loro imports are idempotent and devices
compare version vectors (VVs) to find and fetch whatever they missed. Loss, reorder
and duplicates only cost time. The one thing that must stay durable is what only
one device holds and another is waiting for: a message sent to a host that's
offline. That goes in a small queue on the DO until the host confirms it has saved
it.

## Roles

| Device | Its own ops go | Why |
|---|---|---|
| Host, generation 3 | Direct links and DO relay. Not stored on the edge. | The host's doc is the source of truth. Anyone behind asks it. |
| Other devices, generation 3 | Durable queue on the DO, plus a direct copy to the host | A sent message must survive the host being offline. The copy is for latency. |
| Anyone, generation 2 | Stored rows, as today | Unchanged. |

## Protocol

Frames extend the chat2 codec (`chat-frames.ts`, `chat_frames.rs`). The DO still
never parses a payload.

A client that wants the new layer adds `relay: 1` to `hello`. A DO with the flag on
answers with `relay: 1` and `legacyPeers: n` in `state`. A DO with the flag off
ignores the field, so the client stays on stored rows for that session. With the
client flag off, `hello` is byte-identical to today's.

| Type | Frame | Direction | What it does |
|---|---|---|---|
| 0x0c | `relay` | both | One update. Forwarded to live relay-capable sockets except the sender and `skip`. Never stored. |
| 0x0d | `announce` | both | Payload is the sender's VV. `hello: true` asks peers to answer if the sender is behind. |
| 0x0e | `syncReq` | both | Directed by `to`. Payload is the requester's VV, or `xfer` and `offset` to continue a transfer. |
| 0x0f | `syncData` | both | Directed by `to`. A fragment of one update blob: `xfer`, `offset`, `total`. |
| 0x10 | `enqueue` | c→s | One durable update for the offline queue. |
| 0x11 | `queued` | s→c | The ack: the row is durable. |
| 0x12 | `queueReq` | c→s | Send me the queue. |
| 0x13 | `queueDone` | s→c | End of the queue backlog. |
| 0x14 | `delivered` | c→s | The host has saved these queue ids. Delete them. |
| 0x15 | `legacyPeer` | s→c | A reader that can't take relay frames is here. See [Old clients](#what-old-clients-see). |

Queue rows reach sockets as `relay` frames that also carry `queueId`.

Over a direct link the same frames travel length-prefixed, and the link itself
identifies the sender.

### Catch-up for a late joiner

1. `hello`, then `state`. If the room holds a checkpoint the device's doc doesn't
   contain, fetch it over HTTP, then read any stored rows. This is today's path, and
   it means existing rows still seed readers.
2. `queueReq`: read what's waiting in the queue.
3. `announce{hello}`. Each peer that has ops the joiner lacks answers with its own
   `announce`.
4. The joiner sends one `syncReq` to one peer, preferring the host. The peer exports
   `updates(since joiner's VV)`, or a full snapshot if the joiner's VV is empty.
5. The joiner imports, then compares against other peers' announced VVs and repeats
   if anyone still has more.

An export is one causally complete blob, split into fragments of at most 256 KiB
and sent 1 MiB per request. The requester imports only the complete blob and pulls
each window itself, which gives flow control on slow links and resume by `offset`.
The responder keeps a finished export for 60 s; if it has been dropped, the
requester starts again from its current VV.

Independently importable chunks would be simpler, but they can livelock: a chunk
that parks on missing dependencies leaves the requester's VV unchanged, so the
responder keeps resending it and never reaches the dependencies.

### Staying converged

- A `relay` frame that parks on missing history schedules a `syncReq` to its sender
  750 ms later, which lets a late frame arrive first.
- After a local change a device announces at +2 s and again at +30 s, then goes
  quiet. A lost tail recovers in about 2 s, and an idle chat sends nothing, so idle
  DOs still hibernate.
- Every reconnect sends `announce{hello}`.
- Sync is symmetric. A host that lost ops in a crash gets them back from whichever
  viewer still has them.

## Offline queue

The queue is a separate table, `relay_queue`, not `rows`.

Deleting rows in the middle of `rows` would leave holes in the seq space, and an old
client treats a hole as a gap to repair and redials forever. With a separate table
the old log stays a valid, quiet log, and deletes can't touch existing rows.

1. A viewer sends `enqueue`. The row and its host wake commit in one transaction,
   then `queued` goes back. The viewer's outbox retires the batch only then.
2. The row goes live to connected relay-capable sockets. An offline host learns of
   it through the existing wake path.
3. The host imports it, saves its snapshot, and only then sends `delivered`. A crash
   in between redelivers the row, and importing it again is a no-op.
4. The DO deletes the row. It refuses a `delivered` from the row's own author, and
   the client only sends one from the host.

Devices that were offline get the content from the host afterwards through VV sync.

A network that blocks WebSockets uses `POST /rows`, the stored path every client
already has. It stays fully supported in generation 3.

## Cloud checkpoints

Cloud hosts relay their ops like any host and also store a checkpoint through the
existing `POST /checkpoint`:

- at turn end, approximated by 3 s without a local commit, at least 10 s apart;
- about every 60 s during a long turn;
- before sleep, with sleep waiting for the checkpoint to land.

These checkpoints pass `seqCovered` equal to the current floor, so they prune
nothing. A device opening a sleeping Cloud chat gets the checkpoint from the DO, and
a message sent to it queues and wakes it, the same as any offline host.

Device-hosted chats post no checkpoints in generation 3 unless an old client needs
one, and they stop needing the nightly backup, since their rooms stop changing.

## Who decides the generation

`roomGen` on the chat row decides it, and **only the chat's host writes it**. It's an
LWW field the registry already syncs, and every client reads it as `>= 2`, so 3 means
"chat2" to anything old.

The host may set 3 when all of these hold:

- the client flag is on and the DO answered `relay: 1`;
- every relevant device seen in the last 14 days advertises `chat-relay-v1` in its
  device row's `capabilities`. Relevant means every non-Cloud device on the account,
  the chat's host, and the host of its parent chat. Other Cloud machines never open
  this chat, so they don't count.

**Abandoned devices can't block forever:** past 14 days unseen, a device stops
counting. If it comes back it's caught by the old-peer signal below, and its device
row updates on boot, which moves chats back to 2.

The capability is better than comparing `version`: it says exactly what's needed,
it's off on builds that have the flag off, and it can't be fooled by a version
string. `version` stays for display and for the rollout dashboards.

The host re-evaluates on join, every 30 s while the chat is open, and on a device
row change. Other devices just follow `roomGen`. A viewer with a stale value still
behaves safely either way, because the host reads rows and queue alike and
re-publishes queue rows while it's storing.

## Downgrade, and the one rule that makes it safe

A host switches to stored mode when `roomGen` falls below 3, the DO stops offering
relay, or an old reader appears. **It always switches in this order:**

1. Set the mode to stored, so new commits go to rows.
2. Post a checkpoint exported after that point. It covers everything relayed so far.
3. Keep retrying until the checkpoint lands.

Reversing steps 1 and 2 would be a bug: commits in the gap would be in neither the
checkpoint nor the rows, and rows after them would park on every old reader. Tests
pin this order.

While storing, the host also re-publishes queue rows it receives as stored rows, for
the same reason.

## What old clients see

| Stage | Old client's experience |
|---|---|
| Edge flag off, or client flag off | Nothing changes. Byte-identical. |
| Flag on, chat at generation 2 | Nothing changes. New clients add `relay: 1`, and the host stores as before. |
| Flip to 3 with no old devices seen in 14 days | No old client is around to see it. |
| Old client comes back to a generation 3 chat | Its `hello` or `GET /rows` makes the DO send `legacyPeer`. The host downgrades within seconds: a current checkpoint, then stored rows while the old reader stays around, plus 10 minutes. The old client catches up normally. |
| Same, but the host is offline | It sees the last checkpoint, frozen, and stays converged on what it has. It catches up when the host returns. This matches the product decision. |
| Old client sends a message | `push` is stored and relayed, and the wake goes out. The host imports it. This always worked and still does. |
| New client on a network without WebSockets | Treated like an old reader: its fallback `GET /rows` carries `fallback=1`. |

Old clients don't receive any new frame type. The DO only sends those to sockets that
said `relay: 1`.

`roomGen: 3` is safe for all shipped readers: desktop, iOS and Android read it through
`room_gen >= 2` and the Rust client. iOS and Android apps have no separate check.

## Migrating existing rows

There's no migration, and nothing is deleted. Existing rows and checkpoints stay where
they are, keep seeding late joiners, and are served to anyone who asks. A flip writes
one chat row. Once a chat is on 3 its log simply stops growing. Removing old rows is a
separate, later decision with its own review.

## Failure modes

| Failure | What happens |
|---|---|
| Relay frame lost | Trailing `announce` shows the gap, and `syncReq` fills it. About 2 s. |
| Frames reordered | Loro parks them. A `syncReq` fills any real gap. |
| DO restarts mid-stream | Sockets drop, clients reconnect with `announce{hello}`. Nothing was held in the DO's memory except unsent relay frames. |
| Host crashes mid-stream | Commits since its last snapshot stay in its local outbox, go up as stored rows on restart, and read back as they do today. Viewers holding newer ops serve them back through symmetric sync. |
| Host offline, viewer sends | Queued and acked. The wake waits for the host. Other devices see it through the queue. |
| Host offline, fresh device | Sees the last checkpoint if any, plus the queue. Accepted by product decision. |
| Host offline and device-hosted chat with no checkpoint | An empty chat until the host returns. Accepted, and the riskiest visible behaviour: see below. |
| Direct link dies | Sends fall back to the DO at once. The periodic redial retries with backoff. |
| Both paths down | Local-first as today. Sync resumes on reconnect. |
| Viewer's `delivered` is lost | The row is redelivered next join. Import is a no-op. |
| Chat moves to a new host | The new host reads the queue and acks. Sync is symmetric, so it also collects what the old host published. |
| Edge flag turned off while hosts are in relay mode | `state` loses `relay`, the host follows the downgrade rule. |
| Oversized update | The existing row cap and rejection path still apply to stored paths. A `relay` or `syncData` frame over the cap is refused by the runtime; VV sync re-fragments it. |

## WebRTC

### What exists and why the old stack went away

Two different things have the name. `ARCHITECTURE.md` (line 300) and
`docs/research/feature-inventory.md` say the WebRTC mesh for device control was replaced
by the DeviceRoom relay, and that the predecessor app had already made that move before this
repo began. **Neither records why**, and the commit history here doesn't either, so the
reasons can't be recovered from this repo. Nothing from that stack remains.

What's in the repo today is newer and works: `crates/preview` is a `webrtc`-based
stack with a stream multiplexer (`mux`), signalling over the `PreviewRoom` DO, and STUN
through public servers with no TURN. Terminals on other devices use it through
`crates/term-link`, on desktop and on phones (`zeron-mobile` runs that code), and fall
back to the device relay mid-session with no gap. **We don't build a stack; we add a
stream service to one that already ships.**

### Design

A viewer showing a chat opens `chatlink/{chatId}` to the host through
`PreviewService::open_peer_stream`, which pairs if needed. The host's handler accepts
only chats it hosts, and only from the owner's non-Cloud devices. The link carries
`relay`, `announce`, `syncReq` and `syncData`.

Sending a host op: down every open link, then to the DO with `skip` naming those
devices, so each device gets one copy and a joiner in a race gets it from somewhere.
Sending a viewer op: down the link to the host for latency, plus `enqueue` for
durability. Whichever arrives second is a no-op.

Links are opened only for chats on screen, not for all warm chats, so background chats
use no extra keepalive traffic.

### Signalling

Already solved. SDP goes through `PreviewRoom`, which authenticates the user and limits
each Cloud machine to answering devices that contacted it.

### How often it will fail

There's no good primary measurement. Vendor and secondary sources cluster around 75-85%
of sessions connecting directly with STUN and 15-25% needing TURN, with more failures
on corporate and cellular networks. Treat that as a rough estimate. Our own traffic is
the real source of truth, and direct-link success should be measured per platform before
any wider rollout.

### TURN

Not needed. The DO is the fallback, so a device that can't pair directly just stays on the
relay. For the record, Cloudflare's TURN lists at $0.05/GB with 1,000 GB free per month.

### Mobile and battery

The phone already runs this stack for terminals. A link is open only while a chat is on
screen, and iOS suspends it in the background anyway, at which point the phone reconnects
on return like any other socket. Each open link adds ICE and DTLS keepalives. Those are
small but not measured here. Phones aren't part of this first implementation: the sink
hooks are defaulted so `crates/client` still builds unchanged, and a phone seen in the last
14 days keeps its account's chats on generation 2 until mobile ships this.

### Cloud machines

A viewer always starts the pairing, and `PreviewRoom` already lets a Cloud machine answer
only devices that contacted it. Whether the sandbox provider's network lets hole punching
succeed is unknown: it hasn't been tested on a real sandbox. If it doesn't, Cloud links stay
on the DO, so Cloud P2P is unproven even though the code path is the same. When a Cloud
machine sleeps the link ends, and reads then come from its checkpoint.

### Latency

The direct path skips the trip through the DO's region.

- **Relay path:** device → nearest Cloudflare location → the DO's region → nearest
  location → device. A DO lives in one region, so two devices in the same room pay for
  that trip even when they sit side by side.
- **Direct path, same LAN:** a low-single-digit millisecond path, and the most likely
  case for a laptop and a desktop or phone at home.
- **Direct path, different networks:** the internet path between the two, usually
  shorter than going through the DO's region, though not always.

The savings have **not** been measured on a real network. The loopback test in
`crates/engine/tests/chat_link_p2p.rs` shows the link adds no holdup of its own, which
isn't the same as measuring a WAN saving. Before a rollout, compare `relay` against direct
one-way delay between two real networks. Streaming text arrives in 120 ms commits, so
savings smaller than that are hard to see while streaming. Where it should be felt is
the first token after send, and typing a steer into a running agent.

### Is it sound?

Yes, for this reason: **the direct path carries the same idempotent frames as the relay
and is never the only copy of anything durable.** It can fail at any time, in any order,
and the worst outcome is the delay until the next `announce`. The design doesn't depend
on how reliable it is.

## Sidebar churn

Not implemented here. RegistryRoom has the same per-batch bookkeeping ChatRoom had
(`seq`, `pushOutcomes`, `backupDirty`), so the same fix applies: derive or defer them. An
update also rewrites the `rows_seq` index. That's the cheap, safe first step.

Moving fields to live-only frames is riskier, for these reasons:

- `lastMessagePreview` and `lastMessageAt` sort the sidebar and mark unseen chats, and a
  device must see them on reconnect, so they can't be purely live. They can be written at
  turn end instead of on every change, with a live frame for the in-between.
- Session status flips drive push notifications in `RegistryRoom` (`notificationFor`).
  Terminal transitions must stay stored or a phone misses its notification. "Working"
  ticks can go live-only.
- Runner filtering applies to any new frame, so live frames need the same visibility checks.

## Testing before any real user

Done locally, see the PR for counts: flag-off identity, DO relay and queue behaviour against
real workerd, convergence (two devices plus host, late joiner, host offline, reconnect,
reordered delivery, an old client in the mix) with real Loro docs, and a real WebRTC pair
on loopback.

Still needed, in this order:

1. Two engines and real rooms on `wrangler dev`, with the flags on, for a day of real work.
2. Staging edge, with a fleet of the owner's own devices: one old build in the mix on purpose.
3. A real long-running agent turn with disconnects: airplane mode, sleep, Wi-Fi to cellular.
4. The 1.2 Mbps and 2 KiB/s profiles from `docs/transport-reliability.md` for a late joiner of a
   large chat over the relay, with no HTTP checkpoint to fall back on.
5. A full 30-chat warm set, checking DO message counts, CPU and battery against today.
6. Direct-link success rate and one-way delay per platform and network.
7. A real Cloud machine: checkpoint cadence, sleep, wake on a queued message, and whether P2P
   pairs at all.
8. Old iOS and Android builds against a generation 3 chat, end to end.
9. Rollback: turn the edge flag off with hosts mid-turn, and confirm that readers converge.

## Rollout

1. Ship the code with flags off.
2. Turn on the edge flag on staging, then production. Nothing changes until a client asks.
3. Turn on the engine flag for the owner's devices.
4. Ship the capability to desktop and mobile, turned on by default in a release. Chats flip as
   old versions age out of the 14-day window.
5. Only after the fleet has settled, consider removing old rows and backups for
   device-hosted chats.

## What's built

See the PR for the current state. Anything listed here as a rule of the design but not listed
in the PR as implemented isn't implemented.

## Sources

- TURN usage estimates: [metered.ca](https://www.metered.ca/blog/what-is-a-turn-server/), [getstream.io](https://getstream.io/resources/projects/webrtc/advanced/stun-turn/), [bloggeek.me](https://bloggeek.me/webrtcglossary/turn/)
- TURN pricing: [Cloudflare Realtime TURN FAQ](https://developers.cloudflare.com/realtime/turn/faq/), [Cloudflare Realtime pricing](https://developers.cloudflare.com/realtime/sfu/pricing/)
