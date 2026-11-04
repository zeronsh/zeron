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

### Scoped Scaffold host transport

A verified device grant with `session.sync.write` may use the existing typed
RegistryClient transport only when its device is exactly
`comet-scaffold-<sandboxId>-e<lifecycleEpoch>`. The private host profile must use
that same device ID and must never reuse a user's broad registry cache.
HTTP initial/delta/repair and WebSocket state/broadcasts expose only the bound
chat/status row, device row, and spaces owned by that single-session sandbox
device. Spaces may arrive before the chat through upstream's normal separate
pending batches; host-written space paths must stay beneath `/workspace`.

The room validates typed columns, device ownership, owner/project/deployment,
and current lifecycle binding before mutation. Scoped hosts cannot delete rows,
reset the registry, read stats or push targets, or use arbitrary registry kinds.
The controller retains Crew environment/route metadata and its clock even when
an empty host profile initially upserts a chat without that metadata. A newer
binding fences old hosts regardless of their LWW timestamps. Full user access
continues to see the canonical host updates.

Device rows accept nullable `devboxProvider` (`namespace` or `boat`) and nullable
string `devboxId`, including independent field clears. These are native host
metadata, not authorization: the row ID must still equal the granted device ID.

The native host derives registry completeness from its verified profile. A
single-session device view can be synchronized without being the owner's complete
registry: it neither initializes nor prunes owner sidebar preferences, and it
never reports those preferences as synchronized. Device, chat and session lists
retain their normal synchronization behavior.

Hello and presence identities come from the verified grant; forged devices are
rejected. Native RegistryClient presence covers WebSocket and HTTP fallback,
without a second DeviceRoom-to-Registry presence bridge. DeviceRoom's liveness
alarms and established-socket grant revocation checks remain in place.

### Crew directory projection

`crew_directory::start(auth, services, workspace, doc_host)` returns the worker
handle owned by the engine shutdown path. It observes registry changes (including
ACKs whose optimistic sidebar values did not change), snapshot commits, and auth
changes. `chat_sync_jobs` retains coalesced pending directory work across restart;
`crew_directory_state` retains only locally observed source-version evidence and
whether the directory acknowledged it, including after transcript deletion.

Live registry rows without a local Loro transcript snapshot remain pending without
directory get/upsert requests, including on repeated drains. Saving a local
snapshot queues and wakes directory work; publication then follows the existing
identity and version fences. Acknowledged deletions can still use retained local
source-version evidence after their transcript snapshot has been removed.

The projection uses the acknowledged chat row's real `seq` as `r:registry1`,
alongside the saved SessionDoc oplog version vector (`s:<peer>`). Pending overlay
fields are never published under an older acknowledged sequence.

A server reset renumbers every row, so a replica that observes one stops
publishing (`directory_reconciliation_required`): renumbered rows would reuse
fences it already sent. Only an answer below the cursor its own request carried
counts. The pull and the socket apply answers concurrently, so an answer whose
cursor has moved since (`RegistryDoc::apply_answer`) is dropped, as is a late
duplicate `state`. Older builds applied those as resets. An answer below the
cursor its request carried always applies, so a reset is never missed. The
directory worker clears the flag once three things hold
(`RegistryDoc::clear_directory_reconciliation`):

- no chat's acknowledged `seq` is below the fence this device sent for it;
- no chat row is ahead of the cursor: a row kept across a reset keeps its old
  `seq` until the wiped server acknowledges it again;
- no chat write awaits the server.

A false alarm clears at once. After a real reset the flag stays until the new
sequence passes this device's own fences, or clears once its re-seed is merged
if it sent none. From then on its fences never fall below or collide with its
own. The directory still refuses them below another device's pre-reset fence,
and accepts them above, the same exposure a fresh device has. While the flag
stays, the worker keeps its jobs and doesn't attempt to publish. New same-owner
registry chats without Crew metadata use `CrewServices::adopt_chat` and wait for
its registry acknowledgement. The existing title lifecycle remains responsible
for generated titles; directory publishing performs no inference.

Only the verified profile and exact deployment are used for directory get/upsert.
Platform checks existing chat2 ownership through the read-only
`/session/<UUID>/directory-authority?deploymentId=<exact>` endpoint. Empty chats
without a chat2 owner stay pending: indexing never opens, claims, attaches, or
executes a chat. Cached snapshots are projected one at a time (32 MiB compressed
input cap), with bounded registry/work pages and chunked transcript text scanning.
Slack/Notion URLs retain the source extraction semantics; server upsert and
SourceURL search share canonical normalization.

Each publication attempt with eligible local evidence reads the current directory
revision before an expected-revision upsert. Stale/concurrent transcript evidence
waits for synchronization; unseen
server counters are never copied into a candidate. A server sequence rollback
persists a reconciliation-required flag, even when ordinary registry re-seeding
succeeds. Directory writes then fail closed; recovery requires reconciling the
directory and registry lineage rather than inventing a higher source version.

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
  `SELECT * WHERE seq > cursor`.
