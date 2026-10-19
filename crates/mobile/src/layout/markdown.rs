//! Markdown blocks → prepared text → placed display primitives.
//!
//! `prepare_block` does all measurement once (width-independent). `place`
//! is the single geometry routine: with `out = None` it only computes the
//! height (pure arithmetic — used for virtualization), with `Some` it emits the
//! same geometry as primitives. One routine for both means a row's measured
//! height and its painted content can never disagree.

use std::sync::Arc;
use zeron_markdown::parser::{Block, InlineRun, InlineStyle, TableAlign};
use zeron_text::{InlineBox, OverflowWrap, PrepareOptions, Prepared, Span, WhiteSpace, WidthCache, line_growth};

use super::display::{ColorRole, Decoration, DisplayBuilder, TextRun, WidgetKind};
use super::style::{Family, Resolved, TYPE, Typography, Weight, baseline};

pub(crate) struct Ctx<'a> {
    pub typo: &'a mut Typography,
    pub cache: &'a mut WidthCache,
    pub math: MathSettings,
    pub math_failures: &'a mut MathFailures,
    /// Preparing a table cell: a scrolling table is one canvas as wide and
    /// tall as all its rows, so formulas there are held shorter.
    pub in_table: bool,
    /// Canvas area the current row's formulas take, in points² (reset per
    /// row): a row is one backing store, so its formulas share a budget.
    pub math_area: f32,
}

/// Formulas zatex rejected, so preparing their rows again (a rebuild, a
/// streaming update, the same bad formula repeated) skips straight to the
/// source fallback instead of paying the failure again — some inputs only
/// fail after real work (a runaway `\\` stack fails on its extent). An LRU
/// of `CAP` fixed-size keys: a hash of (source, display, face, size), so its
/// memory is `CAP × 8` bytes whatever the sources. A hash collision would
/// show one formula as another — at 64 bits with a per-process seed, never in
/// practice. Successes
/// are kept too, bounded by count and item bytes: a streamed token re-prepares
/// its whole paragraph, and must not lay out every formula in it again.
#[derive(Default)]
pub(crate) struct MathFailures {
    /// Seeded per process, so no one can build colliding sources offline.
    seed: std::collections::hash_map::RandomState,
    /// Most recently used last.
    keys: std::collections::VecDeque<u64>,
    pub hits: u64,
    /// Formulas served from `laid` instead of laid out again.
    pub laid_hits: u64,
    /// Laid-out formulas by the same key, with the tick of their last use;
    /// `laid_order` holds (key, tick) uses oldest first, stale entries
    /// skipped, so a hit is O(1).
    laid: std::collections::HashMap<u64, (Arc<zatex::Formula>, u64)>,
    laid_order: std::collections::VecDeque<(u64, u64)>,
    laid_bytes: usize,
    tick: u64,
}

impl MathFailures {
    pub const CAP: usize = 256;
    const LAID_CAP: usize = 4096;
    const LAID_BYTES: usize = 8 << 20;

    fn bytes(formula: &zatex::Formula) -> usize {
        formula.items.len() * std::mem::size_of::<zatex::Item>()
    }

    fn touch(&mut self, key: u64) -> u64 {
        self.tick += 1;
        self.laid_order.push_back((key, self.tick));
        self.tick
    }

    /// Drop stale uses once they outnumber the live entries. Called only
    /// after the entry just touched holds its new tick, so its own use is
    /// never the one dropped (an entry without a use could never be evicted).
    #[cfg(test)]
    pub fn laid_stats(&self) -> (usize, usize, usize) {
        let live_uses = self
            .laid_order
            .iter()
            .filter(|(key, tick)| self.laid.get(key).is_some_and(|entry| entry.1 == *tick))
            .count();
        (self.laid.len(), live_uses, self.laid_bytes)
    }

    fn compact(&mut self) {
        if self.laid_order.len() > 4 * Self::LAID_CAP.max(self.laid.len()) {
            let laid = &self.laid;
            self.laid_order.retain(|(key, tick)| laid.get(key).is_some_and(|entry| entry.1 == *tick));
        }
    }

    fn laid(&mut self, key: u64) -> Option<Arc<zatex::Formula>> {
        let formula = self.laid.get(&key)?.0.clone();
        let tick = self.touch(key);
        if let Some(entry) = self.laid.get_mut(&key) {
            entry.1 = tick;
        }
        self.compact();
        self.laid_hits += 1;
        Some(formula)
    }

    fn keep(&mut self, key: u64, formula: Arc<zatex::Formula>) {
        self.laid_bytes += Self::bytes(&formula);
        let tick = self.touch(key);
        if let Some((old, _)) = self.laid.insert(key, (formula, tick)) {
            self.laid_bytes -= Self::bytes(&old);
        }
        self.compact();
        while self.laid.len() > Self::LAID_CAP || self.laid_bytes > Self::LAID_BYTES {
            let Some((old, tick)) = self.laid_order.pop_front() else { break };
            if self.laid.get(&old).is_some_and(|entry| entry.1 == tick)
                && let Some((old, _)) = self.laid.remove(&old)
            {
                self.laid_bytes -= Self::bytes(&old);
            }
        }
    }

    fn key(&self, tex: &str, display: (bool, bool), face: zatex::bundled::Face, size: f32) -> u64 {
        use std::hash::BuildHasher;
        self.seed.hash_one((tex, display, face, size.to_bits()))
    }

