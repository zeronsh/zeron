//! chat3 transcript plane (docs/chat3-sync.md): the host turns its local
//! transcript doc into stored rows and live frames; readers apply them back
//! into theirs.
//!
//! A row is keyed by `(ord, idx)`: `idx = -1` is an entry header (the entry
//! map's scalar fields), `idx >= 0` is part `idx` as its doc JSON — the same
//! shape [`SessionDoc::read_entries`] decodes. `ord` is the entry's position,
//! stamped once by the split doc's insert paths.
//!
//! Publishing is a pure state machine over the doc ([`Publisher`]): the
//! engine feeds it the transcript doc's change events and drains
//! [`Publication`]s on its commit tick. What it persists versus streams is the
//! cost model of the whole design — see the table in the doc.

use std::collections::{BTreeSet, HashMap};

use loro::event::DiffEvent;
use loro::{
    Container, ContainerID, ContainerTrait, Index, LoroList, LoroMap, LoroValue, ToJson, ValueOrContainer,
};
use serde::{Deserialize, Serialize};

use crate::parts::MessageStatus;
use crate::schema::{
    DocError, MessageRole, SessionDoc, SessionMessageEntry, entry_ord, insert_part, overwrite_part,
    part_from_json, push_part_pub, write_entry_fields_pub,
};

/// A streaming text part is also stored at most this often (if it changed),
/// so a viewer who joins after the room restarted mid-part still sees it.
pub const DURABLE_MS: i64 = 15_000;


/// A row key. `idx = -1` is the entry header.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ItemKey {
    pub ord: f64,
    pub idx: i64,
}

impl ItemKey {
    pub fn header(ord: f64) -> Self {
        Self { ord, idx: -1 }
    }

    /// Hashable/orderable identity (`f64` is neither).
    pub fn bits(&self) -> (u64, i64) {
        (self.ord.to_bits(), self.idx)
    }

    /// `ord:idx`, as the room's `?before=` cursor expects.
    pub fn cursor(&self) -> String {
        format!("{}:{}", self.ord, self.idx)
    }

    fn cmp_key(&self, other: &Self) -> std::cmp::Ordering {
        self.ord.total_cmp(&other.ord).then(self.idx.cmp(&other.idx))
    }
}

/// One row the host stores (`itemPut`).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub key: ItemKey,
    pub entry: String,
    pub json: Vec<u8>,
}

impl Item {
    /// Content hash recorded as "published" — FNV-1a, stable across builds.
    pub fn hash(&self) -> u64 {
        fnv1a(&self.json)
    }
}

/// One row as a reader receives it (page, feed, or relay).
#[derive(Debug, Clone, PartialEq)]
pub struct ReceivedItem {
    pub key: ItemKey,
    pub entry: String,
    pub rev: u64,
    pub json: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum LiveOp {
    /// The part's full doc JSON.
    Part(Vec<u8>),
    /// Text appended to the part's body after UTF-8 offset `at`.
    Append { at: usize, text: String },
}

/// One live frame (relayed, never stored).
#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    pub key: ItemKey,
    pub entry: String,
    pub op: LiveOp,
    /// A `Part` frame carries its entry's header row JSON, so a reader that
    /// hasn't received the stored header yet (it travels by HTTP, live frames
    /// by socket) can still place the part.
    pub header: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Publication {
    Store(Item),
    Live(Live),
}

// ── host: publisher ─────────────────────────────────────────────────────────

#[derive(Default)]
struct PartTrack {
    /// Hash of the last stored version.
    stored: Option<u64>,
    stored_at: i64,
    /// Body length (UTF-8) the viewers have, for `append` frames.
    live_len: Option<usize>,
    live_hash: Option<u64>,
}

struct EntryTrack {
    header: Option<u64>,
    parts: Vec<PartTrack>,
}

#[derive(Default)]
struct EntryDirty {
    header: bool,
    list: bool,
    parts: BTreeSet<usize>,
}

/// Host-side publisher for one split doc.
pub struct Publisher {
    known: HashMap<(u64, i64), u64>,
    entries: HashMap<ContainerID, EntryTrack>,
    dirty: HashMap<ContainerID, EntryDirty>,
    rescan: bool,
}

impl Publisher {
    /// `known`: hashes of rows already stored (persisted across restarts, so
    /// a host reopening a long chat publishes only what changed).
    pub fn new(known: HashMap<(u64, i64), u64>) -> Self {
        Self {
            known,
            entries: HashMap::new(),
            dirty: HashMap::new(),
            rescan: true,
        }
    }

    /// Re-examine the whole transcript on the next collect (open, reconnect).
    pub fn mark_all(&mut self) {
        self.entries.clear();
        self.dirty.clear();
        self.rescan = true;
    }

    /// Record what a transcript-doc change touched. Cheap: paths only.
    pub fn observe(&mut self, event: &DiffEvent<'_>) {
        for change in &event.events {
            let path = change.path;
            if !matches!(path.first(), Some((_, Index::Key(key))) if key.as_str() == "messages") {
                continue;
            }
            if path.len() == 1 {
                self.rescan = true;
                continue;
            }
            let entry = path[1].0.clone();
            let dirty = self.dirty.entry(entry).or_default();
            match path.len() {
                2 => dirty.header = true,
                3 => dirty.list = true,
                _ => {
                    if matches!(&path[2].1, Index::Key(key) if key.as_str() == "parts")
                        && let Index::Seq(ix) = path[3].1
                    {
                        dirty.parts.insert(ix);
                    }
                }
            }
        }
    }

    /// After a host pulls rows an earlier host wrote: re-examine the whole
    /// doc and count the `pulled` keys as stored (the room has them).
    /// Returns what still needs uploading — rows this host wrote but never
    /// got out — and the adopted hashes to persist.
    pub fn adopt(
        &mut self,
        doc: &SessionDoc,
        pulled: &std::collections::HashSet<(u64, i64)>,
        now_ms: i64,
    ) -> (Vec<Publication>, Vec<((u64, i64), u64)>) {
        self.mark_all();
        let mut upload = Vec::new();
        let mut adopted = Vec::new();
        for publication in self.collect(doc, now_ms) {
            match publication {
                Publication::Store(item) if pulled.contains(&item.key.bits()) => {
                    adopted.push((item.key.bits(), item.hash()));
                }
                other => upload.push(other),
            }
        }
        (upload, adopted)
    }

