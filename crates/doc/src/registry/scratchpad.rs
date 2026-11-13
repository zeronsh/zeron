//! The Control Room's Scratchpad (`zeron_proto::scratchpad`): the document
//! and its history as registry rows, synced like the rest of orchestration.
//!
//! - `scratchpad` / `main`: the document now. Its text and its drawing (the
//!   shapes as JSON) are each split into parts small enough for the room's
//!   op limit (`t0`, `t1`, … and `s0`, `s1`, …). Every part carries the tag
//!   of the edit that wrote it (`textRev` / `shapesRev`), so a part from
//!   another edit (two devices writing at once, per-field LWW) is seen,
//!   never stitched in: the reader falls back to the newest whole snapshot.
//!   A text edit leaves the drawing's parts alone, and the reverse.
//! - `scratchpadHistory` / `slot00` … : [`HISTORY_SLOTS`] snapshots, the
//!   same parts plus what history lists (who, when, a preview). Edits by the
//!   same editor on the same device within [`SNAPSHOT_EVERY_SECS`] of a
//!   snapshot's start fold into it; any other edit starts the next one in
//!   the slot written longest ago. Slots are written over, never deleted.
//!   The newest snapshot is always the document as it is now.
//!
//! Nothing here makes or touches any other row: a Scratchpad edit is never
//! work.

use serde_json::Map;
use zeron_proto::scratchpad::{
    Editor, HISTORY_SLOTS, Revision, RevisionInfo, SNAPSHOT_EVERY_SECS, Scratchpad,
    ScratchpadState, Shape, clamp_text, preview, tidy_shapes,
};

use super::*;

pub const KIND_SCRATCHPAD: &str = "scratchpad";
pub const KIND_SCRATCHPAD_HISTORY: &str = "scratchpadHistory";
/// The one document's row id.
pub const SCRATCHPAD_ROW: &str = "main";

/// The most one part's value takes serialized (well inside the op limit
/// with its field name, tag and the op's own fields).
const PART_BYTES: usize = 10 * 1024;

const TEXT_PART: &str = "t";
const SHAPES_PART: &str = "s";
const TEXT_PARTS: &str = "textParts";
const SHAPES_PARTS: &str = "shapeParts";
const TEXT_REV: &str = "textRev";
const SHAPES_REV: &str = "shapesRev";

/// One edit, as [`RegistryDoc::write_scratchpad`] applies it.
#[derive(Debug, Clone, Default)]
pub struct ScratchpadEdit {
    /// The new text (`None`: as it is).
    pub text: Option<String>,
    /// The new drawing (`None`: as it is).
    pub shapes: Option<Vec<Shape>>,
    pub by: Editor,
    pub now: DateTime<Utc>,
    /// The snapshot this edit puts back.
    pub restored_from: Option<String>,
}

fn slot_id(i: usize) -> String {
    format!("slot{i:02}")
}

/// How long `s` is as a JSON string, quotes included (serde_json's escapes).
fn json_len(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if (c as u32) < 0x20 => 6,
        c => c.len_utf8(),
    }
}

/// `s` in pieces, each at most [`PART_BYTES`] as JSON.
fn split_parts(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut size = 2;
    for (i, c) in s.char_indices() {
        let n = json_len(c);
        if size + n > PART_BYTES {
            parts.push(&s[start..i]);
            start = i;
            size = 2;
        }
        size += n;
    }
    if start < s.len() || parts.is_empty() {
        parts.push(&s[start..]);
    }
    parts
}

/// The fields of one body (text or shapes JSON) under `prefix`, tagged.
fn body_fields(
    fields: &mut Map<String, Value>,
    prefix: &str,
    count_key: &str,
    rev_key: &str,
    tag: &str,
    body: &str,
) {
    let parts = split_parts(body);
    fields.insert(count_key.into(), json!(parts.len()));
    fields.insert(rev_key.into(), json!(tag));
    for (i, part) in parts.into_iter().enumerate() {
        fields.insert(format!("{prefix}{i}"), json!({ "tag": tag, "s": part }));
    }
}