    /// Whether `key` failed before (and mark it recently used).
    fn known(&mut self, key: u64) -> bool {
        let Some(i) = self.keys.iter().rposition(|k| *k == key) else { return false };
        self.keys.remove(i);
        self.keys.push_back(key);
        self.hits += 1;
        true
    }

    fn insert(&mut self, key: u64) {
        if self.keys.len() >= Self::CAP {
            self.keys.pop_front();
        }
        self.keys.push_back(key);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.keys.len()
    }
}

/// `tex` laid out per the math settings at `size`, or None for its source
/// fallback (math off, or zatex can't render it — remembered in
/// `ctx.math_failures`).
/// Longest display-math source shown as a (non-wrapping) `math` code block
/// when it can't be typeset; longer source wraps as inline code instead.
const MATH_SOURCE_BLOCK_MAX: usize = 512;

/// Most a display formula may cover, in points², and its widest: a display
/// formula wider than the column scrolls in one canvas, so its whole area is
/// a backing store (3M pt² is ~108 MB at @3x; a 40-row `aligned` derivation
/// at 200% is under two thirds of that).
const MATH_MAX_AREA: f32 = 3e6;
const MATH_MAX_WIDTH: f32 = 8000.0;
/// Tallest inline formula, in ems: shrinking one to fit its line scales its
/// paint, not the line it grew. In a table cell, whose rows all share one
/// scrolling canvas, a formula is held to a few lines.
const MATH_MAX_INLINE_EMS: f32 = 30.0;
const MATH_MAX_CELL_EMS: f32 = 12.0;
/// Every formula in a row shares one backing store's budget, in points²
/// (~144 MB at @3x, about what a long code block or wide table costs). A
/// formula is charged its own area plus the band it opens across the row: a
/// display formula its whole height, an inline one what it grows its line
/// by — across the widest row a device shows (an iPad's), as preparing is
/// width-independent.
const MATH_ROW_AREA: f32 = 4e6;
const MATH_ROW_WIDTH: f32 = 1400.0;
/// Largest table canvas (points²) set with its formulas: about a wide text
/// table's. A table past it shows its formulas as source.
const MATH_TABLE_AREA: f32 = 4e6;

fn layout_math(ctx: &mut Ctx, tex: &str, display: bool, size: f32) -> Option<Arc<zatex::Formula>> {
    if !ctx.math.enabled {
        return None;
    }
    // Acceptance differs in a table cell, so it's part of the key.
    let key = ctx.math_failures.key(tex, (display, ctx.in_table), ctx.math.face, size);
    if ctx.math_failures.known(key) {
        return None;
    }
    if let Some(formula) = ctx.math_failures.laid(key) {
        return charge(ctx, formula, display);
    }
    let options = if display { zatex::Options::display(size) } else { zatex::Options::inline(size) };
    let formula = zatex::bundled::layout(tex, ctx.math.face, &options).ok().filter(|f| {
        if display {
            f.span() <= MATH_MAX_WIDTH && f.span() * f.height() <= MATH_MAX_AREA
        } else {
            let ems = if ctx.in_table { MATH_MAX_CELL_EMS } else { MATH_MAX_INLINE_EMS };
            f.height() <= ems * size
        }
    });
    match formula {
        Some(formula) => {
            let formula = Arc::new(formula);
            ctx.math_failures.keep(key, formula.clone());
            charge(ctx, formula, display)
        }
        None => {
            ctx.math_failures.insert(key);
            None
        }
    }
}

/// `formula` if its row's budget ([`MATH_ROW_AREA`]) still holds it, charged
/// to that budget; past it, the formula shows as source.
fn charge(ctx: &mut Ctx, formula: Arc<zatex::Formula>, display: bool) -> Option<Arc<zatex::Formula>> {
    let band = if display {
        formula.height()
    } else {
        (formula.height() - ctx.typo.px(TYPE.body.1)).max(0.0)
    };
    let cost = formula.span() * formula.height() + band * ctx.typo.px(MATH_ROW_WIDTH);
    if ctx.math_area + cost > MATH_ROW_AREA {
        return None;
    }
    ctx.math_area += cost;
    Some(formula)
}

/// How formulas render: in `face` when `enabled`, else as their source (an
/// inline code chip, a `math` code block) — the same fallback as TeX zatex
/// can't lay out. Formulas lay out at `size_percent` of the text around them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MathSettings {
    pub enabled: bool,
    pub face: zatex::bundled::Face,
    pub size_percent: u16,
}

impl MathSettings {
    pub const SIZE_PERCENT: std::ops::RangeInclusive<u16> = 75..=200;

    pub fn new(enabled: bool, face: zatex::bundled::Face, size_percent: u16) -> Self {
        Self { enabled, face, size_percent: size_percent.clamp(*Self::SIZE_PERCENT.start(), *Self::SIZE_PERCENT.end()) }
    }

    /// A formula's font size beside text of `text_px`.
    pub fn size(&self, text_px: f32) -> f32 {
        text_px * f32::from(self.size_percent) / 100.0
    }
}

