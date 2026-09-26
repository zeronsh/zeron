//! Analytic transcript layout.
//!
//! A [`TranscriptView`] owns one worker thread holding fonts, width caches,
//! incremental parsers and row caches. Inputs (session snapshots, viewport
//! width, disclosure toggles) are coalesced; each pass publishes an immutable
//! [`LayoutFrame`] — exact row heights and prefix-sum offsets — then pings the
//! platform. Platforms query frames from any thread without locks: visible
//! ranges are a binary search, display lists are built on demand from the
//! frame's prepared rows (pure arithmetic, no measurement).

pub mod display;
mod markdown;
mod rows;
mod style;

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::Instant;

use zeron_doc::parts::MessagePart;
use zeron_doc::parts::MessageStatus;
use zeron_doc::schema::{MessageRole, SessionMessageEntry};
use zeron_text::WidthCache;

use display::{DisplayBuilder, RowDisplay};
use markdown::{Ctx, Px};
pub use rows::{PendingUser, RowKind, TranscriptInput};
use rows::{Gap, Placed, RowBuilder, RowCore, place_row};
pub use style::{FaceRole, StyleDesc};
use style::Typography;

/// Measures text the bundled faces can't render (emoji, CJK…) with the
/// platform's own text engine — pretext's "browser as ground truth".
/// Self-describing (face, size, ligatures) because style ids are per-view.
#[uniffi::export(with_foreign)]
pub trait PlatformMeasurer: Send + Sync {
    fn measure(&self, face: FaceRole, size: f32, ligatures: bool, text: String) -> f32;
    /// Lay `text` out as one run and return one advance per Unicode scalar
    /// (a cluster's width on its first scalar, zero on the rest), so fallback
    /// fonts and kerning chosen *in context* match what the engine draws.
    /// Empty = unsupported.
    fn measure_run(&self, face: FaceRole, size: f32, ligatures: bool, text: String) -> Vec<f32>;
}

/// Maps a view's style ids back to font descriptions for the platform.
struct MeasurerBridge {
    platform: Arc<dyn PlatformMeasurer>,
    styles: Arc<Mutex<HashMap<u16, StyleDesc>>>,
}

impl zeron_text::FallbackMeasurer for MeasurerBridge {
    fn measure(&self, style: zeron_text::StyleId, text: &str) -> f32 {
        let Some(desc) = self.styles.lock().unwrap().get(&style.0).cloned() else {
            return 0.0;
        };
        self.platform.measure(desc.face, desc.size, desc.ligatures, text.to_owned())
    }

    fn measure_run(&self, style: zeron_text::StyleId, text: &str, advances: &mut Vec<f32>) -> bool {
        let Some(desc) = self.styles.lock().unwrap().get(&style.0).cloned() else {
            return false;
        };
        let run = self.platform.measure_run(desc.face, desc.size, desc.ligatures, text.to_owned());
        if run.len() != text.chars().count() {
            return false;
        }
        advances.extend(run);
        true
    }
}

#[derive(uniffi::Record)]
pub struct FaceData {
    pub role: FaceRole,
    pub bytes: Vec<u8>,
}

/// Registered faces + fallback measurer, shared by every transcript.
#[derive(uniffi::Object)]
pub struct TextSystem {
    faces: Vec<(FaceRole, Arc<Vec<u8>>)>,
    measurer: Option<Arc<dyn PlatformMeasurer>>,
}

#[uniffi::export]
impl TextSystem {
    #[uniffi::constructor]
    pub fn new(faces: Vec<FaceData>, measurer: Option<Arc<dyn PlatformMeasurer>>) -> Arc<Self> {
        Arc::new(Self {
            faces: faces.into_iter().map(|f| (f.role, Arc::new(f.bytes))).collect(),
            measurer,
        })
    }
}

/// Frame-ready notifications (called on the layout thread).
#[uniffi::export(with_foreign)]
pub trait LayoutListener: Send + Sync {
    fn frame_ready(&self, revision: u64);
}