/// A body back from its parts; `None` when one is missing or from another
/// edit.
fn read_body(
    fields: &BTreeMap<String, Value>,
    prefix: &str,
    count_key: &str,
    rev_key: &str,
) -> Option<String> {
    let count = fields.get(count_key)?.as_u64()? as usize;
    let tag = fields.get(rev_key)?.as_str()?;
    let mut out = String::new();
    for i in 0..count {
        let part = fields.get(&format!("{prefix}{i}"))?;
        if part.get("tag")?.as_str()? != tag {
            return None;
        }
        out.push_str(part.get("s")?.as_str()?);
    }
    Some(out)
}

fn body_whole(
    fields: &BTreeMap<String, Value>,
    prefix: &str,
    count_key: &str,
    rev_key: &str,
) -> bool {
    let (Some(count), Some(tag)) = (
        fields.get(count_key).and_then(Value::as_u64),
        fields.get(rev_key).and_then(Value::as_str),
    ) else {
        return false;
    };
    (0..count as usize).all(|i| {
        fields
            .get(&format!("{prefix}{i}"))
            .and_then(|p| p.get("tag"))
            .and_then(Value::as_str)
            == Some(tag)
    })
}

/// Text and shapes of a row, when whole.
fn read_content(fields: &BTreeMap<String, Value>) -> Option<(String, Vec<Shape>)> {
    let text = read_body(fields, TEXT_PART, TEXT_PARTS, TEXT_REV)?;
    let shapes = read_body(fields, SHAPES_PART, SHAPES_PARTS, SHAPES_REV)?;
    // (Shapes a newer build wrote that this one can't read are passed over.)
    let shapes: Vec<Value> = serde_json::from_str(&shapes).ok()?;
    let shapes = shapes
        .into_iter()
        .filter_map(|s| serde_json::from_value(s).ok())
        .collect();
    Some((text, shapes))
}

fn content_whole(fields: &BTreeMap<String, Value>) -> bool {
    body_whole(fields, TEXT_PART, TEXT_PARTS, TEXT_REV)
        && body_whole(fields, SHAPES_PART, SHAPES_PARTS, SHAPES_REV)
}

fn get<T: serde::de::DeserializeOwned>(fields: &BTreeMap<String, Value>, key: &str) -> Option<T> {
    fields
        .get(key)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
}

fn shapes_json(shapes: &[Shape]) -> String {
    serde_json::to_string(shapes).unwrap_or_else(|_| "[]".into())
}

/// A snapshot slot as read.
struct Slot {
    row: String,
    info: RevisionInfo,
    whole: bool,
    fields: BTreeMap<String, Value>,
}

fn read_slot(row: RegistryRow) -> Option<Slot> {
    let f = &row.fields;
    let info = RevisionInfo {
        id: get(f, "id")?,
        at: get(f, "at")?,
        edited_at: get(f, "editedAt").or_else(|| get(f, "at"))?,
        by: get(f, "by").unwrap_or(Editor::Other),
        device_id: get(f, "deviceId"),
        restored_from: get(f, "restoredFrom"),
        chars: get(f, "chars").unwrap_or(0),
        shape_count: get(f, "shapeCount").unwrap_or(0),
        preview: get(f, "preview").unwrap_or_default(),
    };
    Some(Slot {
        whole: content_whole(f),
        row: row.id,
        info,
        fields: row.fields,
    })
}

impl RegistryDoc {
    /// Changes whenever the Scratchpad or its history may have changed.
    pub fn scratchpad_version(&self) -> u64 {
        self.kind_version(KIND_SCRATCHPAD)
            .wrapping_add(self.kind_version(KIND_SCRATCHPAD_HISTORY))
    }

    /// Every snapshot slot that reads, newest edit first.
    fn scratchpad_slots(&self) -> Vec<Slot> {
        let mut slots: Vec<Slot> = self
            .overlay_rows(KIND_SCRATCHPAD_HISTORY)
            .into_iter()
            .filter(|r| !r.deleted)
            .filter_map(read_slot)
            .collect();
        slots.sort_by(|a, b| {
            b.info
                .edited_at
                .cmp(&a.info.edited_at)
                .then_with(|| b.info.id.cmp(&a.info.id))
        });
        slots
    }