impl Default for MathSettings {
    fn default() -> Self {
        Self { enabled: true, face: zatex::bundled::Face::Fira, size_percent: 100 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SpanPaint {
    pub color: ColorRole,
    pub decoration: Decoration,
    pub link: Option<u16>,
    /// Inline code: painted on a chip; the text sits inside the span padding.
    pub chip: bool,
}

/// A wrapped run of rich text (paragraph, heading, cell, label).
pub(crate) struct PText {
    pub p: Prepared,
    pub lh: f32,
    pub base: f32,
    pub paints: Vec<SpanPaint>,
    pub links: Vec<String>,
    /// Chip rect within a line box: (top offset, height).
    pub chip: (f32, f32),
    /// Spans painted as mention chips (the desktop's file badge: a soft pill
    /// with an icon well at its start), with each one's icon.
    pub badges: Vec<(usize, String)>,
    /// Spans that are inline formulas (an inline box over one U+FFFC), with
    /// each one's layout.
    pub math: Vec<(usize, Arc<zatex::Formula>)>,
}

pub(crate) struct PCode {
    pub label: Option<PText>,
    pub body: PText,
    pub lines: usize,
    pub content_width: f32,
    pub source: String,
}

/// Display math: the formula, centered in the column (scrolling when wider).
pub(crate) struct PMath {
    pub formula: Arc<zatex::Formula>,
    pub color: ColorRole,
}

pub(crate) struct PItem {
    pub marker: Option<PText>,
    pub task: Option<bool>,
    pub children: Vec<PBlock>,
}

pub(crate) struct PTable {
    /// Row 0 is the header.
    pub cells: Vec<Vec<PText>>,
    pub align: Vec<TableAlign>,
    pub cols: Vec<f32>,
}

pub(crate) enum PBlock {
    Text(PText),
    Heading(PText),
    Code(Box<PCode>),
    Math(Box<PMath>),
    Quote(Vec<PBlock>),
    List { items: Vec<PItem>, marker_w: f32 },
    Table(Box<PTable>),
    Rule,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextKind {
    Body,
    Heading(u8),
    TableHeader,
}

fn size_lh(kind: TextKind) -> (f32, f32) {
    match kind {
        TextKind::Body | TextKind::TableHeader => TYPE.body,
        TextKind::Heading(l) => TYPE.h[(l.clamp(1, 4) - 1) as usize],
    }
}

/// Measure inline runs as one wrapping flow.
pub(crate) fn prepare_runs(ctx: &mut Ctx, runs: &[InlineRun], kind: TextKind, muted: bool) -> PText {
    let (size, lh) = size_lh(kind);
    let heading = matches!(kind, TextKind::Heading(_) | TextKind::TableHeader);
    let base_weight = if heading { Weight::Semibold } else { Weight::Regular };
    let body = ctx.typo.style(Family::Sans, base_weight, false, size);
    let code = ctx.typo.style(Family::Mono, Weight::Regular, false, size * TYPE.inline_code / TYPE.body.0);
    let pad = ctx.typo.px(4.0);
    let chip_h = code.ascent + code.descent + ctx.typo.px(4.0);
    let lh = ctx.typo.px(lh);
    // A thin space either side of a formula, like TeX's math surround.
    let math_pad = ctx.typo.px(1.5);
    let math_size = ctx.math.size(ctx.typo.px(size));

    let mut text = String::new();
    let mut spans = Vec::with_capacity(runs.len());
    let mut paints = Vec::with_capacity(runs.len());
    let mut links: Vec<String> = Vec::new();
    let mut math = Vec::new();
    for run in runs {
        let s = &run.style;
        if s.task.is_some() {
            continue; // Lists draw task markers as checkboxes.
        }
        let (content, link) = match &s.image {
            Some(img) => (
                if img.alt.is_empty() { "image".to_owned() } else { img.alt.clone() },
                Some(img.link.clone().unwrap_or_else(|| img.source.clone())),
            ),
            None => (run.text.clone(), s.link.clone()),
        };
        if content.is_empty() {
            continue;
        }
        let link = link.map(|url| {
            let ix = links.iter().position(|l| *l == url).unwrap_or_else(|| {
                links.push(url);
                links.len() - 1
            });
            ix as u16
        });
        // Inline math is one U+FFFC laid out as the formula's box. Source
        // zatex can't render (an unknown command, a half-streamed formula),
        // or any with math rendering off, shows as a code chip, delimiters
        // and all, so nothing is lost.
        let formula = if s.math { layout_math(ctx, &content, false, math_size) } else { None };
        let chip = s.code || (s.math && formula.is_none());
        let style = if chip {
            code
        } else {
            let weight = if s.bold { Weight::Semibold.max(base_weight) } else if link.is_some() { Weight::Medium.max(base_weight) } else { base_weight };
            ctx.typo.style(Family::Sans, weight, s.italic, size)
        };
        let start = text.len();
        let span = match formula {
            Some(f) => {
                text.push('\u{FFFC}');
                // Everything the formula paints, not just its advance:
                // negative space or overhanging ink must not reach the text
                // beside it.
                let b = InlineBox {
                    width: f.span(),
                    ascent: f.ascent,
                    descent: f.descent,
                };
                math.push((spans.len(), f));
                Span {
                    pad_start: math_pad,
                    pad_end: math_pad,
                    ..Span::new(start..text.len(), style.id)
                }
                .with_inline_box(b)
            }
            None => {
                if s.math {
                    text.push('$');
                    text.push_str(&content);
                    text.push('$');
                } else {
                    text.push_str(&content);
                }
                Span {
                    pad_start: if chip { pad } else { 0.0 },
                    pad_end: if chip { pad } else { 0.0 },
                    ..Span::new(start..text.len(), style.id)
                }
            }
        };
        spans.push(span);
        let color = if link.is_some() {
            ColorRole::Link
        } else if chip {
            ColorRole::InlineCodeText
        } else if muted {
            ColorRole::TextSecondary
        } else {
            ColorRole::Text
        };
        paints.push(SpanPaint {
            color,
            decoration: if s.strikethrough { Decoration::Strikethrough } else { Decoration::None },
            link,
            chip,
        });
    }
    let p = zeron_text::prepare(
        &ctx.typo.book,
        ctx.cache,
        &text,
        &spans,
        &PrepareOptions {
            white_space: WhiteSpace::PreLine,
            overflow_wrap: OverflowWrap::Anywhere,
            ..PrepareOptions::default()
        },
    );
    PText {
        p,
        lh,
        base: baseline(lh, body),
        paints,
        links,
        chip: ((lh - chip_h) / 2.0, chip_h),
        badges: Vec::new(),
        math,
    }
}

/// Plain single-style text (labels, markers, tool rows).
pub(crate) fn prepare_plain(
    ctx: &mut Ctx,
    text: &str,
    style: Resolved,
    lh: f32,
    color: ColorRole,
    white_space: WhiteSpace,
) -> PText {
    let spans = [Span {
        range: 0..text.len(),
        style: style.id,
        pad_start: 0.0,
        pad_end: 0.0,
        atomic: false,
        inline_box: None,
    }];
    let p = zeron_text::prepare(
        &ctx.typo.book,
        ctx.cache,
        text,
        if text.is_empty() { &[] } else { &spans },
        &PrepareOptions {
            white_space,
            overflow_wrap: OverflowWrap::Anywhere,
            ..PrepareOptions::default()
        },
    );
    PText {
        p,
        lh,
        base: baseline(lh, style),
        paints: vec![SpanPaint {
            color,
            decoration: Decoration::None,
            link: None,
            chip: false,
        }],
        links: Vec::new(),
        chip: (0.0, 0.0),
        badges: Vec::new(),
        math: Vec::new(),
    }
}

pub(crate) fn prepare_block(ctx: &mut Ctx, block: &Block, depth: usize, muted: bool) -> PBlock {
    match block {
        Block::Paragraph { runs } => PBlock::Text(prepare_runs(ctx, runs, TextKind::Body, muted)),
        Block::Heading { level, runs } => {
            PBlock::Heading(prepare_runs(ctx, runs, TextKind::Heading(*level), muted))
        }
        Block::CodeBlock { language, code } => PBlock::Code(Box::new(prepare_code(ctx, language.as_deref(), code))),
        // Laid out once here, at body size (scaled per the math setting);
        // source zatex can't render (or any, with math rendering off) shows
        // as a `math` code block — or, when long, as wrapping code text: a
        // code block never wraps, and its scroller's canvas spans the whole
        // line (a 16 KB formula would be a ~360 MB backing store at @3x).
        Block::Math { tex } => {
            let size = ctx.math.size(ctx.typo.px(TYPE.body.0));
            match layout_math(ctx, tex, true, size) {
                Some(formula) => PBlock::Math(Box::new(PMath {
                    formula,
                    color: if muted { ColorRole::TextSecondary } else { ColorRole::Text },
                })),
                None if tex.len() <= MATH_SOURCE_BLOCK_MAX => PBlock::Code(Box::new(prepare_code(ctx, Some("math"), tex))),
                None => {
                    let source = InlineRun { text: format!("$${tex}$$"), style: InlineStyle { code: true, ..InlineStyle::default() } };
                    PBlock::Text(prepare_runs(ctx, &[source], TextKind::Body, muted))
                }
            }
        }
        Block::BlockQuote { children } => PBlock::Quote(
            children.iter().map(|b| prepare_block(ctx, b, depth, true)).collect(),
        ),
        Block::List { ordered_start, items } => {
            let (size, lh) = TYPE.body;
            let style = ctx.typo.style(Family::Sans, Weight::Regular, false, size);
            let lh = ctx.typo.px(lh);
            let mut marker_w: f32 = ctx.typo.px(18.0);
            let items = items
                .iter()
                .enumerate()
                .map(|(i, children)| {
                    let task = children.first().and_then(|b| match b {
                        Block::Paragraph { runs } => runs.first().and_then(|r| r.style.task.as_ref()).map(|t| t.checked),
                        _ => None,
                    });
                    let marker = if task.is_some() {
                        marker_w = marker_w.max(ctx.typo.px(27.0));
                        None
                    } else {
                        let label = match ordered_start {
                            Some(start) => format!("{}.", start + i as u64),
                            None => ["•", "◦", "▪"][depth.min(2)].to_owned(),
                        };
                        let m = prepare_plain(ctx, &label, style, lh, ColorRole::TextSecondary, WhiteSpace::Pre);
                        marker_w = marker_w.max(m.p.max_content_width() + ctx.typo.px(8.0));
                        Some(m)
                    };
                    PItem {
                        marker,
                        task,
                        children: children.iter().map(|b| prepare_block(ctx, b, depth + 1, muted)).collect(),
                    }
                })
                .collect();
            PBlock::List { items, marker_w }
        }
        Block::Table { header, rows, align } => {
            let (table, area) = prepare_table(ctx, header, rows, align, muted);
            // A table is one scrolling canvas as wide as all its columns and
            // as tall as all its rows: formulas that widen a column or raise
            // a row multiply across the whole table. Past the budget it's set
            // with its formulas as source, i.e. as a text table.
            let math = table.cells.iter().flatten().any(|cell| !cell.math.is_empty());
            if math && area > MATH_TABLE_AREA {
                let enabled = std::mem::replace(&mut ctx.math.enabled, false);
                let (plain, _) = prepare_table(ctx, header, rows, align, muted);
                ctx.math.enabled = enabled;
                PBlock::Table(Box::new(plain))
            } else {
                PBlock::Table(Box::new(table))
            }
        }
        Block::Rule => PBlock::Rule,
    }
}

/// A table's prepared cells and column widths, and the area (points²) of
/// the canvas it sets: every column (padding included) across, every row
/// down — independent of the width it's shown at.
fn prepare_table(
    ctx: &mut Ctx,
    header: &[Vec<InlineRun>],
    rows: &[Vec<Vec<InlineRun>>],
    align: &[TableAlign],
    muted: bool,
) -> (PTable, f32) {
    let mut cells = Vec::with_capacity(rows.len() + 1);
    // Restored after the table (a header-only table has no rows to reset
    // it), so the blocks after it keep their own limits.
    let outer = std::mem::replace(&mut ctx.in_table, true);
    cells.push(header.iter().map(|c| prepare_runs(ctx, c, TextKind::TableHeader, muted)).collect::<Vec<_>>());
    for row in rows {
        cells.push(row.iter().map(|c| prepare_runs(ctx, c, TextKind::Body, muted)).collect());
    }
    ctx.in_table = outer;
    let ncols = cells.iter().map(Vec::len).max().unwrap_or(0);
    let cap = ctx.typo.px(240.0);
    // A column grows past the cap to its widest formula (an atomic box,
    // padding included); a table wider than the column scrolls. Capped: a
    // wider formula shrinks to fit its cell instead.
    let formula_cap = ctx.typo.px(1200.0);
    let widest_box = |t: &PText| {
        let spans = t.p.spans();
        t.math
            .iter()
            .map(|(s, f)| (f.span() + spans[*s].pad_start + spans[*s].pad_end).min(formula_cap))
            .fold(0f32, f32::max)
    };
    let cols: Vec<f32> = (0..ncols)
        .map(|c| {
            let (min, max, boxed) = cells.iter().filter_map(|r| r.get(c)).fold((0f32, 0f32, 0f32), |(mn, mx, bx), t| {
                (mn.max(t.p.min_content_width()), mx.max(t.p.max_content_width()), bx.max(widest_box(t)))
            });
            max.min(cap).max(min.min(cap)).max(boxed).max(ctx.typo.px(24.0)).ceil()
        })
        .collect();
    let (pad_x, pad_y) = (ctx.typo.px(geom::CELL_PAD_X), ctx.typo.px(geom::CELL_PAD_Y));
    let width: f32 = cols.iter().map(|w| w + pad_x * 2.0).sum();
    let height: f32 = cells
        .iter()
        .map(|row| {
            row.iter().zip(&cols).map(|(cell, w)| place_text(cell, 0.0, 0.0, *w, None)).fold(0f32, f32::max) + pad_y * 2.0
        })
        .sum();
    (PTable { cells, align: align.to_vec(), cols }, width * height)
}

fn syntax_color(kind: zeron_syntax::HighlightKind) -> ColorRole {
    use zeron_syntax::HighlightKind as K;
    match kind {
        K::Comment => ColorRole::SyntaxComment,
        K::Keyword => ColorRole::SyntaxKeyword,
        K::String | K::StringSpecial => ColorRole::SyntaxString,
        K::Escape => ColorRole::SyntaxEscape,
        K::Number | K::Boolean => ColorRole::SyntaxNumber,
        K::Type | K::TypeBuiltin | K::Constructor => ColorRole::SyntaxType,
        K::Function | K::FunctionBuiltin | K::Macro => ColorRole::SyntaxFunction,
        K::Property => ColorRole::SyntaxProperty,
        K::Constant => ColorRole::SyntaxConstant,
        K::Variable | K::VariableSpecial | K::Parameter => ColorRole::SyntaxVariable,
        K::Operator => ColorRole::SyntaxOperator,
        K::Punctuation => ColorRole::SyntaxPunctuation,
        K::Tag | K::Label => ColorRole::SyntaxTag,
        K::Attribute => ColorRole::SyntaxAttribute,
        _ => ColorRole::CodeText,
    }
}

/// Highlight spans as contiguous (range, color) cover of `source`.
fn highlight_cover(source: &str, language: Option<&str>) -> Vec<(std::ops::Range<usize>, ColorRole)> {
    let mut cover = Vec::new();
    let mut cursor = 0usize;
    let doc = (source.len() <= 64 * 1024)
        .then(|| {
            zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                source,
                path: None,
                fence_tag: language,
            })
            .ok()
        })
        .flatten();
    if let Some(doc) = doc {
        let mut line_start = 0usize;
        for (i, line) in doc.lines.iter().enumerate() {
            if i > 0 {
                line_start = source[line_start..].find('\n').map_or(source.len(), |n| line_start + n + 1);
            }
            for span in line {
                let (s, e) = (line_start + span.range.start, line_start + span.range.end);
                if s < cursor || e > source.len() {
                    continue; // Nested capture already covered by its parent.
                }
                if s > cursor {
                    cover.push((cursor..s, ColorRole::CodeText));
                }
                cover.push((s..e, syntax_color(span.kind)));
                cursor = e;
            }
        }
    }
    if cursor < source.len() {
        cover.push((cursor..source.len(), ColorRole::CodeText));
    }
    cover
}

fn prepare_code(ctx: &mut Ctx, language: Option<&str>, code: &str) -> PCode {
    let source = code.strip_suffix('\n').unwrap_or(code);
    let (size, lh) = TYPE.code;
    let style = ctx.typo.style(Family::Mono, Weight::Regular, false, size);
    let lh = ctx.typo.px(lh);
    let cover = highlight_cover(source, language);
    let spans: Vec<Span> = cover
        .iter()
        .map(|(r, _)| Span {
            range: r.clone(),
            style: style.id,
            pad_start: 0.0,
            pad_end: 0.0,
            atomic: false,
            inline_box: None,
        })
        .collect();
    let p = zeron_text::prepare(
        &ctx.typo.book,
        ctx.cache,
        source,
        &spans,
        &PrepareOptions {
            white_space: WhiteSpace::Pre,
            overflow_wrap: OverflowWrap::Normal,
            tab_size: 4,
        },
    );
    let lines = p.line_count(f32::INFINITY).max(1);
    let content_width = p.max_content_width();
    let body = PText {
        p,
        lh,
        base: baseline(lh, style),
        paints: cover
            .iter()
            .map(|(_, color)| SpanPaint {
                color: *color,
                decoration: Decoration::None,
                link: None,
                chip: false,
            })
            .collect(),
        links: Vec::new(),
        chip: (0.0, 0.0),
        badges: Vec::new(),
        math: Vec::new(),
    };
    let label = language.filter(|l| !l.is_empty()).map(|l| {
        let (size, lh) = TYPE.small;
        let style = ctx.typo.style(Family::Sans, Weight::Medium, false, size - 1.0);
        let lh = ctx.typo.px(lh);
        prepare_plain(ctx, l, style, lh, ColorRole::TextTertiary, WhiteSpace::Pre)
    });
    PCode {
        label,
        body,
        lines,
        content_width,
        source: source.to_owned(),
    }
}

/// Geometry constants in points at text scale 1.0.
pub(crate) mod geom {
    pub const PARA_GAP: f32 = 10.0;
    pub const ITEM_GAP: f32 = 5.0;
    pub const QUOTE_INDENT: f32 = 15.0;
    pub const QUOTE_BAR: f32 = 3.0;
    pub const CODE_HEADER: f32 = 32.0;
    pub const CODE_PAD_X: f32 = 14.0;
    pub const CODE_PAD_BOTTOM: f32 = 12.0;
    pub const CODE_RADIUS: f32 = 12.0;
    pub const CELL_PAD_X: f32 = 10.0;
    pub const CELL_PAD_Y: f32 = 7.0;
    pub const RULE_HEIGHT: f32 = 21.0;
    /// Above and below display math.
    pub const MATH_PAD_Y: f32 = 6.0;
}

/// Layout scale (text scale) for geometry constants.
#[derive(Clone, Copy)]
pub(crate) struct Px(pub f32);
impl Px {
    pub fn v(self, v: f32) -> f32 {
        v * self.0
    }
}

/// Place wrapped text at (x, y) within `width`; returns its height. Lines are
/// `lh` tall, each grown by `line_growth` around its inline formulas: the
/// growth above lands before the baseline, the growth below after the line.
pub(crate) fn place_text(t: &PText, x: f32, y: f32, width: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    let Some(out) = out else {
        return t.p.height(width, t.lh, t.base);
    };
    let lines = t.p.lines(width);
    if lines.is_empty() {
        return 0.0;
    }
    let base16 = out.push_text(t.p.text());
    let spans = t.p.spans();
    // Each span's formula, if it is one: one lookup per fragment, not a scan.
    let mut formulas: Vec<Option<&zatex::Formula>> = vec![None; spans.len()];
    for (span, formula) in &t.math {
        if let Some(slot) = formulas.get_mut(*span) {
            *slot = Some(&**formula);
        }
    }
    let mut top = y;
    for line in &lines {
        let (above, below) = line_growth(t.p.line_box_extents(line), t.lh, t.base);
        let line_h = t.lh + above + below;
        // Chips and badges stay centered on the text, not the grown box.
        let text_top = top + above;
        let baseline = text_top + t.base;
        for f in &line.fragments {
            let paint = t.paints.get(f.span).copied().unwrap_or(SpanPaint {
                color: ColorRole::Text,
                decoration: Decoration::None,
                link: None,
                chip: false,
            });
            let span = &spans[f.span];
            let mut text_x = x + f.x;
            let mut text_w = f.width;
            if span.pad_start > 0.0 && f.range.start == span.range.start {
                text_x += span.pad_start;
                text_w -= span.pad_start;
            }
            if span.pad_end > 0.0 && f.range.end == span.range.end {
                text_w -= span.pad_end;
            }
            if paint.chip {
                out.fill(x + f.x, text_top + t.chip.0, f.width, t.chip.1, 5.0, ColorRole::InlineCodeBackground);
            }
            if let Some((_, icon)) = t.badges.iter().find(|(span, _)| *span == f.span) {
                let (chip_top, chip_h) = t.chip;
                out.fill(x + f.x, text_top + chip_top, f.width, chip_h, 5.0, ColorRole::ToolBadge);
                if f.range.start == span.range.start {
                    let well = chip_h - 2.0;
                    let (wx, wy) = (x + f.x + 1.0, text_top + chip_top + 1.0);
                    out.fill(wx, wy, well, well, 4.0, ColorRole::ToolWell);
                    let size = well - 4.0;
                    out.widget(
                        WidgetKind::Icon { name: icon.clone(), color: ColorRole::TextSoft },
                        (wx + (well - size) / 2.0, wy + (well - size) / 2.0, size, size),
                        None,
                    );
                }
            }
            if f.utf16.is_empty() {
                continue;
            }
            let start = base16 + f.utf16.start as u32;
            // A formula's placeholder is never drawn as text.
            if let Some(formula) = formulas.get(f.span).copied().flatten() {
                // A formula wider than the column (alone on its line — it's
                // atomic) shrinks to fit. Only the paint scales: the line's
                // extents stay those of the unscaled box in both the measure
                // and paint paths, and the scaled formula (ascent·s ≤ ascent,
                // descent·s ≤ descent) sits on the same baseline inside them,
                // so measured and painted heights can't disagree.
                let avail = x + width - text_x - span.pad_end;
                let span = formula.span();
                let scale = if span > avail && avail > 0.0 { avail / span } else { 1.0 };
                out.formula(formula, text_x - formula.left * scale, baseline, scale, paint.color, start);
                // Decorations cross the formula like the text around it
                // (the painter's run offsets: strike 5 above, underline 2 below).
                let dy = match paint.decoration {
                    Decoration::None => None,
                    Decoration::Strikethrough => Some(-5.0),
                    Decoration::Underline => Some(2.0),
                };
                if let Some(dy) = dy {
                    out.formula_fill(text_x, baseline + dy, span * scale, 1.0, paint.color, start);
                }
            } else {
                out.runs.push(TextRun {
                    start,
                    len: (f.utf16.end - f.utf16.start) as u32,
                    x: text_x,
                    baseline,
                    width: text_w.max(0.0),
                    style: span.style.0,
                    color: paint.color,
                    decoration: paint.decoration,
                    scroller: out.scroller,
                    reveal_ms: None,
                });
            }
            if let Some(link) = paint.link {
                out.links.push(super::display::LinkHit {
                    x: x + f.x,
                    y: top,
                    w: f.width,
                    h: line_h,
                    url: t.links[link as usize].clone(),
                    scroller: out.scroller,
                });
            }
        }
        top += line_h;
    }
    top - y
}

/// Stack blocks vertically with container gaps.
pub(crate) fn place_stack(
    blocks: &[PBlock],
    px: Px,
    x: f32,
    y: f32,
    width: f32,
    gap: f32,
    mut out: Option<&mut DisplayBuilder>,
) -> f32 {
    let mut h = 0.0;
    for (i, b) in blocks.iter().enumerate() {
        if i > 0 {
            h += px.v(gap);
        }
        h += place(b, px, x, y + h, width, out.as_deref_mut());
    }
    h
}

/// The geometry routine: height of `block` at `width`, emitting primitives when
/// `out` is given.
pub(crate) fn place(block: &PBlock, px: Px, x: f32, y: f32, width: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    use geom::*;
    match block {
        PBlock::Text(t) | PBlock::Heading(t) => place_text(t, x, y, width, out),
        PBlock::Code(c) => place_code(c, px, x, y, width, out),
        PBlock::Math(m) => place_math(m, px, x, y, width, out),
        PBlock::Quote(children) => {
            let indent = px.v(QUOTE_INDENT);
            let mut out = out;
            let h = place_stack(children, px, x + indent, y, (width - indent).max(1.0), PARA_GAP, out.as_deref_mut());
            if let Some(out) = out {
                out.fill(x, y, px.v(QUOTE_BAR), h, px.v(QUOTE_BAR) / 2.0, ColorRole::QuoteBar);
            }
            h
        }
        PBlock::List { items, marker_w } => {
            let mut out = out;
            let mut h = 0.0;
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    h += px.v(ITEM_GAP);
                }
                let top = y + h;
                let child_x = x + marker_w;
                let child_w = (width - marker_w).max(1.0);
                let ih = place_stack(&item.children, px, child_x, top, child_w, PARA_GAP, out.as_deref_mut());
                if let Some(out) = out.as_deref_mut() {
                    // Markers sit on the first line's text, below any growth.
                    let top = top + first_line_above(&item.children, child_w);
                    if let Some(m) = &item.marker {
                        // Right-align ordinals so "9." and "10." share a column.
                        let mw = m.p.max_content_width();
                        let mx = x + (marker_w - px.v(8.0) - mw).max(0.0);
                        place_text(m, mx, top, mw + 1.0, Some(out));
                    } else if let Some(checked) = item.task {
                        let s = px.v(18.0);
                        let lh = px.v(TYPE.body.1);
                        out.widget(
                            WidgetKind::Icon {
                                name: if checked { "checkmark.square.fill" } else { "square" }.into(),
                                color: if checked { ColorRole::Accent } else { ColorRole::TextTertiary },
                            },
                            (x, top + (lh - s) / 2.0, s, s),
                            None,
                        );
                    }
                }
                h += ih.max(px.v(TYPE.body.1));
            }
            h
        }
        PBlock::Table(t) => place_table(t, px, x, y, width, out),
        PBlock::Rule => {
            let h = px.v(RULE_HEIGHT);
            if let Some(out) = out {
                out.fill(x, y + (h / 2.0).floor(), width, 1.0, 0.0, ColorRole::Rule);
            }
            h
        }
    }
}

