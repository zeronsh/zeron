//! The Control Room's Scratchpad: one document per account for the person's
//! long-term ideas, notes and drawings of how they want things built.
//! Nothing written here is work: no op routes it, makes an item or a route,
//! or tells an agent anything.
//!
//! The document is a Markdown body plus a drawing layer ([`Shape`]s, plain
//! JSON any renderer can draw). It syncs through the registry like the rest
//! of orchestration (`zeron_doc` `registry/scratchpad.rs`) and keeps a short
//! history: a snapshot at most about once a minute per editor, the newest
//! [`HISTORY_SLOTS`] of them, enough to undo and to go back to an earlier
//! version.
//!
//! Wire: `Scratchpad` RPC params are one [`ScratchpadOp`];
//! `WatchScratchpad` streams [`ScratchpadState`] (whole, on every change).
//! The person may do every op. An agent: the Brain dump reads, writes and
//! appends; an orchestrator and its crew only read; any other chat nothing.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The body is cut to this many bytes (at a character boundary).
pub const MAX_TEXT_BYTES: usize = 256 * 1024;
/// At most this many shapes are kept (the lowest `z` go first).
pub const MAX_SHAPES: usize = 4000;
/// Snapshots kept (the oldest is written over by the next).
pub const HISTORY_SLOTS: usize = 50;
/// Edits by the same editor on the same device within this long of the
/// newest snapshot update that snapshot instead of starting another.
pub const SNAPSHOT_EVERY_SECS: i64 = 60;

/// Who made an edit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Editor {
    #[default]
    Person,
    BrainDump,
    #[serde(other)]
    Other,
}

impl Editor {
    /// How history names it.
    pub fn label(self) -> &'static str {
        match self {
            Editor::Person => "You",
            Editor::BrainDump => "Brain dump",
            Editor::Other => "Someone",
        }
    }
}

/// Shape kinds this build knows; any other string is kept as it came.
pub mod shape_kind {
    /// A freehand line through `points`.
    pub const STROKE: &str = "stroke";
    /// The box `x, y, w, h`.
    pub const RECT: &str = "rect";
    /// The ellipse inside `x, y, w, h`.
    pub const ELLIPSE: &str = "ellipse";
    /// A line with a head, from `points[0]` to the last point.
    pub const ARROW: &str = "arrow";
    /// `text`, its box `x, y, w, h`.
    pub const TEXT: &str = "text";
}

/// One mark on the drawing layer. Renderer-agnostic: canvas units (the
/// drawing's own, not pixels), a colour token (`fg`, `muted`, `accent`,
/// `red`, `orange`, `yellow`, `green`, `blue`, `purple`, …) and a z-order.
/// Fields a renderer adds that this build doesn't know ride in `extra` and
/// are written back unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Shape {
    pub id: String,
    /// One of [`shape_kind`] (or a newer renderer's).
    pub kind: String,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub w: f64,
    #[serde(default)]
    pub h: f64,
    /// A stroke's line, an arrow's ends: `[x, y]` pairs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points: Vec<[f64; 2]>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    #[serde(default = "default_color")]
    pub color: String,
    /// Line width in canvas units (the renderer's default when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stroke: Option<f64>,
    /// Higher draws on top.
    #[serde(default)]
    pub z: i64,
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

fn default_color() -> String {
    "fg".into()
}

/// The document as it is now.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scratchpad {
    /// Markdown.
    pub text: String,
    /// By `z`, then id.
    pub shapes: Vec<Shape>,
    /// Changes with every edit (`""` before the first).
    pub rev: String,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_by: Editor,
    #[serde(default)]
    pub device_id: Option<String>,
}

