# chat3: a transcript plane beside the chat2 log

Status: BEHIND A FLAG · Flag: `preferences/labs.chat3` (per user, synced; `zeron labs chat3 on`) · Room gen: 3
Prior art: `docs/chat2-sync.md` (the relay this keeps), `docs/registry-sync.md`.

## Why

chat2 made the edge dumb and unwedgeable. Two costs remain, and both scale with how
long a chat runs, not with what the user sees:

1. **Every streamed commit is a billed row.** The host commits the folded segment every
   `STREAM_COMMIT_MS` (120 ms), every commit publishes one Loro update, and every update
   is one `rows` insert in the room. On top of that the room rewrites `headSeq`,
   `backupDirty` and the `pushOutcomes` JSON per push, the `batch_id UNIQUE` index costs a
   row, and a threshold checkpoint deletes every 200 rows (deletes are billed writes too).
   About 7 rows written per push, up to ~8 pushes/s while text streams.
2. **A cold open downloads the whole chat.** The transcript is one Loro doc, so a new
   device, the web client or an evicted chat fetches the full checkpoint and imports
   every entry before it can paint, no matter how far back the history goes.

And one gap: tool outputs never reach the transcript. claude, codex and cursor drop
them at the harness, the fold clears what ACP sends, and the parked R2 sidecar never
uploads. Users see one-line chips for commands that printed pages.

## What changes

A gen-3 chat keeps the chat2 log for the state the devices actually co-edit, and moves
the transcript into a **plane of server-ordered rows** that only the host writes.

| Data | Writers | gen 2 | gen 3 |
|---|---|---|---|
| `meta`, `commands`, `queue` | every device | chat2 Loro log | chat2 Loro log (unchanged, smaller doc) |
| `messages` (entries + parts) | the host only | same Loro log | **items**: one row per entry header / part |
| in-progress text, running tools | the host only | a log row every 120 ms | **live frames**: relayed, never stored |
| full tool output | — | dropped | **outputs**: one row per tool, fetched on expand |

Clients never write `messages` (the inventory behind this doc found no client-side
writer outside the in-process demo host), so a single-writer row model needs no CRDT.

### Local model: one `SessionDoc`, two Loro docs

`SessionDoc` gains a second `LoroDoc` for the transcript. For gen ≤ 2 both handles are
the same doc, so nothing changes. For gen 3 the transcript doc is **local only**:

- On the host the engine writes it exactly as today (`SegmentWriter` at 120 ms,
  `push_message`, status flips). Its commits are cheap SQLite snapshots, not edge rows.
- On readers it is a cache the room fills: items and live frames are applied as local
  writes (commit origin `zeron:transcript-replay` for history, so it does not animate).
- Every transcript reader in the codebase (the engine's `publish_messages_locked` for
  desktop, the client `Tracker` for mobile, `TranscriptHistory`) keeps reading
  `messages` through `SessionDoc`; only their subscriptions move to the transcript doc.

Only the state doc is published through the chat2 `ChatClient`, so its log carries
commands and queue edits, not tokens.

### Items

Each item is one row keyed by `(ord, idx)`:

- `idx = -1` is the **entry header**: `{id, ord, role, createdAt, deviceId, status?,
  continuationOf?, durationMs?}`.
- `idx ≥ 0` is **part `idx`** of that entry, as its doc JSON (the same shape
  `read_entries` decodes).

`ord` is the entry's position, a float the host assigns once when the entry is created:
`max(last + 1, now_ms)` for appends, the midpoint of its neighbours for the
provider-switch seam's `insert_message_before_last`. The wall-clock floor means a host
whose local transcript is behind the room (a moved or restored host that hasn't paged
history in yet) still appends after every existing row instead of reusing a key, so it
never has to block on catching up. The `ord` is stored on the local entry map and never
changes, so a row's key is stable for its whole life.

Rows are upserts. Each write takes the next `rev` (a room-wide counter, indexed), which
is the change feed readers resume from.

### When the host persists

The host's publisher watches the transcript doc and classifies each changed part:

| Part state | Stored | Live |
|---|---|---|
| last text/reasoning part of a streaming entry | every 15 s if changed (durability) | `append` frames each commit |
| running tool (unresolved) | once when it appears | `part` frames as its output tail grows |
| anything else (settled text, resolved tool, input, error, image…) | on every change | — |
| entry header | on first sight and when status/duration change | — |

