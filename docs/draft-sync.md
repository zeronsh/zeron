# Synced composer drafts

Status: DESIGN · 2026-09-29
Prior art: `docs/chat2-sync.md` (the room protocol this reuses), `docs/registry-sync.md`.

## Why

A chat's transcript is already synced across every device on an account. The
text being typed into its composer is not: `Composer.drafts` is an in-memory
`HashMap<String, String>` per window. This adds a per-chat **draft** that every
device of the same user edits live, Google-Docs style: concurrent typing from
several devices merges (no last-writer-wins clobbering), and the draft follows
you from device to device.

## Goals

- Typing in an existing chat's composer updates a shared draft; other devices
  show it live and can type into it at the same time.
- Sending consumes the draft on every device, and leaves **no history behind**:
  the draft doc is thrown away, not cleared.
- Desktop first (embedded engine and IPC daemon). The primitive is built so the
  thin client / iOS / Android can adopt it later without redesign.

## Non-goals

- The new-chat canvas (it has no chat id yet).
- Attachments and appshots (they stay local).
- Remote-cursor / selection display of other devices.
- iOS / Android UI wiring, `zeron-client` and the mobile FFI.
- `crates/localedge` (not in `main` yet). When it lands it needs the same room
  ported next to `chat2` (routes + tables); the wire contract below is what it
  must match.

## Why a separate doc (not a field in the session doc)

Session docs keep their whole op history until a trim, and the project's
priority is small session docs. Typing history in the transcript doc would
bloat it and cannot be removed per field. A draft is its own tiny Loro doc in
its own room, so discarding it deletes its history entirely.

## Design

### 1. Room (`edge/`)

New Durable Object `DraftRoom`, copied from `ChatRoom` and reusing
`chat-frames.ts`, `chat-log.ts` and `blobs.ts` unchanged. Same binary frames,
row log, ack/dup semantics, checkpoint endpoint and per-device push quota.
Removed: tail/diff sidecars and the R2 backup alarm.

- Binding `DRAFT_ROOMS`, migration `v5` (`new_sqlite_classes: ["DraftRoom"]`).
- Worker route `/draft/:orgId/:chatId/{ws,checkpoint,rows,epoch,discard,stats}`.
  It requires `auth.orgId === orgId` (403 otherwise) and names the room
  `draft1/{orgId}/{userId}/{chatId}`, so a user can only reach their own rooms
  and there is no ownership-claim race. `chatId` is validated with `ID_RE`.
- **Epoch.** `meta.epoch` (integer, starts at 1). `GET /epoch` returns it.
  `ws`, `rows` and `checkpoint` take `?epoch=N`; a mismatch is rejected
  (`409 epoch_mismatch` / close `4411`) so a stale device can never push into a
  newer epoch.
- **Discard.** `POST /discard?epoch=N`: if `N` equals the current epoch,
  delete all rows, checkpoint blobs and idle state, set `epoch = N + 1`, close
  every socket with `4411 "draft discarded"`. A stale `N` is a no-op returning
  the current epoch (idempotent, so retries and races are safe).
- **Idle expiry.** Every write (re)arms an alarm 30 days out; when it fires on
  an idle room the DO calls `ctx.storage.deleteAll()`. Abandoned drafts never
  accumulate.
- Room size is tiny, so `MAX_CHECKPOINT_BYTES` and `MAX_ROW_BYTES` are lowered
  (checkpoint 256 KiB, row 64 KiB) and reject oversized drafts instead of
  growing.

### 2. Doc and pure primitive (`crates/doc`, `crates/proto`)

- `zeron_doc::DraftDoc` (`draft.rs`): one `LoroDoc` with a single root `LoroText`.
  `set_text` applies a single contiguous splice (common prefix/suffix, so typing maps to one
  op), `snapshot` / `from_snapshot`, `export_since`, `import`, and
  `subscribe_local_update` (the encoded update of each local commit — what gets pushed).
- `import_tracking(bytes, &mut offsets)` imports a remote update and carries UTF-8 byte
  offsets (caret, selection edges) through it with Loro cursors. No hand-written operational
  transform anywhere: every writer — a device, the engine, or a composer window — is a Loro
  replica, and Loro does the merging and the caret math.
- Wire types in `zeron_proto::draft` (`DraftTarget`, `EditDraft`, `DraftFrame`) and the
  capability `draft-sync-v1`.
- An empty doc is the "no draft" state.

### 3. Engine (`crates/engine`)

New `draft_host.rs`, next to `doc_host.rs`, owning one `DraftHandle` per chat
with a watcher or unsynced edits.

- Reuses `zeron_sync::ChatClient` as the room client with a draft sink,
  persister and URL provider. Drafts are **not** in `chat_outbox` /
  `chat_sync_jobs`, so the chat sync scheduler never mistakes them for chats.
- Join loop: `GET /epoch` → if it differs from the local epoch, reset the local
  doc to empty and adopt it → run `ChatClient` on `?epoch=N`. On
  `Disconnected` or `ServerReset`, re-check the epoch before redialing.