    /// Whether this host has stored the row at `key`.
    pub fn knows(&self, key: &ItemKey) -> bool {
        self.known.contains_key(&key.bits())
    }

    /// True when a collect would have something to look at.
    pub fn is_dirty(&self) -> bool {
        self.rescan || !self.dirty.is_empty()
    }

    /// Drain what changed into stored rows and live frames. Stored rows come
    /// out in key order (headers before their parts), live frames after.
    pub fn collect(&mut self, doc: &SessionDoc, now_ms: i64) -> Vec<Publication> {
        let messages = doc.transcript_doc().get_list("messages");
        if std::mem::take(&mut self.rescan) {
            for index in 0..messages.len() {
                let Some(ValueOrContainer::Container(Container::Map(map))) = messages.get(index) else {
                    continue;
                };
                let cid = map.id();
                if !self.entries.contains_key(&cid) {
                    let dirty = self.dirty.entry(cid).or_default();
                    dirty.header = true;
                    dirty.list = true;
                }
            }
        }
        let mut stores = Vec::new();
        let mut lives = Vec::new();
        for (cid, dirty) in std::mem::take(&mut self.dirty) {
            let Some(Container::Map(map)) = doc.transcript_doc().get_container(cid.clone()) else {
                continue;
            };
            self.collect_entry(&cid, &map, dirty, now_ms, &mut stores, &mut lives);
        }
        stores.sort_by(|a: &Item, b: &Item| a.key.cmp_key(&b.key));
        let mut out: Vec<Publication> = stores.into_iter().map(Publication::Store).collect();
        out.extend(lives.into_iter().map(Publication::Live));
        out
    }

    fn collect_entry(
        &mut self,
        cid: &ContainerID,
        map: &LoroMap,
        dirty: EntryDirty,
        now_ms: i64,
        stores: &mut Vec<Item>,
        lives: &mut Vec<Live>,
    ) {
        let Some(id) = string_field(map, "id") else {
            return; // torn import: its id lands in a later change
        };
        let Some(ord) = entry_ord(map) else {
            return; // not a split-doc entry
        };
        let parts = match map.get("parts") {
            Some(ValueOrContainer::Container(Container::List(list))) => Some(list),
            _ => None,
        };
        let parts_len = parts.as_ref().map(|p| p.len()).unwrap_or(0);
        let fresh = !self.entries.contains_key(cid);
        let track = self.entries.entry(cid.clone()).or_insert_with(|| EntryTrack {
            header: self.known.get(&ItemKey::header(ord).bits()).copied(),
            parts: Vec::new(),
        });
        while track.parts.len() < parts_len {
            let idx = track.parts.len() as i64;
            track.parts.push(PartTrack {
                stored: self.known.get(&ItemKey { ord, idx }.bits()).copied(),
                // The durability clock starts when the part appears: its
                // first copy is due DURABLE_MS later, not on its first tick.
                stored_at: now_ms,
                ..PartTrack::default()
            });
        }

        // Header.
        let header = header_json(map);
        let status = header.get("status").and_then(|s| s.as_str()).unwrap_or("").to_owned();
        let streaming = status == "streaming";
        let mut settling = false;
        if dirty.header || fresh {
            let json = serde_json::to_vec(&header).unwrap_or_default();
            let hash = fnv1a(&json);
            if track.header != Some(hash) {
                // Leaving `streaming` settles every part still held live.
                settling = !streaming;
                track.header = Some(hash);
                self.known.insert(ItemKey::header(ord).bits(), hash);
                stores.push(Item {
                    key: ItemKey::header(ord),
                    entry: id.clone(),
                    json,
                });
            }
        }

        // Which parts to look at: the dirty ones; on a list change the old
        // last part (it may have just stopped being last) and every new one;
        // on settle, everything not yet stored as-is.
        let mut examine: BTreeSet<usize> = dirty.parts;
        if dirty.list || fresh {
            let from = if fresh { 0 } else { parts_len.saturating_sub(1) };
            let prior = track.parts.iter().position(|p| p.stored.is_none()).unwrap_or(parts_len);
            examine.extend(from.min(prior)..parts_len);
            // Parts appended after a live part end its streaming: every part
            // still held live is re-examined so it settles.
            for (ix, part) in track.parts.iter().enumerate() {
                if part.live_len.is_some() || part.live_hash.is_some() {
                    examine.insert(ix);
                }
            }
        }
        if settling {
            for (ix, part) in track.parts.iter().enumerate() {
                if part.live_len.is_some() || part.live_hash.is_some() || part.stored.is_none() {
                    examine.insert(ix);
                }
            }
        }
        let Some(parts) = parts else { return };
        let header_json = serde_json::to_vec(&header).unwrap_or_default();
        for ix in examine.into_iter().filter(|ix| *ix < parts_len) {
            let Some(value) = parts.get(ix) else { continue };
            let json_value = value.get_deep_value().to_json_value();
            let kind = json_value.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            let last = ix + 1 == parts_len;
            let key = ItemKey { ord, idx: ix as i64 };
            let json = serde_json::to_vec(&json_value).unwrap_or_default();
            let hash = fnv1a(&json);
            let part = &mut track.parts[ix];
            let running_tool =
                kind == "tool" && !json_value.get("resolved").and_then(|r| r.as_bool()).unwrap_or(false);
            let live_text = streaming && last && (kind == "text" || kind == "reasoning");
            let mut store = |part: &mut PartTrack, known: &mut HashMap<(u64, i64), u64>, json: Vec<u8>| {
                part.stored = Some(hash);
                part.stored_at = now_ms;
                known.insert(key.bits(), hash);
                stores.push(Item {
                    key,
                    entry: id.clone(),
                    json,
                });
            };
            if running_tool && streaming {
                // Stored once when it appears (a long command must survive a
                // room restart); its growing output tail streams live.
                if part.stored.is_none() {
                    part.live_hash = Some(hash);
                    store(part, &mut self.known, json);
                } else if part.stored != Some(hash) && part.live_hash != Some(hash) {
                    part.live_hash = Some(hash);
                    lives.push(Live {
                        key,
                        entry: id.clone(),
                        op: LiveOp::Part(json),
                        header: Some(header_json.clone()),
                    });
                }
            } else if live_text {
                let field = if kind == "text" { "text" } else { "reasoning" };
                let body = json_value.get(field).and_then(|t| t.as_str()).unwrap_or("");
                match part.live_len {
                    Some(sent) if body.len() > sent && body.is_char_boundary(sent) => {
                        lives.push(Live {
                            key,
                            entry: id.clone(),
                            op: LiveOp::Append {
                                at: sent,
                                text: body[sent..].to_owned(),
                            },
                            header: None,
                        });
                        part.live_len = Some(body.len());
                    }
                    Some(sent) if body.len() == sent => {}
                    _ => {
                        lives.push(Live {
                            key,
                            entry: id.clone(),
                            op: LiveOp::Part(json.clone()),
                            header: Some(header_json.clone()),
                        });
                        part.live_len = Some(body.len());
                    }
                }
                if part.stored != Some(hash) && now_ms - part.stored_at >= DURABLE_MS {
                    store(part, &mut self.known, json);
                }
            } else {
                part.live_len = None;
                part.live_hash = None;
                if part.stored != Some(hash) {
                    store(part, &mut self.known, json);
                }
            }
        }
    }
}

/// The entry header row: the entry map's scalar fields (never its parts).
fn header_json(map: &LoroMap) -> serde_json::Map<String, serde_json::Value> {
    let mut out = serde_json::Map::new();
    for key in map.keys() {
        if key.as_str() == "parts" || key.as_str() == "partBase" {
            continue;
        }
        if let Some(ValueOrContainer::Value(value)) = map.get(&key) {
            out.insert(key.to_string(), value.to_json_value());
        }
    }
    out
}

fn string_field(map: &LoroMap, key: &str) -> Option<String> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => Some(s.to_string()),
        _ => None,
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

// ── reader: apply rows and live frames ──────────────────────────────────────

/// Commit origin for page/feed application, so presentation treats it as
/// history (no streaming animation) — the import origin chat2 replays use.
pub const PLANE_REPLAY_ORIGIN: &str = "zeron:transcript-replay";

/// What a reader persists about its window of the plane. Lives in the local
/// transcript doc's `plane` map, so it saves with the transcript snapshot.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PlaneCursor {
    /// Highest rev applied (the change feed resumes after it).
    pub head_rev: u64,
    /// Oldest row held; `None` until the first page lands.
    pub oldest: Option<ItemKey>,
    /// Older rows exist on the server.
    pub more_before: bool,
}

