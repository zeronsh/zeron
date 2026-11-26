//! Keeping queued ops inside the room's per-op size limit ([`MAX_OP_BYTES`]).
//!
//! The room refuses an op whose `JSON.stringify` is longer than the limit,
//! and with it the whole batch, for good: replaying it can never succeed.
//! So nothing over the limit may be queued ([`fit_op`]), and anything over it
//! that an older build queued is dropped on load ([`never_accepted`]).

use serde_json::Value;

use super::{MAX_OP_BYTES, OpKind, RowOp, fits_op_budget};

/// The longest a string is left at when an op is cut to fit.
const MIN_CUT_STRING: usize = 64;

/// `op` as one or more ops that each fit the room's limit, with the same
/// effect: field writes are spread over several ops (the first keeps the
/// op's kind, the rest are `update`s that land on the row it wrote), and a
/// field too big on its own has its longest strings cut. An op that fits is
/// returned as is.
pub(super) fn fit_op(op: RowOp) -> Vec<RowOp> {
    if fits_op_budget(&op) {
        return vec![op];
    }
    let bytes = serialized_len(&op);
    let Some(set) = op.set.clone() else {
        // Deletes carry no fields; only an id could make one this big, and
        // the room refuses such an id anyway.
        return vec![op];
    };
    let mut pieces: Vec<RowOp> = Vec::new();
    let mut current = piece(&op, op.op);
    let mut cut: Vec<String> = Vec::new();
    for (key, value) in set {
        let alone = with_field(&piece(&op, OpKind::Update), &op, &key, value.clone());
        let value = if fits_op_budget(&alone) {
            value
        } else {
            match shrink_field(&op, &key, value) {
                Some(value) => {
                    cut.push(key.clone());
                    value
                }
                None => {
                    tracing::warn!(
                        kind = %op.kind,
                        id = %op.id,
                        field = %key,
                        "registry: a field write cannot fit the room's op limit; not queued"
                    );
                    continue;
                }
            }
        };
        let grown = with_field(&current, &op, &key, value.clone());
        if fits_op_budget(&grown) {
            current = grown;
            continue;
        }
        if current.set.as_ref().is_some_and(|s| !s.is_empty()) {
            pieces.push(current);
        }
        current = with_field(&piece(&op, OpKind::Update), &op, &key, value);
    }
    if current.set.as_ref().is_some_and(|s| !s.is_empty()) {
        pieces.push(current);
    }
    // The first piece keeps the op's kind (an upsert creates or revives the
    // row), every later one is an update of that row.
    for (i, p) in pieces.iter_mut().enumerate() {
        p.op = if i == 0 { op.op } else { OpKind::Update };
    }
    tracing::warn!(
        kind = %op.kind,
        id = %op.id,
        bytes,
        limit = MAX_OP_BYTES,
        pieces = pieces.len(),
        cut = ?cut,
        "registry: op over the room's limit; split to fit"
    );
    pieces
}

/// The most a batch's ops may weigh, in UTF-8 bytes: under the room's
/// socket frame cap (1,000,000 UTF-16 units, and the runtime's 1 MiB) and
/// its HTTPS body cap (2 MB), with room for the push envelope.
pub(super) const MAX_BATCH_BYTES: usize = 900_000;

/// `groups` laid into batches of at most `max_ops` ops and
/// [`MAX_BATCH_BYTES`], in order, never splitting a group. An op that fits
/// the room is at most 3 UTF-8 bytes per unit it measures, so a few ops
/// (every local write) are not measured.
pub(super) fn chunk_groups(
    groups: impl IntoIterator<Item = Vec<RowOp>>,
    max_ops: usize,
) -> Vec<Vec<RowOp>> {
    let groups: Vec<Vec<RowOp>> = groups.into_iter().collect();
    let ops: usize = groups.iter().map(Vec::len).sum();
    if ops <= max_ops && ops * (3 * MAX_OP_BYTES + 1) <= MAX_BATCH_BYTES {
        let all: Vec<RowOp> = groups.into_iter().flatten().collect();
        return if all.is_empty() {
            Vec::new()
        } else {
            vec![all]
        };
    }
    let mut chunks: Vec<Vec<RowOp>> = Vec::new();
    let mut bytes = 0;
    for group in groups {
        let group_bytes: usize = group.iter().map(|op| serialized_len(op) + 1).sum();
        match chunks.last_mut() {
            Some(last)
                if last.len() + group.len() <= max_ops
                    && bytes + group_bytes <= MAX_BATCH_BYTES =>
            {
                last.extend(group);
                bytes += group_bytes;
            }
            _ => {
                chunks.push(group);
                bytes = group_bytes;
            }
        }
    }
    chunks.retain(|c| !c.is_empty());
    chunks
}