/// How much the first line of `blocks` grew above its text to fit an inline
/// formula (0 for anything but a paragraph or heading).
fn first_line_above(blocks: &[PBlock], width: f32) -> f32 {
    let Some(PBlock::Text(t) | PBlock::Heading(t)) = blocks.first() else { return 0.0 };
    if !t.p.has_inline_boxes() {
        return 0.0;
    }
    let first = t.p.layout_next_line(zeron_text::Cursor::START, width);
    line_growth(first.and_then(|l| t.p.line_range_box_extents(&l)), t.lh, t.base).0
}

fn place_code(c: &PCode, px: Px, x: f32, y: f32, width: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    use geom::*;
    let header = px.v(CODE_HEADER);
    let body_h = c.lines as f32 * c.body.lh + px.v(CODE_PAD_BOTTOM);
    let h = header + body_h;
    let Some(out) = out else { return h };
    out.fill(x, y, width, h, px.v(CODE_RADIUS), ColorRole::CodeBackground);
    out.hairline(x, y, width, h, px.v(CODE_RADIUS), ColorRole::CodeBorder);
    if let Some(label) = &c.label {
        let lw = width - px.v(CODE_PAD_X) - px.v(44.0);
        place_text(label, x + px.v(CODE_PAD_X), y + (header - label.lh) / 2.0, lw.max(1.0), Some(out));
    }
    let bw = px.v(44.0);
    out.widget(WidgetKind::CopyCode, (x + width - bw, y, bw, header), Some(c.source.clone()));
    let pad = px.v(CODE_PAD_X);
    out.begin_scroller(x, y + header, width, body_h, c.content_width + pad * 2.0);
    place_text(&c.body, pad, 0.0, f32::INFINITY, Some(out));
    out.end_scroller();
    h
}