    /// The document now: its row when whole, else the newest whole snapshot
    /// (empty before the first edit).
    pub fn scratchpad(&self) -> Scratchpad {
        let row = self
            .overlay_row(KIND_SCRATCHPAD, SCRATCHPAD_ROW)
            .filter(|r| !r.deleted);
        if let Some(row) = &row {
            if let Some((text, shapes)) = read_content(&row.fields) {
                return Scratchpad {
                    text,
                    shapes,
                    rev: get(&row.fields, "rev").unwrap_or_default(),
                    updated_at: get(&row.fields, "updatedAt"),
                    updated_by: get(&row.fields, "updatedBy").unwrap_or(Editor::Other),
                    device_id: get(&row.fields, "deviceId"),
                };
            }
        }
        for slot in self.scratchpad_slots() {
            if let Some((text, shapes)) = read_content(&slot.fields) {
                return Scratchpad {
                    text,
                    shapes,
                    rev: get(&slot.fields, "tag").unwrap_or_else(|| slot.info.id.clone()),
                    updated_at: Some(slot.info.edited_at),
                    updated_by: slot.info.by,
                    device_id: slot.info.device_id,
                };
            }
        }
        Scratchpad::default()
    }

    /// The document and its history (whole snapshots, newest first).
    pub fn read_scratchpad(&self) -> ScratchpadState {
        ScratchpadState {
            pad: self.scratchpad(),
            history: self
                .scratchpad_slots()
                .into_iter()
                .filter(|s| s.whole)
                .map(|s| s.info)
                .collect(),
        }
    }

    /// One snapshot, whole.
    pub fn scratchpad_revision(&self, id: &str) -> Option<Revision> {
        let slot = self
            .scratchpad_slots()
            .into_iter()
            .find(|s| s.info.id == id)?;
        let (text, shapes) = read_content(&slot.fields)?;
        let info = slot.info;
        Some(Revision {
            id: info.id,
            at: info.at,
            edited_at: info.edited_at,
            by: info.by,
            device_id: info.device_id,
            restored_from: info.restored_from,
            text,
            shapes,
        })
    }

    /// Apply `edit`: the document's row and its snapshot. The new `rev`, or
    /// the current one when nothing changed.
    pub fn write_scratchpad(&mut self, edit: ScratchpadEdit) -> String {
        let current = self.scratchpad();
        let text = edit
            .text
            .map(clamp_text)
            .unwrap_or_else(|| current.text.clone());
        let shapes = edit
            .shapes
            .map(tidy_shapes)
            .unwrap_or_else(|| current.shapes.clone());
        let text_changed = text != current.text;
        let shapes_changed = shapes != current.shapes;
        if !text_changed && !shapes_changed && !current.rev.is_empty() {
            return current.rev;
        }
        let rev = self.next_hlc();
        let device = self.device_id.clone();
        let row = self
            .overlay_row(KIND_SCRATCHPAD, SCRATCHPAD_ROW)
            .filter(|r| !r.deleted);
        let whole_row = row.as_ref().is_some_and(|r| content_whole(&r.fields));
        let shapes_body = shapes_json(&shapes);

        // The document's row: a body is rewritten only when it changed (or
        // the row isn't whole).
        let mut next = row
            .as_ref()
            .map(|r| r.fields.clone().into_iter().collect())
            .unwrap_or_else(Map::new);
        next.insert("rev".into(), json!(rev));
        next.insert("updatedAt".into(), json!(edit.now));
        next.insert("updatedBy".into(), json!(edit.by));
        next.insert("deviceId".into(), json!(device));
        if text_changed || !whole_row {
            next.retain(|k, _| !is_part(k, TEXT_PART));
            body_fields(&mut next, TEXT_PART, TEXT_PARTS, TEXT_REV, &rev, &text);
        }
        if shapes_changed || !whole_row {
            next.retain(|k, _| !is_part(k, SHAPES_PART));
            body_fields(
                &mut next,
                SHAPES_PART,
                SHAPES_PARTS,
                SHAPES_REV,
                &rev,
                &shapes_body,
            );
        }
        self.write_row_fields(KIND_SCRATCHPAD, SCRATCHPAD_ROW, next);

        // The snapshot: fold into the newest when it's this editor's on this
        // device and young, else the next slot.
        let slots = self.scratchpad_slots();
        let newest = slots.first().filter(|s| s.whole);
        let fold = newest.filter(|s| {
            edit.restored_from.is_none()
                && s.info.restored_from.is_none()
                && s.info.by == edit.by
                && s.info.device_id.as_deref() == Some(device.as_str())
                && (edit.now - s.info.at).num_seconds() < SNAPSHOT_EVERY_SECS
                && (edit.now - s.info.at).num_seconds() >= 0
        });
        let (slot_row, id, at, mut fields) = match fold {
            Some(s) => (
                s.row.clone(),
                s.info.id.clone(),
                s.info.at,
                s.fields.clone().into_iter().collect::<Map<String, Value>>(),
            ),
            None => {
                let used: HashSet<&str> = slots.iter().map(|s| s.row.as_str()).collect();
                let row = (0..HISTORY_SLOTS)
                    .map(slot_id)
                    .find(|id| !used.contains(id.as_str()))
                    .or_else(|| slots.last().map(|s| s.row.clone()))
                    .unwrap_or_else(|| slot_id(0));
                (row, rev.clone(), edit.now, Map::new())
            }
        };
        let fresh = fold.is_none();
        fields.insert("id".into(), json!(id));
        fields.insert("at".into(), json!(at));
        fields.insert("editedAt".into(), json!(edit.now));
        fields.insert("by".into(), json!(edit.by));
        fields.insert("deviceId".into(), json!(device));
        fields.insert("restoredFrom".into(), json!(edit.restored_from));
        fields.insert("tag".into(), json!(rev));
        fields.insert("chars".into(), json!(text.chars().count()));
        fields.insert("shapeCount".into(), json!(shapes.len()));
        fields.insert("preview".into(), json!(preview(&text)));
        if fresh || text_changed {
            fields.retain(|k, _| !is_part(k, TEXT_PART));
            body_fields(&mut fields, TEXT_PART, TEXT_PARTS, TEXT_REV, &rev, &text);
        }
        if fresh || shapes_changed {
            fields.retain(|k, _| !is_part(k, SHAPES_PART));
            body_fields(
                &mut fields,
                SHAPES_PART,
                SHAPES_PARTS,
                SHAPES_REV,
                &rev,
                &shapes_body,
            );
        }
        self.write_row_fields(KIND_SCRATCHPAD_HISTORY, &slot_row, fields);
        rev
    }