/// One snapshot, whole (`Scratchpad{op: revision}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Revision {
    pub id: String,
    /// When it was started.
    pub at: DateTime<Utc>,
    /// Its last edit (later than `at` when edits were folded into it).
    pub edited_at: DateTime<Utc>,
    pub by: Editor,
    #[serde(default)]
    pub device_id: Option<String>,
    /// The revision this one put back, when it's a restore.
    #[serde(default)]
    pub restored_from: Option<String>,
    pub text: String,
    pub shapes: Vec<Shape>,
}

/// A snapshot as history lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionInfo {
    pub id: String,
    pub at: DateTime<Utc>,
    pub edited_at: DateTime<Utc>,
    pub by: Editor,
    #[serde(default)]
    pub device_id: Option<String>,
    #[serde(default)]
    pub restored_from: Option<String>,
    /// Characters in its text.
    pub chars: usize,
    pub shape_count: usize,
    /// Its first non-empty line, short.
    pub preview: String,
}

impl Revision {
    pub fn info(&self) -> RevisionInfo {
        RevisionInfo {
            id: self.id.clone(),
            at: self.at,
            edited_at: self.edited_at,
            by: self.by,
            device_id: self.device_id.clone(),
            restored_from: self.restored_from.clone(),
            chars: self.text.chars().count(),
            shape_count: self.shapes.len(),
            preview: preview(&self.text),
        }
    }
}

/// What `WatchScratchpad` sends.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchpadState {
    pub pad: Scratchpad,
    /// Newest first; the first is the document as it is now.
    pub history: Vec<RevisionInfo>,
}

/// `Scratchpad` RPC params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum ScratchpadOp {
    /// Replies [`ScratchpadRead`]. (agent: Brain dump, orchestrators, crew)
    Read,
    /// Replace the text. Replies `{ rev }`.
    SetText { text: String },
    /// Replace the drawing. Replies `{ rev }`.
    SetShapes { shapes: Vec<Shape> },
    /// Replace the whole text, or with `heading` only that section's body
    /// (a section the text lacks is added at the end). Replies `{ rev }`.
    /// (agent: Brain dump)
    Write {
        text: String,
        #[serde(default)]
        heading: Option<String>,
    },
    /// Add to the end, as its own paragraph. Replies `{ rev }`.
    /// (agent: Brain dump)
    Append { text: String },
    /// Put a snapshot back (itself a new edit; nothing is lost). Replies
    /// `{ rev }`.
    #[serde(rename_all = "camelCase")]
    Restore { revision_id: String },
    /// One snapshot, whole. Replies [`Revision`].
    Revision { id: String },
}

/// `Scratchpad{op: read}`'s reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScratchpadRead {
    pub text: String,
    /// The drawing in words ([`describe_shapes`]).
    pub drawing: String,
    pub shapes: Vec<Shape>,
    pub rev: String,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    pub updated_by: Editor,
}

/// `text` cut to [`MAX_TEXT_BYTES`] at a character boundary.
pub fn clamp_text(mut text: String) -> String {
    if text.len() > MAX_TEXT_BYTES {
        let mut end = MAX_TEXT_BYTES;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
    text
}

/// `shapes` sorted by `z` then id, the first of each id kept, at most
/// [`MAX_SHAPES`] (the topmost), with non-finite numbers zeroed.
pub fn tidy_shapes(shapes: Vec<Shape>) -> Vec<Shape> {
    let mut seen = std::collections::HashSet::new();
    let fin = |v: f64| if v.is_finite() { v } else { 0.0 };
    let mut out: Vec<Shape> = shapes
        .into_iter()
        .filter(|s| !s.id.is_empty() && seen.insert(s.id.clone()))
        .map(|mut s| {
            s.x = fin(s.x);
            s.y = fin(s.y);
            s.w = fin(s.w);
            s.h = fin(s.h);
            s.stroke = s.stroke.filter(|w| w.is_finite());
            for p in &mut s.points {
                *p = [fin(p[0]), fin(p[1])];
            }
            s
        })
        .collect();
    out.sort_by(|a, b| a.z.cmp(&b.z).then_with(|| a.id.cmp(&b.id)));
    if out.len() > MAX_SHAPES {
        out.drain(..out.len() - MAX_SHAPES);
    }
    out
}

/// The first non-empty line, without Markdown heading or list marks, at
/// most 80 characters.
pub fn preview(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let line = line
        .trim_start_matches('#')
        .trim_start_matches(['-', '*', '>'])
        .trim();
    let mut out: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        out.push('…');
    }
    out
}

