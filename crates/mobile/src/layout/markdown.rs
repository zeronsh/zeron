//! Markdown blocks → prepared text → placed display primitives.
//!
//! `prepare_block` does all measurement once (width-independent). `place`
//! is the single geometry routine: with `out = None` it only computes the
//! height (pure arithmetic — used for virtualization), with `Some` it emits the
//! same geometry as primitives. One routine for both means a row's measured
//! height and its painted content can never disagree.

use zeron_markdown::parser::{Block, InlineRun, TableAlign};
use zeron_text::{OverflowWrap, PrepareOptions, Prepared, Span, WhiteSpace, WidthCache};

use super::display::{ColorRole, Decoration, DisplayBuilder, TextRun, WidgetKind};
use super::style::{Family, Resolved, TYPE, Typography, Weight, baseline};

pub(crate) struct Ctx<'a> {
    pub typo: &'a mut Typography,
    pub cache: &'a mut WidthCache,
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
}

pub(crate) struct PCode {
    pub label: Option<PText>,
    pub body: PText,
    pub lines: usize,
    pub content_width: f32,
    pub source: String,
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

    let mut text = String::new();
    let mut spans = Vec::with_capacity(runs.len());
    let mut paints = Vec::with_capacity(runs.len());
    let mut links: Vec<String> = Vec::new();
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
        let style = if s.code {
            code
        } else {
            let weight = if s.bold { Weight::Semibold.max(base_weight) } else if link.is_some() { Weight::Medium.max(base_weight) } else { base_weight };
            ctx.typo.style(Family::Sans, weight, s.italic, size)
        };
        let start = text.len();
        text.push_str(&content);
        spans.push(Span {
            range: start..text.len(),
            style: style.id,
            pad_start: if s.code { pad } else { 0.0 },
            pad_end: if s.code { pad } else { 0.0 },
            atomic: false,
        });
        let color = if link.is_some() {
            ColorRole::Link
        } else if s.code {
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
            chip: s.code,
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
    }
}

pub(crate) fn prepare_block(ctx: &mut Ctx, block: &Block, depth: usize, muted: bool) -> PBlock {
    match block {
        Block::Paragraph { runs } => PBlock::Text(prepare_runs(ctx, runs, TextKind::Body, muted)),
        Block::Heading { level, runs } => {
            PBlock::Heading(prepare_runs(ctx, runs, TextKind::Heading(*level), muted))
        }
        Block::CodeBlock { language, code } => PBlock::Code(Box::new(prepare_code(ctx, language.as_deref(), code))),
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
            let mut cells = Vec::with_capacity(rows.len() + 1);
            cells.push(header.iter().map(|c| prepare_runs(ctx, c, TextKind::TableHeader, muted)).collect::<Vec<_>>());
            for row in rows {
                cells.push(row.iter().map(|c| prepare_runs(ctx, c, TextKind::Body, muted)).collect());
            }
            let ncols = cells.iter().map(Vec::len).max().unwrap_or(0);
            let cap = ctx.typo.px(240.0);
            let cols = (0..ncols)
                .map(|c| {
                    let (min, max) = cells.iter().filter_map(|r| r.get(c)).fold((0f32, 0f32), |(mn, mx), t| {
                        (mn.max(t.p.min_content_width()), mx.max(t.p.max_content_width()))
                    });
                    max.min(cap).max(min.min(cap)).max(ctx.typo.px(24.0)).ceil()
                })
                .collect();
            PBlock::Table(Box::new(PTable {
                cells,
                align: align.clone(),
                cols,
            }))
        }
        Block::Rule => PBlock::Rule,
    }
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
}

/// Layout scale (text scale) for geometry constants.
#[derive(Clone, Copy)]
pub(crate) struct Px(pub f32);
impl Px {
    pub fn v(self, v: f32) -> f32 {
        v * self.0
    }
}

/// Place wrapped text at (x, y) within `width`; returns its height.
pub(crate) fn place_text(t: &PText, x: f32, y: f32, width: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    let Some(out) = out else {
        return t.p.line_count(width) as f32 * t.lh;
    };
    let lines = t.p.lines(width);
    if lines.is_empty() {
        return 0.0;
    }
    let base16 = out.push_text(t.p.text());
    let spans = t.p.spans();
    for (i, line) in lines.iter().enumerate() {
        let top = y + i as f32 * t.lh;
        let baseline = top + t.base;
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
                out.fill(x + f.x, top + t.chip.0, f.width, t.chip.1, 5.0, ColorRole::InlineCodeBackground);
            }
            if f.utf16.is_empty() {
                continue;
            }
            out.runs.push(TextRun {
                start: base16 + f.utf16.start as u32,
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
            if let Some(link) = paint.link {
                out.links.push(super::display::LinkHit {
                    x: x + f.x,
                    y: top,
                    w: f.width,
                    h: t.lh,
                    url: t.links[link as usize].clone(),
                    scroller: out.scroller,
                });
            }
        }
    }
    lines.len() as f32 * t.lh
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