/// A row's position in a frame.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct RowPlacement {
    pub index: u32,
    pub key: u64,
    /// Changes whenever the row's painted content may change (not its width).
    pub version: u64,
    pub y: f32,
    pub height: f32,
    pub kind: RowKind,
}

struct FrameRow {
    core: Arc<RowCore>,
    gap: Gap,
    y: f32,
    height: f32,
}

impl FrameRow {
    fn version(&self) -> u64 {
        self.core.version ^ ((self.gap as u64 + 1) << 58)
    }
}

/// An immutable, fully measured transcript at one width.
#[derive(uniffi::Object)]
pub struct LayoutFrame {
    revision: u64,
    width: f32,
    scale: f32,
    rows: Vec<FrameRow>,
    total: f32,
    styles: Arc<Vec<StyleDesc>>,
    index: OnceLock<HashMap<u64, u32>>,
    /// Worker time for the pass that produced this frame (µs).
    build_micros: u64,
}

#[uniffi::export]
impl LayoutFrame {
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn width(&self) -> f32 {
        self.width
    }
    pub fn total_height(&self) -> f32 {
        self.total
    }
    pub fn row_count(&self) -> u32 {
        self.rows.len() as u32
    }
    pub fn build_micros(&self) -> u64 {
        self.build_micros
    }
    pub fn styles(&self) -> Vec<StyleDesc> {
        self.styles.as_ref().clone()
    }
    pub fn style_count(&self) -> u32 {
        self.styles.len() as u32
    }

    /// Rows intersecting `[y0, y1)`.
    pub fn rows_in(&self, y0: f32, y1: f32) -> Vec<RowPlacement> {
        let start = self.rows.partition_point(|r| r.y + r.height <= y0);
        self.rows[start..]
            .iter()
            .take_while(|r| r.y < y1)
            .enumerate()
            .map(|(i, r)| self.placement_of(start + i, r))
            .collect()
    }

    pub fn placement(&self, index: u32) -> Option<RowPlacement> {
        self.rows.get(index as usize).map(|r| self.placement_of(index as usize, r))
    }

    pub fn index_of(&self, key: u64) -> Option<u32> {
        self.index
            .get_or_init(|| self.rows.iter().enumerate().map(|(i, r)| (r.core.key, i as u32)).collect())
            .get(&key)
            .copied()
    }

    /// Index of the row containing `y` (clamped).
    pub fn index_at(&self, y: f32) -> Option<u32> {
        if self.rows.is_empty() {
            return None;
        }
        let i = self.rows.partition_point(|r| r.y + r.height <= y);
        Some(i.min(self.rows.len() - 1) as u32)
    }

    pub fn display(&self, index: u32) -> Option<RowDisplay> {
        let r = self.rows.get(index as usize)?;
        let mut out = DisplayBuilder::default();
        let height = place_row(&r.core, r.gap, Px(self.scale), self.width, Some(&mut out));
        debug_assert!((height - r.height).abs() < 0.01, "paint/measure divergence");
        Some(RowDisplay {
            key: r.core.key,
            version: r.version(),
            width: self.width,
            height: r.height,
            text: out.text,
            runs: out.runs,
            boxes: out.boxes,
            links: out.links,
            scrollers: out.scrollers,
            widgets: out.widgets,
            copy_text: r.core.copy_text.clone(),
        })
    }

    /// The whole transcript as markdown-ish plain text (Copy Transcript).
    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        let mut last: Option<&str> = None;
        for r in &self.rows {
            if r.core.copy_text.is_empty() {
                continue;
            }
            if !out.is_empty() {
                out.push_str(if last == Some(&*r.core.entry_id) { "\n\n" } else { "\n\n---\n\n" });
            }
            out.push_str(&r.core.copy_text);
            last = Some(&r.core.entry_id);
        }
        out
    }

    /// Copyable text of the whole message owning `index` (context menus).
    pub fn message_text(&self, index: u32) -> Option<String> {
        let entry = &self.rows.get(index as usize)?.core.entry_id;
        let parts: Vec<&str> = self
            .rows
            .iter()
            .filter(|r| &r.core.entry_id == entry && !r.core.copy_text.is_empty())
            .map(|r| r.core.copy_text.as_str())
            .collect();
        Some(parts.join("\n\n"))
    }
}