/// A Markdown ATX heading's level and text (`## Plan` → `(2, "Plan")`).
fn heading_of(line: &str) -> Option<(usize, &str)> {
    let trimmed = line.trim_start();
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let level = trimmed.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    Some((level, rest.trim().trim_end_matches('#').trim()))
}

/// `text` with the body of the section headed `heading` (any level; the
/// first match, case-insensitive, `#`s optional) replaced by `body`. The
/// section runs to the next heading of the same or a higher level. A
/// heading the text lacks is added at the end (`##`, or the level given).
pub fn replace_section(text: &str, heading: &str, body: &str) -> String {
    let wanted_level = heading.trim().chars().take_while(|c| *c == '#').count();
    let wanted = heading.trim().trim_start_matches('#').trim();
    let body = body.trim_matches('\n');
    let lines: Vec<&str> = text.split_inclusive('\n').collect();
    let start = lines.iter().position(|l| {
        heading_of(l.trim_end_matches(['\n', '\r']))
            .is_some_and(|(_, t)| t.eq_ignore_ascii_case(wanted))
    });
    let Some(start) = start else {
        let level = if (1..=6).contains(&wanted_level) {
            wanted_level
        } else {
            2
        };
        let mut out = text.trim_end().to_owned();
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&"#".repeat(level));
        out.push(' ');
        out.push_str(wanted);
        out.push('\n');
        if !body.is_empty() {
            out.push('\n');
            out.push_str(body);
            out.push('\n');
        }
        return out;
    };
    let (level, _) = heading_of(lines[start].trim_end_matches(['\n', '\r'])).unwrap_or((2, ""));
    let end = lines[start + 1..]
        .iter()
        .position(|l| {
            heading_of(l.trim_end_matches(['\n', '\r'])).is_some_and(|(lv, _)| lv <= level)
        })
        .map_or(lines.len(), |i| start + 1 + i);
    let mut out: String = lines[..=start].concat();
    if !out.ends_with('\n') {
        out.push('\n');
    }
    if !body.is_empty() {
        out.push('\n');
        out.push_str(body);
        out.push('\n');
    }
    if end < lines.len() {
        out.push('\n');
        out.push_str(&lines[end..].concat());
    }
    out
}

/// `text` with `more` added as its own paragraph.
pub fn append_paragraph(text: &str, more: &str) -> String {
    let more = more.trim_matches('\n');
    if more.trim().is_empty() {
        return text.to_owned();
    }
    let mut out = text.trim_end().to_owned();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(more);
    out.push('\n');
    out
}

/// How far from a shape's box an arrow's end still counts as touching it.
const TOUCH: f64 = 24.0;

fn label_of(s: &Shape) -> Option<String> {
    let t = s.text.trim().replace('\n', " ");
    (!t.is_empty()).then(|| format!("'{t}'"))
}

/// What an arrow's end at `p` touches: the topmost box, ellipse or text
/// within [`TOUCH`] of it (nearest first).
fn touched<'a>(shapes: &'a [Shape], p: &[f64; 2]) -> Option<&'a Shape> {
    shapes
        .iter()
        .filter(|s| {
            matches!(
                s.kind.as_str(),
                shape_kind::RECT | shape_kind::ELLIPSE | shape_kind::TEXT
            )
        })
        .filter_map(|s| {
            let (x0, x1) = (s.x.min(s.x + s.w), s.x.max(s.x + s.w));
            let (y0, y1) = (s.y.min(s.y + s.h), s.y.max(s.y + s.h));
            let dx = (x0 - p[0]).max(p[0] - x1).max(0.0);
            let dy = (y0 - p[1]).max(p[1] - y1).max(0.0);
            let d = dx.hypot(dy);
            (d <= TOUCH).then_some((d, s))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| b.1.z.cmp(&a.1.z)))
        .map(|(_, s)| s)
}