    /// LWW write of the fields of row `kind/id` that differ from `next` (a
    /// field the row has that `next` lacks is cleared), in as many ops as
    /// the room's size limit needs.
    fn write_row_fields(&mut self, kind: &str, id: &str, next: Map<String, Value>) {
        let current = self.overlay_row(kind, id);
        let mut set: BTreeMap<String, Value> = BTreeMap::new();
        for (key, value) in &next {
            let now = current.as_ref().and_then(|row| row.fields.get(key));
            if now != Some(value) && !(now.is_none() && value.is_null()) {
                set.insert(key.clone(), value.clone());
            }
        }
        if let Some(row) = &current {
            for (key, value) in &row.fields {
                if !next.contains_key(key) && !value.is_null() {
                    set.insert(key.clone(), Value::Null);
                }
            }
        }
        if set.is_empty() {
            return;
        }
        let hlc = self.next_hlc();
        let op_with = |set: BTreeMap<String, Value>| RowOp {
            kind: kind.to_owned(),
            id: id.to_owned(),
            op: OpKind::Upsert,
            set: Some(set),
            hlc: hlc.clone(),
            clocks: None,
        };
        let mut ops = Vec::new();
        let mut chunk: BTreeMap<String, Value> = BTreeMap::new();
        for (key, value) in set {
            chunk.insert(key.clone(), value.clone());
            if fits_op_budget(&op_with(chunk.clone())) {
                continue;
            }
            chunk.remove(&key);
            if !chunk.is_empty() {
                ops.push(op_with(std::mem::take(&mut chunk)));
            }
            let alone = BTreeMap::from([(key.clone(), value)]);
            if fits_op_budget(&op_with(alone.clone())) {
                chunk = alone;
            } else {
                tracing::warn!(kind, row = %id, field = %key, "scratchpad field too large to sync; left out");
            }
        }
        if !chunk.is_empty() {
            ops.push(op_with(chunk));
        }
        self.enqueue_ops(ops);
    }
}

