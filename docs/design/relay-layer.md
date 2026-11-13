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
- The DO relays and holds the offline queue. Direct peer-to-peer links are parked: see
  [Direct links, parked](#direct-links-parked).
- Nothing ships unless old clients keep working.
- Relaying doesn't remove local-first. See [Local-first](#local-first).

## The idea in one paragraph

**Delivery stops being a guarantee and becomes a hint.** A chat's ops reach other
devices through the DO's relay or the stored queue.
Neither has to be reliable, because Loro imports are idempotent and devices
compare version vectors (VVs) to find and fetch whatever they missed. Loss, reorder
and duplicates only cost time. The one thing that must stay durable is what only
one device holds and another is waiting for: a message sent to a host that's
offline. That goes in a small queue on the DO until the host confirms it has saved
it.

## Roles

| Device | Its own ops go | Why |
|---|---|---|
| Host, generation 3 | DO relay. Not stored on the edge. | The host's doc is the source of truth. Anyone behind asks it. |
| Other devices, generation 3 | Durable queue on the DO | A sent message must survive the host being offline. |
| Anyone, generation 2 | Stored rows, as today | Unchanged. |

## Local-first

**Every client keeps storing chats locally. A device can load any chat it has loaded
before, up to what it has stored, with no network and with the host offline.** This
holds on generation 3 exactly as it does on generation 2: what changes is who else has a
copy, not whether this device keeps its own.

- Everything a device imports, by any path (the room's relay, the queue,
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
| 0x0c | `relay` | both | One update. Forwarded to live relay-capable sockets except the sender. Never stored. |
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

1. A viewer sends `enqueue`. Connected relay-capable sockets get a copy at once, before the
   write: anything a room sends once a write has begun waits for that write to be durable, and
   that wait sat between every send and its host. The copy is only a hint, like any relay frame.
2. The row and its host wake commit in one transaction, then `queued` goes back. The viewer's
   outbox retires the batch only then. An offline host learns of the row through the existing
   wake path. A write that never lands leaves the sender retrying, and a host that already
   imported the early copy imports it again as a no-op.
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

A checkpoint can be all the room holds, so while the layer is on no checkpoint can replace it
with less, relay-era or not: an ordinary one, from a device that isn't the host or from an older
build, is behind the host's as often as not. On generation 3 only the chat's host posts one at
all. A host posts one only after this process has seen its doc catch up with the room. The DO
refuses one with an empty version over an existing checkpoint, or one whose version doesn't
include the stored one (`409 stale`, after which the host reads what the room has). A version vector is the
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

- the engine runs the layer and the DO answered `relay: 1`;
- the chat's snapshot is known to be under 24 MiB: the larger of what was loaded and what the
  last save wrote. A chat opened from disk is known from its load; one with neither waits. A checkpoint has to be able to bring any reader up
  to date, and the room takes at most 32 MiB. A chat that grows past 24 MiB goes back to stored
  rows while there's still room for its heal.

**Other devices don't hold a chat back.** A client without the layer may be on the account, offline
or active. It's invisible to it either way: it reads `roomGen` as `>= 2`, and when it opens a chat
the room tells the host, and the host stores for as long as it's there (see
[Downgrade](#downgrade-and-the-one-rule-that-makes-it-safe)). That reader sees what it always has, a
checkpoint and rows. So storage is paid per chat, only while an old client has that chat open, and
it ends by itself as old versions go away.

Rejected: holding a whole account on stored rows while any device without the layer had been seen in
14 days. One old app kept every chat storing, and a rollout had to wait out the window. Also rejected:
having the room number relayed frames as rows for old readers, with nothing stored. The room can't
read a frame's dependencies, so it can't promise an old reader a row it can apply, and a shipped
reader that gets one it can't redials and re-downloads the checkpoint in a loop (see
[The room's memory of relaying](#the-rooms-memory-of-relaying)).

`chat-relay-v1` in a device row says the host reads the queue, which is what a sender checks before
using it.

The host re-evaluates on join and every 10 s while the chat is open. Other devices just
follow `roomGen`. A viewer with a stale value still
behaves safely either way, because the host reads rows and queue alike and
re-publishes queue rows while it's storing.

## Downgrade, and the one rule that makes it safe

A host switches to stored mode when `roomGen` falls below 3, the DO stops offering
relay, or an old reader appears, in which case it stores for 10 minutes after last hearing of one and
then relays again, with `roomGen` still 3. **It always switches in this order:**

1. Set the mode to stored, so new commits go to rows.
2. Post a checkpoint exported after that point, at the current floor so it prunes nothing.
   It covers everything relayed so far.
3. Keep retrying until the checkpoint lands.
4. Only then, if the reason was a chat too big to relay, write `roomGen: 2`.

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
| Flip to 3 with an old client on the account | It reads `roomGen` as `>= 2`. If it opens the chat, the host stores while it's there. |
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
| Room unreachable | Local-first as today. Sync resumes on reconnect. |
| Viewer's `delivered` is lost | The row is redelivered next join. Import is a no-op. |
| Chat moves to a new host | The new host reads the queue and acks. Sync is symmetric, so it also collects what the old host published. |
| Edge flag turned off while hosts are in relay mode | `state` loses `relay`, the host follows the downgrade rule, and queued rows move into `rows` once its heal lands. |
| A checkpoint keeps failing | Retried with backoff. Until it lands, old readers stay behind, a downgrade can't finish and a Cloud machine won't sleep. A chat too big to checkpoint is the exception: it doesn't hold the socket or the sleep, and logs. A snapshot over the room's 32 MiB isn't sent at all (the room only refuses it after receiving all of it), and once the room has refused a chat's checkpoint as too big its relay checkpoints stop. Flips stop at 24 MiB so this should only be reachable by one huge update. |
| Oversized update | The existing row cap and rejection path still apply to stored paths. A `relay` or `syncData` frame over the cap is refused by the runtime; VV sync re-fragments it. |

## Latency

Everything goes through the DO, so a send pays device → DO → host, and the reply host → DO →
device. Measured with real engines (2026-11-07): a Mac on a home network viewing a chat hosted on
a server in another region, mock agent, both on a test worker, p50:

| | Before | After |
|---|---|---|
| Send to first token on the viewer (n=30) | 257 ms | 172 ms |
| The same after 15 s idle (n=8) | 299 ms | 163 ms |
| Steer round trip (n=25) | 313 ms | 195 ms |
| Viewer send to host receive (n=39) | 90 ms | 46 ms |
| Host publish to viewer receive, every frame while streaming (n=770 / 902) | 50 ms | 49 ms |

Two changes made the difference:

- **The room forwards a queued send before writing it** (see [Offline queue](#offline-queue)). That halved the
  viewer-to-host leg.
- **The first change after a pause commits at once.** A turn's text was committed a window (120 ms)
  after its first change, so a turn's first token and a steer's echo waited that long. Now a change
  commits at once unless one went out less than a window ago, and then a window after it. Streaming
  still commits at most once per window, so a long turn costs the same.

What's left of a send to first token, by hop (n=39, p50): 2 ms to the viewer's socket, 46 ms through
the room to the host, 17 ms to start the turn, 26 ms to the agent's first token, 2 ms to commit, 26 ms
through the room to the viewer. The DO lives in one region, so the two room legs are about one
device-to-region round trip; nothing in the room's own handling adds measurably to that. A hibernated
room showed no extra cost: after 15 s idle the first token came no later than on a warm room.

## Direct links, parked

Peer-to-peer links carried the same frames over the WebRTC stack in `crates/preview`. They worked,
and they saved 40-130 ms on a send or steer, but the owner parked them to ship the relay alone and
make it as good as it can be. Nothing here waits on a link or prefers one. The code is on the branch
`zeron/relay-direct-link`, ready to come back: a link is only ever a second path for frames the relay
already carries, so it can return without changing anything durable.

What those runs left behind for the stack terminals use is on its own: a connection now outlives a
signaling socket ending, and a device ignores another's private LAN addresses when their public
addresses differ.

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

Done on a deployed test worker with real engines on real networks (2026-11-06 and 07): latency per
the table above, network failures with recovery, restarts of every role, the room dropped,
released v0.2.137 and v0.2.116 in the account, the offline queue including a host change, local-first after
SIGKILL, a real Cloud machine, and chaos with real `gpt-6.1-terra` turns.

Still needed, in this order:

1. The owner's own devices with the flags on for a day of real work.
2. Airplane mode and laptop sleep on a real laptop, which these runs simulated on servers.
3. The 1.2 Mbps and 2 KiB/s profiles from `docs/transport-reliability.md` for a late joiner of a
   large chat over the relay, with no HTTP checkpoint to fall back on.
4. Real iOS and Android builds from the App Store against a generation 3 chat. The iOS
   simulator has been run with the code from `main`; Android hasn't.

## Rollout

1. Ship the code. Every engine and app runs the layer, and with the edge switch off a room offers it to
   nobody, so everything stays on stored rows exactly as before.
2. Set `CHAT_RELAY_LAYER` to the owner's user id on staging, then production. Their chats move to
   generation 3 as their hosts open them.
3. Add testers' ids, then `1` for everyone.
4. Only after the fleet has settled, consider removing old rows and backups for
   device-hosted chats.

## Rollback

The order matters. A host on generation 3 has relayed ops nothing else stores, and a host on a build
that can't heal will never store them.

1. Turn the edge switch off, or take the user off its list. Each host, as it opens a chat, sees the room no longer offers the layer,
   stores a heal, and moves the chat back. Queued rows go into `rows` when that heal lands.
2. Wait. A chat only heals when its host opens it, and there's no sweep, so give it as long as devices
   take to come back. `GET /chat2/{id}/stats` shows `relayDirty` and `relayQueue` for a chat.
3. `ZERON_RELAY_LAYER=0` turns the layer off in one engine, as a kill switch. A build from this PR on with it
   off still owes a heal for a chat it hosts on generation 3, and moves it back after. That's the only thing
   the off path does, and an account that never ran the layer has no such chat.
4. **Never go below this PR's edge code while a room may hold a queue or a chat may be unhealed.** Older
   edge code can't read the queue, and those senders have retired their outboxes. Keep it deployed for at
   least 14 days after step 1.
5. An engine can go back further than that, one device at a time: an app downgrade. A host on an older
   build can't heal, so the room would keep what's queued for good. Instead, when a device without the
   layer joins and the registry says it hosts the chat, the room moves the queue into rows for it: the
   host holds every op those messages depend on. Its next stored rows still park on old readers until
   it upgrades and heals, as they would have anyway.

## What's built

Behind `CHAT_RELAY_LAYER` on the edge: `1` for everyone, or a comma-separated list of user ids. It
defaults off. Engines run the layer against every edge unless `ZERON_RELAY_LAYER` is set to anything but `1`,
and follow what the room answers: a room that doesn't offer it (an edge without this code, or with its switch off)
leaves every chat on stored rows, exactly as before (`relay_layer_on` in `crates/engine/src/doc_host/relay.rs`).

**Built and tested:**

- The room's relay frames, offline queue and old-client signals (`edge/src/chat-room.ts`, `chat-relay.ts`).
- The relay task: version exchange, resumable transfers, parked rows (`crates/sync/src/chat_relay.rs`,
  `chat_client.rs`).
- The host's side: moving a chat between generations, the downgrade rule, the outbox rules
  (`crates/engine/src/doc_host/relay.rs`, `crates/proto/src/relay_gen.rs`).

**Built, but only covered by reading and unit-level tests:**

- A host on a network with HTTPS but no sockets going back to stored rows.

**Not built:**

- Sidebar churn.
- Removing old rows, or backups for device-hosted chats. The nightly backup still runs for a room whose
  rows change, which a generation 3 room's mostly don't.
- A sweep that re-evaluates chats that aren't open. Each host evaluates a chat when it opens it.

**Known limits:**

- A device that opens a chat for the first time writes two metadata ops of its own (`SessionDoc::init`),
  which go out as one queue row. It doesn't wake a sleeping Cloud machine (checked for 150 s), and the
  host imports it as a no-op when it next wakes. On generation 2 the same open stores one row.

- A viewer on a chat whose host has no socket and no wake route stays where it is.
- An old client reading only over HTTP doesn't wake an absent host: its read carries no host hint.