impl LayoutFrame {
    fn placement_of(&self, index: usize, r: &FrameRow) -> RowPlacement {
        RowPlacement {
            index: index as u32,
            key: r.core.key,
            version: r.version(),
            y: r.y,
            height: r.height,
            kind: r.core.kind,
        }
    }

    fn empty() -> Self {
        Self {
            revision: 0,
            width: 0.0,
            scale: 1.0,
            rows: Vec::new(),
            total: 0.0,
            styles: Arc::new(Vec::new()),
            index: OnceLock::new(),
            build_micros: 0,
        }
    }
}

/// Rich markdown exercising every block type (lab screen, benches, tests).
#[uniffi::export]
pub fn layout_fixture_markdown() -> String {
    FIXTURE.to_owned()
}

pub(crate) const FIXTURE: &str = include_str!("fixture.md");

/// Rust's line breaks for `text` in one face/size at `width`, as UTF-16
/// offsets of each line start — the accuracy harness compares these with
/// CoreText's own framesetter (platform engine as ground truth).
#[uniffi::export]
pub fn debug_line_starts(text_system: Arc<TextSystem>, face: FaceRole, size: f32, width: f32, text: String) -> Vec<u32> {
    let styles = Arc::new(Mutex::new(HashMap::new()));
    let fallback = text_system.measurer.clone().map(|platform| {
        Arc::new(MeasurerBridge { platform, styles: styles.clone() }) as Arc<dyn zeron_text::FallbackMeasurer>
    });
    let mut typo = Typography::new(&text_system.faces, fallback, styles);
    let (family, weight, italic) = style::decompose(face);
    let style = typo.style(family, weight, italic, size);
    let spans = [zeron_text::Span::new(0..text.len(), style.id)];
    let p = zeron_text::prepare(
        &typo.book,
        &mut WidthCache::new(),
        &text,
        if text.is_empty() { &[] } else { &spans },
        &zeron_text::PrepareOptions {
            white_space: zeron_text::WhiteSpace::PreWrap,
            overflow_wrap: zeron_text::OverflowWrap::Anywhere,
            ..Default::default()
        },
    );
    p.lines(width).iter().map(|l| p.utf16_offset(l.range.start) as u32).collect()
}

/// A debug/demo transcript entry (markdown text), for fixtures and benches.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DebugEntry {
    pub id: String,
    pub user: bool,
    pub text: String,
    pub streaming: bool,
}

enum Msg {
    Input(TranscriptInput),
    Viewport { width: f32, scale: f32 },
    Toggle(u64),
    Shutdown,
}

struct Shared {
    frame: Mutex<Arc<LayoutFrame>>,
}

/// One transcript's layout engine.
#[derive(uniffi::Object)]
pub struct TranscriptView {
    tx: Mutex<Sender<Msg>>,
    shared: Arc<Shared>,
    /// Live session subscription (Rust→Rust; rows never cross FFI).
    watch: Mutex<Option<zeron_client::SnapshotWatch>>,
}

