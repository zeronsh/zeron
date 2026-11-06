# The relay layer on phones

How a phone follows a relayed chat, and what Android has to do to catch up with iOS. Read
[relay-layer.md](relay-layer.md) first: it holds the protocol, the rules about old clients, and the
local-first guarantee. This covers what's different when the device is a phone: no engine, suspended
without warning, killed without warning, and paying for every byte.

## In one paragraph

A phone is a viewer that never hosts. It says it runs the layer to the registry and to each room, reads
relayed frames, journals them until a save holds them, sends through the outbox into the room's queue,
catches up from the host by version exchange, reads a checkpoint when only the room can help, and opens
a direct link only while someone is watching a chat do something. All of that is in `crates/client`.
The platform's job is to report what the OS is doing, and to show one new state.

## What Android gets for free

Everything below lives in `crates/client` and `crates/sync`, and runs the moment `relayLayer` is true.

| Behaviour | Where |
|---|---|
| Registry `hello` and pulls say `relay: 1`, so the room doesn't list this phone as one that can't follow | `live/mod.rs`, `live/urls.rs`, `zeron_sync::registry::announce_relay_layer` |
| Each room's `hello` says `relay: 1`, `gen: 3` when the row does, and carries `hostDevice` | `live/room.rs` (`RoomRelay::apply_route`) |
| Live relay frames into the doc and the transcript | `ViewerSink::relay_import` |
| The journal: kept in `chat_inbox` until a save covers it, replayed on open | `ViewerSink::relay_import`, `Persister::flush`, `load_local` |
| Sends: outbox, then `enqueue` for a relayed chat whose host reads the queue, `push` otherwise | `ChatClient::push_kind`, `RelayHandle::set_queue_pushes` |
| Catch-up by version exchange, with resumable transfers | `crates/sync/src/chat_relay.rs` |
| Cloud checkpoints, at most one download per checkpoint, none off screen | `fetch_once`, `relay_defer_checkpoint`, `Room::set_route` |
| The direct link, its retry pacing, and letting go of the connection | `live/chat_link.rs` |
| No serving other viewers: only the host can pull from a phone | `RelayHandle::set_serve_host_only` |
| Following a generation flip, a host move, or a host losing the capability, without rejoining | `SessionCore::refresh_relay` |
| Why an empty transcript is empty | `ComposerState::transcript_load` |
| A send isn't marked "Not delivered" while it's only waiting on a path or a host | `derive_pending` |

The tests that prove these are `crates/client/tests/relay.rs` and `crates/client/src/live/room.rs`, and they
don't know which platform they're on.

**A device row isn't needed.** Phones write none. `relay_allowed` only looks at rows that exist, plus the
room's list of clients seen without the layer. A phone that says `relay: 1` blocks nothing.

## What Android has to do

There are three changes, and none of them is large. The FFI was added so that nothing breaks until you do
them: `relay_layer` defaults to false and Kotlin's `CoreConfig(...)` call compiles unchanged. A build that
hasn't done step 1 is a phone without the layer, which is safe and holds the account on stored rows.

### 1. Turn it on

`apps/android/.../core/AppModel.kt`, in `start()`:

```kotlin
val config = CoreConfig(
    ...
    relayLayer = true,
)
```

iOS does this in `AppModel.runsRelayLayer`, with a debug-only `-no-relay-layer` argument for playing an old
phone. Add the same switch as a debug intent extra: it's the cheapest way to test the old-client path.

Shipping it on is safe before any edge flag: the room's answer decides everything, and a room without the
flag ignores the field. **Don't gate it on a remote flag**: the room has to hear `relay: 1` from this phone's
build for the 14 days before any engine flips, or it lists the phone as old.

### 2. Keep the lifecycle calls accurate

Android already makes these calls, so check them rather than add them. Every one matters more here than it
did on stored rows.

