//! Session notes over FFI: read, watch and compare-and-set the user's
//! scratchpad, plus the text helpers an editor needs to merge without
//! moving the caret. Offsets that cross here are UTF-16 code units, the
//! unit of `NSRange` / `UITextView`.

use std::sync::{Arc, Mutex};

use zeron_client as zc;

use super::session::SessionHandle;
use super::types::CoreResult;

/// The notes text as one viewport last saw it; `hash` is the base of the
/// next `set_notes`. `epoch` and `seq` order the states: see
/// `notes_supersedes`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct NotesSnapshot {
    pub text: String,
    pub hash: String,
    pub epoch: u64,
    pub seq: u64,
}

impl From<zc::NotesSnapshot> for NotesSnapshot {
    fn from(n: zc::NotesSnapshot) -> Self {
        Self {
            text: n.text,
            hash: n.hash,
            epoch: n.epoch,
            seq: n.seq,
        }
    }
}

impl From<NotesSnapshot> for zc::NotesSnapshot {
    fn from(n: NotesSnapshot) -> Self {
        Self {
            text: n.text,
            hash: n.hash,
            epoch: n.epoch,
            seq: n.seq,
        }
    }
}

/// Whether `snapshot` is a state after `base`. A watch can report a write of
/// this screen's back after a newer one: such a report is stale and must be
/// dropped. Anything newer is a real change, even if its text is one this
/// screen wrote before.
#[uniffi::export]
pub fn notes_supersedes(snapshot: NotesSnapshot, base: NotesSnapshot) -> bool {
    zc::NotesSnapshot::from(snapshot).supersedes(&base.into())
}

/// What a `set_notes` did.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum NotesWrite {
    /// The notes now read as written; `hash`, `epoch` and `seq` are the new
    /// base.
    Applied { hash: String, epoch: u64, seq: u64 },
    /// They changed since the base: rebase onto `current` and retry.
    Conflict { current: NotesSnapshot },
    /// The write would grow them past `limit` bytes; nothing was written.
    TooLarge { limit: u64 },
}

impl From<zc::NotesWrite> for NotesWrite {
    fn from(w: zc::NotesWrite) -> Self {
        match w {
            zc::NotesWrite::Applied { hash, epoch, seq } => Self::Applied { hash, epoch, seq },
            zc::NotesWrite::Conflict { current } => Self::Conflict {
                current: current.into(),
            },
            zc::NotesWrite::TooLarge { limit } => Self::TooLarge {
                limit: limit as u64,
            },
        }
    }
}

/// Receives the notes at once, then on every change, local or remote.
/// Called on a client thread: hop to the main thread before touching UI.
#[uniffi::export(with_foreign)]
pub trait NotesListener: Send + Sync {
    fn on_notes(&self, notes: NotesSnapshot);
}

/// A live notes subscription. Stops on `cancel()` or when released.
#[derive(uniffi::Object)]
pub struct NotesSubscription {
    watch: Mutex<Option<zc::SnapshotWatch>>,
}