fn count(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The drawing in words, for an agent: what's on it ("3 boxes labelled
/// 'Engine', 'App', 'Edge', an arrow from 'Engine' to 'App'"), then one
/// line per shape, bottom to top.
pub fn describe_shapes(shapes: &[Shape]) -> String {
    if shapes.is_empty() {
        return "No drawing.".into();
    }
    let n = |v: f64| format!("{}", v.round() as i64);
    let at = |p: &[f64; 2]| format!("({}, {})", n(p[0]), n(p[1]));
    let name = |s: &Shape| {
        label_of(s).unwrap_or_else(|| {
            let what = match s.kind.as_str() {
                shape_kind::RECT => "box",
                shape_kind::ELLIPSE => "ellipse",
                _ => "text",
            };
            format!("the {what} at ({}, {})", n(s.x), n(s.y))
        })
    };
    let of = |kind: &str| shapes.iter().filter(|s| s.kind == kind).collect::<Vec<_>>();
    let mut summary: Vec<String> = Vec::new();
    for (kind, one, many) in [
        (shape_kind::RECT, "box", "boxes"),
        (shape_kind::ELLIPSE, "ellipse", "ellipses"),
    ] {
        let all = of(kind);
        if all.is_empty() {
            continue;
        }
        let labels: Vec<String> = all.iter().filter_map(|s| label_of(s)).collect();
        let mut part = count(all.len(), one, many);
        if !labels.is_empty() {
            part.push_str(&format!(" labelled {}", labels.join(", ")));
            if labels.len() < all.len() {
                part.push_str(&format!(" ({} unlabelled)", all.len() - labels.len()));
            }
        }
        summary.push(part);
    }
    let texts: Vec<String> = of(shape_kind::TEXT)
        .iter()
        .filter_map(|s| label_of(s))
        .collect();
    if !texts.is_empty() {
        summary.push(format!(
            "{} {}",
            count(texts.len(), "note", "notes"),
            texts.join(", ")
        ));
    }
    let arrow_words = |s: &Shape| -> String {
        let (Some(a), Some(b)) = (s.points.first(), s.points.last()) else {
            return "an arrow".into();
        };
        let from = touched(shapes, a).map(name).unwrap_or_else(|| at(a));
        let to = touched(shapes, b).map(name).unwrap_or_else(|| at(b));
        let mut out = format!("an arrow from {from} to {to}");
        if let Some(label) = label_of(s) {
            out.push_str(&format!(" labelled {label}"));
        }
        out
    };
    for arrow in of(shape_kind::ARROW) {
        summary.push(arrow_words(arrow));
    }
    let strokes = of(shape_kind::STROKE).len();
    if strokes > 0 {
        summary.push(count(strokes, "freehand line", "freehand lines"));
    }
    let others = shapes
        .iter()
        .filter(|s| {
            ![
                shape_kind::RECT,
                shape_kind::ELLIPSE,
                shape_kind::TEXT,
                shape_kind::ARROW,
                shape_kind::STROKE,
            ]
            .contains(&s.kind.as_str())
        })
        .count();
    if others > 0 {
        summary.push(count(others, "other shape", "other shapes"));
    }
    let mut lines = vec![
        format!("{}.", summary.join(", ")),
        format!(
            "{} (canvas units, x right, y down, bottom to top):",
            count(shapes.len(), "shape", "shapes")
        ),
    ];
    for s in shapes {
        let label = label_of(s).map(|l| format!(" {l}")).unwrap_or_default();
        let line = match s.kind.as_str() {
            shape_kind::STROKE => match (s.points.first(), s.points.last()) {
                (Some(a), Some(b)) => format!(
                    "{} freehand line, {} points, {} to {}",
                    s.color,
                    s.points.len(),
                    at(a),
                    at(b)
                ),
                _ => format!("{} freehand line", s.color),
            },
            shape_kind::ARROW => format!("{} {}", s.color, arrow_words(s)),
            shape_kind::RECT | shape_kind::ELLIPSE => format!(
                "{} {}{label} at ({}, {}), {}×{}",
                s.color,
                if s.kind == shape_kind::RECT {
                    "box"
                } else {
                    "ellipse"
                },
                n(s.x),
                n(s.y),
                n(s.w),
                n(s.h)
            ),
            shape_kind::TEXT => format!("{} text{label} at ({}, {})", s.color, n(s.x), n(s.y)),
            other => format!("{} {other}{label} at ({}, {})", s.color, n(s.x), n(s.y)),
        };
        lines.push(format!("- {line}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn shape(id: &str, kind: &str, z: i64) -> Shape {
        Shape {
            id: id.into(),
            kind: kind.into(),
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 40.0,
            points: vec![],
            text: String::new(),
            color: "fg".into(),
            stroke: None,
            z,
            extra: Map::new(),
        }
    }

    #[test]
    fn shapes_are_plain_json_and_keep_what_a_renderer_adds() {
        let raw = json!({ "id": "s1", "kind": "rect", "x": 1, "y": 2, "w": 3, "h": 4, "color": "accent", "z": 2, "roundness": 8 });
        let s: Shape = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(s.kind, shape_kind::RECT);
        assert_eq!(s.extra["roundness"], json!(8));
        let back = serde_json::to_value(&s).unwrap();
        assert_eq!(back["roundness"], json!(8));
        assert_eq!(back["color"], json!("accent"));
        // A kind from a newer renderer is kept as it came.
        let s: Shape = serde_json::from_value(json!({ "id": "s2", "kind": "frame" })).unwrap();
        assert_eq!((s.kind.as_str(), s.color.as_str()), ("frame", "fg"));
    }

    #[test]
    fn ops_read_as_camel_case() {
        let op: ScratchpadOp =
            serde_json::from_value(json!({ "op": "restore", "revisionId": "r1" })).unwrap();
        assert_eq!(
            op,
            ScratchpadOp::Restore {
                revision_id: "r1".into()
            }
        );
        let op: ScratchpadOp =
            serde_json::from_value(json!({ "op": "write", "text": "x", "heading": "Plan" }))
                .unwrap();
        assert_eq!(
            op,
            ScratchpadOp::Write {
                text: "x".into(),
                heading: Some("Plan".into())
            }
        );
        let op: ScratchpadOp = serde_json::from_value(json!({ "op": "read" })).unwrap();
        assert_eq!(op, ScratchpadOp::Read);
    }

    #[test]
    fn a_section_is_replaced_up_to_the_next_heading_of_its_level() {
        let text = "# Ideas\n\nintro\n\n## Sync\n\nold sync\n\n### Detail\n\nold detail\n\n## Map\n\nmap stuff\n";
        let out = replace_section(text, "sync", "new sync");
        assert_eq!(
            out,
            "# Ideas\n\nintro\n\n## Sync\n\nnew sync\n\n## Map\n\nmap stuff\n"
        );
        // `#`s are optional and the last section runs to the end.
        let out = replace_section(text, "## Map", "a map");
        assert!(out.ends_with("## Map\n\na map\n"), "{out}");
        assert!(out.contains("old sync"));
        // A missing section is added at the end.
        let out = replace_section("intro\n", "Later", "one day");
        assert_eq!(out, "intro\n\n## Later\n\none day\n");
        assert_eq!(replace_section("", "### Deep", "x"), "### Deep\n\nx\n");
        // Not a heading: `#tag`, or inside indentation.
        let out = replace_section("#tag\n    # code\n", "code", "x");
        assert!(out.starts_with("#tag\n    # code\n\n## code"), "{out}");
    }

    #[test]
    fn append_is_its_own_paragraph() {
        assert_eq!(append_paragraph("a\n\n\n", "b"), "a\n\nb\n");
        assert_eq!(append_paragraph("", "b"), "b\n");
        assert_eq!(append_paragraph("a", "  \n"), "a");
    }

    #[test]
    fn the_drawing_reads_in_words() {
        let boxed = |id: &str, x: f64, label: &str| {
            let mut s = shape(id, shape_kind::RECT, 0);
            s.x = x;
            s.text = label.into();
            s
        };
        let engine = boxed("e", 0.0, "Engine");
        let app = boxed("a", 300.0, "App");
        let edge = boxed("g", 600.0, "Edge");
        let mut arrow = shape("r1", shape_kind::ARROW, 1);
        // From Engine's right side to just short of App, through an elbow.
        arrow.points = vec![[100.0, 40.0], [200.0, 0.0], [290.0, 40.0]];
        arrow.color = "accent".into();
        arrow.text = "sync".into();
        let mut loose = shape("r2", shape_kind::ARROW, 1);
        loose.points = vec![[700.0, 40.0], [900.4, 300.0]];
        let mut c = shape("c", shape_kind::STROKE, 2);
        c.points = vec![[0.0, 0.0], [5.0, 5.0], [9.0, 1.0]];
        let mut note = shape("t", shape_kind::TEXT, 3);
        note.text = "v2 only".into();
        note.y = 300.0;
        let words = describe_shapes(&[engine, app, edge, arrow, loose, c, note]);
        let first = words.lines().next().unwrap();
        assert_eq!(
            first,
            "3 boxes labelled 'Engine', 'App', 'Edge', 1 note 'v2 only', an arrow from 'Engine' to 'App' labelled 'sync', \
             an arrow from 'Edge' to (900, 300), 1 freehand line."
        );
        assert!(
            words.contains("- fg box 'API'")
                || words.contains("- fg box 'Engine' at (0, 20), 100×40"),
            "{words}"
        );
        assert!(
            words.contains("- accent an arrow from 'Engine' to 'App' labelled 'sync'"),
            "{words}"
        );
        assert!(words.contains("freehand line, 3 points"), "{words}");
        assert!(words.contains("7 shapes"), "{words}");
        // Unlabelled boxes are named by place.
        let mut bare = shape("b", shape_kind::RECT, 0);
        bare.text.clear();
        let mut to_bare = shape("x", shape_kind::ARROW, 1);
        to_bare.points = vec![[-200.0, 0.0], [5.0, 30.0]];
        let words = describe_shapes(&[bare, to_bare]);
        assert!(
            words.starts_with("1 box, an arrow from (-200, 0) to the box at (10, 20)."),
            "{words}"
        );
        assert_eq!(describe_shapes(&[]), "No drawing.");
    }

    #[test]
    fn shapes_are_tidied() {
        let mut bad = shape("x", shape_kind::RECT, 5);
        bad.x = f64::NAN;
        let out = tidy_shapes(vec![
            bad,
            shape("a", shape_kind::RECT, 9),
            shape("x", shape_kind::RECT, 0),
            shape("", "rect", 0),
        ]);
        assert_eq!(
            out.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            vec!["x", "a"]
        );
        assert_eq!(out[0].x, 0.0);
    }

    #[test]
    fn preview_is_the_first_line_plain() {
        assert_eq!(preview("\n\n## Sync plan\nmore"), "Sync plan");
        assert_eq!(preview("- buy"), "buy");
        assert_eq!(preview(""), "");
        assert!(preview(&"x".repeat(200)).ends_with('…'));
    }
}