| Call | iOS | Android | Why the layer needs it |
|---|---|---|---|
| `onForeground()` | `sceneWillEnterForeground` | `ProcessLifecycleOwner.onStart` (`ZeronApplication.kt`) | Redials the chat on screen after 8 s away, probes the rest, retries the link |
| `onBackground()` | `sceneDidEnterBackground` | `ProcessLifecycleOwner.onStop` | Saves every doc, which retires the journal, and stops direct links |
| `setNetworkOnline` | `NWPathMonitor` status | `registerDefaultNetworkCallback` | Parks dials instead of burning them, and drives the Offline state |
| `networkPathChanged` | `NWPathMonitor` interface change | `onCapabilitiesChanged` / `onLost` on the default network | Drops connections whose path is gone, and retries the link now |
| `setViewAttached(true/false)` | `CoreSessionSource.attach/detach` | The session screen's composition | **Decides who gets a link and who may read a checkpoint.** A screen that stays attached off screen keeps a radio up |

Check two things on Android that iOS doesn't have:

- **`onStop` isn't always a suspension.** With `DeliveryService` running, the process keeps going and sockets stay
  up. That's fine: rooms stay joined, and only links stop. The `DELIVERY_GRACE` it holds is the iOS 25 s
  background task's equivalent.
- **A process death arrives with no `onStop`.** The journal is what covers it. Don't add a save on a timer.

### 3. Show the new state

`ComposerState.transcriptLoad` says what to put in place of an empty transcript:

| Value | Text | Glyph |
|---|---|---|
| `READY` | none: show the transcript | |
| `SYNCING` | Syncing… | spinner |
| `RECONNECTING` | Reconnecting… | spinner |
| `OFFLINE` | Offline — this chat loads when you're back online | warning dot |
| `HOST_AWAY` | This chat lives on another device — it loads when that device is back online | warning dot |

Use exactly these strings: they're the desktop's. iOS draws them centered where its loader sits
(`SessionViewController.renderEmptyNotice`), and any of them also ends the loader's wait. On Android,
put it over `Transcript(...)` in `SessionScreen.kt`. A chat with messages stored here is always `READY`, and it
never shows these. Don't add a spinner to the warning states: nothing is happening, and that's the point.

Nothing else in the UI changes. The composer's existing banner states cover sends.

## Lifecycle: what happens when

| Event | What the phone does | Radio and data |
|---|---|---|
| Launch | Opens the local store, restores the registry, opens 4 warm sessions from local snapshots | Each warm room dials once |
| Chat opened | Local snapshot and journal first, so content is on screen before any socket. Room kicked, link opened if a turn's running or just attached | Link for 60 s, longer if active |
| Chat left on screen, idle | After 60 s the link closes and, if no terminal uses it, so does the connection | Nothing until something happens. The room's socket probes every 15 min |
| Background | Saves docs, stops links. The OS suspends the rest | None |
| Killed | Nothing runs. Outbox and journal are on disk | None |
| Relaunch | Delivery resumes any send with a pending receipt without waiting for a screen | One room per pending chat |
| Network switch | `networkPathChanged`: connections close, the room redials, the link retries | One pairing |
| Foreground after 8 s or more | The chat on screen redials at once instead of waiting out a 10 s probe | One socket and a version exchange |
| Push notification | Alert only: it doesn't wake the app, so nothing syncs until it's tapped | None |

### What a suspended phone can't do

Frames sent while it's suspended are simply lost to it. That's expected: the next `announce{hello}` finds them.
**Never try to keep the socket through a suspension.** A foreground service held for sync on Android would
break the battery promise and gain nothing.

## Local-first: every delivery path

Every path into a phone's doc, and how it survives a kill.

| Path | Kept by | Window before it's safe |
|---|---|---|
| Local send | Outbox written in the same call as the commit | None |
| Relay frame, direct or through the room | `chat_inbox`, written right after the import | The gap between the import and that write. Version exchange recovers it if the host is around |
| Sync transfer | Same, plus an urgent save | Same |
| Queue row read back | Same: it arrives as a relay frame | Same |
| Stored row | Debounced save with its cursor. The room still has it | Refetched from the cursor |
| Checkpoint | Urgent save with its cursor. The room still has it | Refetched |
| A send's ack | Saves first, then retires the outbox row | None |