/// Reader-side state for one split doc (in memory; rebuilt per open).
#[derive(Default)]
pub struct Reader {
    /// rev of the newest version applied per row, so a late relay can't
    /// overwrite a newer feed row.
    revs: HashMap<(u64, i64), u64>,
    /// Parts that arrived before the rows between them and the window.
    pending: HashMap<String, Vec<ReceivedItem>>,
}

impl Reader {
    pub fn cursor(doc: &SessionDoc) -> PlaneCursor {
        let plane = doc.transcript_doc().get_map("plane");
        let num = |key: &str| match plane.get(key) {
            Some(ValueOrContainer::Value(LoroValue::I64(v))) => Some(v),
            Some(ValueOrContainer::Value(LoroValue::Double(v))) => Some(v as i64),
            _ => None,
        };
        let oldest = match (plane.get("oldestOrd"), num("oldestIdx")) {
            (Some(ValueOrContainer::Value(LoroValue::Double(ord))), Some(idx)) => Some(ItemKey { ord, idx }),
            (Some(ValueOrContainer::Value(LoroValue::I64(ord))), Some(idx)) => Some(ItemKey { ord: ord as f64, idx }),
            _ => None,
        };
        PlaneCursor {
            head_rev: num("headRev").unwrap_or(0).max(0) as u64,
            oldest,
            more_before: matches!(plane.get("moreBefore"), Some(ValueOrContainer::Value(LoroValue::Bool(true)))),
        }
    }

    /// Advance the feed cursor without applying anything (a host whose own
    /// uploads were the only rows written).
    pub fn note_head_rev(doc: &SessionDoc, rev: u64) -> Result<(), DocError> {
        let mut cursor = Self::cursor(doc);
        if rev <= cursor.head_rev {
            return Ok(());
        }
        cursor.head_rev = rev;
        Self::save_cursor(doc, &cursor)?;
        doc.transcript_doc().commit();
        Ok(())
    }

    fn save_cursor(doc: &SessionDoc, cursor: &PlaneCursor) -> Result<(), DocError> {
        let plane = doc.transcript_doc().get_map("plane");
        plane.insert("headRev", cursor.head_rev as i64)?;
        if let Some(oldest) = cursor.oldest {
            plane.insert("oldestOrd", oldest.ord)?;
            plane.insert("oldestIdx", oldest.idx)?;
        }
        plane.insert("moreBefore", cursor.more_before)?;
        Ok(())
    }

