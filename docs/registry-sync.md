# Registry sync — the workspace index without a CRDT

**Status: shipped behind the `reg1/` room namespace; replaces the `ws4/` Loro workspace doc.**

## Why

The workspace doc (sidebar index: devices, spaces, chats, session status) was a Loro CRDT
synced through a Durable Object. Three incidents in one week (2026-07-30, 2026-08-04,
2026-08-05) traced to the same root: the CRDT keeps *history*, and the DO must replay or
re-export that history in wasm under a CPU limit. Loro's full-snapshot export deterministically
RangeErrors on workerd for this doc (works locally), so the room could not self-compact — the
log only grew, cold replays crept toward the CPU limit, and the room wedged. All of that
machinery bought convergence-without-a-server that the topology never uses: every write
already flows through one DO, which can simply *be* the authority.

The sidebar's data is a keyed set of rows with independently-updatable scalar fields —
a replicated table, not a document. The registry stores **current state only**:

- 10,000 sessions ≈ a few MB of rows, bounded forever; history is discarded the moment it
  stops being true.
- The DO cold-starts by reading a SQLite table. No replay, no wasm, no compaction, no
  fold budgets, no wedge class.
- Conflict semantics are unchanged in practice: the Loro schema resolved same-field
  conflicts by last-write-wins map sets; the registry runs the same rule as ~40 lines of
  auditable code (per-field hybrid logical clocks) instead of a black-box wasm library.

Session docs (transcripts) stay on Loro — concurrent text has no "newest wins" answer,
and that side is measured and healthy.

## Topology

```
engine A ── RegistryDoc (rows + pending ops, SQLite-persisted) ── RegistryClient ─┐
                                                                                  ├─ RegistryRoom DO (reg1/{org}/{user})
engine B ── RegistryDoc ── RegistryClient ────────────────────────────────────────┘   rows table + seq counter
```

- **RegistryRoom DO** (`edge/src/registry-room.ts`): authoritative row table in DO SQLite.
  Applies pushed ops with per-field LWW (HLC compare), bumps a monotonic `seq` per batch,
  broadcasts merged rows to every socket. Cursor sync: a client joining with `cursor=N`
  gets only rows with `seq > N`. Pure merge logic lives in `registry-core.ts` (unit-tested;
  mirrored 1:1 in Rust).
- **RegistryDoc** (`crates/doc/src/registry.rs`): the client-side table. Authoritative rows
  (server truth) + a pending-op queue (offline writes, replayed as an overlay for reads).
  Typed API is a drop-in for the old `WorkspaceDoc`. Serialized whole into `DocsStore`
  (`registry1` snapshot row) — offline restarts keep full state, cursor, and queue.
- **RegistryClient** (`crates/sync/src/registry.rs`): WS transport — hello/cursor handshake,
  push/ack, rows broadcasts, presence, probe/redial liveness (same deaf-socket discipline
  as `RoomClient`), reconnect with backoff. Fills the same `RoomStatsSnapshot` the
  SyncStatus RPC and `zeron sync` already render.

## Wire protocol (JSON text frames)

Client→server:

| frame | shape |
|---|---|
| hello | `{t:"hello", cursor: number\|null, device}` — must be first |
| push | `{t:"push", batch, ops: [Op]}` |
| presence | `{t:"presence", at}` |
| probe | `{t:"probe"}` |

Server→client:

| frame | shape |
|---|---|
| state | `{t:"state", seq, full, rows, gcFloor, presence}` — hello reply; `full=false` ⇒ rows are the `seq > cursor` delta |
| rows | `{t:"rows", seq, rows}` — merged full rows for every touched row, broadcast to all sockets |
| ack | `{t:"ack", batch, seq, applied}` |
| presence | `{t:"presence", device, at}` |
| probe-ok | `{t:"probe-ok", seq}` |

`Op = {kind, id, op: "upsert"|"update"|"delete", set?: {field: value|null}, hlc, clocks?}`
`Row = {kind, id, seq, deleted, delHlc?, fields, clocks}`

## Merge rules (identical in TS and Rust; shared test vectors)

- HLC strings `"{ms:013}-{counter:06}-{device}"` — lexicographic order = causal order,
  device id breaks ties totally.
- A field set applies iff its clock > the stored clock for that field. `null` deletes the
  field (still a clocked write).
- `update` ops never create or revive rows (the old "never invent rows" discipline);
  `upsert` creates, and revives a tombstone iff newer than `delHlc`.
- `delete` tombstones iff newer than every… no: iff newer than `delHlc`; fields/clocks are
  cleared. Tombstones GC after 30 days (daily alarm); `gcFloor` forces a full resync for
  cursors older than the horizon.
- Re-applying any op is a no-op (`>` compare) — reconnect re-pushes are idempotent by
  construction.

## Cursor + recovery

- `seq` bumps once per accepted batch; every touched row is stamped with it. Delta sync is
  `SELECT * WHERE seq > cursor`. The room's head is derived from the table (`MAX(seq)`, or
  `gcFloor` when GC removed the top rows), so it can't disagree with the rows.