- Server behind the client (`state.seq < cursor`, e.g. wiped DO storage): the client keeps
  its rows and re-seeds the server from them with **original per-field clocks** (`clocks`
  on the op) — the ws4 manual repair recipe, automated and lossless.
- Local-only rows on a full resync re-seed the same way; unpushed writes always live in
  the pending queue and replay over whatever the server returns.

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

## Explicit Crew legacy import

Crew's project-scoped `workspace3` is **not** the upstream `workspace2`
automatic migration. Use
`zeron_sync::crew_import::import_legacy_crew_backup(backup, destination, principal, device_id)`
off the UI/runtime thread. The caller supplies a freshly verified controller
principal; a manifest is not authentication. The destination must not exist and
must be outside the immutable backup. Never select the live old profile.

For fenced cloud imports, `comet import-remote-backup` and
`comet verify-remote-backup` require `--destination-root` (the absolute native
`COMET_DATA_DIR`), `--resource` (control origin), `--edge-origin`, `--context-file`
and `--backup`. Both derive the destination with
`zeron_engine::profile::scaffold_profile_directory`, the same pure helper used
by `EngineProfile`: `<COMET_DATA_DIR>/profiles/scaffold/<scope_key>`, never the
data root itself. The scope key uses the existing `sandbox_storage_key` over
control resource, Edge origin, user, project, identity deployment, device
deployment, logical session and sandbox. Lifecycle epochs and credentials are
not inputs; authority verification and fencing remain the provider's job.
The CLI rejects incomplete bound context and returns the exact directory as
`destinationProfile`; the provider must preserve that path for native readback.

After stopping every legacy writer and checkpointing SQLite, inventory a copied
profile in `crew-backup.json` using `CrewBackupManifest` version 1:
`writerStopped`, `sourceProfileRoot` (the original absolute profile path),
`ownerSubject`, `projectId`, `deploymentId`, `files` (every relative file path
mapped to its SHA-256), and `chats` (chat IDs mapped to
`CrewChatMetadata`: owner, exact session scope, session ID, environment and
credential-free Agent Auth route). The inventory excludes the manifest itself.
Keep attachment, native-history and settings paths relative to the profile.
For every OMP native resume, include `nativeHistories` mapping the canonical
native ID to an inventoried `native-histories/<root>/sessions/…` journal copy,
with referenced native bytes in sibling `blobs/<sha256>` files. Header IDs and
initial hashes are checked; missing OMP histories are errors. At startup
`imported_native_histories(profile)` feeds a one-time atomic hydration into
`native-resumes/<nativeID>.jsonl`. The inventoried raw journal stays unchanged;
only this durable launch copy may append. Host-only selectors for both resume
and fork use the launch copy, retaining the exact native ID and cwd. Restart
validates and reuses it, never overwriting native followup turns with the archive.
OMP's source-backed `SessionManager` resolves/writes blobs in `getBlobsDir()`,
not relative to an absolute `--resume` journal. Initial hydration therefore
inlines SHA-256-verified bytes using the existing historical-attachment contract;
provider image-URL blobs contain the original UTF-8 data URL, not raw image bytes.
Unavailable historical attachments receive the existing explicit markers; other
missing/corrupt native image fields fail before publishing the launch copy.
New native-written blob references in launch/branch journals resolve against the
configured native runtime store, including when preparing a later fork. No
undocumented blob-root setting or transcript reconstruction is used.

The runnable release smoke is the ignored engine test
`imported_native_blobs_resume_in_real_omp`: provide `COMET_TEST_OMP_EXECUTABLE`,
`COMET_TEST_OMP_MODEL`, `COMET_TEST_OMP_BLOB_PNG`,
`COMET_TEST_OMP_VISUAL_NONCE` (text visible only inside a PNG of at least 1 KiB),
and `COMET_TEST_CREW_AUTH_ROOT` (an isolated profile authenticated through normal
Crew browser login, containing `scaffold-session.json`). The test derives the
configured origins/scope from that session, re-verifies it with `Auth::detect`,
and uses `CrewServices::prepare_inference` plus the ordinary `RunContext` route
and auth-gateway extension. Never copy provider credentials or replace managed
inference with a direct provider key. Device/sandbox authority is rejected.
Run `cargo test -p zeron-engine imported_native_blobs_resume_in_real_omp -- --ignored --nocapture`
with a disposable `HOME` and documented `PI_CODING_AGENT_DIR`, a managed vision
model, and no `OMP_PROFILE`/`PI_PROFILE` override. `COMET_TEST_*` values are test
inputs, not claimed OMP settings. Select the actual installed release executable;
on the inspected host, `command -v omp` resolves through
`/Users/czhen/Library/Application Support/Tern/omp/bin/omp` to
`/Users/czhen/Library/Application Support/Tern/omp/versions/18.12.3`.
The check performs native resume, restart/resume with a new attachment, and fork/inference;
requires that OMP actually externalizes a new blob reference and that the fork
hydrates those runtime-written bytes without an unavailable-attachment marker;
checks exact native IDs, recalls the visual nonce and both old/new textual
tokens, and checks the raw archive remains unchanged. Byte-only hydration tests
are not that smoke.