- No per-edit outbox: after each (re)join catches up, if local changes are
  unacknowledged the host pushes one full-snapshot row (small; merge is
  idempotent).
- Local persistence: a snapshot under store doc id `draft/{chatId}` so a draft
  survives an app restart offline; deleted on discard.
- Commits are debounced to 250 ms. The edge allows 300 pushes per device per
  minute, and continuous typing at 250 ms is ~240.
- Send/clear: `ClearDraft` empties the local doc, `POST /discard?epoch=N`,
  adopts `N + 1` and deletes the local snapshot. If offline, a `pending_discard`
  flag is kept and the discard runs on the next join. Discard is idempotent.
- Handles are pinned in the LRU while they have a watcher or unacknowledged
  changes.
- Draft rooms count against the process-wide socket budget; only chats with an
  open composer watcher join (not every chat).

RPC (`crates/rpc`, `crates/proto`), all served by the **local** engine and
deliberately not `forwardable` (drafts are per-user replicated state; every
device runs its own room client). Updates are opaque base64 Loro bytes:

- `WatchDraft { chatId }` → stream of `DraftFrame`. First a `reset` frame with the
  engine's snapshot, then incremental updates (echoes of a window's own edits
  import as no-ops). A later `reset` frame means the draft was discarded (sent
  from any device) — drop the replica and adopt the snapshot.
- `EditDraft { chatId, update }`: merge a window's local update into the
  engine's replica.
- `ClearDraft { chatId }`: the draft was sent; discard it everywhere.
- Capability `draft-sync-v1`; the UI only uses drafts when the engine advertises
  it, so older daemons keep today's local-only drafts.

### 4. Composer (`crates/ui`)

- `AppState` runs a draft watch per chat with an open composer (like
  `spawn_queue_watch`). Each composer window owns a `DraftDoc` replica seeded
  from the first `reset` frame.
- Local user edits: on `Edited`, `DraftDoc::set_text(new_text)`; its
  local-update callback sends `EditDraft` (coalesced). Programmatic changes —
  loading a draft on navigation, clear on submit, restore after a failed send,
  queue-edit recovery, applying a remote update — are flagged and never echoed
  back as user edits.
- Remote updates: `import_tracking` with the caret and selection offsets, then
  `ComposerInput::apply_remote_text(new_text, offsets)`: replaces `content`,
  restores the tracked selection, drops mention/slash token state, bumps
  `edit_revision` (so pending paste rewrites abort), refreshes the projection and
  does **not** set `follow_cursor`. Remote updates are deferred while an IME
  composition is active and applied right after it commits.
- Undo/redo stacks hold whole-content snapshots, so they are cleared when a
  remote edit lands (documented trade-off).
- Send: the existing clear-on-submit path also calls `ClearDraft`. A failed send
  restores the text as a fresh draft.
- The existing `drafts` map stays as the fallback for engines without the
  capability and for the new-chat canvas.

## Failure modes

| Case | Behaviour |
|---|---|
| Offline while typing | Local doc keeps the edits; merged on next join. |
| Two devices type at once | Loro merges; both edits survive. |
| Device offline during send, reconnects later | Epoch mismatch → drops its old copy; never resurrects the sent text. |
| Discard races with a remote keystroke | The few ms of text typed after the sender's discard is lost (accepted). |
| Rate limit / oversize | Client backs off (existing quota retry); oversized drafts are rejected, the local text is kept. |
| Old device offline for the whole 30-day idle expiry | The room is recreated at epoch 1, so a device that still holds unsent text from epoch 1 can push it back. Accepted: needs 30+ days offline *and* a draft sent elsewhere in between. |
| Old daemon without the capability | Falls back to today's local-only drafts. |

## Testing

- `zeron-doc`: splice diff, two-writer concurrent merge, caret tracking through
  imports, multi-byte offsets.
- `edge`: unit tests for frame/epoch behaviour; `test/workerd` tests for
  discard, epoch mismatch, idle-expiry alarm and privacy across users; a live
  `scripts/draft-check.mjs` against `wrangler dev`.
- `engine`: integration test with a loopback fake room and two engines —
  live sync, concurrent typing, offline edit + reconnect, discard on send with an
  offline third device, restart persistence, LRU pinning.
- `ui`: gpui tests for caret preservation through remote splices before, inside
  and after the caret; IME deferral; programmatic edits not echoed; capability
  fallback.
- Live: `scripts/e2e-draft.sh` runs the real `DraftRoom` under `wrangler dev` with two
  headless engines (one user, two devices) and the `draft_driver` example: live typing A→B,
  concurrent merge, discard on send, typing under the new epoch.
- PR verification: `cargo test` for the touched crates, `cargo clippy`, and
  `npm run typecheck && npm test` in `edge/`.

## Extending to mobile / thin client

Everything above the UI is reusable: `DraftDoc` lives in
`zeron-doc` with no I/O; the room client is the generic `ChatClient`. A later
change adds a draft room next to `Room` in `zeron-client`, exposes
`draft` state and `draft_edit`/`draft_clear` through the FFI, and maps a native
text view's edits to `DraftDoc::set_text` calls.