fn place_math(m: &PMath, px: Px, x: f32, y: f32, width: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    let f = &m.formula;
    let pad = px.v(geom::MATH_PAD_Y);
    let h = f.height() + pad * 2.0;
    let Some(out) = out else { return h };
    let start = out.push_text("\u{FFFC}");
    // Placed by everything it paints (negative space, overhanging ink),
    // not just its advance.
    let span = f.span();
    if span > width {
        // Too wide for the column: scroll it, like a code block. Side
        // padding keeps the edges off the viewport's at both scroll ends.
        let side = px.v(4.0);
        out.begin_scroller(x, y, width, h, span + side * 2.0);
        out.formula(f, side - f.left, pad + f.ascent, 1.0, m.color, start);
        out.end_scroller();
    } else {
        out.formula(f, x + (width - span) / 2.0 - f.left, y + pad + f.ascent, 1.0, m.color, start);
    }
    h
}

fn place_table(t: &PTable, px: Px, x: f32, y: f32, width: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    use geom::*;
    let pad_x = px.v(CELL_PAD_X);
    let pad_y = px.v(CELL_PAD_Y);
    let row_heights: Vec<f32> = t
        .cells
        .iter()
        .map(|row| {
            row.iter()
                .zip(&t.cols)
                .map(|(cell, w)| place_text(cell, 0.0, 0.0, *w, None))
                .fold(0f32, f32::max)
                + pad_y * 2.0
        })
        .collect();
    let h: f32 = row_heights.iter().sum();
    let Some(out) = out else { return h };
    let content_w: f32 = t.cols.iter().map(|w| w + pad_x * 2.0).sum();
    let scrolls = content_w > width;
    let table_w = content_w.min(width);
    let (ox, oy) = if scrolls {
        out.begin_scroller(x, y, width, h, content_w);
        (0.0, 0.0)
    } else {
        (x, y)
    };
    let radius = px.v(10.0);
    if let Some(first) = row_heights.first() {
        // Rounded top corners only: a rounded fill plus a square lower half.
        out.fill(ox, oy, content_w, *first, radius, ColorRole::TableHeaderBackground);
        out.fill(ox, oy + first / 2.0, content_w, first / 2.0, 0.0, ColorRole::TableHeaderBackground);
    }
    out.hairline(ox, oy, if scrolls { content_w } else { table_w }, h, radius, ColorRole::TableBorder);
    let mut ry = oy;
    for (r, row) in t.cells.iter().enumerate() {
        if r > 0 {
            out.fill(ox, ry, content_w, 1.0, 0.0, ColorRole::TableBorder);
        }
        let mut cx = ox;
        for (c, w) in t.cols.iter().enumerate() {
            if let Some(cell) = row.get(c) {
                let natural = cell.p.stats(*w).max_line_width;
                let dx = match t.align.get(c).copied().unwrap_or_default() {
                    TableAlign::Left => 0.0,
                    TableAlign::Center => ((w - natural) / 2.0).max(0.0),
                    TableAlign::Right => (w - natural).max(0.0),
                };
                place_text(cell, cx + pad_x + dx, ry + pad_y, *w, Some(out));
            }
            cx += w + pad_x * 2.0;
        }
        ry += row_heights[r];
    }
    if scrolls {
        out.end_scroller();
    }
    h
}
