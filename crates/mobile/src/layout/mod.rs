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
mod file_icons;
mod rows;
mod style;
mod tools;

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

    /// Approximate heap held by prepared text across all rows (diagnostics).
    pub fn prepared_heap_bytes(&self) -> u64 {
        self.rows.iter().map(|r| rows::content_heap_bytes(&r.core.content) as u64).sum()
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
            fades: out.fades,
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

/// Width-cache size past which the layout worker starts a fresh one.
const WIDTH_CACHE_BUDGET: usize = 4 << 20;

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
    ToggleDetail { row: u64, detail: u64, open: bool },
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
    /// Show how the last turn ended at the transcript's end (opt-in per
    /// platform; see [`TranscriptView::set_turn_end_marker`]).
    turn_end: Arc<std::sync::atomic::AtomicBool>,
    /// Head the transcript with "loading earlier messages" while only its
    /// newest rows are here (opt-in; see [`TranscriptView::set_history_marker`]).
    history_marker: Arc<std::sync::atomic::AtomicBool>,
    /// [`TranscriptView::set_debug_history_pending`]: bytes received, or -1.
    debug_history: std::sync::atomic::AtomicI64,
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
            turn_end: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            history_marker: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            debug_history: std::sync::atomic::AtomicI64::new(-1),
        })
    }

    /// Follow a session's transcript: every snapshot (coalesced by the
    /// client) becomes a layout input. Returns false for an unknown chat.
    pub fn attach(&self, client: Arc<crate::client_ffi::CoreClient>, chat_id: String) -> bool {
        let Some(handle) = client.session_handle(&chat_id) else {
            return false;
        };
        let tx = Mutex::new(self.tx.lock().unwrap().clone());
        let turn_end = self.turn_end.clone();
        let history_marker = self.history_marker.clone();
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
                outcome: if turn_end.load(std::sync::atomic::Ordering::Relaxed) {
                    snap.outcome
                } else {
                    None
                },
                history_pending: (snap.history_pending
                    && history_marker.load(std::sync::atomic::Ordering::Relaxed))
                .then_some(snap.history_received_bytes),
            };
            let _ = tx.lock().unwrap().send(Msg::Input(input));
        });
        *self.watch.lock().unwrap() = Some(guard);
        true
    }

    /// Once no turn runs, end the transcript with how the last one ended
    /// (a done check / failed dot and the time; `WidgetKind::TurnEnd`).
    /// Off by default; call before `attach`.
    pub fn set_turn_end_marker(&self, on: bool) {
        self.turn_end.store(on, std::sync::atomic::Ordering::Relaxed);
    }

    /// While only a transcript's newest rows are here (a Direct link's
    /// opening tail) and the older ones are still downloading, head it with
    /// a spinner, "Loading earlier messages…" and how much has come in
    /// (`WidgetKind::HistoryPending`). Off by default; call before `attach`.
    pub fn set_history_marker(&self, on: bool) {
        self.history_marker.store(on, std::sync::atomic::Ordering::Relaxed);
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

    /// Flip one tool row's inline detail (`open` = its state as painted).
    pub fn toggle_detail(&self, row: u64, detail: u64, open: bool) {
        self.send(Msg::ToggleDetail { row, detail, open });
    }

    /// The latest published frame.
    pub fn frame(&self) -> Arc<LayoutFrame> {
        self.shared.frame.lock().unwrap().clone()
    }

    /// Feed markdown fixtures directly (demo screens, benchmarks, tests).
    pub fn set_debug_entries(&self, entries: Vec<DebugEntry>, working: bool) {
        let mut input = debug_input(entries, working);
        let history = self.debug_history.load(std::sync::atomic::Ordering::Relaxed);
        input.history_pending = u64::try_from(history).ok();
        self.send(Msg::Input(input));
    }

    /// Head the next [`TranscriptView::set_debug_entries`] fixtures with the
    /// "loading earlier messages" row (`received_bytes` so far), or not
    /// (`None`). Renders and tests.
    pub fn set_debug_history_pending(&self, received_bytes: Option<u64>) {
        let v = received_bytes.map_or(-1, |b| i64::try_from(b).unwrap_or(i64::MAX));
        self.debug_history.store(v, std::sync::atomic::Ordering::Relaxed);
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
        outcome: None,
        history_pending: None,
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
    /// First paint after attach renders only the newest entries; the rest
    /// follow in the very next pass (`followup`), behind the first frame.
    opened: bool,
    followup: bool,
    shared: Arc<Shared>,
    listener: Option<Arc<dyn LayoutListener>>,
}

/// Parts kept for the first frame of a cold open (a couple of screens;
/// everything above lands in the immediate second pass).
const OPENING_PARTS: usize = 48;

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
            opened: false,
            followup: false,
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
                            // New sizes: every prepared paragraph is stale —
                            // but what the user opened or folded stays so.
                            let old = std::mem::take(&mut self.builder);
                            self.builder.expanded = old.expanded;
                            self.builder.collapsed = old.collapsed;
                            self.builder.detail_open = old.detail_open;
                            self.heights.clear();
                            self.cache = WidthCache::new();
                        }
                        self.width = width;
                        dirty = true;
                    }
                    Msg::ToggleDetail { row, detail, open } => {
                        self.builder.detail_open.insert(detail, !open);
                        self.builder.invalidate(row);
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
                // A first frame that showed only the newest entries is
                // followed at once by the whole transcript.
                while self.followup {
                    self.pass();
                }
                // The width cache never evicts, and a streaming block that
                // falls back to the platform (CJK, emoji) or can't break (a
                // long hash) adds a whole-prefix entry per update. Past the
                // budget, start over: re-measuring is cheap next to the growth.
                if self.cache.stats().bytes > WIDTH_CACHE_BUDGET {
                    self.cache = WidthCache::new();
                }
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

    /// What the next pass lays out. A cold open keeps only the newest
    /// ~[`OPENING_PARTS`] parts (marked `followup` so the caller re-runs at
    /// once): preparing a transcript's every row before the first frame
    /// held a big chat's paint for seconds after its data had arrived.
    /// The dropped prefix is honest about it — the "loading earlier" head
    /// row is what that row exists for.
    fn opening_input(&mut self) -> (TranscriptInput, bool) {
        if self.opened || self.followup {
            return (self.input.clone(), false);
        }
        let mut parts = 0usize;
        let mut drop = 0usize;
        for (i, entry) in self.input.entries.iter().enumerate().rev() {
            parts += entry.parts.len();
            if parts >= OPENING_PARTS {
                drop = i;
                break;
            }
        }
        if drop == 0 {
            return (self.input.clone(), false);
        }
        let mut input = self.input.clone();
        input.entries.drain(..drop);
        input.history_pending = input.history_pending.or(Some(0));
        (input, true)
    }

    pub(crate) fn pass(&mut self) -> Arc<LayoutFrame> {
        let started = Instant::now();
        let (input, deferred) = self.opening_input();
        // An empty pass (attach fires an empty snapshot before the mirror
        // delivers) must not count as "opened": the real transcript still
        // gets its segmented first paint.
        let had_entries = !input.entries.is_empty();
        let placed: Vec<Placed> = {
            let mut ctx = Ctx {
                typo: &mut self.typo,
                cache: &mut self.cache,
            };
            self.builder.build(&mut ctx, &input)
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
        self.opened = self.opened || (!deferred && had_entries);
        self.followup = deferred;
        frame
    }
}

#[cfg(test)]
mod tests;