#[uniffi::export]
impl TranscriptView {
    #[uniffi::constructor]
    pub fn new(text: Arc<TextSystem>, listener: Arc<dyn LayoutListener>) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            frame: Mutex::new(Arc::new(LayoutFrame::empty())),
        });
        let worker_shared = shared.clone();
        thread::Builder::new()
            .name("zeron-layout".into())
            .spawn(move || Worker::new(&text, worker_shared, listener).run(rx))
            .expect("spawn layout thread");
        Arc::new(Self {
            tx: Mutex::new(tx),
            shared,
            watch: Mutex::new(None),
        })
    }

    /// Follow a session's transcript: every snapshot (coalesced by the
    /// client) becomes a layout input. Returns false for an unknown chat.
    pub fn attach(&self, client: Arc<crate::client_ffi::CoreClient>, chat_id: String) -> bool {
        let Some(handle) = client.session_handle(&chat_id) else {
            return false;
        };
        let tx = Mutex::new(self.tx.lock().unwrap().clone());
        let guard = handle.watch(move |snap| {
            let input = TranscriptInput {
                entries: snap.transcript_messages(),
                pending: snap
                    .pending
                    .iter()
                    .map(|p| PendingUser {
                        id: p.message_id.clone(),
                        text: p.text.clone(),
                    })
                    .collect(),
                working: snap.working,
                working_since_ms: snap.working_since_ms,
                streaming: snap.streaming,
            };
            let _ = tx.lock().unwrap().send(Msg::Input(input));
        });
        *self.watch.lock().unwrap() = Some(guard);
        true
    }

    pub fn set_viewport(&self, width: f32, text_scale: f32) {
        self.send(Msg::Viewport {
            width,
            scale: text_scale,
        });
    }

    /// Expand/collapse a disclosure (tool group, long user message).
    pub fn toggle(&self, key: u64) {
        self.send(Msg::Toggle(key));
    }

    /// The latest published frame.
    pub fn frame(&self) -> Arc<LayoutFrame> {
        self.shared.frame.lock().unwrap().clone()
    }

    /// Feed markdown fixtures directly (demo screens, benchmarks, tests).
    pub fn set_debug_entries(&self, entries: Vec<DebugEntry>, working: bool) {
        self.send(Msg::Input(debug_input(entries, working)));
    }

    pub fn close(&self) {
        self.watch.lock().unwrap().take();
        self.send(Msg::Shutdown);
    }
}

impl TranscriptView {
    /// Feed a session snapshot (called by the client bridge in Rust).
    pub fn set_input(&self, input: TranscriptInput) {
        self.send(Msg::Input(input));
    }

    fn send(&self, msg: Msg) {
        let _ = self.tx.lock().unwrap().send(msg);
    }
}

impl Drop for TranscriptView {
    fn drop(&mut self) {
        self.send(Msg::Shutdown);
    }
}

pub(crate) fn debug_input(entries: Vec<DebugEntry>, working: bool) -> TranscriptInput {
    TranscriptInput {
        entries: entries
            .into_iter()
            .map(|e| {
                Arc::new(SessionMessageEntry {
                    parts: vec![MessagePart::Text {
                        id: "t0".into(),
                        text: e.text,
                    }],
                    id: e.id,
                    role: if e.user { MessageRole::User } else { MessageRole::Assistant },
                    created_at: 0,
                    device_id: String::new(),
                    status: Some(if e.streaming { MessageStatus::Streaming } else { MessageStatus::Complete }),
                    continuation_of: None,
                    duration_ms: None,
                })
            })
            .collect(),
        pending: Vec::new(),
        working,
        working_since_ms: None,
        streaming: working,
    }
}

struct HeightMemo {
    core: Arc<RowCore>,
    gap: Gap,
    width: f32,
    height: f32,
}

pub(crate) struct Worker {
    typo: Typography,
    cache: WidthCache,
    builder: RowBuilder,
    heights: HashMap<u64, HeightMemo>,
    input: TranscriptInput,
    width: f32,
    revision: u64,
    shared: Arc<Shared>,
    listener: Option<Arc<dyn LayoutListener>>,
}

