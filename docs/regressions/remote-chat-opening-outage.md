# Remote chat opening reads as an outage

Base: `c04e3749` (main, v0.2.124); the work began on `1ec71a31` (v0.2.120).
Verified on macOS with unit, gpui and in-process relay tests, against a local
`wrangler dev` edge with two headless engines, and in captures of the
production shell. The reporting desktop and its host were not exercised, so
this is not a claim about the installed application.

Opening a chat hosted on another device over a slow edge showed a blank
transcript with no indicator, then "Messages will send once the connection
recovers." although nothing had been sent. Revisits repeated both with no
loading cue. The desktop log for the reported chat shows its first
STATE frame 12.7 s after admission on the cold open and 8.7 s on a revisit,
and the revisit replaying rows from the checkpoint (`repair from=1741 to=5`).

Ten defects were found:

- C1: four seconds after opening, a chat whose room had not answered yet read
  as an outage (`!net.connected` under a connected registry), although the
  engine allows 20 s to connect and 15 s for HELLO.
- C2: the "messages will send" clause showed with no draft, queued message or
  pending send.
- C3: the sidebar pill described the selected chat's sync ("Syncing…", "Sync
  queued — changes are saved") while the composer described an outage.
- C4: a never-synced remote chat resets to an authoritative empty transcript,
  which renders as a blank page.
- C5: navigation ends the watch and the room retires after 10 s idle, so every
  revisit is a cold rejoin, with no visible cue.
- C6: legacy cursor repair ran on every rejoin of a chat first opened cold.
  Its born snapshot is unverified and the verified flag was read once per
  handle, so each rejoin rewound to the checkpoint and downloaded the rows
  again. Other chats in the same log show the same pattern.
- C7: host presence and the edge link shared one outage message. In the
  incident the host was up; this desktop's registry socket had gone deaf
  (`all live peer presence went dark; probing registry room (deaf-socket
  tripwire)`).
- C8: under a connected registry `delivery_live` was ignored, so a chat
  delivering over HTTP fallback read as degraded.
- C9: side chats share `Transcript` and `Composer`, so they inherited C1–C5.
- C10: forking closed its card before the request, showed nothing for up to
  the 30 s relay deadline, reported errors in the main composer, and opened
  nothing when the parent was not listed.

## Fix

- Engine (C6): `ChatPersistence` holds the verified flag as an `AtomicBool`,
  seeded from the store and set by the first verified write.
  `EngineChatSink::cursor_is_verified` reads it, so repair runs once per legacy
  snapshot and never once per rejoin. The thin client already reads the flag
  from the store.
- Link state (C1, C7, C8): `AppState::chat_link_at` returns
  `ChatLink { sync, host_unreachable }`. An unsynced remote chat is `Syncing`,
  captioned after 4 s, and becomes `Stalled` only once its room has stayed
  down for 35 s of the sync (the engine's 20 s connect plus 15 s hello
  budget); a joined room catching up never stalls. The clocks are the view's
  own: opening a chat restarts its hold and stall, a room moved back to
  waiting for a slot keeps its clock (so a sync that never delivers always
  stalls instead of spinning), and a new engine's first frame restarts them
  all.
  Both moments are clock transitions for the open chat and for chats with a
  pending send. `delivery_live` counts as live unless the engine is offline.
  Host presence is a separate flag, and a sleeping Cloud machine never sets it.
