//! Cards: the transcript's bordered status surfaces (the goal strip, the todo
//! strip, a workflow run). One block list laid out top to bottom, so every
//! card is the same three things: header (icon, title, status pill, trailing
//! controls), rows (a glyph, a title, a trailing word, an optional second
//! line) and chip rails (horizontal scrollers).
//!
//! Like every row here it is measured once, width-independent ([`Card`] holds
//! prepared text), and [`place_card`] is the only geometry routine: with `out`
//! it paints, without it it only measures, so a card's height and its pixels
//! cannot disagree. Taps are `WidgetKind::Action` rects carrying an action
//! string the Rust side decides (`TranscriptView::act`), never a callback.

use super::display::{ColorRole, DisplayBuilder, WidgetKind};
use super::markdown::{Ctx, PBlock, PText, Px, geom::PARA_GAP, place_stack, place_text, prepare_plain};
use super::rows::place_text_lines;
use super::style::{Family, Resolved, TYPE, Weight};

use zeron_text::WhiteSpace;

/// Geometry in points at text scale 1.0.
pub(crate) mod geom {
    pub const PAD_X: f32 = 12.0;
    pub const PAD_Y: f32 = 10.0;
    pub const RADIUS: f32 = 14.0;
    pub const ICON: f32 = 16.0;
    pub const ICON_GAP: f32 = 10.0;
    pub const CONTROL: f32 = 32.0;
    pub const ROW_MIN: f32 = 32.0;
    pub const ROW_PAD: f32 = 6.0;
    pub const GLYPH: f32 = 14.0;
    pub const CHIP_H: f32 = 32.0;
    pub const CHIP_GAP: f32 = 6.0;
    pub const BTN_H: f32 = 34.0;
    pub const BTN_GAP: f32 = 8.0;
    pub const PILL_H: f32 = 20.0;
    pub const TAP_MIN: f32 = 44.0;
}

/// What a tap means; the string is interpreted by `TranscriptView::act`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Act {
    pub payload: String,
    /// The control's accessibility label.
    pub label: String,
}

impl Act {
    pub fn new(payload: impl Into<String>, label: impl Into<String>) -> Self {
        Self { payload: payload.into(), label: label.into() }
    }
}

/// A small mark at the start of a row or chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Glyph {
    None,
    /// A filled dot (a lit lamp).
    Dot(ColorRole),
    /// A hollow ring (not reached / waiting).
    Ring(ColorRole),
    Spinner,
    Icon(&'static str, ColorRole),
}

pub(crate) struct Header {
    pub icon: Glyph,
    pub title: PText,
    /// A status word in a tinted capsule after the title.
    pub pill: Option<PText>,
    /// Right-aligned on the title row; dropped when it does not fit.
    pub trail: Option<PText>,
    pub sub: Option<PText>,
    pub sub_lines: usize,
    /// One icon button before the chevron (pause / resume / dismiss).
    pub button: Option<(&'static str, ColorRole, Act)>,
    /// `Some(open)` draws a chevron.
    pub chevron: Option<bool>,
    /// Tapping the header (the whole band) does this.
    pub act: Option<Act>,
    /// A live spinner after the title (the run is working).
    pub spinner: bool,
}

pub(crate) struct Line {
    pub glyph: Glyph,
    pub title: PText,
    pub title_lines: usize,
    pub trail: Option<PText>,
    pub sub: Option<PText>,
    pub sub_lines: usize,
    pub act: Option<Act>,
    /// Extra left inset in points (scale 1.0).
    pub indent: f32,
}

pub(crate) struct ChipSpec {
    pub lead: Glyph,
    pub label: PText,
    pub trail: Option<PText>,
    pub act: Option<Act>,
    pub selected: bool,
}

pub(crate) struct Btn {
    pub label: PText,
    pub act: Act,
}

// Items are built once per card and shared; boxing the header only adds a hop.
#[allow(clippy::large_enum_variant)]
pub(crate) enum Item {
    Header(Header),
    Line(Line),
    /// A horizontally scrolling row of capsules.
    Chips(Vec<ChipSpec>),
    Text { text: PText, lines: usize, lead: Glyph },
    Buttons(Vec<Btn>),
    /// Markdown (an artifact preview), laid out at the card's inner width.
    Blocks(Vec<PBlock>),
    Space(f32),
}

pub(crate) struct Card {
    pub items: Vec<Item>,
}

/// The text styles every card builder shares, resolved once per build.
pub(crate) struct CardStyles {
    pub title: Resolved,
    pub label: Resolved,
    pub body: Resolved,
    pub small: Resolved,
    pub lh: f32,
    pub small_lh: f32,
}

impl CardStyles {
    pub fn new(ctx: &mut Ctx) -> Self {
        let (size, lh) = TYPE.small;
        Self {
            title: ctx.typo.style(Family::Sans, Weight::Semibold, false, size),
            label: ctx.typo.style(Family::Sans, Weight::Medium, false, size),
            body: ctx.typo.style(Family::Sans, Weight::Regular, false, size),
            small: ctx.typo.style(Family::Sans, Weight::Medium, false, size - 1.5),
            lh: ctx.typo.px(lh),
            small_lh: ctx.typo.px(lh - 3.0),
        }
    }