- Server behind the client (`state.seq < cursor`, e.g. wiped DO storage): the client keeps
  its rows and re-seeds the server from them with **original per-field clocks** (`clocks`
  on the op) — the ws4 manual repair recipe, automated and lossless.
- Local-only rows on a full resync re-seed the same way; unpushed writes always live in
  the pending queue and replay over whatever the server returns.

## Write cost

The room is billed per SQLite row written, index entries included. A push writes only the
rows it moves: two billed rows each (the row and its `rows_seq` entry). Nothing is stored
per push besides: the stored copy of `seq`, the per-device push counts and the push log
are kept in memory and flushed by the alarm at most every five minutes, and the backup
mark is written once per backup cycle. `/stats` shows `writesSinceStart`, the billed rows
the object wrote since it last started.

Writers keep their rows quiet too:

- A session row is restamped (`updatedAt`) only while it says something that expires
  (a live status, running subagents, background processes, a wait), and at most every
  `SESSION_HEARTBEAT_MS` (25 s, checked every 5 s, so never more than 30 s apart).
  Shipped readers drop such a row after 45 s with no other signal, so this is as slow as
  they allow with room for delivery and clock skew. A session parked idle
  after its turn is not restamped at all; it used to be every 15 s for 30 minutes.
- `lastMessagePreview`/`lastMessageAt` are written when a message is sent and when a
  segment ends, never per streamed delta.
- Host stamps that repeat every turn (harness session, source context, branch, checkout,
  cwd, `roomGen`) push only fields whose value changed.

Rolling the edge back past the derived head needs `REGISTRY_ROOM_WRITE_THROUGH=1` and one
flush (five minutes) first: older code reads the stored `seq`, and a stale one would stamp
new rows below cursors clients already hold.

## Migration (instant, on first boot after update)

`WorkspaceHost::open`: if no `registry1` snapshot exists, read the legacy `workspace2`
Loro snapshot, convert every row into pending upsert ops (HLCs derived from the rows' own
timestamps, so genuinely-newer live writes beat migrated values), and save. The UI reads
the overlay immediately — zero visible gap. Each device seeds the same converged values
idempotently; the old `ws4` rooms are simply never joined again (hibernated, ~zero cost).
The legacy snapshot is retained for rollback.

## Parity notes

- Presence stays ephemeral: in-memory map in the DO, 15s beats, 45s freshness window,
  same relay-probe fallback and deaf-socket tripwire in the host.
- Per-device push attribution (`pushOutcomes`) is on `/registry/:orgId/stats` from day one —
  the only per-device surface that made the 2026-08-05 incident debuggable.
- Nightly R2 backup of the row table (seq-monotonic guard), `/registry/:orgId/rows` repair
  read, `POST /registry/:orgId/reset` operator wipe (fleet re-seeds automatically).
- Legacy `/workspace/*` routes remain for older engine builds; nothing new joins them.

## Repository looks

A project's name and icon belong to its repository, so every copy of it shows the
same ones. A `repositoryLooks` row holds them, keyed by the sha-256 of the shared
`repositoryId` (which may name a subfolder in any spelling, so can't be an id itself).
A `local:` or absent identity shares nothing: that folder keeps its own fields.

A set writes `name` or `icon` to the look row and to the one space it was made on, in
one batch. `RegistryDoc` decides what a space reads, per field: the newest clock among
the look row and its copies' fields. So disagreements settle with no migration, and
what an older engine or a Cloud runner (refused this kind, and not shown it) writes to
its own row still reaches copies on current engines. A field written by a row's
creating upsert (same clock as `createdAt`) counts only if it says something: a null,
or the folder's own name, is no choice. Older engines ignore the unknown kind and read
each row's own fields. The row outlives its copies: a repository added again gets its
look back.

## Sidebar sections

Signed-in profiles store section metadata in `sidebarSections` rows and session
placement in `sidebarLocations` rows in the same per-user registry as pins.
A session's single `location` field is either a section ID, `pinned`, or empty.
Concurrent placements converge by field clock; renaming and collapsing a section
update separate fields. Deletion marks the section deleted without deleting chats.
Archived chats retain membership. Section IDs are never reused or revived.

Sections and their members keep a placed order through `orderKey` fields, minted
between the neighbours' keys as pins are. A section's key lives on its own
`sidebarSections` row, so concurrent moves of one section converge by clock and
moves of different sections both survive. Sections no move has keyed yet (all of
them before the first reorder, and any an older app creates) follow the keyed
ones by creation. Moving sections is desktop-only and needs an engine advertising
`sidebar-section-order-v1`; apps that predate it keep showing creation order.

The existing sidebar preferences watch includes sections. Pin and section intents
share the UI's ordered optimistic queue; a rejected write reveals the latest
confirmed state. Later pin toggles from older apps still take effect through their
existing `pinned` field clocks. Section syncing itself requires an updated app.

Legacy per-profile UI sections import after an authoritative registry snapshot.
Import skips existing section IDs (including deleted ones) and existing explicit
placements, so replay does not overwrite newer edits. The engine persists the
registry snapshot and outbox before acknowledging migration; only then does the
UI remove its legacy copy. A failed migration retains that copy and can retry on
a new engine attachment/restart. Local workspaces continue using local settings.
The deployed generic registry protocol already accepts these row kinds.