- Sends: "Queued" uses `send_would_queue`; a syncing chat does not queue. Past
  the 120 s grace, a send on a chat still syncing is not "Not delivered": its
  pushes leave before catch-up (`crates/sync/src/chat_client.rs`, "Pending
  pushes ALSO go before catch-up completes") and its adoption shows once the
  copy catches up.
- Composer (C2, C5): `link_notice` adds the "will send" clause only with a
  draft, queued message or pending send. Without one it says "Syncing…",
  "Reconnecting…" or "Offline", and nothing while the transcript loader shows.
  "Can't reach {host}" appears only on the send path.
- Sidebar pill (C3): global connectivity only, plus a storage error from any
  chat.
- Transcript (C4, C5): `AppState::transcript_loading` returns `Opening`,
  `Syncing`, `Reconnecting` or `Offline` for a blank remote transcript that has
  not caught up. The loader (the house `ring_spinner` and a caption) appears
  after a 300 ms timer, which reduced motion does not skip. A still wrapper
  positions it: `motion::fade_in` restyles its element `relative` every frame,
  which would lay the overlay out below the full-height list, where the pane
  clips it. A cached revisit never shows it, nor does a subagent tab, an
  unsaved side chat, a chat hosted here or a local profile.
- Side chats (C9, C10): they get the composer and transcript fixes, each
  reading its tab's own `AppState`. A fork opens its tab immediately with
  "Forking on {host}…"; failure shows the error and Retry in that tab. Each
  fork tracks its own request, so a retry never waits on another fork. The
  host writes the fork's row before it answers, and the answer can time out
  after the fork exists: opening the row turns the waiting tab into the chat.
  A tab closed while forking stays closed, and the fork lists under its parent.

## Screenshots

`crates/ui/examples/remote-sync-fixture.rs` attaches the production shell to
a real engine with isolated data, as at launch, and plays the edge and the
host "omarchy" over that engine: the chats and devices, each room's
connectivity, each transcript's opening frame, and a fork that waits 6 s
before the engine, which has no edge, refuses it. The same fixture ran on
`c04e3749` and on this branch. Captures are taken 5 s after opening, past the
composer's 4 s hold. In [`docs/screenshots/remote-chat-sync`](../screenshots/remote-chat-sync):

| Scene | Before (`before-*.png`) | After (`after-*.png`) |
| --- | --- | --- |
| `cold-open`: never synced here, room dialing | Blank; "Messages will send once the connection recovers."; sidebar "Syncing…" | Centered "Syncing…" loader |
| `revisit`: cached copy, room rejoining | The same false notice and pill | "Syncing…" above the composer |
| `outage`: this device lost the edge | "Messages will send once the connection recovers." | "Reconnecting…" |
| `outage-draft`: the same, with a draft | The same notice | "Reconnecting — messages will send once the connection recovers." |
| `side-chat`: a side chat opened cold | Blank tab with the false notice | Loader in the tab |
| `fork-waiting`: 2.5 s into a fork | Nothing | "Forking on omarchy…" tab |
| `fork-failed`: the host's answer fails | Error in the parent's composer | Error and Retry in the tab |

The `after-fork-failed.png` capture predates the maintainer's restyle of that
state ("Couldn't fork on omarchy", the reason as muted detail, a ghost Retry);
the fixture's `fork` scene renders the current one.

## Before/after proof

The engine tests were run with the line that sets the flag on a verified write
removed, which restores the base behaviour (the flag read once per handle
open). Both failed:

```text
chat_persistence::tests::first_verified_write_ends_legacy_cursor_repair ... FAILED
doc_host::sync_lifecycle_tests::rejoin_after_idle_retirement_resumes_from_its_cursor ... FAILED
assertion `left == right` failed: rejoin rewound to the checkpoint
  left: Some(2)
 right: Some(5)
```

`rejoin_after_idle_retirement_resumes_from_its_cursor` runs the real host
against an in-process relay that serves a 5-row log with its checkpoint at
row 2. It opens the chat cold, lets it retire idle and reopens it; the second
connection must ask for rows after 5. `slow_hello_reads_as_connecting_until_the_room_answers`
holds HELLO past the 4 s grace and checks the chat reports `Connecting`, not
`Offline`, until the room answers, then `Synced`.

The UI tests were checked by mutation: production rules were reverted and
the build rerun. The stall-without-deadline mutation (`Stalled` as soon as the
room is down), reverted alone, failed 4 tests. Six more, reverted together,
failed 7 tests, each on its own rule's assertion:

```text
mutation                                  failing assertion
"will send" clause without intent         left: Some("Reconnecting — messages will send once the connection recovers.")
                                          right: Some("Reconnecting…")
loader delay 0 ms                         left: Some(Opening) right: None
fork completion ignores a closed tab      assertion failed: shell.side_chats.is_empty()
storage error needs every chat            left: Some(("Offline — sends are saved", true))
                                          right: Some(("Changes could not be saved", true))
host flag from "online", not "warns"      left: ChatLink { sync: Live, host_unreachable: true }
                                          right: ChatLink { sync: Live, host_unreachable: false }
syncing deadlines removed                 the caption shows on time  left: 0 right: 1
```

With the rules restored, all pass.

## End to end on a real edge

The two-device harness (`scripts/e2e-smoke.sh`: `wrangler dev` in dev-auth
mode and two headless engines) ran with one more step: once the driver's
watch on device B ended, B's room for the chat retired idle; 30 s later a
client reopened the chat on B and held it for 20 s. B's log then shows the
chat admitted twice. With the line that certifies the cursor removed, the
rejoin rewinds as in the incident:

```text
cursor repairs on B: 1
INFO zeron_sync::chat_client: chat2: one-time legacy cursor repair from=1 to=0
```

With the fix, in two runs, the smoke passes and the rejoin repairs nothing
(`cursor repairs on B: 0`). The extra step was a scratch script and example
outside the repository; the harness itself is unchanged. A shell started from
inside Zeron inherits its `ZERON_*` environment, and `ZERON_WORKOS_CLIENT_ID`
alone turns the harness's dev-mode engines into local profiles that never
sync, so the run cleared the environment (`env -i`).

## Counter-review

An independent review of the diff found the transcript loader never visible:
`fade_in` overrode its absolute positioning, so the overlay sat below the list
where the pane clipped it, while the composer hid its own caption for it. The
loader tests had read only its state; `transcript_loader_renders_centered_in_its_pane`
now measures where it renders. With the old layout restored it fails exactly
as reported:

```text
the loader lies inside its pane: Bounds { origin: Point { x: 371px, y: 600px }, size: Size { 58px × 47.5px } }
  in Bounds { origin: Point { x: 0px, y: 0px }, size: Size { 800px × 600px } }
```

Every fix below was checked the same way. The loader layout and six of the UI
rules were reverted together in one build, a slot-waiting rule since replaced
(see the findings) alone in a second, and the engine rule in an engine build.
Each test failed on its own rule's assertion:

```text
reverted rule                              failing test, assertion
first frame restarts sync clocks           a_new_engine_restarts_sync_clocks: left: Stalled right: Syncing { shown: true }
opening restarts the caption hold          waiting_for_a_sync_slot_never_reads_as_an_outage (since replaced by a_sync_that_never_delivers_stalls_even_back_in_the_queue): left: Syncing { shown: true } right: Syncing { shown: false }
syncing is not undelivered                 a_send_on_a_syncing_chat_is_not_undelivered: !s.send_undelivered("c-remote", late)
deadlines for open chat, pending sends     chat_sync_caption_and_stall_are_clock_transitions: left: 2 right: 0
retry per tab                              retrying_a_fork_never_waits_on_another: the retry goes out at once
row finishes a pending fork                opening_a_pending_forks_row_opens_the_chat: the row opens the chat, not the wait
only a landed write certifies (engine)     failed_write_keeps_legacy_cursor_repair: !persistence.cursor_verified()
```

The review's findings and what was done:

| Finding | Outcome |
| --- | --- |
| Loader laid out below the list, clipped | Fixed; render-bounds test |
| A send held while syncing also read "Not delivered" | Fixed: not undelivered while syncing |
| Sync clocks kept across an engine restart | Fixed: the first frame after attach restarts them |
| A chat that waited for a sync slot read as stalled when opened | Fixed: opening restarts the caption hold and the stall |
| A room moved back to waiting for a slot restarted its stall, so a sync that never delivered could spin "Syncing…" forever | Fixed (maintainer): only delivery or opening the chat restarts the clock; `a_sync_that_never_delivers_stalls_even_back_in_the_queue` |
| Transcript loader spun while hard offline; fork failure printed the engine error in red | Fixed (maintainer): offline is the amber dot, as in the composer and sidebar; the failure is a plain empty state with a ghost Retry |
| Opening a pending fork's row showed the wait | Fixed: the row finishes the fork |
| Retry did nothing while another fork ran | Fixed: per-tab request tracking |
| Clock notifications for every background syncing chat | Fixed: the open chat and pending sends only |
| Pending-fork loader repainted the shell at 30 Hz | Fixed: `ring_spinner` repaints its own view |
| Done chime swallowed while a send is held past 120 s | Kept: the chime agrees with the visible "Sending"; within 120 s it was already swallowed |
| Unacknowledged pushes on a live room no longer shown | Kept: the old pill flashed "Syncing…" on every slow ACK; the composer owns the per-chat story |
| A stale `Local` frame shows a warm, empty chat blank for about a second | Kept: a room retires only once caught up |

A later review found one line the engine fix now relies on that no test
covered: `ChatPersistence::new` opens a row whose cursor is not verified at
cursor 0 (`crates/engine/src/chat_persistence.rs:49`). Before, the repair
backed it up at every rejoin. Now a landed write ends the repair, so a stored
cursor above the doc's rows would never be fetched again.
`an_unverified_cursor_opens_at_zero` pins it. With the zeroing removed it
fails, and every other test passes:

```text
chat_persistence::tests::an_unverified_cursor_opens_at_zero ... FAILED
  left: 500
 right: 0
```

For a doc opened from an unverified row, the repair still runs at a rejoin
until a write lands, so that rejoin downloads from the checkpoint again:
within about a second of the doc's first rows (longer while the shared writer
saves another chat), and for as long as writes fail. From the code, the host's
first join after its own pushes can do the same, when the room's state arrives
before the acked cursor is saved. Each costs a download, never data.