`load_local` reads the snapshot, then the outbox, then the journal, with no network and no room. A journal row is
only retired by a save whose export began after it was noted, so a save can't retire what it doesn't hold.

Not covered, because nothing was stored here to begin with:

- A chat never opened on this phone. It's `HOST_AWAY` or `OFFLINE`, not empty.
- Subagent transcripts and terminals, which are live host RPCs and were never local.

Warm rooms make this better than it sounds: the 4 preloaded chats and anything opened recently follow along while
the app is in front, so their copies are current when the host goes away.

## What changed in shared code

Android gets all of these automatically, and has nothing to adopt. They're listed because they change behaviour
that engines share.

- `ChatClient` holds a batch meant for the queue out of `POST /rows` until a socket answers or this network turns out
  to have no sockets. A stored row in a room that remembers relaying can park on an old reader.
- The bootstrap pull and the first socket share one checkpoint download (`fetch_once`). They used to download it
  twice on every fresh open, on stored chats too.
- A parked row doesn't read a checkpoint this doc already holds, with the layer on. That read can't fill the gap.
- `delete_snapshot` clears `chat_inbox`.
- `Mux::stream_count` and `ViewerPeers::release_if_idle` are new.
- A send waiting on a path or a host doesn't age into "Not delivered". This applies to stored chats too, and the 120 s
  clock starts when the path returns.

## Old and new together