#[uniffi::export]
impl NotesSubscription {
    pub fn cancel(&self) {
        self.watch.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

#[uniffi::export]
impl SessionHandle {
    /// The session's notes as last published.
    pub fn notes(&self) -> NotesSnapshot {
        self.inner.notes().into()
    }

    pub fn watch_notes(&self, listener: Arc<dyn NotesListener>) -> Arc<NotesSubscription> {
        let watch = self
            .inner
            .watch_notes(move |notes| listener.on_notes(notes.into()));
        Arc::new(NotesSubscription {
            watch: Mutex::new(Some(watch)),
        })
    }

    /// Compare-and-set the notes against `base_hash`. Works offline: an
    /// applied write is durable at once and syncs when the room is reachable.
    pub fn set_notes(&self, base_hash: String, text: String) -> CoreResult<NotesWrite> {
        Ok(self.inner.set_notes(&base_hash, &text)?.into())
    }
}

/// One contiguous replacement in UTF-16 code units: `length` units at
/// `location` become `text`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TextReplacement {
    pub location: u64,
    pub length: u64,
    pub text: String,
}

/// The single replacement turning `old` into `new` (`None` when equal). Apply
/// it to the text storage in place so the caret and scroll position survive a
/// remote change.
#[uniffi::export]
pub fn notes_text_replacement(old: String, new: String) -> Option<TextReplacement> {
    let edit = zeron_doc::single_range_diff(&old, &new)?;
    Some(TextReplacement {
        location: utf16_len(&old[..edit.range.start]),
        length: utf16_len(&old[edit.range.clone()]),
        text: edit.insert,
    })
}

/// Merge the local edit (`base` → `local`) with a remote one (`base` →
/// `remote`). Both edits survive; see `zeron_doc::rebase_notes`.
#[uniffi::export]
pub fn notes_rebase(base: String, local: String, remote: String) -> String {
    zeron_doc::rebase_notes(&base, &local, &remote)
}

/// The base hash of `text`, for writes from a viewport that only has text.
#[uniffi::export]
pub fn notes_hash(text: String) -> String {
    zeron_doc::notes_hash(&text)
}

/// How a live preview paints one range of the notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LiveStyleKind {
    /// A Markdown marker off the caret's line: takes no room.
    Hidden,
    /// A list bullet or task box marker, drawn quietly.
    Marker,
    Heading {
        level: u8,
    },
    Strong,
    Emphasis,
    Code,
    Strikethrough,
}

/// One styled range, in UTF-16 code units. Apply in order: later styles
/// refine earlier ones (a heading's size, then its bold).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LiveStyle {
    pub location: u64,
    pub length: u64,
    pub kind: LiveStyleKind,
}

/// The live-preview styling of `text` with the given selection (UTF-16),
/// read by the same rules as the desktop editor: Markdown markers hide
/// except on the lines the selection touches.
#[uniffi::export]
pub fn markdown_live_styles(
    text: String,
    selection_location: u64,
    selection_length: u64,
) -> Vec<LiveStyle> {
    let start = byte_offset(&text, selection_location);
    let end = byte_offset(&text, selection_location.saturating_add(selection_length));
    let active = line_start(&text, start)..line_end(&text, end.max(start));
    let mut styled: Vec<(std::ops::Range<usize>, LiveStyleKind)> = Vec::new();
    for (range, level) in zeron_markdown::live::headings(&text) {
        styled.push((range, LiveStyleKind::Heading { level }));
    }
    for (range, face) in zeron_markdown::live::faces(&text) {
        let kind = match face {
            zeron_markdown::live::Face::Bold => LiveStyleKind::Strong,
            zeron_markdown::live::Face::Italic => LiveStyleKind::Emphasis,
            zeron_markdown::live::Face::Code => LiveStyleKind::Code,
            zeron_markdown::live::Face::Strikethrough => LiveStyleKind::Strikethrough,
        };
        styled.push((range, kind));
    }
    for (range, replacement) in zeron_markdown::live::decorations(&text, active) {
        let kind = if replacement.is_empty() {
            LiveStyleKind::Hidden
        } else {
            LiveStyleKind::Marker
        };
        styled.push((range, kind));
    }
    let units = Utf16Offsets::new(
        &text,
        styled
            .iter()
            .flat_map(|(range, _)| [range.start, range.end]),
    );
    styled
        .into_iter()
        .map(|(range, kind)| {
            let location = units.of(range.start);
            LiveStyle {
                location,
                length: units.of(range.end) - location,
                kind,
            }
        })
        .collect()
}

/// The byte offset of a UTF-16 offset, clamped to the text.
fn byte_offset(text: &str, utf16: u64) -> usize {
    let mut units = 0u64;
    for (at, ch) in text.char_indices() {
        if units >= utf16 {
            return at;
        }
        units += ch.len_utf16() as u64;
    }
    text.len()
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |ix| ix + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |ix| at + ix)
}