A text part settles when a later part appears or the entry leaves `streaming`. So a
turn costs about one row per part plus a header or two, regardless of how long it
streamed. Stored writes go through a durable local outbox keyed by `(doc, ord, idx)`
that coalesces to the latest bytes, and are POSTed in batches. A crash replays the
outbox; a dropped live frame only delays text until the part settles.

### Live frames

`live{entry, ord, idx, op}`: `op: "part"` carries the full part JSON; `op: "append"`
carries `at` (the UTF-8 length the sender had before) and the appended text. The room
relays them to other sockets and keeps the current open parts in memory (bounded) so a
viewer who joins mid-stream gets them with its next read. Nothing is written. A stored
row for the same key drops the live copy. A `part` frame also carries its entry's
header row, because live frames ride the socket and can beat the stored header (which
the host uploads by HTTP). A reader applies an `append` only when `at` matches its
local length; one that doesn't line up asks for a feed catch-up, whose response
replays the room's copy of the open part.

### Pagination

`GET /chat3/{id}/items?limit=N[&before=ord:idx]` returns the newest `N` rows before the
cursor in ascending order, plus the header of a partially included first entry, and
`more` when older rows exist. `?since=rev` returns the change feed. A fresh reader paints
from one page; older pages load when the transcript asks for them (`LoadEarlierMessages`
on desktop, `load_earlier` on mobile). A reader keeps `partBase` on an entry it only
holds the tail of, so partially loaded entries render their loaded parts. Updates to
rows older than the loaded window are ignored; paging returns their current version.

A host pulls the change feed in the background when it joins and adopts rows an
earlier host wrote (recorded as stored, never re-sent); it uploads only what it wrote
itself. Because of the `ord` floor this never gates running a command.

### Tool outputs

- Harnesses now emit output: claude reads `tool_result` content, codex reads
  `aggregatedOutput` and streams `item/commandExecution/outputDelta` as the new
  `AgentEvent::ToolOutputDelta`. Output is capped at `TOOL_OUTPUT_CAP` (256 KiB,
  head and tail kept) at the harness.
- gen 2 is unchanged: the fold still keeps outputs out of the doc, and output deltas
  are not journaled.
- gen 3: while a tool runs, its part carries the last lines of output (local doc only,
  streamed as `part` live frames — viewers watch a build scroll). On `ToolResult` the
  host queues the full text (durable local outbox) for `/chat3/{id}/outputs/{partId}`
  and the part settles with the one-line summary, `outputBytes` and
  `outputRef = chat3:{chatId}/{partId}`. The desktop's existing "Show full output
  (N KB)" affordance fetches it through `FetchToolBlob`, which routes `chat3:` refs to
  the room (or to the host's own queue if it hasn't uploaded yet). iOS gets the live
  tail and `SessionHandle.toolOutput`; its full-output viewer is a follow-up.
- Outputs are not in the nightly R2 backup (they are large and re-derivable on the
  host); transcript rows are.

Outputs live in the user's owner-only room, the same trust boundary as the transcript.
Tool *inputs* keep their existing `sanitize_tool_call` strip.

## Flag and rollout

- `preferences/labs` registry row, field `chat3: bool`. It syncs to every device of the
  user. Only chat minting reads it: a new chat is born gen 3 when it is on. Turning it
  off affects new chats only; gen-3 chats stay gen 3.
- Switch it with `zeron labs chat3 on|off` (or the `SetLabsFlag` RPC). Mint sites in the
  engine (`CreateChat`, forks) and the app client read it. A device that opens a chat
  before its registry row has synced assumes the flag's generation (a subagent doc, its
  parent chat's); if the row later disagrees, the handle is replaced on the row's
  generation and an empty assumed-generation doc is set aside with a rollback copy.
- **Old apps read `roomGen >= 2` as chat2.** This release makes every client exact about
  the generations it knows (`MAX_KNOWN_ROOM_GEN = 3`) and opens newer ones read-only from
  local state instead of dialing the wrong room. Enable the flag for a user only once all
  their apps run this release, or an old app will dial an empty chat2 room for a gen-3
  chat.
- No migration of existing chats in this change. gen-2 chats keep working untouched.

## Edge

Same `ChatRoom` class and `CHAT_ROOMS` namespace (no new DO migration); a gen-3 room is
named `chat3/{chatId}`. The chat2 surface (`ws`, `rows`, `checkpoint`, `stats`, `reset`)
is served unchanged on `/chat3/...` for the state log. New:

| Route / frame | What |
|---|---|
| `POST /items` | host batch upsert (framed `itemPut`s); relays `item` to sockets |
| `GET /items?limit&before` / `?since` | page / change feed (framed `item`s + `itemsDone`) |
| `PUT/GET /outputs/{partId}` | full tool output, chunked like blobs |
| WS `live` (0x11) | relay + in-memory open parts; never stored |
| WS `item` (0x12), `itemsDone` (0x13) | server → client |

Tables (`WITHOUT ROWID` so the key is the table, no second b-tree):
`items(ord REAL, idx INTEGER, entry TEXT, rev INTEGER, bytes BLOB, PRIMARY KEY(ord, idx))`
with `items_rev ON items(rev)`, and `outputs(part TEXT, chunk INTEGER, bytes BLOB,
PRIMARY KEY(part, chunk))`.

Both generations meter their own SQL (`rowsWritten`/`rowsRead` from each cursor, the
numbers Cloudflare bills) and report them in `/stats`.

## Clients

- Desktop: the engine serves a gen-3 transcript through the same `WatchDocMessages`
  stream, plus `historyMore`; the transcript shows a "Load earlier messages" pill while
  scrolled to its first row (`FetchEarlierMessages`).
- iOS: the client crate reads gen-3 chats (split local doc, pages, feed, live frames);
  the Rust layout puts a "Load earlier messages" row first while older history exists
  and Swift answers its `load-earlier` action.

## Proving the cost

`crates/doc/examples/chat3_trace.rs` replays agent events through the real fold,
`SegmentWriter`, both doc layouts and the gen-3 publisher on a virtual clock (deltas
30 ms apart, tools 2.5 s, a 2 KB output stored per tool — chat2 stores none), and writes
what each generation sends its room. `edge/test/workerd/chat3-cost.workerd.test.ts`
replays those traces into a real gen-2 and a real gen-3 `ChatRoom` on DO SQLite and
reads what each was billed — the rooms meter their own SQL with the runtime's
`rowsWritten`/`rowsRead` — and what a cold open downloads. The recorded workload is
this feature's own build session with every text replaced by same-length filler
(`edge/test/workerd/fixtures/chat3-cost-traces.json` holds only ids, sizes and hashes).

Regenerate with
`cargo run -p zeron-doc --example chat3_trace -- <journal.jsonl> > edge/test/workerd/fixtures/chat3-cost-traces.json`.

## Measured

Rows written are what Cloudflare bills ($1.00 per million past the plan's 50 M).

| Workload | Length | chat2 rows written | chat3 rows written | chat3 / chat2 |
|---|---|---|---|---|
| Recorded session (50 entries, 787 parts, 378 tools) | 28 min | 40,341 | 3,318 | 8.2% |
| One long answer (reasoning + text, no tools) | 3 min | 9,034 | 71 | 0.8% |
| Tool-heavy turn (80 shell commands) | 4 min | 2,874 | 607 | 21% |

| Workload | chat2 cold open | chat3 cold open | chat2 stored | chat3 stored |
|---|---|---|---|---|
| Recorded session | 297.6 KB | 117.5 KB | 295.7 KB | 1,212.0 KB |
| One long answer | 63.7 KB | 62.6 KB | 58.0 KB | 62.0 KB |
| Tool-heavy turn | 39.7 KB | 37.3 KB | 35.1 KB | 187.1 KB |

- Writes: chat2 pays ~6 rows per streamed commit (row + `batch_id` index + `headSeq`,
  `backupDirty`, `pushOutcomes`, plus checkpoint deletes); chat3 pays ~2 per stored row
  (row + `rev` index), and stores rows per part. The tool-heavy turn is chat3's worst
  case: every tool part is stored twice (when it starts, when it resolves) and its output
  once — work chat2 never did.
- Cold open: chat3 downloads the newest page and the small state doc; chat2 the whole
  checkpoint. The gap grows with the chat: at 28 minutes it is already 61% less, and a
  page stays ~120 KB however long the chat runs. (A cold open can read more rows on
  chat3 — a page is 200 rows — at a thousandth of a write's price.)
- Stored bytes grow on chat3 because it now keeps full tool outputs (378 × 2 KB here),
  which chat2 dropped. Storage is $0.20/GB-month; rows written dominate.
- In dollars at $1 per million rows: the recorded session bills ~86k rows (~$0.086) per
  hour of agent work on chat2 and ~7k (~$0.007) on chat3.
- chat2's busiest minute held 472–504 pushes from one device in the streaming
  workloads — past its 300-a-minute per-device quota, so today a long stream gets
  throttled into head-only retries and viewers lag. chat3 streams on live frames, which
  the quota never sees.