| Mix | What happens | Proven by |
|---|---|---|
| A shipped phone in an account of new devices | Its registry `hello` has no `relay`, so the room lists it, hosts see the list and hold chats on stored rows. Arriving at a chat on 3 brings the host back to heal | See [test status](#test-status) |
| A new phone with old devices | It says `relay: 1`, the old device's row lacks the capability, so nothing flips. It behaves as a stored client | `an_edge_with_the_flag_off…`, `a_chats_generation_decides…` |
| A host moves back to an older build | Its row loses `chat-relay-v1`, and sends go back to rows at once | `a_chats_generation_decides…` |
| Flip 2 → 3 → 2 mid-session | Each send goes where the current generation says. None is lost | `a_chats_generation_decides…` |
| Edge flag off | The room doesn't answer `relay: 1`: stored rows | `an_edge_with_the_flag_off…` |
| A phone upgrades in place | Same device id. Its next `hello` removes it from the room's list | Room tests in `edge/test/workerd` |

## Test plan for Android

**1. Shared tests: nothing to do.** They're already green on the code you link.

```sh
cargo test -p zeron-client --test relay
```

**2. The rig.** An emulator reaches the host as `10.0.2.2`. Everything else matches what iOS uses. Clear the app's
inherited environment first: an engine with none of this set would touch production.

```sh
(cd edge && npx wrangler dev --port 27650 --var AUTH_MODE:dev --var CHAT_RELAY_LAYER:1)
python3 proxy.py 27660 27650   # a forwarder you can take away, for playing a dead network

env -u ZERON_APP -u ZERON_DATA_DIR -u ZERON_EDGE_URL -u ZERON_IPC_PORT -u ZERON_MANAGED_ENGINE \
    -u ZERON_WORKOS_CLIENT_ID -u ZERON_CALLBACK_PORT \
  ZERON_DATA_DIR=/tmp/relay-host ZERON_IPC_PORT=27811 ZERON_DEVICE_NAME=relay-host \
  ZERON_EDGE_URL=http://localhost:27650 ZERON_EDGE_TOKEN=relaytest@org1 ZERON_ORG_ID=org1 \
  ZERON_HARNESS=mock ZERON_MOCK_DELAY_MS=400 ZERON_RELAY_LAYER=1 RUST_LOG=info \
  target/debug/zeron headless

cargo run -p zeron-rpc --example relay_rig -- 27811 /tmp/relay-host create   # prints the chat id
cargo run -p zeron-rpc --example relay_rig -- 27811 /tmp/relay-host run <chat> "go"
curl -H 'authorization: Bearer relaytest@org1' http://localhost:27650/chat2/<chat>/stats
```

`stats` is the truth for what's on the edge: `headSeq` should not move during a turn on generation 3, and `relayQueue`
holds what's waiting. Read the phone's own store to see the journal and the outbox:
`sqlite3 <data dir>/docs.sqlite3 'select count(*) from chat_inbox; select count(*) from chat_outbox;'`.

**3. Flows to run on an emulator, then a device.** Each of these found something, or could have, on iOS.

| # | Flow | Pass |
|---|---|---|
| 1 | Streaming turn, watching | Text arrives, `headSeq` doesn't move, `chat_inbox` drains to 0 |
| 2 | Force-stop mid-stream, relaunch with the edge down | Everything received shows, with no spinner |
| 3 | Send with the network off, force-stop, relaunch, network on | Echo stays, the outbox drains, `relayQueue` rises then empties when the host returns |
| 4 | Host stopped, chat never opened on this phone | `HOST_AWAY` text, no spinner. Start the host: the chat fills by itself |
| 5 | Network off, chat never opened | `OFFLINE` text |
| 6 | Room with a checkpoint, host stopped | Loads once. Count downloads across 5 relaunches |
| 7 | Background for 30 s during a turn, return | Caught up within seconds of returning, no duplicates |
| 8 | Wi-Fi to cellular mid-stream, on a device | Link drops, room carries it, link returns. Nothing's lost |
| 9 | Debug switch off, in an account with a host on the layer | Host's chats stay on or return to generation 2 |
| 10 | Leave a chat on screen, idle, for 2 minutes | `chat link down` in the log, no further traffic |

For a link on an emulator, expect pairing to depend on your NAT. Falling back to the room is a pass.

**4. Measure on a real device before relying on it:** data used by a day of warm rooms, and battery with a chat left on
screen. Neither has been measured on any phone.

## Traps

These cost real time on iOS, or would have.

- **A `RelayHandle` must be spawned inside the runtime.** `Room::start` runs on a platform thread. `RelayHandle::spawn`
  calls `tokio::spawn`, and it panics there without `runtime::handle().enter()`. Already handled; keep it.
- **`relay=1` on every pull, not just the socket.** A registry or chat pull without it marks the device as an old reader,
  and one poll is enough to put the whole account back on stored rows.
- **`announce_relay_layer` is process-wide**, and it has to run before the registry's first `hello`. It can't be
  un-said, so tests of a client without the layer belong in their own test binary, or only assert on chat frames.
- **`hydrated` doesn't mean loaded.** On a relayed chat, being caught up with a room that stores nothing says nothing
  about content. Use `transcriptLoad`. Neither app reads `hydrated`.
- **Don't drop `run_link`'s future when stopping a link.** Its last act tells the relay task the link is gone, and until
  then frames keep going down it and being skipped on the room. Cancel its token and await it.
- **`watch::Ref` isn't `Send`.** Awaiting across `wait_for`'s return in a `select!` won't compile on a spawned task. See
  `chat_link::until`.
- **A closed stream doesn't close the connection.** ICE keeps talking under an open `Mux` with no streams, which is why
  `release_if_idle` exists.
- **A phone's ids sort after a host's only sometimes.** Whoever sorts lower makes the WebRTC offer. Both directions
  work, but when reading a pairing log, don't assume the phone offered.
- **Launch arguments can hide what you're testing.** `-demo` and `-signedout` change far more than they say. The rig uses
  `-dev` and `-edge`, and on iOS, sim launch args mask persisted settings.
- **A simulator shares its Mac's network.** There's no airplane mode on one, so `-network-offline` reports no path and the
  forwarder takes the edge away. An Android emulator can do this for real: `adb shell svc wifi disable` and
  `svc data disable`.
- **`Not delivered` used to appear while offline.** It was on a 120 s clock from when the message was sent, and it
  looked like loss. It now counts only time with a path and a host.

## Test status