/// Whether `ops` is over a batch's op count or [`MAX_BATCH_BYTES`].
pub(super) fn batch_too_big(ops: &[RowOp], max_ops: usize) -> bool {
    ops.len() > max_ops
        || ops.iter().map(|op| serialized_len(op) + 1).sum::<usize>() > MAX_BATCH_BYTES
}

/// `ops` grouped so the pieces of one split op (consecutive ops on the
/// same row with the same clock) stay together.
pub(super) fn split_pieces(ops: Vec<RowOp>) -> Vec<Vec<RowOp>> {
    let mut groups: Vec<Vec<RowOp>> = Vec::new();
    for op in ops {
        match groups.last_mut() {
            Some(g) if g[0].kind == op.kind && g[0].id == op.id && g[0].hlc == op.hlc => g.push(op),
            _ => groups.push(vec![op]),
        }
    }
    groups
}

/// An empty op of `kind` on `op`'s row with `op`'s clock.
fn piece(op: &RowOp, kind: OpKind) -> RowOp {
    RowOp {
        kind: op.kind.clone(),
        id: op.id.clone(),
        op: kind,
        set: Some(Default::default()),
        hlc: op.hlc.clone(),
        clocks: None,
    }
}

/// `piece` plus the write of `key`, keeping that write's original clock.
fn with_field(piece: &RowOp, op: &RowOp, key: &str, value: Value) -> RowOp {
    let mut next = piece.clone();
    next.set
        .get_or_insert_with(Default::default)
        .insert(key.to_owned(), value);
    if let Some(clock) = op.clocks.as_ref().and_then(|c| c.get(key)) {
        next.clocks
            .get_or_insert_with(Default::default)
            .insert(key.to_owned(), clock.clone());
    }
    next
}

/// `value` with its longest strings halved (at a char boundary, marked with
/// "…") until a write of it alone fits; `None` when cutting every string
/// down to [`MIN_CUT_STRING`] is still not enough.
fn shrink_field(op: &RowOp, key: &str, mut value: Value) -> Option<Value> {
    loop {
        if fits_op_budget(&with_field(
            &piece(op, OpKind::Update),
            op,
            key,
            value.clone(),
        )) {
            return Some(value);
        }
        if !halve_longest_string(&mut value) {
            return None;
        }
    }
}

fn halve_longest_string(value: &mut Value) -> bool {
    fn longest(value: &mut Value) -> Option<&mut String> {
        match value {
            Value::String(s) => Some(s),
            Value::Array(items) => items.iter_mut().filter_map(longest).max_by_key(|s| s.len()),
            Value::Object(map) => map.values_mut().filter_map(longest).max_by_key(|s| s.len()),
            _ => None,
        }
    }
    match longest(value) {
        Some(s) if s.len() > MIN_CUT_STRING => {
            let keep = (s.len() / 2).max(MIN_CUT_STRING);
            *s = cut_str(s, keep);
            true
        }
        _ => false,
    }
}

/// `s` cut to at most `max` bytes at a char boundary, marked with "…" when
/// anything was cut.
pub(crate) fn cut_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_owned();
    }
    let mut end = max.saturating_sub('…'.len_utf8());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

fn serialized_len(op: &RowOp) -> usize {
    serde_json::to_vec(op).map_or(usize::MAX, |b| b.len())
}

/// The op's `JSON.stringify` length as the room measures it, when that is
/// provably over [`MAX_OP_BYTES`]: the room can never accept the op, under
/// any replay. `None` when it might fit.
///
/// The room parses the op this client sent and measures `JSON.stringify` of
/// it in UTF-16 code units. Strings, keys, nulls, booleans and safe
/// integers are measured exactly; any other number counts as one character
/// (its shortest possible form), so this is a lower bound and never claims
/// "too big" for an op the room would take.
pub(super) fn never_accepted(op: &RowOp) -> Option<usize> {
    let value = serde_json::to_value(op).ok()?;
    let len = js_len_lower_bound(&value);
    (len > MAX_OP_BYTES).then_some(len)
}

/// Whether the room's size check passes `op` under any reading: its
/// `JSON.stringify` length in UTF-16 code units, with every number that
/// might print differently in JS counted at its longest. Measured as the
/// room does, so text outside Latin script (up to 3 UTF-8 bytes per UTF-16
/// unit) is not cut or split when the room would take it whole.
pub(super) fn surely_fits(op: &RowOp) -> bool {
    serde_json::to_value(op).is_ok_and(|value| js_len_upper_bound(&value) <= MAX_OP_BYTES)
}

/// The longest a JS number prints: `-2.2250738585072014e-308`.
const LONGEST_JS_NUMBER: usize = 24;

