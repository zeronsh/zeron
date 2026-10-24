# Session notes

Each main chat has one Markdown scratchpad, written by the user. On the
desktop the session card's **Notes** row opens it as a right-pane tab; on iOS
it is **Notes** in the session's menu. Side chats have no row: a fork copies
its source's notes, so they are there if the fork becomes a main chat.

## Data

The text is the session doc's root `notes` LoroText (`crates/doc/src/notes.rs`).

- The chat2 room relays update blobs without parsing them, so a new root
  container syncs with no edge change. Clients that predate notes skip the
  container (`crates/client/src/session/transcript.rs`).
- It must stay a root container. A child created concurrently by two devices
  (say under `meta`) would be two containers, and one side's text would lose.
- Any device writes it, like `queue`. Writes never go through the chat's host:
  every engine and the iOS client keep a full replica, so notes stay editable
  while the host is offline, and offline writes reach the room through the
  durable outbox on reconnect.
- Local-only profiles persist notes with the rest of the doc. Archiving leaves
  them alone; deleting the chat deletes them.
- Legacy chats rebuilt for chat2 (`rebuild_thin_doc`) and forks carry notes over.
- Writes may not grow the notes past 256 KB (`NOTES_MAX_BYTES`). Shrinking is
  always allowed.

## Writing: compare-and-set on the text's hash

The desktop UI holds no Loro doc; it reads and writes notes over RPC:

| Method | Params | Result |
| --- | --- | --- |
| `WatchSessionNotes` | `{ chatId }` | stream of `{ text, hash, epoch, seq }` |
| `SetSessionNotes` | `{ chatId, baseHash, text }` | `{ kind: "applied", hash, epoch, seq }`, `{ kind: "conflict", current }` or `{ kind: "tooLarge", limit }` |

Both are served by the caller's own engine and are not relay-forwardable.

A write applies only if the notes still hash to `baseHash`, the text the
viewport started from. The doc then records only the changed range as
character ops, so edits from different devices still merge character by
character in Loro. The hash (`notes_hash`, FNV-1a 64) covers the notes text
alone: doc frontiers move with every streamed token and would make every write
during a turn a conflict.

A viewport that loses the race merges with `rebase_notes`, a three-way merge of
two single-range edits. Disjoint edits both apply. Overlapping ones honor both
deletions and keep both insertions, the remote one first. Nobody's typing is
dropped.

A hash says what the notes read, not when. The watch reports a viewport's own
writes back, sometimes after a newer write has landed, and another device can
restore text the viewport wrote earlier: the same hash with opposite meanings.
So snapshots and `applied` replies carry a version. `seq` counts the distinct
states one doc instance has reported, under the gate, so a version and its
text never come apart. `epoch` is random per instance, so a reopened doc's
restarted count never passes for an old one. A viewport drops any snapshot
that does not supersede its base (same epoch, `seq` no higher) and merges
every newer one, whatever its text. The only snapshot adopted without a merge
is one naming a write whose reply was lost: that write landed, and the editor
already holds it.

The iOS client writes its local replica directly (`SessionHandle::set_notes`)
with the same compare-and-set, and watches through `watch_notes`.

## Engine cost

Each notes commit is one row in the room, and the room allows 300 pushes per
minute per device. Viewports wait 750 ms after the last keystroke before
writing, and write at once when the editor loses focus or closes.

The doc subscription counts event batches that only touched the notes
separately (`NotesTouch`). The chat worker skips its transcript pass for
them: no transcript or queue republish, no command or queue drain. Snapshot
persistence and chat2 maintenance still run. A push from a device that is not
the host still wakes the host through the room; that wake is coalesced and
cannot be avoided without an edge change.

## Editing

Both platforms read a note with `zeron_markdown::live`, the composer's
source-aware rules: faces, headings, list continuation, and the markers a live
preview hides off the caret's line.

- **Desktop** (`crates/ui/src/notes.rs`): a `ComposerInput` with
  `with_markdown()` (live Markdown without the composer's chips),
  `with_fill_height()`, and the `DocumentInput` key context, where Enter breaks
  the line and nothing submits. Headings get their own row heights. Layout
  reshapes only lines an edit changed, and paint skips rows out of view. A
  change from another device lands through `apply_external_edit`, which
  carries the caret, selection and scroll position across it. Undo and redo
  survive it as long as the user's steps touch different text
  (`rebase_history`).
- **iOS** (`apps/ios/Zeron/Session/NotesViewController.swift`): a
  `UITextView` styled from `markdownLiveStyles`, ranges in UTF-16 units.
  Remote changes apply in place from `notesTextReplacement`. UIKit's undo
  history is cleared when one lands.

The live preview reveals markers per line. Fences and tables are never hidden,
so a block-level reveal would only show more source.

## Not done

- The agent cannot read or write notes; that would need MCP tools and puts the
  notes into prompts.
- There is one note per session. Several would be a `LoroMovableList` of
  `{ id, title, body: LoroText }`, each entry created by one device.
- Right-pane tabs, the notes tab included, are not restored after a restart.
- No sidebar indicator shows that a session has notes.