/// UTF-16 offsets of a set of byte offsets, in one pass over the text.
struct Utf16Offsets(Vec<(usize, u64)>);

impl Utf16Offsets {
    fn new(text: &str, offsets: impl Iterator<Item = usize>) -> Self {
        let mut wanted: Vec<usize> = offsets.collect();
        wanted.sort_unstable();
        wanted.dedup();
        let mut found = Vec::with_capacity(wanted.len());
        let mut wanted = wanted.into_iter().peekable();
        let mut units = 0u64;
        for (at, ch) in text.char_indices().chain([(text.len(), '\0')]) {
            while wanted.peek().is_some_and(|&next| next <= at) {
                found.push((wanted.next().unwrap_or(at), units));
            }
            units += ch.len_utf16() as u64;
        }
        Self(found)
    }

    fn of(&self, byte: usize) -> u64 {
        let ix = self.0.partition_point(|(at, _)| *at < byte);
        self.0.get(ix).map_or(0, |(_, units)| *units)
    }
}

fn utf16_len(s: &str) -> u64 {
    s.encode_utf16().count() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_counts_utf16_units() {
        // 😀 is two UTF-16 units; é is one.
        let r = notes_text_replacement("a😀é b".into(), "a😀é c b".into()).unwrap();
        assert_eq!(
            r,
            TextReplacement {
                location: 5,
                length: 0,
                text: "c ".into(),
            }
        );
        let r = notes_text_replacement("x😀y".into(), "xy".into()).unwrap();
        assert_eq!((r.location, r.length, r.text.as_str()), (1, 2, ""));
        assert_eq!(notes_text_replacement("same".into(), "same".into()), None);
    }

    #[test]
    fn live_styles_hide_markers_off_the_caret_line_in_utf16() {
        let text = "# Título\n**negrita** 😀 _x_\n- [ ] tarea".to_owned();
        // Caret at the very end: on the task line.
        let caret = text.encode_utf16().count() as u64;
        let styles = markdown_live_styles(text.clone(), caret, 0);
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let slice = |s: &LiveStyle| {
            String::from_utf16(&utf16[s.location as usize..(s.location + s.length) as usize])
                .unwrap()
        };
        let of = |kind: LiveStyleKind| -> Vec<String> {
            styles
                .iter()
                .filter(|s| s.kind == kind)
                .map(slice)
                .collect()
        };
        assert_eq!(of(LiveStyleKind::Heading { level: 1 }), ["# Título\n"]);
        assert_eq!(of(LiveStyleKind::Strong), ["# Título\n", "**negrita**"]);
        assert_eq!(of(LiveStyleKind::Emphasis), ["_x_"]);
        // The heading and emphasis markers hide off the caret's line. On it,
        // the task item keeps its box editable and only its bullet is drawn.
        let hidden = of(LiveStyleKind::Hidden);
        assert_eq!(hidden, ["# ", "**", "**", "_", "_"]);
        assert_eq!(of(LiveStyleKind::Marker), ["-"]);
        // With the caret on the heading, its marker shows, and the task item
        // off the caret's line is drawn as a whole box.
        let styles = markdown_live_styles(text.clone(), 0, 0);
        assert!(
            styles
                .iter()
                .any(|s| s.kind == LiveStyleKind::Marker && slice(s) == "- [ ]")
        );
        assert!(
            !styles
                .iter()
                .any(|s| s.kind == LiveStyleKind::Hidden && slice(s) == "# ")
        );
    }

    #[test]
    fn rebase_and_hash_match_the_doc() {
        assert_eq!(
            notes_rebase("ab".into(), "aLb".into(), "aRb".into()),
            "aRLb"
        );
        assert_eq!(notes_hash(String::new()), zeron_doc::notes_hash(""));
    }
}