    /// Apply a page (`more` = older rows exist before it). `older` is a page
    /// requested before the reader's oldest row: whatever it returns, the
    /// window now reaches its start, so `more` is the new `more_before`. A
    /// fresh newest page only extends the window if it reaches further back.
    pub fn apply_page(
        &mut self,
        doc: &SessionDoc,
        items: &[ReceivedItem],
        more: bool,
        head_rev: u64,
        older: bool,
    ) -> Result<usize, DocError> {
        let mut cursor = Self::cursor(doc);
        let first = page_start(items);
        let extends = first.is_some_and(|f| cursor.oldest.is_none_or(|o| f.cmp_key(&o).is_lt()));
        if extends {
            cursor.oldest = first;
        }
        if older || extends || cursor.oldest.is_none() {
            cursor.more_before = more;
        }
        cursor.head_rev = cursor.head_rev.max(head_rev);
        let changed = self.apply(doc, items)?;
        Self::save_cursor(doc, &cursor)?;
        commit(doc, true);
        Ok(changed)
    }

    /// Apply change-feed or relayed rows. Rows older than the window are
    /// skipped while older history exists (paging returns their current
    /// version); their revs still advance the cursor. Only pages move the
    /// window.
    pub fn apply_feed(&mut self, doc: &SessionDoc, items: &[ReceivedItem], head_rev: Option<u64>, replay: bool) -> Result<usize, DocError> {
        let mut cursor = Self::cursor(doc);
        let max_rev = items.iter().map(|i| i.rev).max().unwrap_or(0);
        cursor.head_rev = cursor.head_rev.max(max_rev).max(head_rev.unwrap_or(0));
        let in_window: Vec<ReceivedItem> = items
            .iter()
            .filter(|i| !cursor.more_before || cursor.oldest.is_none_or(|o| !i.key.cmp_key(&o).is_lt()))
            .cloned()
            .collect();
        let changed = self.apply(doc, &in_window)?;
        Self::save_cursor(doc, &cursor)?;
        commit(doc, replay);
        Ok(changed)
    }

    fn apply(&mut self, doc: &SessionDoc, items: &[ReceivedItem]) -> Result<usize, DocError> {
        let mut sorted: Vec<&ReceivedItem> = items
            .iter()
            .filter(|item| self.revs.get(&item.key.bits()).is_none_or(|rev| item.rev >= *rev))
            .collect();
        sorted.sort_by(|a, b| a.key.cmp_key(&b.key));
        let messages = doc.transcript_doc().get_list("messages");
        let mut changed = 0;
        // Headers first (an entry must exist before its parts can land).
        for item in sorted.iter().filter(|i| i.key.idx < 0) {
            if upsert_header(&messages, item)? {
                changed += 1;
            }
            self.revs.insert(item.key.bits(), item.rev);
        }
        // Parts grouped per entry, so an older page prepends contiguously.
        let mut by_entry: Vec<(String, Vec<ReceivedItem>)> = Vec::new();
        for item in sorted.iter().filter(|i| i.key.idx >= 0) {
            match by_entry.iter_mut().find(|(id, _)| *id == item.entry) {
                Some((_, group)) => group.push((*item).clone()),
                None => by_entry.push((item.entry.clone(), vec![(*item).clone()])),
            }
        }
        for (entry, mut group) in by_entry {
            if let Some(waiting) = self.pending.remove(&entry) {
                group.extend(waiting);
                group.sort_by(|a, b| a.key.idx.cmp(&b.key.idx));
                group.dedup_by(|a, b| a.key.idx == b.key.idx);
            }
            let Some(map) = find_entry(&messages, &entry) else {
                self.pending.insert(entry, group);
                continue;
            };
            let (applied, left) = apply_parts(&map, &entry, group)?;
            changed += applied.len();
            for (bits, rev) in applied {
                self.revs.insert(bits, rev);
            }
            if !left.is_empty() {
                self.pending.insert(entry, left);
            }
        }
        // Headers may have unblocked parts that were waiting on them.
        let waiting: Vec<String> = self.pending.keys().cloned().collect();
        for entry in waiting {
            let Some(map) = find_entry(&messages, &entry) else { continue };
            let group = self.pending.remove(&entry).unwrap_or_default();
            let (applied, left) = apply_parts(&map, &entry, group)?;
            changed += applied.len();
            for (bits, rev) in applied {
                self.revs.insert(bits, rev);
            }
            if !left.is_empty() {
                self.pending.insert(entry, left);
            }
        }
        Ok(changed)
    }

    /// Apply one live frame. Returns true when the doc changed. Never
    /// regresses: an append must start at the local body's end, a part frame
    /// may not shorten a body or reopen a resolved tool.
    pub fn apply_live(&mut self, doc: &SessionDoc, live: &Live) -> Result<bool, DocError> {
        let messages = doc.transcript_doc().get_list("messages");
        if find_entry(&messages, &live.entry).is_none()
            && let Some(header) = &live.header
        {
            // The stored header hasn't arrived yet: place the entry from the
            // copy the frame carries (the stored row refreshes it later).
            let row = ReceivedItem {
                key: ItemKey::header(live.key.ord),
                entry: live.entry.clone(),
                rev: 0,
                json: header.clone(),
            };
            upsert_header(&messages, &row)?;
        }
        let Some(map) = find_entry(&messages, &live.entry) else {
            return Ok(false);
        };
        let Some(ValueOrContainer::Container(Container::List(parts))) = map.get("parts") else {
            return Ok(false);
        };
        let base = part_base(&map);
        let Some(local) = live.key.idx.checked_sub(base).filter(|l| *l >= 0).map(|l| l as usize) else {
            return Ok(false);
        };
        let changed = match &live.op {
            LiveOp::Append { at, text } => {
                let Some(ValueOrContainer::Container(Container::Map(part))) = parts.get(local) else {
                    return Ok(false);
                };
                let field = match string_field(&part, "kind").as_deref() {
                    Some("reasoning") => "reasoning",
                    _ => "text",
                };
                match part.get(field) {
                    Some(ValueOrContainer::Container(Container::Text(body))) if body.len_utf8() == *at => {
                        body.insert(body.len_unicode(), text)?;
                        true
                    }
                    _ => false,
                }
            }
            LiveOp::Part(json) => {
                let Ok(value) = serde_json::from_slice::<serde_json::Value>(json) else {
                    return Ok(false);
                };
                let Some(next) = part_from_json(&value, &live.entry, live.key.idx as usize) else {
                    return Ok(false);
                };
                match parts.get(local) {
                    Some(ValueOrContainer::Container(Container::Map(part))) => {
                        let current = part.get_deep_value().to_json_value();
                        if regresses(&current, &value) {
                            false
                        } else {
                            replace_part(&parts, local, &part, &current, &next)?;
                            true
                        }
                    }
                    None if local == parts.len() => {
                        push_part_pub(&parts, &next)?;
                        true
                    }
                    _ => false,
                }
            }
        };
        if changed {
            doc.transcript_doc().commit();
        }
        Ok(changed)
    }
}

