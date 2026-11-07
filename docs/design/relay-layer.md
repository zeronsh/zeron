# Relay layer (chat generation 3)

Status: design, with a first implementation behind flags that default off. See
[What's built](#whats-built) for exactly which parts exist. Rows-per-push figures are in the PR for
`ChatRoom: write only the row on push`.

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
- Relaying doesn't remove local-first. See [Local-first](#local-first).

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

## Local-first

**Every client keeps storing chats locally. A device can load any chat it has loaded
before, up to what it has stored, with no network and with the host offline.** This
holds on generation 3 exactly as it does on generation 2: what changes is who else has a
copy, not whether this device keeps its own.

- Everything a device imports, by any path (direct link, the room's relay, the queue,
  catch-up from a peer, stored rows, a checkpoint), goes into its doc, and its doc is saved
  to its local store within about a second.
- That gap is closed for what only came by relay. Nothing on the edge can give those ops
  back, so each one is also written to a local journal (`chat_inbox`) as it's imported, and
  replayed when the chat next opens. A row in the journal is retired only once a later save
  covers it, so a crash or power loss right after a frame arrives loses nothing.
- A local change goes to the durable outbox first, as it always has. With no network, a sent
  message waits there and goes out on reconnect.
- A chat this device has never loaded, with its host offline, has nothing to show unless the
  room holds a checkpoint. That isn't data loss, and the app says so: "This chat lives on another
  device", with no spinner. Being caught up with a room that stores nothing doesn't count as
  loaded.

## Protocol

Frames extend the chat2 codec (`chat-frames.ts`, `chat_frames.rs`). The DO still
never parses a payload.

A client that wants the new layer adds `relay: 1` to `hello`, and says what it knows:
`gen: 3` if its chat row is on generation 3, and `relaying: 1` if it's the host and
relaying. A DO with the flag on answers with `relay: 1` and `legacyPeers: n` in `state`,
plus `relayDirty: 1` while it [remembers relaying](#the-rooms-memory-of-relaying). A DO with
the flag off ignores these, so the client stays on stored rows for that session. With the
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
| 0x14 | `delivered` | c→s | The host has saved these batch ids. Delete them. |
| 0x15 | `legacyPeer` | s→c | A reader that can't take relay frames is here. See [Old clients](#what-old-clients-see). |

Queue rows reach sockets as `relay` frames that also carry `queued` and `batchId`.

Over a direct link the same frames travel length-prefixed, and the link itself
identifies the sender.

### Reaching a host that isn't in the room

An idle host closes its chat socket, so a peer's `announce` can't reach it, and neither can
the `legacyPeer` signal. So a reader arriving (`hello` carrying a `hostDevice` hint) with the
host absent queues the same durable wake a sent message does, at most once a minute per
room. Only for a chat in the relay era: its `hello` said `gen: 3`, or the room remembers
relaying. A stored chat's readers read rows and need no host, so the edge flag alone wakes
nobody. A host keeps its socket for 15 s after a peer's last frame, so a peer
it just woke isn't cut off mid-sync.

A wake isn't done until the socket has answered and its queue read has come back. The HTTPS
bootstrap can finish a catch-up sooner, but it doesn't read the queue.

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

An export is one causally complete blob, split into fragments of at most 64 KiB
and sent 512 KiB per request. The requester imports only the complete blob and pulls
each window itself, which gives flow control on slow links and resume by `offset`.
The responder keeps a finished export for 60 s; if it has been dropped, the
requester starts again from its current VV.

Independently importable chunks would be simpler, but they can livelock: a chunk
that parks on missing dependencies leaves the requester's VV unchanged, so the
responder keeps resending it and never reaches the dependencies.

### A stored row that depends on relayed ops

Stored rows are still possible in generation 3: the HTTPS fallback, a crash upload, or an old
client's push. Such a row can depend on ops that were only ever relayed, and a reader that
missed those parks it. No checkpoint or row backfill can fill that gap, and treating it as
one would redial forever.

So with the layer on, a parked row gets one ordinary checkpoint read (it may be an old
cursor's gap, which a checkpoint does fill). After that the reader keeps its session and holds
its cursor below the row. Later rows still import, it asks peers for what they have, and it
re-reads from the cursor when something new arrives, at most every 5 s and backing off to
every 5 minutes. The cursor holding means a restart re-reads the row rather than losing it. With
the host away the reader stays where it is, which matches the product decision.

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
4. The DO deletes the row, but only for the device the registry says hosts the chat. A
   device's own idea of who hosts isn't enough: one that hasn't heard of a move could
   confirm a row the new host still needs, and that device might not come back. So the DO
   asks `RegistryRoom` (`/chat-host`), each time, and without an answer it keeps the row
   and redelivers. A `delivered` is rare enough for that: one per sent message at most. `delivered` names batch ids, not positions, so a late or repeated one
   can't delete a newer row. A device that becomes the host re-reads the queue, so what it
   read before can now be confirmed.

Devices that were offline get the content from the host afterwards through VV sync.

A queue row a build can't read is kept, not confirmed, so a newer build can read it later.

**The queue can't strand a message.** A sender retires its outbox on `queued`, so the room has
to keep the row reachable:

- When a reader that can't use the queue arrives or is connected, the DO moves queued rows into
  `rows` in one transaction and broadcasts them, unless the room
  [remembers relaying](#the-rooms-memory-of-relaying). Then they wait for the host's heal.
- When the edge flag is off and a room still has the table, the same happens on the next `hello`
  or read, under the same condition, and the table is dropped. That check is one schema read per
  object lifetime, and a room that never had the layer does nothing else.
- A device only queues if the chat's host advertises `chat-relay-v1`. A host back on an older build
  keeps the generation it wrote, but its device row stops saying it can read the queue.

A network that blocks WebSockets uses `POST /rows`, the stored path every client
already has. It stays supported in generation 3. A device counts as socketless only once its dials
keep failing while HTTPS works, which rules out a laptop asleep. Its reads then say so, and a host on
such a network stops relaying and goes back to stored rows.

## Cloud checkpoints

Cloud hosts relay their ops like any host and also store a checkpoint through the
existing `POST /checkpoint`:

- at turn end, approximated by 3 s without a local commit, at least 10 s apart;
- about every 60 s during a long turn;
- before sleep, with sleep refused unless the checkpoint lands within 20 s.

A Cloud host's socket stays open and its handle stays in memory until its checkpoint covers its doc.
A failed checkpoint backs off from 2 s to 5 minutes, because each try is a full export and upload.

These checkpoints pass `atFloor=1`, so the DO uses its own current floor and they prune
nothing. A device opening a sleeping Cloud chat gets the checkpoint from the DO, and
a message sent to it queues and wakes it, the same as any offline host.

Device-hosted chats post no checkpoints in generation 3 unless an old client needs
one, and they stop needing the nightly backup, since their rooms stop changing.

### Only a checkpoint that moves forward

A checkpoint can be all the room holds, so a relay-era one can't replace it with less. A host
posts one only after this process has seen its doc catch up with the room. The DO refuses one
with an empty version over an existing checkpoint, or one whose version doesn't include the
stored one (`409 stale`, after which the host reads what the room has). A version vector is the
only thing the DO decodes, and only to compare. A chat over the size limit gets a `413`, stops
being retried against the sleep gate, and logs: refusing to sleep would only keep the machine
billing, and no checkpoint will ever land.

## The room's memory of relaying

An old reader can't fill a gap that only relayed ops can fill. It parks the row, redials, reads
the checkpoint, parks it again, and does so for as long as the socket lives, downloading the whole
checkpoint each time. So the room must never hand it such a row.

A room marks itself `relayDirty` (one write, once) when a relaying host says hello or when it
forwards a relay frame. While that's set, ops may exist that no checkpoint holds, and the queue
isn't moved into rows for an old reader or a flag-off room. The mark clears when the host posts
its heal (`atFloor=1&heal=1`): the checkpoint exported after it stopped relaying. Whatever was held
back is stored then, and old sockets are closed so they rejoin and read the checkpoint.

With the host away, an old reader stays quiet on the last checkpoint. The heal happens when the
host returns. Current builds also stop redialling on their own when reading the same checkpoint
hasn't helped; shipped builds can't change, which is why the room has to do this.

## Who decides the generation

`roomGen` on the chat row decides it, and **only the chat's host writes it**. It's an
LWW field the registry already syncs, and every client reads it as `>= 2`, so 3 means
"chat2" to anything old.

The host may set 3 when all of these hold:

- the client flag is on and the DO answered `relay: 1`;
- every relevant device seen in the last 14 days advertises `chat-relay-v1` in its
  device row's `capabilities`. Relevant means every non-Cloud device on the account,
  the chat's host, and the host of its parent chat. Other Cloud machines never open
  this chat, so they don't count;
- no client without the layer has joined the account's registry room in the last 14 days;
- the chat's saved snapshot is under 24 MiB. A checkpoint has to be able to bring any reader up
  to date, and the room takes at most 32 MiB. A chat that grows past 24 MiB goes back to stored
  rows while there's still room for its heal.

"Seen" uses live registry presence as well as the stored `last_seen_at`, which only moves at
boot and shutdown.

**Phones write no device row**, so rows alone can't hold a chat back for them. The registry room
is the one place every client joins, and a client with the layer says so in its `hello`. The room
remembers each client that doesn't (`legacy_clients`: one write per client, then hourly at most) and
tells relay-capable clients the list on join and when it changes. A host that hasn't heard the list
yet doesn't flip anything.

Turn the edge flag on at least 14 days before any engine flag, or the list starts empty and a phone
that's been away flips nothing back until it returns.

**Abandoned devices can't block forever:** past 14 days unseen, a device stops
counting. If it comes back it's caught by the old-peer signal below, and its device
row updates on boot, which moves chats back to 2 as each one's host next evaluates it (there's no
sweep across chats that aren't open).

The capability is better than comparing `version`: it says exactly what's needed,
it's off on builds that have the flag off, and it can't be fooled by a version
string. `version` stays for display and for the rollout dashboards.

The host re-evaluates on join and every 10 s while the chat is open. Other devices just
follow `roomGen`. A viewer with a stale value still
behaves safely either way, because the host reads rows and queue alike and
re-publishes queue rows while it's storing.

## Downgrade, and the one rule that makes it safe

A host switches to stored mode when `roomGen` falls below 3, the DO stops offering
relay, or an old reader appears. **It always switches in this order:**

1. Set the mode to stored, so new commits go to rows.
2. Post a checkpoint exported after that point, at the current floor so it prunes nothing.
   It covers everything relayed so far.
3. Keep retrying until the checkpoint lands.
4. Only then, if the reason was a device that can't follow, write `roomGen: 2`.

Reversing steps 1 and 2 would be a bug: commits in the gap would be in neither the
checkpoint nor the rows, and rows after them would park on every old reader. A host owes
this checkpoint whenever it isn't relaying a chat on generation 3, even right after a
restart, because a restarted host can't know what it relayed before. Step 4 comes last so
that a crash leaves the chat on 3 and the next start owes the same checkpoint.

A room that hasn't answered can't start a downgrade. Between sessions a host keeps the
room's last answer, or a host offline for a day would upload a day of commits as stored rows
when it returned.

While storing, the host also stores whatever else it imports through this layer: exactly the new ops,
however many times they're delivered, for the same reason.

A checkpoint only settles this if its export began after the host stopped relaying. One already in
flight may lack the last relayed commits, so it doesn't count.

The hold for an old reader lasts 10 minutes and is renewed for as long as it stays: the DO answers each
of the host's announces with `legacyPeer` while such a reader is connected.

## What old clients see

| Stage | Old client's experience |
|---|---|
| Edge flag off, or client flag off | Nothing changes. Byte-identical. |
| Flag on, chat at generation 2 | Nothing changes. New clients add `relay: 1`, and the host stores as before. |
| Flip to 3 with no old devices seen in 14 days | No old client is around to see it. |
| Old client comes back to a generation 3 chat | Its `hello` or `GET /rows` makes the DO send `legacyPeer`, and wakes a host that isn't in the room (from `hello` only: an old HTTP read carries no host hint). The registry also tells every host, so chats move back as they're opened. The host posts a current checkpoint and stores while the old reader stays, plus 10 minutes. The DO then closes the old socket so it rejoins and reads the checkpoint. |
| Same, but the host is offline | It sees the last checkpoint, frozen, and stays quiet: the room holds back the queue so no row can park. It catches up when the host returns. This matches the product decision. |
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
| Host crashes mid-stream | Relayed commits stay in its local outbox until a saved snapshot covers them. Any left at restart go up as stored rows and read back, as they do today. Viewers holding newer ops serve them back through symmetric sync. |
| Host offline, viewer sends | Queued and acked. The wake waits for the host. Other devices see it through the queue. |
| Host offline, fresh device | Sees the last checkpoint if any, plus the queue. Accepted by product decision. |
| Host offline and device-hosted chat with no checkpoint | An empty chat until the host returns, on a device that has never loaded it. A device that has keeps what it has: see [Local-first](#local-first). |
| Direct link dies | A closed link falls back at once. A silent one takes up to 10 s to be declared dead, and frames sent in that time were kept off the room; `announce{hello}` then recovers them. The redial retries with backoff. |
| Both paths down | Local-first as today. Sync resumes on reconnect. |
| Viewer's `delivered` is lost | The row is redelivered next join. Import is a no-op. |
| Chat moves to a new host | The new host reads the queue and acks. Sync is symmetric, so it also collects what the old host published. |
| Edge flag turned off while hosts are in relay mode | `state` loses `relay`, the host follows the downgrade rule, and queued rows move into `rows` once its heal lands. |
| A checkpoint keeps failing | Retried with backoff. Until it lands, old readers stay behind, a downgrade can't finish and a Cloud machine won't sleep. A chat too big to checkpoint is the exception: it doesn't hold the socket or the sleep, and logs. Flips stop at 24 MiB so this should only be reachable by one huge update. |
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

A viewer showing a chat opens `chatlink/{chatId}` (engine: `doc_host/relay.rs`, protocol: `run_link` in
`crates/sync/src/chat_relay.rs`) to the host through
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

Vendor and secondary sources cluster around 75-85% of sessions connecting directly with
STUN. Fresh pairings measured on real networks (2026-11-06), on one account:

| Pair | Direct | Pairing p50 | RTT |
|---|---|---|---|
| Mac on home NAT ↔ public server | 60/60 | 1.1 s | 69 ms |
| Mac on home NAT ↔ Boat machine | 30/30 | 1.6 s | 152 ms |
| Public server ↔ Boat machine | 30/30 | 1.1 s | 79 ms |
| Mac ↔ Mac on one LAN (paired over Tailscale) | 30/30 | 0.7 s | 23 ms |
| Public server ↔ a laptop on a phone hotspot | 25/30 | 5.2 s | 540 ms |
| Home NAT ↔ the same hotspot | 26/30 | 5.8 s | 465 ms |
| Home NAT ↔ a simulated Linux NAT | 0/30 | | |

The hotspot had a weak signal, so those two rows may be worse than a typical cell connection.
The simulated NAT fails because a packet arriving first creates a conntrack entry and the NAT
moves the outbound port. Real Linux-based routers may do the same. A failed attempt costs a
30 s wait, and the chat stays on the room throughout. Measure per platform before any wider
rollout, because this is one account on a few networks.

### TURN

Not needed. The DO is the fallback, so a device that can't pair directly just stays on the
relay. For the record, Cloudflare's TURN lists at $0.05/GB with 1,000 GB free per month.

### Mobile and battery

The phone already runs this stack for terminals. A link is open only while a chat is on
screen, and iOS suspends it in the background anyway, at which point the phone reconnects
on return like any other socket. Each open link adds ICE and DTLS keepalives. Those are
small but not measured here. [relay-mobile.md](relay-mobile.md) covers what a phone does. iOS
runs the layer. Android doesn't yet, and until it does, an Android phone seen in the last 14 days
keeps its account's chats on generation 2.

### Cloud machines

A viewer always starts the pairing, and `PreviewRoom` already lets a Cloud machine answer
only devices that contacted it. Boat machines pair directly from a home NAT and from a public server (see above), so Cloud links
work on that provider. Two Cloud machines don't pair with each other, by design: neither has
contacted the other, and a link's host refuses Cloud peers. When a Cloud
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

Measured with a mock agent on real networks (2026-11-06), p50, direct against relay:

| Pair | Commit to viewer | First token after send | Steer round trip |
|---|---|---|---|
| Mac on home NAT ↔ public server | 30-42 ms / 52-54 ms | 236-283 ms / 332-382 ms | 211-244 ms / 322-336 ms |
| Mac ↔ Mac on one LAN (over Tailscale) | not measured | 218-222 ms / 350-357 ms | 166-228 ms / 347-363 ms |
| Mac on home NAT ↔ Boat machine | not measured | 281-300 ms / 328-339 ms | 296-309 ms / 335-342 ms |
| Public server ↔ hotspot, weak signal | not measured | 664-699 ms / 566-763 ms | 574 ms / 639-715 ms |

Streaming text arrives in 120 ms commits, which is about the size of a token's whole wait (p50 94-111 ms
direct, 118-125 ms relay on the first row), so a 10-20 ms saving while streaming won't be seen. A send or
steer saves 40-130 ms on the good networks, and that's where a person could feel it. On a slow radio
link the saving disappears. These round trips also include the rest of a turn's start, which a real agent
makes much longer.

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

Done on a deployed test worker with real engines on real networks (2026-11-06): pairing and latency
per the tables above, link and network failures with recovery, restarts of every role, the room dropped,
released v0.2.137 and v0.2.116 in the account, the offline queue including a host change, local-first after
SIGKILL, a real Cloud machine, and chaos with real `gpt-6.1-terra` turns.

Still needed, in this order:

1. The owner's own devices with the flags on for a day of real work.
2. Airplane mode and laptop sleep on a real laptop, which these runs simulated on servers.
3. The 1.2 Mbps and 2 KiB/s profiles from `docs/transport-reliability.md` for a late joiner of a
   large chat over the relay, with no HTTP checkpoint to fall back on.
4. Real iOS and Android builds from the App Store against a generation 3 chat. The iOS
   simulator has been run with the code from `main`; Android hasn't.
5. Direct-link success rate per platform, from real traffic.

## Rollout

1. Ship the code with flags off.
2. Turn on the edge flag on staging, then production. Stored chats behave as before; the registry
   starts remembering clients without the layer. Leave it for 14 days.
3. Turn on the engine flag for the owner's devices.
4. Ship the capability to desktop and mobile, turned on by default in a release. Chats flip as
   old versions age out of the 14-day window.
5. Only after the fleet has settled, consider removing old rows and backups for
   device-hosted chats.

## Rollback

The order matters. A host on generation 3 has relayed ops nothing else stores, and a host on a build
that can't heal will never store them.

1. Turn the edge flag off. Each host, as it opens a chat, sees the room no longer offers the layer,
   stores a heal, and moves the chat back. Queued rows go into `rows` when that heal lands.
2. Wait. A chat only heals when its host opens it, and there's no sweep, so give it as long as devices
   take to come back. `GET /chat2/{id}/stats` shows `relayDirty` and `relayQueue` for a chat.
3. Only then turn the engine flag off, if you want it off. A build from this PR on with the flag off
   still owes a heal for a chat it hosts on generation 3, and moves it back after. That's the only thing
   the flag-off path does, and an account that never ran the layer has no such chat.
4. **Never go below this PR's code while a room may hold a queue or a chat may be unhealed.** Older edge
   code can't read the queue, and those senders have retired their outboxes. Older engine code can't
   heal, so its next stored row parks on every reader. Keep this PR's edge and engine code deployed
   for at least 14 days after step 1.

## What's built

Behind `CHAT_RELAY_LAYER=1` on the edge and `ZERON_RELAY_LAYER=1` in the engine. Both default off.

**Built and tested:**

- The room's relay frames, offline queue and old-client signals (`edge/src/chat-room.ts`, `chat-relay.ts`).
- The relay task: version exchange, resumable transfers, parked rows (`crates/sync/src/chat_relay.rs`,
  `chat_client.rs`).
- Direct links on the existing WebRTC stack, with fallback to the room.
- The host's side: moving a chat between generations, the downgrade rule, the outbox rules
  (`crates/engine/src/doc_host/relay.rs`, `crates/proto/src/relay_gen.rs`).

**Built, but only covered by reading and unit-level tests:**

- A host on a network with HTTPS but no sockets going back to stored rows.

**Not built:**

- Android. `crates/client` has the layer and iOS runs it ([relay-mobile.md](relay-mobile.md)). Android has
  yet to turn it on, and a phone without it seen in the last 14 days holds its account's chats on generation 2.
- Sidebar churn.
- Removing old rows, or backups for device-hosted chats. The nightly backup still runs for a room whose
  rows change, which a generation 3 room's mostly don't.
- A sweep that re-evaluates chats that aren't open. Each host evaluates a chat when it opens it.

**Known limits:**

- A viewer on a chat whose host has no socket, no wake route and no direct link stays where it is.
- An old client reading only over HTTP doesn't wake an absent host: its read carries no host hint.

## Sources

- TURN usage estimates: [metered.ca](https://www.metered.ca/blog/what-is-a-turn-server/), [getstream.io](https://getstream.io/resources/projects/webrtc/advanced/stun-turn/), [bloggeek.me](https://bloggeek.me/webrtcglossary/turn/)
- TURN pricing: [Cloudflare Realtime TURN FAQ](https://developers.cloudflare.com/realtime/turn/faq/), [Cloudflare Realtime pricing](https://developers.cloudflare.com/realtime/sfu/pricing/)