fn js_len_lower_bound(value: &Value) -> usize {
    js_len(value, 1)
}

fn js_len_upper_bound(value: &Value) -> usize {
    js_len(value, LONGEST_JS_NUMBER)
}

/// `JSON.stringify(value).length`, exact but for numbers JS might print
/// differently from Rust (floats, integers past 2^53), which count as
/// `other_number`.
fn js_len(value: &Value, other_number: usize) -> usize {
    match value {
        Value::Null => 4,
        Value::Bool(true) => 4,
        Value::Bool(false) => 5,
        Value::Number(n) => {
            const SAFE: u64 = (1 << 53) - 1;
            match (n.as_i64(), n.as_u64()) {
                (Some(i), _) if i.unsigned_abs() <= SAFE => i.to_string().len(),
                (_, Some(u)) if u <= SAFE => u.to_string().len(),
                _ => other_number,
            }
        }
        Value::String(s) => js_string_len(s),
        Value::Array(items) => {
            2 + items.iter().map(|v| js_len(v, other_number)).sum::<usize>()
                + items.len().saturating_sub(1)
        }
        Value::Object(map) => {
            2 + map
                .iter()
                .map(|(k, v)| js_string_len(k) + 1 + js_len(v, other_number))
                .sum::<usize>()
                + map.len().saturating_sub(1)
        }
    }
}