impl Worker {
    fn new(text: &TextSystem, shared: Arc<Shared>, listener: Arc<dyn LayoutListener>) -> Self {
        let styles = Arc::new(Mutex::new(HashMap::new()));
        let fallback = text.measurer.clone().map(|platform| {
            Arc::new(MeasurerBridge {
                platform,
                styles: styles.clone(),
            }) as Arc<dyn zeron_text::FallbackMeasurer>
        });
        Self {
            typo: Typography::new(&text.faces, fallback, styles),
            cache: WidthCache::new(),
            builder: RowBuilder::default(),
            heights: HashMap::new(),
            input: TranscriptInput::default(),
            width: 0.0,
            revision: 0,
            shared,
            listener: Some(listener),
        }
    }

    fn run(mut self, rx: Receiver<Msg>) {
        while let Ok(first) = rx.recv() {
            // Coalesce everything queued: only the latest input matters.
            let mut dirty = false;
            for msg in std::iter::once(first).chain(rx.try_iter()) {
                match msg {
                    Msg::Shutdown => return,
                    Msg::Input(input) => {
                        self.input = input;
                        dirty = true;
                    }
                    Msg::Viewport { width, scale } => {
                        if (scale - self.typo.scale).abs() > f32::EPSILON {
                            self.typo.scale = scale;
                            // New sizes: every prepared paragraph is stale.
                            self.builder = RowBuilder::default();
                            self.heights.clear();
                            self.cache = WidthCache::new();
                        }
                        self.width = width;
                        dirty = true;
                    }
                    Msg::Toggle(key) => {
                        if !self.builder.expanded.remove(&key) && !self.builder.collapsed.remove(&key) {
                            // First toggle flips the row's default.
                            if self.is_open(key) {
                                self.builder.collapsed.insert(key);
                            } else {
                                self.builder.expanded.insert(key);
                            }
                        }
                        self.builder.invalidate(key);
                        dirty = true;
                    }
                }
            }
            if dirty && self.width > 0.0 && self.typo.has_faces() {
                self.pass();
            }
        }
    }

    fn is_open(&self, key: u64) -> bool {
        let frame = self.shared.frame.lock().unwrap().clone();
        frame
            .index_of(key)
            .and_then(|i| frame.rows.get(i as usize))
            .is_some_and(|r| match &r.core.content {
                rows::Content::Tools(t) => t.expanded,
                rows::Content::User(u) => u.expanded,
                _ => false,
            })
    }

    pub(crate) fn pass(&mut self) -> Arc<LayoutFrame> {
        let started = Instant::now();
        let placed: Vec<Placed> = {
            let mut ctx = Ctx {
                typo: &mut self.typo,
                cache: &mut self.cache,
            };
            self.builder.build(&mut ctx, &self.input)
        };
        let px = Px(self.typo.scale);
        let mut rows = Vec::with_capacity(placed.len());
        let mut y = 0.0f32;
        let mut live = HashMap::with_capacity(placed.len());
        for p in placed {
            let height = match self.heights.remove(&p.core.key) {
                Some(m) if Arc::ptr_eq(&m.core, &p.core) && m.gap == p.gap && m.width == self.width => m.height,
                _ => place_row(&p.core, p.gap, px, self.width, None),
            };
            live.insert(
                p.core.key,
                HeightMemo {
                    core: p.core.clone(),
                    gap: p.gap,
                    width: self.width,
                    height,
                },
            );
            rows.push(FrameRow {
                core: p.core,
                gap: p.gap,
                y,
                height,
            });
            y += height;
        }
        self.heights = live;
        // Bottom breathing room below the last row.
        let total = y + px.v(16.0);
        self.revision += 1;
        let frame = Arc::new(LayoutFrame {
            revision: self.revision,
            width: self.width,
            scale: self.typo.scale,
            rows,
            total,
            styles: Arc::new(self.typo.table().to_vec()),
            index: OnceLock::new(),
            build_micros: started.elapsed().as_micros() as u64,
        });
        *self.shared.frame.lock().unwrap() = frame.clone();
        if let Some(l) = &self.listener {
            l.frame_ready(self.revision);
        }
        frame
    }
}

#[cfg(test)]
mod tests;