    /// Wrapping prose in the body face.
    pub fn text(&self, ctx: &mut Ctx, s: &str, color: ColorRole) -> PText {
        prepare_plain(ctx, s, self.body, self.lh, color, WhiteSpace::Normal)
    }

    /// One line in the body face (never wraps: fades at the slot's edge).
    pub fn one(&self, ctx: &mut Ctx, s: &str, color: ColorRole) -> PText {
        prepare_plain(ctx, s, self.body, self.lh, color, WhiteSpace::Pre)
    }

    pub fn label(&self, ctx: &mut Ctx, s: &str, color: ColorRole) -> PText {
        prepare_plain(ctx, s, self.label, self.lh, color, WhiteSpace::Pre)
    }

    pub fn title(&self, ctx: &mut Ctx, s: &str, color: ColorRole) -> PText {
        prepare_plain(ctx, s, self.title, self.lh, color, WhiteSpace::Pre)
    }

    pub fn tiny(&self, ctx: &mut Ctx, s: &str, color: ColorRole) -> PText {
        prepare_plain(ctx, s, self.small, self.small_lh, color, WhiteSpace::Pre)
    }
}

/// Lines `t` fills at `w`, capped at `max` (and at least one for non-empty).
fn lines_at(t: &PText, w: f32, max: usize) -> usize {
    if t.p.text().is_empty() {
        return 0;
    }
    if max == 1 {
        return 1;
    }
    t.p.line_count(w.max(1.0)).clamp(1, max)
}

/// Height of `t` clamped to `max` lines; paints when `out` is given.
fn put(t: &PText, x: f32, y: f32, w: f32, max: usize, px: Px, out: Option<&mut DisplayBuilder>) -> f32 {
    let n = lines_at(t, w, max);
    if n == 0 {
        return 0.0;
    }
    if let Some(out) = out {
        place_text_lines(t, x, y, w.max(1.0), max, px, out);
    }
    n as f32 * t.lh
}

fn natural(t: &PText) -> f32 {
    t.p.max_content_width().ceil() + 1.0
}

fn glyph(g: Glyph, px: Px, x: f32, y: f32, size: f32, out: &mut DisplayBuilder) {
    match g {
        Glyph::None => {}
        Glyph::Dot(c) => {
            let d = size * 0.5;
            out.fill(x + (size - d) / 2.0, y + (size - d) / 2.0, d, d, d / 2.0, c);
        }
        Glyph::Ring(c) => {
            let d = size * 0.58;
            out.hairline(x + (size - d) / 2.0, y + (size - d) / 2.0, d, d, d / 2.0, c);
        }
        Glyph::Spinner => {
            out.widget(WidgetKind::Spinner, (x, y, size, size), None);
        }
        Glyph::Icon(name, c) => {
            let s = size.min(px.v(16.0));
            out.widget(WidgetKind::Icon { name: name.into(), color: c }, (x + (size - s) / 2.0, y + (size - s) / 2.0, s, s), None);
        }
    }
}

fn action(out: &mut DisplayBuilder, act: &Act, rect: (f32, f32, f32, f32)) {
    out.widget(WidgetKind::Action { label: act.label.clone() }, rect, Some(act.payload.clone()));
}

// MARK: - Items

fn place_header(h: &Header, px: Px, x: f32, y: f32, cw: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    use geom::*;
    let pad = px.v(PAD_X);
    let icon = px.v(ICON);
    let tx = x + pad + icon + px.v(ICON_GAP);
    let mut rx = x + cw - pad;
    let row = h.title.lh.max(px.v(PILL_H));
    // Controls (right to left): chevron, then the icon button.
    let mut controls: Vec<(&'static str, ColorRole, f32, Option<&Act>)> = Vec::new();
    if let Some(open) = h.chevron {
        rx -= px.v(18.0);
        controls.push((if open { "chevron.down" } else { "chevron.right" }, ColorRole::TextTertiary, rx, None));
        rx -= px.v(4.0);
    }
    if let Some((name, color, act)) = &h.button {
        rx -= px.v(CONTROL);
        controls.push((name, *color, rx, Some(act)));
    }
    let text_w = (rx - px.v(8.0) - tx).max(px.v(40.0));
    let sub_w = text_w;
    let sub_real = h.sub.as_ref().map_or(0.0, |s| lines_at(s, sub_w, h.sub_lines) as f32 * s.lh);
    let total = row + if sub_real > 0.0 { px.v(2.0) + sub_real } else { 0.0 };
    let Some(o) = out else { return total };

    if let Some(act) = &h.act {
        // The whole band: comfortable to hit, and under the controls.
        let band = (total + px.v(PAD_Y) * 2.0).max(px.v(TAP_MIN));
        action(o, act, (x, y - px.v(PAD_Y), cw, band));
    }
    glyph(h.icon, px, x + pad, y + (row - icon) / 2.0, icon, o);
    // Title row: title, pill, trail.
    let pill_w = h.pill.as_ref().map_or(0.0, |t| natural(t) + px.v(16.0));
    let pill_gap = if h.pill.is_some() { px.v(8.0) } else { 0.0 };
    let spin_w = if h.spinner { px.v(14.0) + px.v(8.0) } else { 0.0 };
    let trail_w = h.trail.as_ref().map_or(0.0, natural);
    let mut title_w = natural(&h.title);
    let fixed = pill_w + pill_gap + spin_w;
    let show_trail = h.trail.is_some() && title_w.min(px.v(90.0)) + fixed + trail_w + px.v(12.0) <= text_w;
    let reserve = fixed + if show_trail { trail_w + px.v(12.0) } else { 0.0 };
    title_w = title_w.min((text_w - reserve).max(px.v(30.0)));
    place_text_lines(&h.title, tx, y + (row - h.title.lh) / 2.0, title_w, 1, px, o);
    let mut cx = tx + title_w;
    if let Some(t) = &h.pill {
        cx += pill_gap;
        let ph = px.v(PILL_H);
        o.fill(cx, y + (row - ph) / 2.0, pill_w, ph, ph / 2.0, ColorRole::ChipBackground);
        place_text(t, cx + px.v(8.0), y + (row - t.lh) / 2.0, natural(t), Some(o));
        cx += pill_w;
    }
    if h.spinner {
        let s = px.v(14.0);
        o.widget(WidgetKind::Spinner, (cx + px.v(8.0), y + (row - s) / 2.0, s, s), None);
    }
    if show_trail && let Some(t) = &h.trail {
        place_text(t, tx + text_w - trail_w, y + (row - t.lh) / 2.0, trail_w, Some(o));
    }
    if let Some(sub) = &h.sub {
        put(sub, tx, y + row + px.v(2.0), sub_w, h.sub_lines, px, Some(o));
    }
    for (name, color, cxp, act) in controls {
        let size = if act.is_some() { px.v(18.0) } else { px.v(14.0) };
        let slot = if act.is_some() { px.v(CONTROL) } else { px.v(18.0) };
        o.widget(WidgetKind::Icon { name: name.into(), color }, (cxp + (slot - size) / 2.0, y + (row - size) / 2.0, size, size), None);
        if let Some(act) = act {
            let hit = px.v(TAP_MIN);
            action(o, act, (cxp + slot / 2.0 - hit / 2.0, y + row / 2.0 - hit / 2.0, hit, hit));
        }
    }
    total
}

fn place_line(l: &Line, px: Px, x: f32, y: f32, cw: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    use geom::*;
    let pad = px.v(PAD_X);
    let gs = px.v(GLYPH);
    let ix = x + pad + px.v(l.indent);
    let tx = ix + gs + px.v(8.0);
    let right = x + cw - pad;
    let trail_w = l.trail.as_ref().map_or(0.0, |t| natural(t) + px.v(8.0));
    let title_w = (right - tx - trail_w).max(px.v(30.0));
    let sub_w = (right - tx).max(px.v(30.0));
    let rp = px.v(ROW_PAD);
    let title_h = lines_at(&l.title, title_w, l.title_lines) as f32 * l.title.lh;
    let sub_h = l.sub.as_ref().map_or(0.0, |s| lines_at(s, sub_w, l.sub_lines) as f32 * s.lh);
    let body = title_h + if sub_h > 0.0 { px.v(1.0) + sub_h } else { 0.0 };
    let h = (rp * 2.0 + body).max(px.v(ROW_MIN));
    let Some(o) = out else { return h };
    if let Some(act) = &l.act {
        action(o, act, (x, y, cw, h));
    }
    let first_lh = l.title.lh;
    glyph(l.glyph, px, ix, y + rp + (first_lh - gs) / 2.0, gs, o);
    let ty = y + (h - body) / 2.0;
    put(&l.title, tx, ty, title_w, l.title_lines, px, Some(o));
    if let Some(t) = &l.trail {
        place_text(t, right - natural(t), ty, natural(t), Some(o));
    }
    if let Some(s) = &l.sub {
        put(s, tx, ty + title_h + px.v(1.0), sub_w, l.sub_lines, px, Some(o));
    }
    h
}

fn chip_width(c: &ChipSpec, px: Px) -> f32 {
    use geom::*;
    let lead = if matches!(c.lead, Glyph::None) { 0.0 } else { px.v(GLYPH) + px.v(6.0) };
    let trail = c.trail.as_ref().map_or(0.0, |t| natural(t) + px.v(6.0));
    px.v(10.0) + lead + natural(&c.label) + trail + px.v(10.0)
}

fn place_chips(chips: &[ChipSpec], px: Px, x: f32, y: f32, cw: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    use geom::*;
    let h = px.v(CHIP_H);
    let Some(o) = out else { return h };
    let pad = px.v(PAD_X);
    let gap = px.v(CHIP_GAP);
    let widths: Vec<f32> = chips.iter().map(|c| chip_width(c, px)).collect();
    let content = widths.iter().sum::<f32>() + gap * chips.len().saturating_sub(1) as f32;
    o.begin_scroller(x + pad, y, cw - pad * 2.0, h, content);
    let mut cx = 0.0;
    for (c, w) in chips.iter().zip(&widths) {
        let wash = if c.selected { ColorRole::AgentTile } else { ColorRole::ChipBackground };
        o.fill(cx, 0.0, *w, h, h / 2.0, wash);
        if c.selected {
            o.hairline(cx, 0.0, *w, h, h / 2.0, ColorRole::AgentCardBorder);
        }
        let mut tx = cx + px.v(10.0);
        if !matches!(c.lead, Glyph::None) {
            glyph(c.lead, px, tx, (h - px.v(GLYPH)) / 2.0, px.v(GLYPH), o);
            tx += px.v(GLYPH) + px.v(6.0);
        }
        place_text(&c.label, tx, (h - c.label.lh) / 2.0, natural(&c.label), Some(o));
        if let Some(t) = &c.trail {
            place_text(t, tx + natural(&c.label) + px.v(6.0), (h - t.lh) / 2.0, natural(t), Some(o));
        }
        if let Some(act) = &c.act {
            action(o, act, (cx, (h - px.v(TAP_MIN)) / 2.0, *w, px.v(TAP_MIN)));
        }
        cx += w + gap;
    }
    o.end_scroller();
    h
}

fn place_buttons(btns: &[Btn], px: Px, x: f32, y: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    use geom::*;
    let h = px.v(BTN_H);
    let Some(o) = out else { return h };
    let mut bx = x + px.v(PAD_X);
    for b in btns {
        let w = natural(&b.label) + px.v(28.0);
        o.fill(bx, y, w, h, h / 2.0, ColorRole::ChipBackground);
        place_text(&b.label, bx + px.v(14.0), y + (h - b.label.lh) / 2.0, natural(&b.label), Some(o));
        action(o, &b.act, (bx, y + (h - px.v(TAP_MIN)) / 2.0, w, px.v(TAP_MIN)));
        bx += w + px.v(BTN_GAP);
    }
    h
}

fn place_item(item: &Item, px: Px, x: f32, y: f32, cw: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    use geom::*;
    let pad = px.v(PAD_X);
    match item {
        Item::Header(h) => place_header(h, px, x, y, cw, out),
        Item::Line(l) => place_line(l, px, x, y, cw, out),
        Item::Chips(c) => place_chips(c, px, x, y, cw, out),
        Item::Buttons(b) => place_buttons(b, px, x, y, out),
        Item::Space(v) => px.v(*v),
        Item::Text { text, lines, lead } => {
            let lead_w = if matches!(lead, Glyph::None) { 0.0 } else { px.v(GLYPH) + px.v(8.0) };
            let w = cw - pad * 2.0 - lead_w;
            let mut out = out;
            if let Some(o) = out.as_deref_mut() {
                glyph(*lead, px, x + pad, y + (text.lh - px.v(GLYPH)) / 2.0, px.v(GLYPH), o);
            }
            put(text, x + pad + lead_w, y, w, *lines, px, out)
        }
        Item::Blocks(blocks) => place_stack(blocks, px, x + pad, y, cw - pad * 2.0, PARA_GAP, out),
    }
}

/// Lay a card out at `width` (its column), measuring or painting. Returns
/// the card's height including its border padding.
pub(crate) fn place_card(card: &Card, px: Px, x: f32, y: f32, cw: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    use geom::*;
    let top = px.v(PAD_Y);
    let mut cursor = top;
    for item in &card.items {
        cursor += place_item(item, px, x, y + cursor, cw, None);
    }
    let h = cursor + top;
    if let Some(o) = out {
        o.fill(x, y, cw, h, px.v(RADIUS), ColorRole::AgentCard);
        o.hairline(x, y, cw, h, px.v(RADIUS), ColorRole::AgentCardBorder);
        let mut cursor = top;
        for item in &card.items {
            cursor += place_item(item, px, x, y + cursor, cw, Some(o));
        }
    }
    h
}

/// Heap held by a card's prepared text (diagnostics).
pub(crate) fn heap_bytes(card: &Card) -> usize {
    let t = |t: &PText| t.p.heap_bytes();
    card.items
        .iter()
        .map(|i| match i {
            Item::Header(h) => t(&h.title) + h.sub.as_ref().map_or(0, t) + h.trail.as_ref().map_or(0, t) + h.pill.as_ref().map_or(0, t),
            Item::Line(l) => t(&l.title) + l.sub.as_ref().map_or(0, t) + l.trail.as_ref().map_or(0, t),
            Item::Chips(c) => c.iter().map(|c| t(&c.label) + c.trail.as_ref().map_or(0, t)).sum(),
            Item::Text { text, .. } => t(text),
            Item::Buttons(b) => b.iter().map(|b| t(&b.label)).sum(),
            Item::Blocks(_) | Item::Space(_) => 0,
        })
        .sum()
}