/// `JSON.stringify(s).length`: quotes, escapes, UTF-16 code units.
fn js_string_len(s: &str) -> usize {
    2 + s
        .chars()
        .map(|c| match c {
            '"' | '\\' | '\u{8}' | '\u{c}' | '\n' | '\r' | '\t' => 2,
            c if (c as u32) < 0x20 => 6,
            c => c.len_utf16(),
        })
        .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn op(set: BTreeMap<String, Value>) -> RowOp {
        RowOp {
            kind: "sessions".into(),
            id: "chat-1".into(),
            op: OpKind::Upsert,
            set: Some(set),
            hlc: "1794626320340-000000-dev-a".into(),
            clocks: None,
        }
    }

    #[test]
    fn js_length_matches_serde_for_plain_json() {
        let value = serde_json::json!({
            "a": "x\"y\\z\n\t\u{1}é😀",
            "b": [1, -2, null, true, false],
            "c": {"d": 9_007_199_254_740_991_u64}
        });
        let text = serde_json::to_string(&value).unwrap();
        // serde_json escapes the same set JSON.stringify does; only the
        // non-ASCII chars differ (UTF-8 bytes vs UTF-16 units).
        let utf16: usize = text.chars().map(char::len_utf16).sum();
        assert_eq!(js_len_lower_bound(&value), utf16);
    }

    #[test]
    fn unsafe_numbers_count_as_their_shortest_form() {
        assert_eq!(js_len_lower_bound(&serde_json::json!(1.5)), 1);
        assert_eq!(js_len_lower_bound(&serde_json::json!(u64::MAX)), 1);
    }

    #[test]
    fn an_op_at_the_limit_is_kept_and_one_past_it_is_not() {
        let base = js_len_lower_bound(
            &serde_json::to_value(op(BTreeMap::from([(
                "note".to_string(),
                Value::String(String::new()),
            )])))
            .unwrap(),
        );
        let at = op(BTreeMap::from([(
            "note".to_string(),
            Value::String("a".repeat(MAX_OP_BYTES - base)),
        )]));
        assert_eq!(
            js_len_lower_bound(&serde_json::to_value(&at).unwrap()),
            MAX_OP_BYTES
        );
        assert_eq!(never_accepted(&at), None);
        let past = op(BTreeMap::from([(
            "note".to_string(),
            Value::String("a".repeat(MAX_OP_BYTES - base + 1)),
        )]));
        assert_eq!(never_accepted(&past), Some(MAX_OP_BYTES + 1));
    }

    #[test]
    fn multibyte_text_is_measured_in_utf16_units() {
        // 9000 two-byte chars: 18000 UTF-8 bytes, but 9000 UTF-16 units —
        // the room takes it, so it must never be dropped.
        let big = op(BTreeMap::from([(
            "note".to_string(),
            Value::String("é".repeat(9000)),
        )]));
        assert!(serde_json::to_vec(&big).unwrap().len() > MAX_OP_BYTES);
        assert_eq!(never_accepted(&big), None);
    }

    #[test]
    fn an_oversized_op_splits_into_pieces_that_each_fit() {
        let mut set = BTreeMap::new();
        for i in 0..4 {
            set.insert(format!("f{i}"), Value::String("x".repeat(6000)));
        }
        let original = op(set.clone());
        let pieces = fit_op(original.clone());
        assert!(pieces.len() > 1);
        assert!(pieces.iter().all(fits_op_budget));
        assert_eq!(pieces[0].op, OpKind::Upsert);
        assert!(pieces[1..].iter().all(|p| p.op == OpKind::Update));
        assert!(
            pieces
                .iter()
                .all(|p| p.hlc == original.hlc && p.id == original.id)
        );
        // Every write is there exactly once, uncut.
        let mut merged = BTreeMap::new();
        for p in &pieces {
            for (k, v) in p.set.as_ref().unwrap() {
                assert!(merged.insert(k.clone(), v.clone()).is_none());
            }
        }
        assert_eq!(merged, set);
    }

    #[test]
    fn a_field_too_big_alone_is_cut_and_keeps_its_clock() {
        let mut original = op(BTreeMap::from([
            ("small".to_string(), Value::from(1)),
            (
                "tasks".to_string(),
                serde_json::json!([{ "command": "y".repeat(40_000), "id": "t1" }]),
            ),
        ]));
        original.clocks = Some(BTreeMap::from([(
            "tasks".to_string(),
            "1794626320000-000000-dev-a".to_string(),
        )]));
        let pieces = fit_op(original);
        assert!(pieces.iter().all(fits_op_budget));
        let tasks = pieces
            .iter()
            .find(|p| p.set.as_ref().unwrap().contains_key("tasks"))
            .unwrap();
        let command = tasks.set.as_ref().unwrap()["tasks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(command.ends_with('…') && command.len() < 16_000);
        assert_eq!(tasks.set.as_ref().unwrap()["tasks"][0]["id"], "t1");
        assert_eq!(
            tasks.clocks.as_ref().unwrap()["tasks"],
            "1794626320000-000000-dev-a"
        );
    }

    #[test]
    fn text_outside_latin_script_the_room_takes_is_neither_cut_nor_split() {
        // 6000 CJK chars: 18000 UTF-8 bytes, over the limit by bytes, but
        // about 6000 UTF-16 units for the room. It goes out whole.
        let title = "漢".repeat(6000);
        let original = op(BTreeMap::from([(
            "title".to_string(),
            Value::String(title.clone()),
        )]));
        assert!(serde_json::to_vec(&original).unwrap().len() > MAX_OP_BYTES);
        assert!(surely_fits(&original));
        assert_eq!(fit_op(original.clone()), vec![original]);
        // Astral chars are 4 bytes and 2 units: still measured as the room does.
        let emoji = op(BTreeMap::from([(
            "title".to_string(),
            Value::String("😀".repeat(6000)),
        )]));
        assert!(surely_fits(&emoji));
    }

    #[test]
    fn text_outside_latin_script_too_big_for_the_room_is_cut_by_its_measure() {
        // 20000 CJK chars is over the room's limit too; one halving (to
        // 10000 chars) fits it. Measured in bytes, it was cut below 5462.
        let original = op(BTreeMap::from([(
            "title".to_string(),
            Value::String("漢".repeat(20_000)),
        )]));
        assert!(!surely_fits(&original));
        let pieces = fit_op(original);
        assert_eq!(pieces.len(), 1);
        assert!(surely_fits(&pieces[0]));
        let title = pieces[0].set.as_ref().unwrap()["title"].as_str().unwrap();
        assert!(title.ends_with('…'));
        assert!(
            title.chars().count() > 9_000,
            "cut to {}",
            title.chars().count()
        );
    }

    #[test]
    fn numbers_js_might_print_longer_count_at_their_longest() {
        assert_eq!(
            js_len_upper_bound(&serde_json::json!(1.5)),
            LONGEST_JS_NUMBER
        );
        assert_eq!(
            js_len_upper_bound(&serde_json::json!(u64::MAX)),
            LONGEST_JS_NUMBER
        );
        assert_eq!(js_len_upper_bound(&serde_json::json!(-42)), 3);
        // An op at the limit by the room's measure fits, one unit past does not.
        let base = js_len_upper_bound(
            &serde_json::to_value(op(BTreeMap::from([(
                "note".to_string(),
                Value::String(String::new()),
            )])))
            .unwrap(),
        );
        let at = |n| {
            op(BTreeMap::from([(
                "note".to_string(),
                Value::String("a".repeat(n)),
            )]))
        };
        assert!(surely_fits(&at(MAX_OP_BYTES - base)));
        assert!(!surely_fits(&at(MAX_OP_BYTES - base + 1)));
    }

    #[test]
    fn cut_str_respects_char_boundaries() {
        assert_eq!(cut_str("short", 10), "short");
        let cut = cut_str(&"é".repeat(100), 11);
        assert!(cut.len() <= 11 && cut.ends_with('…'));
    }
}