/// Whether `key` is one of the parts under `prefix` (`t0`, `t12`, …).
fn is_part(key: &str, prefix: &str) -> bool {
    key.strip_prefix(prefix)
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::super::tests::server_round;
    use super::*;
    use zeron_proto::scratchpad::{MAX_TEXT_BYTES, shape_kind};

    fn ts(secs: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + secs, 0).unwrap()
    }

    fn edit(text: &str, by: Editor, at: i64) -> ScratchpadEdit {
        ScratchpadEdit {
            text: Some(text.into()),
            by,
            now: ts(at),
            ..Default::default()
        }
    }

    fn rect(id: &str) -> Shape {
        serde_json::from_value(
            json!({ "id": id, "kind": shape_kind::RECT, "x": 1, "y": 2, "w": 30, "h": 10, "z": 1 }),
        )
        .unwrap()
    }

    #[test]
    fn text_and_drawing_round_trip_and_an_unchanged_edit_writes_nothing() {
        let mut doc = RegistryDoc::new("dev-a");
        assert_eq!(doc.scratchpad(), Scratchpad::default());
        let rev = doc.write_scratchpad(edit("# Ideas\n\nsync later", Editor::Person, 0));
        doc.write_scratchpad(ScratchpadEdit {
            shapes: Some(vec![rect("a")]),
            now: ts(1),
            ..Default::default()
        });
        let pad = doc.scratchpad();
        assert_eq!(pad.text, "# Ideas\n\nsync later");
        assert_eq!(pad.shapes, vec![rect("a")]);
        assert_ne!(pad.rev, rev);
        assert_eq!(pad.device_id.as_deref(), Some("dev-a"));

        let pending = doc.pending_ops_len();
        let same = doc.write_scratchpad(edit("# Ideas\n\nsync later", Editor::Person, 2));
        assert_eq!(same, pad.rev);
        assert_eq!(doc.pending_ops_len(), pending);
    }

    #[test]
    fn a_text_edit_leaves_the_drawing_parts_alone() {
        let mut doc = RegistryDoc::new("dev-a");
        doc.write_scratchpad(ScratchpadEdit {
            text: Some("a".into()),
            shapes: Some(vec![rect("a")]),
            now: ts(0),
            ..Default::default()
        });
        let before = doc.pending_ops().count();
        doc.write_scratchpad(edit("ab", Editor::Person, 1));
        let ops: Vec<&RowOp> = doc.pending_ops().skip(before).collect();
        for op in ops {
            let set = op.set.as_ref().unwrap();
            assert!(
                !set.keys().any(|k| is_part(k, SHAPES_PART)),
                "{:?}",
                set.keys()
            );
        }
        assert_eq!(doc.scratchpad().shapes, vec![rect("a")]);
    }

    #[test]
    fn a_long_text_splits_into_parts_the_room_accepts() {
        let mut doc = RegistryDoc::new("dev-a");
        // Quotes and newlines double when escaped.
        let text: String = "line \"quoted\"\n".repeat(30_000);
        doc.write_scratchpad(edit(&text, Editor::Person, 0));
        for op in doc.pending_ops() {
            assert!(
                fits_op_budget(op),
                "{} bytes",
                serde_json::to_vec(op).unwrap().len()
            );
        }
        let pad = doc.scratchpad();
        assert_eq!(pad.text.len(), MAX_TEXT_BYTES.min(text.len()));
        assert!(text.starts_with(&pad.text));
        // Shrinking clears the parts it no longer needs.
        doc.write_scratchpad(edit("short", Editor::Person, 1));
        assert_eq!(doc.scratchpad().text, "short");
        let row = doc.overlay_row(KIND_SCRATCHPAD, SCRATCHPAD_ROW).unwrap();
        assert!(row.fields.get("t1").is_none_or(Value::is_null));
    }

    #[test]
    fn history_snapshots_about_once_a_minute_per_editor() {
        let mut doc = RegistryDoc::new("dev-a");
        doc.write_scratchpad(edit("a", Editor::Person, 0));
        doc.write_scratchpad(edit("ab", Editor::Person, 20));
        doc.write_scratchpad(edit("abc", Editor::Person, 50));
        assert_eq!(doc.read_scratchpad().history.len(), 1, "folded into one");
        assert_eq!(doc.read_scratchpad().history[0].chars, 3);
        // A minute on: a new snapshot.
        doc.write_scratchpad(edit("abcd", Editor::Person, 61));
        // The Brain dump's edit is always its own.
        doc.write_scratchpad(edit("abcd\n\nfrom the dump", Editor::BrainDump, 62));
        let history = doc.read_scratchpad().history;
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].by, Editor::BrainDump);
        assert_eq!(history[0].preview, "abcd");
        assert_eq!(history[1].chars, 4);
        assert_eq!(history[2].chars, 3);
        // The newest snapshot is the document now.
        assert_eq!(
            doc.scratchpad_revision(&history[0].id).unwrap().text,
            doc.scratchpad().text
        );
    }

    #[test]
    fn history_keeps_the_newest_slots_and_writes_over_the_oldest() {
        let mut doc = RegistryDoc::new("dev-a");
        for i in 0..(HISTORY_SLOTS as i64 + 5) {
            doc.write_scratchpad(edit(&format!("v{i}"), Editor::Person, i * 61));
        }
        let history = doc.read_scratchpad().history;
        assert_eq!(history.len(), HISTORY_SLOTS);
        assert_eq!(history[0].preview, format!("v{}", HISTORY_SLOTS + 4));
        assert_eq!(history.last().unwrap().preview, "v5");
        assert_eq!(
            doc.overlay_rows(KIND_SCRATCHPAD_HISTORY).len(),
            HISTORY_SLOTS
        );
    }

    #[test]
    fn a_restore_is_a_new_snapshot_and_loses_nothing() {
        let mut doc = RegistryDoc::new("dev-a");
        doc.write_scratchpad(edit("first", Editor::Person, 0));
        doc.write_scratchpad(edit("second", Editor::Person, 70));
        let history = doc.read_scratchpad().history;
        let first = doc.scratchpad_revision(&history[1].id).unwrap();
        assert_eq!(first.text, "first");
        doc.write_scratchpad(ScratchpadEdit {
            text: Some(first.text.clone()),
            shapes: Some(first.shapes.clone()),
            by: Editor::Person,
            now: ts(80),
            restored_from: Some(first.id.clone()),
        });
        let state = doc.read_scratchpad();
        assert_eq!(state.pad.text, "first");
        assert_eq!(state.history.len(), 3);
        assert_eq!(
            state.history[0].restored_from.as_deref(),
            Some(first.id.as_str())
        );
        assert_eq!(state.history[1].preview, "second");
        // The next edit doesn't fold into the restore.
        doc.write_scratchpad(edit("first!", Editor::Person, 85));
        assert_eq!(doc.read_scratchpad().history.len(), 4);
    }

    #[test]
    fn devices_sync_and_a_torn_body_falls_back_to_a_whole_snapshot() {
        let mut a = RegistryDoc::new("dev-a");
        let mut b = RegistryDoc::new("dev-b");
        let mut server = HashMap::new();
        let mut seq = 0u64;
        a.write_scratchpad(edit("from a", Editor::Person, 0));
        server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        assert_eq!(b.scratchpad().text, "from a");
        assert_eq!(a.read_scratchpad(), b.read_scratchpad());

        // A part from another edit: never stitched in.
        let hlc = b.next_hlc();
        b.enqueue_ops(vec![RowOp {
            kind: KIND_SCRATCHPAD.into(),
            id: SCRATCHPAD_ROW.into(),
            op: OpKind::Update,
            set: Some(BTreeMap::from([(
                "t0".to_owned(),
                json!({ "tag": "other", "s": "torn" }),
            )])),
            hlc,
            clocks: None,
        }]);
        assert_eq!(b.scratchpad().text, "from a", "the newest whole snapshot");
        // The next edit makes the row whole again.
        b.write_scratchpad(edit("from b", Editor::Person, 5));
        server_round(&mut server, &mut seq, &mut [&mut a, &mut b]);
        assert_eq!(a.scratchpad().text, "from b");
        assert_eq!(a.read_scratchpad(), b.read_scratchpad());
        // Another device's edit is its own snapshot.
        assert_eq!(a.read_scratchpad().history.len(), 2);
    }

    #[test]
    fn parts_split_on_their_escaped_size() {
        assert_eq!(split_parts(""), vec![""]);
        let s = "é\"\n".repeat(5_000);
        let parts = split_parts(&s);
        assert!(parts.len() > 1);
        assert_eq!(parts.concat(), s);
        for p in parts {
            assert!(serde_json::to_string(p).unwrap().len() <= PART_BYTES);
        }
        assert!(is_part("t12", "t") && !is_part("textRev", "t") && !is_part("t", "t"));
    }
}