/// The first row a page covers. When the window starts inside an entry the
/// room prepends that entry's header so its parts can render; the header is
/// outside the window, and taking it as the oldest row would make the next
/// page skip the rest of that entry.
fn page_start(items: &[ReceivedItem]) -> Option<ItemKey> {
    let mut keys: Vec<ItemKey> = items.iter().map(|i| i.key).collect();
    keys.sort_by(|a, b| a.cmp_key(b));
    match (keys.first(), keys.get(1)) {
        (Some(header), Some(next)) if header.idx == -1 && next.ord == header.ord && next.idx > 0 => Some(*next),
        (first, _) => first.copied(),
    }
}

fn commit(doc: &SessionDoc, replay: bool) {
    if replay {
        doc.transcript_doc().set_next_commit_origin(PLANE_REPLAY_ORIGIN);
    }
    doc.transcript_doc().commit();
}

/// A live part frame that would move a part backwards (shorter body, or a
/// resolved tool reopened) is a stale relay; the stored row already won.
fn regresses(current: &serde_json::Value, next: &serde_json::Value) -> bool {
    let len = |v: &serde_json::Value, key| v.get(key).and_then(|t| t.as_str()).map(str::len).unwrap_or(0);
    if len(next, "text") < len(current, "text") || len(next, "reasoning") < len(current, "reasoning") {
        return true;
    }
    let resolved = |v: &serde_json::Value| v.get("resolved").and_then(|r| r.as_bool()).unwrap_or(false);
    resolved(current) && !resolved(next)
}

fn part_base(map: &LoroMap) -> i64 {
    match map.get("partBase") {
        Some(ValueOrContainer::Value(LoroValue::I64(base))) => base,
        _ => 0,
    }
}

fn find_entry(messages: &LoroList, id: &str) -> Option<LoroMap> {
    // Newest-first: rows land overwhelmingly on the latest entries.
    for index in (0..messages.len()).rev() {
        if let Some(ValueOrContainer::Container(Container::Map(map))) = messages.get(index)
            && string_field(&map, "id").as_deref() == Some(id)
        {
            return Some(map);
        }
    }
    None
}

/// Create the entry at its `ord` position, or refresh its scalar fields.
fn upsert_header(messages: &LoroList, item: &ReceivedItem) -> Result<bool, DocError> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Header {
        id: String,
        role: MessageRole,
        #[serde(default)]
        created_at: i64,
        #[serde(default)]
        device_id: String,
        #[serde(default)]
        status: Option<MessageStatus>,
        #[serde(default)]
        continuation_of: Option<String>,
        #[serde(default)]
        duration_ms: Option<i64>,
    }
    let Ok(header) = serde_json::from_slice::<Header>(&item.json) else {
        tracing::warn!(entry = %item.entry, "transcript plane: unreadable header row; skipped");
        return Ok(false);
    };
    let entry = SessionMessageEntry {
        id: header.id,
        role: header.role,
        parts: Vec::new(),
        created_at: header.created_at,
        device_id: header.device_id,
        status: header.status,
        continuation_of: header.continuation_of,
        duration_ms: header.duration_ms,
    };
    let map = match find_entry(messages, &item.entry) {
        Some(map) => map,
        None => {
            // Entries stay sorted by ord: binary search the insert point.
            let (mut lo, mut hi) = (0usize, messages.len());
            while lo < hi {
                let mid = (lo + hi) / 2;
                let ord = match messages.get(mid) {
                    Some(ValueOrContainer::Container(Container::Map(m))) => entry_ord(&m).unwrap_or(f64::MIN),
                    _ => f64::MIN,
                };
                if ord < item.key.ord {
                    lo = mid + 1;
                } else {
                    hi = mid;
                }
            }
            let map = messages.insert_container(lo, LoroMap::new())?;
            map.insert("ord", item.key.ord)?;
            map.insert_container("parts", LoroList::new())?;
            map
        }
    };
    let before = header_json(&map);
    write_entry_fields_pub(&map, &entry)?;
    Ok(header_json(&map) != before)
}

/// Land a group of one entry's parts (ascending idx). Returns the rows
/// applied and the ones that must wait for a gap to fill.
fn apply_parts(
    map: &LoroMap,
    entry: &str,
    group: Vec<ReceivedItem>,
) -> Result<(Vec<((u64, i64), u64)>, Vec<ReceivedItem>), DocError> {
    let parts = match map.get("parts") {
        Some(ValueOrContainer::Container(Container::List(list))) => list,
        _ => map.insert_container("parts", LoroList::new())?,
    };
    let mut base = part_base(map);
    let mut applied = Vec::new();
    let mut left = Vec::new();
    let decode = |item: &ReceivedItem| {
        serde_json::from_slice::<serde_json::Value>(&item.json)
            .ok()
            .and_then(|v| part_from_json(&v, entry, item.key.idx as usize).map(|p| (v, p)))
    };
    if parts.is_empty()
        && let Some(first) = group.first()
    {
        base = first.key.idx;
        map.insert("partBase", base)?;
    }
    // Older parts prepend: they must run contiguously up to `base - 1`.
    let older: Vec<&ReceivedItem> = group.iter().filter(|i| i.key.idx < base).collect();
    if !older.is_empty() {
        let contiguous = older.windows(2).all(|w| w[1].key.idx == w[0].key.idx + 1)
            && older.last().is_some_and(|l| l.key.idx == base - 1);
        if contiguous {
            for item in older.iter().rev() {
                if let Some((_, part)) = decode(item) {
                    insert_part(&parts, 0, &part)?;
                    applied.push((item.key.bits(), item.rev));
                }
            }
            base = older[0].key.idx;
            map.insert("partBase", base)?;
        } else {
            left.extend(older.into_iter().cloned());
        }
    }
    for item in group.iter().filter(|i| i.key.idx >= base) {
        let local = (item.key.idx - base) as usize;
        let Some((value, part)) = decode(item) else { continue };
        match parts.get(local) {
            Some(ValueOrContainer::Container(Container::Map(existing))) => {
                let current = existing.get_deep_value().to_json_value();
                if current != value {
                    replace_part(&parts, local, &existing, &current, &part)?;
                }
                applied.push((item.key.bits(), item.rev));
            }
            None if local == parts.len() => {
                push_part_pub(&parts, &part)?;
                applied.push((item.key.bits(), item.rev));
            }
            _ => left.push(item.clone()),
        }
    }
    Ok((applied, left))
}