Non-OMP native resumes fail before destination creation with the original
chat/native IDs and cwd reported: an isolated external-native-store importer
is a release/cutover prerequisite, not an approved retirement of those sessions.
No live old-store pointer is carried into the new runtime. External worktree
associations remain unchanged and require a separate consistent backup.
Credentials are excluded and must be reauthenticated; no grants become
registry authority.
The upload resolver loads `imported_attachment_paths(profile)` once at startup:
only inventoried original upload paths alias to the copied files. It never
falls back to reading the old live profile. Pending upload chunks are an error.
Exclude `session.json`, Scaffold session/bootstrap files and the entire
secret-bearing `agent-accounts` directory, not just files named `auth.json`.

The importer validates the entire input before creating the destination. It
preserves chat/native IDs, raw transcript data and native cwd, environment,
semantic annotations, copied asset hashes and all original database bytes
(`legacy-crew.sqlite3`). Terminal command outcomes and semantic journal records
are additionally accessible through `DocsStore::legacy_crew_records(chat_id)`;
they are evidence, never an execution queue. Claimed pending commands remain
claimed; unclaimed pending work and conflicting semantic intents fail closed.
The new personal `registry1` is the only discovery index. Host disconnection
does not remove entries. Import does not attach, launch a harness or publish
to the legacy workspace.

Foreign/granted sessions require authenticated reattachment rather than
relabeling them as owned. Missing host rows/snapshots, pending nudges, unresolved
startup/native-fork state, delayed worktree deletion, unknown tables or workspace
containers, malformed rows, mismatched scope and unlisted/tampered files are
explicit errors. Reconcile those in the legacy lane, then take a new backup.
There is no automatic unsafe replay or silent record dropping.
The known `session_startups` table is accepted only when empty, without requiring
legacy identity columns; bootstrap classification likewise skips that empty
sidecar without modifying it. Any startup row still requires legacy reconciliation
before snapshot loading or destination creation. Every stored scope must match
the verified importing principal as-is;
missing or mixed deployment bindings are not relabeled to make import succeed.
Inspection and source-evidence classification stream SQLite snapshots through the
same historical-reference checks, retaining snapshot identities and one decoded
document rather than every transcript. The repository pins a native-only
`loro-internal` 1.13.9 patch for read-only change metadata and per-operation visitors;
`vendor/loro-internal-1.13.9/UPSTREAM.json` records archive provenance and local changes.
Causal validation walks metadata with the native shallow-root baseline instead of
materializing ancestor version-vector caches. Retained-operation comparison hashes
canonical framed values without exporting the entire history. A shallow or incomplete
source still cannot prove that a bootstrap document has no historical associations.
Export and import reconcile one canonical chat and its private execution aliases at
a time in a private temporary SQLite worktable. Import also spools converted snapshots
and original evidence there, preserves the original semantic-journal order, and reads
back expected/actual records incrementally. No source or live profile is modified;
the destination is created only after all semantic validation succeeds.
Memory still depends on the largest workspace, canonical alias group, individual
document/change and compact identity/outcome indexes; there is no content-dropping
size limit. Qualify peak memory on a representative immutable copy before authorizing
a real-profile import. A database-only observation without native archives or verified
source authority is not a complete backup, and fixture success is not that qualification.

The public inspection API completed on an immutable 3,037-snapshot, 715,402,552-byte
database copy in 95.83 s: 254.94 MiB lifetime peak footprint, 236.17 MiB sampled RSS,
and identical source database SHA-256 before/after. The watchdog retained its 1 GiB
stop threshold; the full-corpus allowance was 120 s after the 15 s diagnostic expired.
It conservatively returned zero inert-bootstrap proofs because shallow history was
present. This verifies bounded copied-database inspection, not authenticated source
capture, native archive completeness, real-profile import, or cutover readiness.

A successful import writes `crew-import-report.json` with preserved IDs and
checksums. Failed writes retain `crew-import-incomplete`; `DocsStore::open`
rejects that destination. Keep it for diagnosis and retry into a new profile.
Retain the original backup through cutover. Once the new profile has writes,
restoring the backup alone is not a lossless rollback.

`Chat.crew` is index metadata, not authority. Restricted clients cannot write
registry rows or opaque chat blobs. Annotation mutations use host-derived
principal/grant checks and `SessionDoc::apply_annotation_command`; annotations
and immutable command results share one Loro commit in `crewAnnotations` and
`crewAnnotationCommands`. Both maps survive thin-document rebuilds. After a
crash between snapshot persistence and external command completion, recover
with `annotation_command_result`; never infer the result from a later-edited
annotation or replay a claim with no persisted result. Durable
Crew control claims store immutable request bytes before delivery, retain an
unknown outcome across restart, and reject conflicting terminal outcomes.