## Reproduce

```sh
cargo +1.99.0 test --locked -p zeron-engine --lib
cargo +1.99.0 test --locked -p zeron-engine --lib -- slow_hello_reads_as_connecting rejoin_after_idle_retirement
cargo +1.99.0 test --locked -p zeron-ui --lib -- --skip a_queued_file_reads_back_with_its_bytes_and_display_name --skip generated_image_chunk_reader_targets_owner_and_bounds_decode
cargo +1.99.0 test --locked -p zeron-ui --lib -- --test-threads=1 a_queued_file_reads_back_with_its_bytes_and_display_name generated_image_chunk_reader_targets_owner_and_bounds_decode
cargo +1.99.0 run --locked -p zeron-ui --features appshots-fixture --example remote-sync-fixture -- <dir> cold-open
```

The fixture's scenes are `cold-open`, `revisit`, `outage`, `outage-draft`,
`side-chat` and `fork`. For the before set, the fixture, its `[[example]]`
entry and its four hooks (`Shell::fixture_open_chat`,
`Shell::fixture_open_side_chat`, `Shell::fixture_fork_chat`,
`Composer::fixture_draft`, each one call into the production path) were
applied unchanged to `c04e3749`.

Validation, after rebasing onto `c04e3749`: 635 engine tests pass (5
ignored) and 1,897 UI tests pass; 5 and 22 of them are new. The mutation runs
and the real-edge run above were made on `1ec71a31`, except the zeroing's,
made after the rebase; the rebase changed nothing in `crates/sync`, the chat
host or its persistence. The two relay
tests passed five consecutive runs. Changed hunks pass rustfmt; `git diff
--check` passes.

The two attachment tests run on their own because they predate this change
and race in parallel: each builds a real macOS platform without the guard
`typography::native_text_system` takes, and two at once abort the whole run
(`Text Input Sources or Text Services Manager API is being called in two
threads concurrently`). CI runs the UI tests under nextest, one process per
test, where they cannot race. Taking that guard in both would let them run
with the rest under `cargo test`.

Not verified: the installed desktop against the reporting host over a slow
link (the UI was verified by gpui tests, including where the loader renders,
and by the captures, whose edge and host are scripted), any added latency on
the local edge, and iOS, which keeps the same 4 s grace
(`crates/client/src/connectivity.rs`). Rooms still retire between visits;
keeping them open longer is a separate change.