/// Overwrite in place when the kind matches; otherwise swap the map.
fn replace_part(
    parts: &LoroList,
    local: usize,
    existing: &LoroMap,
    current: &serde_json::Value,
    next: &crate::parts::MessagePart,
) -> Result<(), DocError> {
    let next_kind = serde_json::to_value(next)
        .ok()
        .and_then(|v| v.get("kind").and_then(|k| k.as_str()).map(str::to_owned));
    let same_kind = current.get("kind").and_then(|k| k.as_str()) == next_kind.as_deref()
        || matches!((current.get("kind").and_then(|k| k.as_str()), next), (Some("input"), crate::parts::MessagePart::Input { .. }));
    if same_kind {
        overwrite_part(existing, next)
    } else {
        parts.delete(local, 1)?;
        insert_part(parts, local, next)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::parts::MessagePart;
    use crate::schema::SegmentWriter;
    use zeron_proto::ToolCall;

    /// A split host doc whose transcript changes feed a publisher, the way
    /// the engine wires it.
    struct Host {
        doc: SessionDoc,
        publisher: Arc<Mutex<Publisher>>,
        _sub: loro::Subscription,
    }

    impl Host {
        fn new(known: HashMap<(u64, i64), u64>) -> Self {
            let doc = SessionDoc::init_split("chat").unwrap();
            Self::over(doc, known)
        }

        fn over(doc: SessionDoc, known: HashMap<(u64, i64), u64>) -> Self {
            let publisher = Arc::new(Mutex::new(Publisher::new(known)));
            let feed = publisher.clone();
            let sub = doc
                .transcript_doc()
                .subscribe_root(Arc::new(move |event| feed.lock().unwrap().observe(&event)));
            Self {
                doc,
                publisher,
                _sub: sub,
            }
        }

        fn collect(&self, now: i64) -> Vec<Publication> {
            self.publisher.lock().unwrap().collect(&self.doc, now)
        }
    }

    fn text(id: &str, body: &str) -> MessagePart {
        MessagePart::Text {
            id: id.into(),
            text: body.into(),
        }
    }

    fn tool(id: &str, resolved: bool, output: Option<&str>) -> MessagePart {
        MessagePart::Tool {
            id: id.into(),
            call: ToolCall::Exec {
                command: "cargo test".into(),
            },
            is_error: false,
            resolved,
            output: output.map(str::to_owned),
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: None,
            subagent_status: None,
            subagent_tail: None,
        }
    }

    fn user(id: &str, body: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: MessageRole::User,
            parts: vec![text("t0", body)],
            created_at: 1,
            device_id: "host".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    /// The room in miniature: rows upserted by key with a rev, live frames
    /// passed straight through.
    #[derive(Default)]
    struct Room {
        rows: Vec<ReceivedItem>,
        rev: u64,
    }

    impl Room {
        fn take(&mut self, out: &[Publication]) -> (Vec<ReceivedItem>, Vec<Live>) {
            let mut stored = Vec::new();
            let mut lives = Vec::new();
            for p in out {
                match p {
                    Publication::Store(item) => {
                        self.rev += 1;
                        let row = ReceivedItem {
                            key: item.key,
                            entry: item.entry.clone(),
                            rev: self.rev,
                            json: item.json.clone(),
                        };
                        self.rows.retain(|r| r.key.bits() != item.key.bits());
                        self.rows.push(row.clone());
                        stored.push(row);
                    }
                    Publication::Live(live) => lives.push(live.clone()),
                }
            }
            (stored, lives)
        }

        fn sorted(&self) -> Vec<ReceivedItem> {
            let mut rows = self.rows.clone();
            rows.sort_by(|a, b| a.key.cmp_key(&b.key));
            rows
        }
    }

    fn stores(out: &[Publication]) -> usize {
        out.iter().filter(|p| matches!(p, Publication::Store(_))).count()
    }

    /// A streamed turn: 200 text ticks, a tool that runs then resolves, more
    /// text, finish. The cost claim of the design in one assertion: stored
    /// rows scale with PARTS, not with ticks.
    #[test]
    fn a_streamed_turn_stores_per_part_not_per_tick() {
        let host = Host::new(HashMap::new());
        host.doc.push_message(&user("u1", "run the tests")).unwrap();
        let mut room = Room::default();
        let mut viewer = SessionDoc::init_split("chat").unwrap();
        let mut reader = Reader::default();
        reader.apply_page(&viewer, &[], false, 0, false).unwrap();
        let mut now = 0;
        let mut total_stores = 0;
        let mut total_lives = 0;
        let mut tick = |host: &Host, room: &mut Room, viewer: &mut SessionDoc, reader: &mut Reader, now: i64| {
            let out = host.collect(now);
            let (stored, lives) = room.take(&out);
            reader.apply_feed(viewer, &stored, None, false).unwrap();
            for live in &lives {
                reader.apply_live(viewer, live).unwrap();
            }
            (stored.len(), lives.len())
        };

        let mut writer = SegmentWriter::begin(&host.doc, "a1", "host", 1).unwrap();
        let mut body = String::new();
        for i in 0..200 {
            body.push_str(&format!("word{i} "));
            writer.sync(&[text("t0", &body)]).unwrap();
            now += 120;
            let (s, l) = tick(&host, &mut room, &mut viewer, &mut reader, now);
            total_stores += s;
            total_lives += l;
        }
        // Mid-stream, the viewer reads the same transcript through live frames.
        assert_eq!(viewer.read_entries().unwrap(), host.doc.read_entries().unwrap());

        let running = vec![text("t0", &body), tool("c1", false, None)];
        writer.sync(&running).unwrap();
        now += 120;
        let (s, _) = tick(&host, &mut room, &mut viewer, &mut reader, now);
        total_stores += s;
        let output_tail = vec![text("t0", &body), tool("c1", false, Some("running 12 tests"))];
        writer.sync(&output_tail).unwrap();
        now += 120;
        let (s, l) = tick(&host, &mut room, &mut viewer, &mut reader, now);
        assert_eq!((s, l), (0, 1), "a running tool's output streams, it is not stored");
        total_stores += s;

        let done = vec![text("t0", &body), tool("c1", true, Some("ok")), text("t1", "All green.")];
        writer.finish(&done, MessageStatus::Complete).unwrap();
        now += 120;
        let (s, _) = tick(&host, &mut room, &mut viewer, &mut reader, now);
        total_stores += s;

        assert_eq!(viewer.read_entries().unwrap(), host.doc.read_entries().unwrap());
        // 200 streamed ticks + a tool lifecycle: the user entry (header + 1
        // part), the assistant header twice (open, settled), t0 a durability
        // copy or two plus its settle, c1 on appearance and resolution, t1.
        assert!(total_stores <= 14, "stored {total_stores} rows");
        assert!(total_lives >= 200, "streamed {total_lives} live frames");
        // Every stored row is distinct by key in the room.
        assert_eq!(room.rows.len(), 2 + 1 + 3);
    }

    #[test]
    fn a_reopened_host_with_known_hashes_publishes_nothing() {
        let host = Host::new(HashMap::new());
        host.doc.push_message(&user("u1", "hello")).unwrap();
        let mut writer = SegmentWriter::begin(&host.doc, "a1", "host", 1).unwrap();
        writer.sync(&[text("t0", "hi there")]).unwrap();
        writer.finish(&[text("t0", "hi there")], MessageStatus::Complete).unwrap();
        let out = host.collect(0);
        let known: HashMap<(u64, i64), u64> = out
            .iter()
            .filter_map(|p| match p {
                Publication::Store(item) => Some((item.key.bits(), item.hash())),
                _ => None,
            })
            .collect();
        assert_eq!(known.len(), 4);

        let reopened = Host::over(
            SessionDoc::from_split(
                host.doc.doc().fork(),
                host.doc.transcript_doc().fork(),
            ),
            known,
        );
        assert!(reopened.collect(0).is_empty());
        // ...but a real change after reopen still goes out.
        reopened.doc.set_message_status("a1", MessageStatus::Aborted).unwrap();
        assert_eq!(stores(&reopened.collect(0)), 1);
    }

    #[test]
    fn pages_fill_a_reader_from_the_newest_end() {
        let host = Host::new(HashMap::new());
        for i in 0..6 {
            host.doc.push_message(&user(&format!("u{i}"), &format!("message {i}"))).unwrap();
            let writer = SegmentWriter::begin(&host.doc, &format!("a{i}"), "host", 1).unwrap();
            let parts: Vec<MessagePart> = (0..5).map(|p| text(&format!("t{p}"), &format!("reply {i}.{p}"))).collect();
            writer.finish(&parts, MessageStatus::Complete).unwrap();
        }
        let mut room = Room::default();
        room.take(&host.collect(0));
        let rows = room.sorted();

        // The room's page rule: the newest N rows, plus the header of an
        // entry the window starts inside.
        let page = |before: Option<ItemKey>, limit: usize| -> (Vec<ReceivedItem>, bool) {
            let older: Vec<&ReceivedItem> = rows
                .iter()
                .filter(|r| before.is_none_or(|b| r.key.cmp_key(&b).is_lt()))
                .collect();
            let start = older.len().saturating_sub(limit);
            let mut out: Vec<ReceivedItem> = older[start..].iter().map(|r| (*r).clone()).collect();
            if let Some(first) = out.first().cloned()
                && first.key.idx >= 0
                && let Some(h) = rows.iter().find(|r| r.key.ord == first.key.ord && r.key.idx == -1)
            {
                out.insert(0, h.clone());
            }
            (out, start > 0)
        };

        let viewer = SessionDoc::init_split("chat").unwrap();
        let mut reader = Reader::default();
        let (last, more) = page(None, 8);
        reader.apply_page(&viewer, &last, more, room.rev, false).unwrap();
        let cursor = Reader::cursor(&viewer);
        assert!(cursor.more_before);
        let shown = viewer.read_entries().unwrap();
        // The window starts mid-entry: that entry shows only its tail.
        assert_eq!(shown.last().unwrap().id, "a5");
        assert!(shown.first().unwrap().parts.len() < 5);

        let mut before = cursor.oldest;
        loop {
            let (older, more) = page(before, 7);
            reader.apply_page(&viewer, &older, more, room.rev, true).unwrap();
            before = Reader::cursor(&viewer).oldest;
            if !more {
                break;
            }
        }
        assert!(!Reader::cursor(&viewer).more_before);
        assert_eq!(viewer.read_entries().unwrap(), host.doc.read_entries().unwrap());
    }

    #[test]
    fn an_entry_inserted_before_the_last_lands_between_by_ord() {
        let host = Host::new(HashMap::new());
        host.doc.push_message(&user("u1", "first")).unwrap();
        host.doc.push_message(&user("u2", "second")).unwrap();
        host.doc.insert_message_before_last(1, &user("seam", "switched provider")).unwrap();
        let ids: Vec<String> = host.doc.read_entries().unwrap().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, ["u1", "seam", "u2"]);

        let mut room = Room::default();
        room.take(&host.collect(0));
        let viewer = SessionDoc::init_split("chat").unwrap();
        let mut reader = Reader::default();
        // A fresh reader's first page of an empty room: it holds everything.
        reader.apply_page(&viewer, &[], false, 0, false).unwrap();
        // Arrival order must not matter: apply newest first.
        let mut rows = room.sorted();
        rows.reverse();
        for row in rows {
            reader.apply_feed(&viewer, &[row], None, false).unwrap();
        }
        assert_eq!(viewer.read_entries().unwrap(), host.doc.read_entries().unwrap());
    }

    #[test]
    fn stale_relays_never_regress_a_settled_part() {
        let host = Host::new(HashMap::new());
        let mut room = Room::default();
        let viewer = SessionDoc::init_split("chat").unwrap();
        let mut reader = Reader::default();
        reader.apply_page(&viewer, &[], false, 0, false).unwrap();
        let mut writer = SegmentWriter::begin(&host.doc, "a1", "host", 1).unwrap();
        writer.sync(&[text("t0", "Hel")]).unwrap();
        let (_, early) = room.take(&host.collect(0));
        writer.finish(&[text("t0", "Hello")], MessageStatus::Complete).unwrap();
        let (stored, _) = room.take(&host.collect(200));
        reader.apply_feed(&viewer, &stored, None, false).unwrap();
        // The early "Hel" part frame arrives late, after the settled row.
        for live in &early {
            assert!(!reader.apply_live(&viewer, live).unwrap());
        }
        // A replayed older row (lower rev) is ignored too.
        let mut old = stored.iter().find(|r| r.key.idx == 0).unwrap().clone();
        old.rev = 0;
        old.json = br#"{"id":"t0","kind":"text","text":"He"}"#.to_vec();
        reader.apply_feed(&viewer, &[old], None, false).unwrap();
        assert_eq!(viewer.read_entries().unwrap(), host.doc.read_entries().unwrap());
    }

    #[test]
    fn a_host_that_moved_appends_after_rows_it_never_saw() {
        // Host A writes a turn; the chat moves to host B, whose local
        // transcript is empty (it hasn't paged history in yet).
        let a = Host::new(HashMap::new());
        for i in 0..3 {
            a.doc.push_message(&user(&format!("a{i}"), "on host A")).unwrap();
        }
        let mut room = Room::default();
        room.take(&a.collect(0));
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = Host::new(HashMap::new());
        b.doc.push_message(&user("b0", "on host B")).unwrap();
        let (stored, _) = room.take(&b.collect(0));
        let a_max = room
            .rows
            .iter()
            .filter(|r| r.entry.starts_with('a'))
            .map(|r| r.key.ord)
            .fold(f64::MIN, f64::max);
        assert!(stored.iter().all(|r| r.key.ord > a_max), "no key reused, order kept");
        // A reader sees A's history then B's message.
        let viewer = SessionDoc::init_split("chat").unwrap();
        let mut reader = Reader::default();
        reader.apply_page(&viewer, &room.sorted(), false, room.rev, false).unwrap();
        let ids: Vec<String> = viewer.read_entries().unwrap().into_iter().map(|e| e.id).collect();
        assert_eq!(ids, ["a0", "a1", "a2", "b0"]);
    }

    #[test]
    fn a_live_part_settles_even_when_two_parts_land_after_it_at_once() {
        let host = Host::new(HashMap::new());
        let mut room = Room::default();
        let mut writer = SegmentWriter::begin(&host.doc, "a1", "host", 1).unwrap();
        let mut body = String::from("streaming");
        writer.sync(&[text("t0", &body)]).unwrap();
        room.take(&host.collect(0));
        // Long enough for a durability copy of the live part.
        body.push_str(" more");
        writer.sync(&[text("t0", &body)]).unwrap();
        let (stored, _) = room.take(&host.collect(DURABLE_MS + 1));
        assert!(stored.iter().any(|r| r.key.idx == 0), "durability copy");
        // One commit appends a tool AND a text part after it.
        body.push_str(" final");
        writer
            .sync(&[text("t0", &body), tool("c1", true, Some("ok")), text("t1", "next")])
            .unwrap();
        room.take(&host.collect(DURABLE_MS + 200));
        let t0 = room.rows.iter().find(|r| r.key.idx == 0).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&t0.json).unwrap();
        assert_eq!(value["text"], body.as_str(), "the old live part settled with its final text");
    }

    #[test]
    fn live_frames_alone_carry_a_stream_before_its_header_row_arrives() {
        let host = Host::new(HashMap::new());
        let viewer = SessionDoc::init_split("chat").unwrap();
        let mut reader = Reader::default();
        let mut writer = SegmentWriter::begin(&host.doc, "a1", "host", 1).unwrap();
        let mut body = String::new();
        for i in 0..20 {
            body.push_str(&format!("t{i} "));
            writer.sync(&[text("t0", &body)]).unwrap();
            // Stored rows (the header) are withheld: live frames travel by
            // socket and can beat them.
            for publication in host.collect(i * 120) {
                if let Publication::Live(live) = publication {
                    reader.apply_live(&viewer, &live).unwrap();
                }
            }
        }
        assert_eq!(
            viewer.read_entries().unwrap()[0].parts,
            vec![text("t0", &body)],
            "the part frame's header placed the entry"
        );
    }

    #[test]
    fn unsplit_docs_are_untouched() {
        let doc = SessionDoc::init("chat").unwrap();
        doc.push_message(&user("u1", "hello")).unwrap();
        assert!(!doc.is_split());
        assert!(doc.export_transcript_snapshot().unwrap().is_none());
        let value = doc.doc().get_deep_value().to_json_value();
        assert!(value["messages"][0].get("ord").is_none(), "gen-2 entries gain no fields");
    }
}
