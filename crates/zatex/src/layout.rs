//! Math list → positioned boxes, following TeX's Appendix G as MathML Core
//! restates it for OpenType MATH fonts. Every size, gap and shift is a font
//! constant scaled to the current style, so the same code serves any font.
//!
//! Coordinates are points with y down; a box's origin is its left baseline,
//! its ink spans `-h ..= d` vertically.

use crate::ast::{Align, Array, ArrayKind, Class, Dim, Node, Style};
use crate::font::{Corner, MathFont, Stretched};
use crate::unicode::{Variant, styled};
use crate::{Error, ErrorKind, Item};

/// TeX's `\nulldelimiterspace` (1.2pt at 10pt), in ems.
const NULL_DELIMITER: f32 = 0.12;
/// `\delimiterfactor` 901 and `\delimitershortfall` 5pt.
const DELIMITER_FACTOR: f32 = 0.901;
const DELIMITER_SHORTFALL: f32 = 0.5;
/// `\big` … `\Bigg` total heights, in ems.
const BIG_SIZES: [f32; 4] = [1.2, 1.8, 2.4, 3.0];

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct St {
    pub style: Style,
    pub cramped: bool,
}

impl St {
    pub fn new(style: Style) -> Self {
        Self {
            style,
            cramped: false,
        }
    }

    fn script(self) -> Style {
        match self.style {
            Style::Display | Style::Text => Style::Script,
            _ => Style::ScriptScript,
        }
    }

    fn sup(self) -> St {
        St {
            style: self.script(),
            cramped: self.cramped,
        }
    }

    fn sub(self) -> St {
        St {
            style: self.script(),
            cramped: true,
        }
    }

    fn frac_part(self) -> Style {
        match self.style {
            Style::Display => Style::Text,
            Style::Text => Style::Script,
            _ => Style::ScriptScript,
        }
    }

    fn num(self) -> St {
        St {
            style: self.frac_part(),
            cramped: self.cramped,
        }
    }

    fn den(self) -> St {
        St {
            style: self.frac_part(),
            cramped: true,
        }
    }

    fn cramp(self) -> St {
        St {
            style: self.style,
            cramped: true,
        }
    }

    fn display(self) -> bool {
        self.style == Style::Display
    }

    fn small(self) -> bool {
        self.style >= Style::Script
    }
}

/// A glyph of some font in the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct G {
    pub font: u8,
    pub id: u16,
}

/// A laid-out box.
#[derive(Debug, Clone, Default)]
pub(crate) struct LBox {
    pub w: f32,
    pub h: f32,
    pub d: f32,
    /// Italic correction of a box that is a single glyph.
    pub italic: f32,
    /// The glyph, when the box is exactly one.
    pub glyph: Option<G>,
    pub items: Vec<Item>,
}

impl LBox {
    /// Moves `other`'s items into `self` at an offset (dimensions untouched).
    fn put(&mut self, other: LBox, dx: f32, dy: f32) {
        self.items.reserve(other.items.len());
        self.items
            .extend(other.items.into_iter().map(|i| i.shifted(dx, dy)));
    }

    fn shift_y(&mut self, dy: f32) {
        for item in &mut self.items {
            *item = item.shifted(0.0, dy);
        }
        self.h -= dy;
        self.d += dy;
    }
}

struct Atom {
    class: Class,
    b: LBox,
    /// A plain character: TeX adds its italic correction to the width.
    char: bool,
    /// Scripts attach as limits (above/below).
    limits: bool,
    /// A large operator: subscripts tuck under its italic overhang.
    op: bool,
}

impl Atom {
    fn new(class: Class, b: LBox) -> Self {
        Self {
            class,
            b,
            char: false,
            limits: false,
            op: false,
        }
    }
}

enum Piece {
    Atom(Atom, St),
    Kern(f32),
}

pub(crate) struct Ctx<'f> {
    pub font: &'f MathFont,
    pub size: f32,
    pub max_items: usize,
    pub items: usize,
    /// Everything laid out so far — atoms, items, cells, assembly parts,
    /// including attempts that get discarded. Never reset, so it bounds time
    /// where the item count alone can't.
    work: usize,
}

type R<T> = Result<T, Error>;

impl<'f> Ctx<'f> {
    pub fn new(font: &'f MathFont, size: f32, max_items: usize) -> Self {
        Self {
            font,
            size,
            max_items,
            items: 0,
            work: 0,
        }
    }

    fn font_size(&self, st: St) -> f32 {
        self.size
            * match st.style {
                Style::Display | Style::Text => 1.0,
                Style::Script => self.font.c.script_scale,
                Style::ScriptScript => self.font.c.script_script_scale,
            }
    }

    /// Points per font unit of the primary font (whose constants drive layout).
    fn k(&self, st: St) -> f32 {
        self.font_size(st) / self.font.upem()
    }

    fn face(&self, g: G) -> &'f MathFont {
        self.font.at(g.font)
    }

    /// Points per font unit of the font `g` comes from.
    fn kg(&self, st: St, g: G) -> f32 {
        self.font_size(st) / self.face(g).upem()
    }

    fn count(&mut self, n: usize) -> R<()> {
        self.items += n;
        if self.items > self.max_items {
            return Err(Error::new(ErrorKind::TooLarge, 0..0));
        }
        self.work(n)
    }

    /// Charges `n` units against the work budget (a few times `max_items`).
    fn work(&mut self, n: usize) -> R<()> {
        self.work = self.work.saturating_add(n);
        if self.work > self.max_items.saturating_mul(4) {
            return Err(Error::new(ErrorKind::TooLarge, 0..0));
        }
        Ok(())
    }

    /// A filled rule; one over negative-width content (`\overline{\hspace{-5em}}`)
    /// draws nothing, as in TeX, rather than leaving hosts to disagree on a
    /// negative rectangle.
    fn rule(&mut self, x: f32, y: f32, width: f32, height: f32) -> R<Item> {
        self.count(1)?;
        Ok(Item::Rule {
            x,
            y,
            width: width.max(0.0),
            height: height.max(0.0),
        })
    }

    fn glyph_box(&mut self, g: G, st: St) -> R<LBox> {
        self.count(1)?;
        let k = self.kg(st, g);
        let face = self.face(g);
        let g = match st.style {
            Style::Script => G {
                id: face.script_glyph(g.id, 1),
                ..g
            },
            Style::ScriptScript => G {
                id: face.script_glyph(g.id, 2),
                ..g
            },
            _ => g,
        };
        let m = face.metrics(g.id);
        Ok(LBox {
            w: m.advance * k,
            h: m.height * k,
            d: m.depth * k,
            italic: face.italic_correction(g.id) * k,
            glyph: Some(g),
            items: vec![Item::Glyph {
                font: g.font,
                glyph: g.id,
                x: 0.0,
                y: 0.0,
                size: self.font_size(st),
            }],
        })
    }

    /// The first font in the chain that has `ch`.
    fn find(&self, ch: char) -> Option<G> {
        self.font
            .chain()
            .enumerate()
            .find_map(|(i, f)| f.glyph(ch).map(|id| G { font: i as u8, id }))
    }

    /// `ch` from the chain, else a look-alike, else an error.
    fn lookup(&self, ch: char, at: u32) -> R<G> {
        self.find(ch)
            .or_else(|| equivalents(ch).iter().find_map(|&alt| self.find(alt)))
            .ok_or_else(|| {
                let at = at as usize;
                Error::new(ErrorKind::MissingGlyph(ch), at..at + ch.len_utf8())
            })
    }

    /// A character in an alphabet: from any font in the chain that has the
    /// styled letter, else the plain character.
    fn char_glyph(&self, ch: char, variant: Variant, at: u32) -> R<G> {
        let mapped = styled(ch, variant);
        match (mapped != ch).then(|| self.find(mapped)).flatten() {
            Some(g) => Ok(g),
            None => self.lookup(ch, at),
        }
    }

    /// `glyph` grown along an axis to `target` points.
    fn stretched(&mut self, glyph: G, target: f32, vertical: bool, st: St) -> R<LBox> {
        let k = self.kg(st, glyph);
        let face = self.face(glyph);
        match face.stretch(glyph.id, target / k, vertical) {
            Stretched::Glyph(id) => self.glyph_box(G { id, ..glyph }, st),
            Stretched::Assembly { parts, length } => {
                self.count(parts.len())?;
                let size = self.font_size(st);
                let mut b = LBox::default();
                for p in &parts {
                    let m = face.metrics(p.glyph);
                    let (x, y) = if vertical {
                        // Stack from the baseline up, each part's ink bottom at its offset.
                        b.w = b.w.max(m.advance * k);
                        (0.0, -(p.offset + m.depth) * k)
                    } else {
                        b.h = b.h.max(m.height * k);
                        b.d = b.d.max(m.depth * k);
                        (p.offset * k, 0.0)
                    };
                    b.items.push(Item::Glyph {
                        font: glyph.font,
                        glyph: p.glyph,
                        x,
                        y,
                        size,
                    });
                }
                if vertical {
                    b.h = length * k;
                } else {
                    b.w = length * k;
                }
                Ok(b)
            }
        }
    }

    /// `ch` grown to `target` points by the first font in the chain that can
    /// reach it (a font without size variants defers to its fallback), else
    /// the largest any of them makes.
    fn stretch_char(&mut self, ch: char, target: f32, vertical: bool, st: St) -> R<LBox> {
        let extent = |b: &LBox| if vertical { b.h + b.d } else { b.w };
        let mut best: Option<LBox> = None;
        let fonts: Vec<u8> = self
            .font
            .chain()
            .enumerate()
            .filter(|(_, f)| f.glyph(ch).is_some())
            .map(|(i, _)| i as u8)
            .collect();
        for font in fonts {
            let id = self.font.at(font).glyph(ch).unwrap_or_default();
            let b = self.stretched(G { font, id }, target, vertical, st)?;
            // 1% slack: variants are measured in rounded font units.
            if extent(&b) >= target * 0.99 {
                return Ok(b);
            }
            if best.as_ref().is_none_or(|best| extent(&b) > extent(best)) {
                best = Some(b);
            }
        }
        match best {
            Some(b) => Ok(b),
            None => {
                let glyph = self.lookup(ch, 0)?;
                self.stretched(glyph, target, vertical, st)
            }
        }
    }

    /// A delimiter at least `target` tall, centered on the math axis.
    fn delimiter(&mut self, ch: Option<char>, target: f32, st: St) -> R<LBox> {
        let Some(ch) = ch else {
            return Ok(LBox {
                w: NULL_DELIMITER * self.font_size(st),
                ..LBox::default()
            });
        };
        let mut b = self.stretch_char(ch, target, true, st)?;
        let axis = self.font.c.axis_height * self.k(st);
        b.shift_y((b.h - b.d) / 2.0 - axis);
        b.glyph = None;
        Ok(b)
    }

    /// TeX's size for delimiters around content with height `h`, depth `d`.
    fn delimiter_target(&self, h: f32, d: f32, st: St) -> f32 {
        let axis = self.font.c.axis_height * self.k(st);
        let delta = (h - axis).max(d + axis);
        (2.0 * delta * DELIMITER_FACTOR).max(2.0 * delta - DELIMITER_SHORTFALL * self.font_size(st))
    }

    pub fn list(&mut self, nodes: &[Node], st: St) -> R<LBox> {
        self.list_lead(nodes, st, false)
    }

    /// Lays out a math list; `lead_ord` spaces the first atom as if an empty
    /// Ord preceded it (the right half of an `aligned` pair starting with `=`).
    fn list_lead(&mut self, nodes: &[Node], st: St, lead_ord: bool) -> R<LBox> {
        let (pieces, _) = self.pieces(nodes, st, false)?;
        Ok(self.assemble(pieces, lead_ord))
    }

    /// A list's atoms and kerns, not yet spaced. With `defer_middles`, each
    /// `\middle` is an empty placeholder whose index (and delimiter and
    /// style) is returned, to be sized once the rest of the body is known.
    #[allow(clippy::type_complexity)]
    fn pieces(
        &mut self,
        nodes: &[Node],
        st: St,
        defer_middles: bool,
    ) -> R<(Vec<Piece>, Vec<(usize, Option<char>, St)>)> {
        let mut st = st;
        let mut pieces = Vec::with_capacity(nodes.len());
        let mut middles = Vec::new();
        for node in nodes {
            match node {
                Node::Style(s) => st.style = *s,
                Node::Space(dim) => {
                    let em = self.font_size(st);
                    pieces.push(Piece::Kern(match dim {
                        Dim::Em(v) => v * em,
                        Dim::Mu(v) => v * em / 18.0,
                    }));
                }
                Node::Middle(delim) if defer_middles => {
                    middles.push((pieces.len(), *delim, st));
                    pieces.push(Piece::Atom(Atom::new(Class::Ord, LBox::default()), st));
                }
                _ => pieces.push(Piece::Atom(self.atom(node, st)?, st)),
            }
        }
        Ok((pieces, middles))
    }

    /// Spaces and concatenates a list's pieces (TeX rules 5, 6 and 20).
    fn assemble(&mut self, mut pieces: Vec<Piece>, lead_ord: bool) -> LBox {
        // Bin atoms with nothing to bind become Ord (TeX rules 5 and 6).
        let mut prev: Option<usize> = None;
        for i in 0..pieces.len() {
            let Piece::Atom(atom, _) = &pieces[i] else {
                continue;
            };
            let class = atom.class;
            let prev_class = prev.and_then(|p| match &pieces[p] {
                Piece::Atom(a, _) => Some(a.class),
                Piece::Kern(_) => None,
            });
            let prev_class = prev_class.or(lead_ord.then_some(Class::Ord));
            let unary = matches!(
                prev_class,
                None | Some(Class::Bin | Class::Op | Class::Rel | Class::Open | Class::Punct)
            );
            if class == Class::Bin
                && unary
                && let Piece::Atom(a, _) = &mut pieces[i]
            {
                a.class = Class::Ord;
            }
            if matches!(class, Class::Rel | Class::Close | Class::Punct)
                && prev_class == Some(Class::Bin)
                && let Some(Piece::Atom(a, _)) = prev.map(|p| &mut pieces[p])
            {
                a.class = Class::Ord;
            }
            prev = Some(i);
        }
        if let Some(Piece::Atom(a, _)) = prev.map(|p| &mut pieces[p])
            && a.class == Class::Bin
        {
            a.class = Class::Ord;
        }
        let single = pieces.len() == 1;
        let mut out = LBox::default();
        let mut x = 0.0f32;
        let mut prev_class = lead_ord.then_some(Class::Ord);
        for piece in pieces {
            match piece {
                Piece::Kern(k) => x += k,
                Piece::Atom(atom, st) => {
                    if let Some(p) = prev_class {
                        x += spacing(p, atom.class, st.small()) * self.font_size(st) / 18.0;
                    }
                    let b = atom.b;
                    out.h = out.h.max(b.h);
                    out.d = out.d.max(b.d);
                    // TeX: a character's italic correction is part of its width
                    // (scripts, laid out apart, place themselves against it).
                    let w = b.w + if atom.char { b.italic } else { 0.0 };
                    if single {
                        out.glyph = b.glyph;
                    }
                    out.put(b, x, 0.0);
                    x += w;
                    prev_class = Some(atom.class);
                }
            }
        }
        out.w = x;
        out
    }

    fn atom(&mut self, node: &Node, st: St) -> R<Atom> {
        self.work(1)?;
        Ok(match node {
            Node::Char {
                ch,
                class,
                variant,
                at,
            } => {
                let glyph = self.char_glyph(*ch, *variant, *at)?;
                let b = self.glyph_box(glyph, st)?;
                Atom {
                    char: true,
                    ..Atom::new(*class, b)
                }
            }
            Node::Group { body, class } => {
                let b = self.list(body, st)?;
                Atom::new(class.unwrap_or(Class::Ord), b)
            }
            Node::Scripts { base, sup, sub } => {
                let base_is_char = matches!(**base, Node::Char { .. });
                let base = self.atom(base, st)?;
                let class = base.class;
                let b = if base.limits {
                    self.limits(base.b, sup.as_deref(), sub.as_deref(), st)?
                } else {
                    self.scripts(base, base_is_char, sup.as_deref(), sub.as_deref(), st)?
                };
                Atom::new(class, b)
            }
            Node::LargeOp {
                ch,
                default_limits,
                limits,
                at,
            } => {
                let mut glyph = self.lookup(*ch, *at)?;
                if st.display() {
                    let face = self.face(glyph);
                    let min =
                        self.font.c.display_operator_min_height * self.k(st) / self.kg(st, glyph);
                    glyph.id = face
                        .variant_at_least(glyph.id, min, true)
                        .unwrap_or_else(|| face.largest_variant(glyph.id, true));
                }
                let mut b = self.glyph_box(glyph, st)?;
                let axis = self.font.c.axis_height * self.k(st);
                b.shift_y((b.h - b.d) / 2.0 - axis);
                Atom {
                    limits: limits.unwrap_or(*default_limits && st.display()),
                    op: true,
                    ..Atom::new(Class::Op, b)
                }
            }
            Node::NamedOp {
                name,
                default_limits,
                limits,
            } => {
                let b = self.text(name, Variant::Upright, 0, st)?;
                Atom {
                    limits: limits.unwrap_or(*default_limits && st.display()),
                    ..Atom::new(Class::Op, b)
                }
            }
            Node::Frac {
                num,
                den,
                bar,
                style,
                delims,
            } => Atom::new(
                Class::Inner,
                self.fraction(num, den, *bar, *style, *delims, st)?,
            ),
            Node::Sqrt { index, body } => {
                Atom::new(Class::Ord, self.radical(index.as_deref(), body, st)?)
            }
            Node::LeftRight { left, right, body } => {
                Atom::new(Class::Inner, self.left_right(*left, body, *right, st)?)
            }
            // A `\middle` outside `\left…\right` keeps its natural size.
            Node::Middle(delim) => Atom::new(Class::Ord, self.delimiter(*delim, 0.0, st)?),
            Node::Big { delim, size, class } => {
                let target = BIG_SIZES[(*size as usize).clamp(1, 4) - 1] * self.size;
                Atom::new(*class, self.delimiter(*delim, target, st)?)
            }
            Node::Accent { ch, wide, body } => {
                Atom::new(Class::Ord, self.accent(*ch, *wide, body, st)?)
            }
            Node::Line { over, body } => Atom::new(Class::Ord, self.line(*over, body, st)?),
            Node::Brace { over, body } => Atom {
                limits: true,
                ..Atom::new(Class::Op, self.brace(*over, body, st)?)
            },
            Node::Stack { base, over, under } => {
                let class = match base.as_slice() {
                    [Node::Char { class, .. }] => *class,
                    _ => Class::Ord,
                };
                let b = self.list(base, st)?;
                Atom::new(
                    class,
                    self.limits(b, over.as_deref(), under.as_deref(), st)?,
                )
            }
            Node::XArrow { ch, over, under } => {
                Atom::new(Class::Rel, self.xarrow(*ch, over, under.as_deref(), st)?)
            }
            Node::Text { text, variant, at } => {
                Atom::new(Class::Ord, self.text(text, *variant, *at, st)?)
            }
            Node::Phantom {
                body,
                width,
                height,
            } => {
                let b = self.list(body, st)?;
                Atom::new(
                    Class::Ord,
                    LBox {
                        w: if *width { b.w } else { 0.0 },
                        h: if *height { b.h } else { 0.0 },
                        d: if *height { b.d } else { 0.0 },
                        ..LBox::default()
                    },
                )
            }
            Node::Boxed(body) => Atom::new(Class::Ord, self.boxed(body, st)?),
            Node::Not(inner) => {
                let atom = self.atom(inner, st)?;
                let class = atom.class;
                Atom::new(class, self.negated(atom.b, st)?)
            }
            Node::Array(array) => {
                let b = self.array(array, st)?;
                if array.left.is_some() || array.right.is_some() {
                    let target = self.delimiter_target(b.h, b.d, st);
                    Atom::new(
                        Class::Inner,
                        self.fence(array.left, b, array.right, target, st)?,
                    )
                } else {
                    Atom::new(Class::Ord, b)
                }
            }
            // Handled by `list`.
            Node::Style(_) | Node::Space(_) => Atom::new(Class::Ord, LBox::default()),
        })
    }

    fn scripts(
        &mut self,
        base: Atom,
        base_is_char: bool,
        sup: Option<&[Node]>,
        sub: Option<&[Node]>,
        st: St,
    ) -> R<LBox> {
        let c = self.font.c;
        let k = self.k(st);
        let b = base.b;
        let sup = sup.map(|l| self.list(l, st.sup())).transpose()?;
        let sub = sub.map(|l| self.list(l, st.sub())).transpose()?;
        // TeX rule 18a: a character base uses the standard shifts; a compound
        // base pushes scripts out from its own top and bottom.
        let (sup_drop, sub_drop) = if base_is_char {
            (0.0, 0.0)
        } else {
            (
                b.h - c.superscript_baseline_drop_max * k,
                b.d + c.subscript_baseline_drop_min * k,
            )
        };
        let mut up = 0.0f32;
        let mut down = 0.0f32;
        if let Some(sp) = &sup {
            let standard = if st.cramped {
                c.superscript_shift_up_cramped
            } else {
                c.superscript_shift_up
            };
            up = (standard * k)
                .max(sup_drop)
                .max(sp.d + c.superscript_bottom_min * k);
        }
        if let Some(sb) = &sub {
            down = (c.subscript_shift_down * k).max(sub_drop);
            if sup.is_none() {
                down = down.max(sb.h - c.subscript_top_max * k);
            }
        }
        if let (Some(sp), Some(sb)) = (&sup, &sub) {
            let gap = (up - sp.d) - (sb.h - down);
            let min = c.sub_superscript_gap_min * k;
            if gap < min {
                let need = min - gap;
                let raise =
                    (c.superscript_bottom_max_with_subscript * k - (up - sp.d)).clamp(0.0, need);
                up += raise;
                down += need - raise;
            }
        }
        let (mut sup_x, mut sub_x) = if base.op {
            (b.w, b.w - b.italic)
        } else {
            (b.w + b.italic, b.w)
        };
        if let (Some(glyph), false) = (b.glyph, base.op) {
            let (face, kg) = (self.face(glyph), self.kg(st, glyph));
            if let Some(sp) = &sup {
                sup_x += face.kern(glyph.id, Corner::TopRight, (up - sp.d) / kg) * kg;
            }
            if let Some(sb) = &sub {
                sub_x += face.kern(glyph.id, Corner::BottomRight, (sb.h - down) / kg) * kg;
            }
        }
        let mut out = LBox {
            w: b.w,
            h: b.h,
            d: b.d,
            ..LBox::default()
        };
        out.put(b, 0.0, 0.0);
        if let Some(sp) = sup {
            out.w = out.w.max(sup_x + sp.w);
            out.h = out.h.max(up + sp.h);
            out.d = out.d.max(sp.d - up);
            out.put(sp, sup_x, -up);
        }
        if let Some(sb) = sub {
            out.w = out.w.max(sub_x + sb.w);
            out.d = out.d.max(down + sb.d);
            out.h = out.h.max(sb.h - down);
            out.put(sb, sub_x, down);
        }
        out.w += c.space_after_script * k;
        Ok(out)
    }

    /// Scripts set above and below (display `\sum`, `\lim`, `\overset`, braces).
    fn limits(
        &mut self,
        b: LBox,
        over: Option<&[Node]>,
        under: Option<&[Node]>,
        st: St,
    ) -> R<LBox> {
        let c = self.font.c;
        let k = self.k(st);
        let over = over.map(|l| self.list(l, st.sup())).transpose()?;
        let under = under.map(|l| self.list(l, st.sub())).transpose()?;
        let delta = b.italic;
        let w =
            b.w.max(over.as_ref().map_or(0.0, |o| o.w))
                .max(under.as_ref().map_or(0.0, |u| u.w));
        let mut out = LBox {
            w,
            h: b.h,
            d: b.d,
            ..LBox::default()
        };
        let (bw, bh, bd) = (b.w, b.h, b.d);
        out.put(b, (w - bw) / 2.0, 0.0);
        if let Some(o) = over {
            let rise = (c.upper_limit_gap_min * k + o.d).max(c.upper_limit_baseline_rise_min * k);
            let y = -(bh + rise);
            out.h = out.h.max(-y + o.h);
            let x = (w - o.w) / 2.0 + delta / 2.0;
            out.put(o, x, y);
        }
        if let Some(u) = under {
            let drop = (c.lower_limit_gap_min * k + u.h).max(c.lower_limit_baseline_drop_min * k);
            let y = bd + drop;
            out.d = out.d.max(y + u.d);
            let x = (w - u.w) / 2.0 - delta / 2.0;
            out.put(u, x, y);
        }
        Ok(out)
    }

    fn fraction(
        &mut self,
        num: &[Node],
        den: &[Node],
        bar: bool,
        style: Option<Style>,
        delims: Option<(Option<char>, Option<char>)>,
        st: St,
    ) -> R<LBox> {
        let st = match style {
            Some(s) => St {
                style: s,
                cramped: st.cramped,
            },
            None => st,
        };
        let c = self.font.c;
        let k = self.k(st);
        let num = self.list(num, st.num())?;
        let den = self.list(den, st.den())?;
        let display = st.display();
        let axis = c.axis_height * k;
        let (u, v, t) = if bar {
            let t = c.fraction_rule_thickness * k;
            let (shift_up, shift_down, gap_num, gap_den) = if display {
                (
                    c.fraction_numerator_display_style_shift_up,
                    c.fraction_denominator_display_style_shift_down,
                    c.fraction_num_display_style_gap_min,
                    c.fraction_denom_display_style_gap_min,
                )
            } else {
                (
                    c.fraction_numerator_shift_up,
                    c.fraction_denominator_shift_down,
                    c.fraction_numerator_gap_min,
                    c.fraction_denominator_gap_min,
                )
            };
            let u = (shift_up * k).max(axis + t / 2.0 + gap_num * k + num.d);
            let v = (shift_down * k).max(den.h + gap_den * k - axis + t / 2.0);
            (u, v, t)
        } else {
            let (shift_up, shift_down, gap) = if display {
                (
                    c.stack_top_display_style_shift_up,
                    c.stack_bottom_display_style_shift_down,
                    c.stack_display_style_gap_min,
                )
            } else {
                (
                    c.stack_top_shift_up,
                    c.stack_bottom_shift_down,
                    c.stack_gap_min,
                )
            };
            let (mut u, mut v) = (shift_up * k, shift_down * k);
            let actual = (u - num.d) - (den.h - v);
            if actual < gap * k {
                let e = (gap * k - actual) / 2.0;
                u += e;
                v += e;
            }
            (u, v, 0.0)
        };
        let pad = if delims.is_some() {
            0.0
        } else {
            NULL_DELIMITER * self.font_size(st)
        };
        let inner = num.w.max(den.w);
        let mut out = LBox {
            w: inner + 2.0 * pad,
            h: (u + num.h).max(axis + t / 2.0),
            d: (v + den.d).max(t / 2.0 - axis),
            ..LBox::default()
        };
        let (nw, dw) = (num.w, den.w);
        out.put(num, pad + (inner - nw) / 2.0, -u);
        out.put(den, pad + (inner - dw) / 2.0, v);
        if bar {
            let rule = self.rule(pad, -(axis + t / 2.0), inner, t)?;
            out.items.push(rule);
        }
        match delims {
            Some((left, right)) => {
                let target = self.delimiter_target(out.h, out.d, st);
                self.fence(left, out, right, target, st)
            }
            None => Ok(out),
        }
    }

    fn radical(&mut self, index: Option<&[Node]>, body: &[Node], st: St) -> R<LBox> {
        let c = self.font.c;
        let k = self.k(st);
        let body = self.list(body, st.cramp())?;
        let gap = if st.display() {
            c.radical_display_style_vertical_gap
        } else {
            c.radical_vertical_gap
        } * k;
        let t = c.radical_rule_thickness * k;
        let extra = c.radical_extra_ascender * k;
        let target = body.h + body.d + gap + t;
        let rad = self.stretch_char('√', target, true, st)?;
        let total = rad.h + rad.d;
        // A taller sign than needed centers the body in the extra room.
        let gap = gap + ((total - target) / 2.0).max(0.0);
        let top = -(body.h + gap + t);
        let rad_dy = top + rad.h;
        let mut x0 = 0.0;
        let mut out = LBox {
            h: body.h + gap + t + extra,
            d: body.d.max(rad.d + rad_dy),
            ..LBox::default()
        };
        if let Some(index) = index {
            let degree = self.list(index, St::new(Style::ScriptScript))?;
            let before = c.radical_kern_before_degree * k;
            let after = c.radical_kern_after_degree * k;
            let bottom = rad_dy + rad.d;
            let y = bottom - c.radical_degree_bottom_raise * total;
            out.h = out.h.max(-y + degree.h);
            let dw = degree.w;
            out.put(degree, before, y);
            x0 = (before + dw + after).max(0.0);
        }
        let (rw, bw) = (rad.w, body.w);
        out.put(rad, x0, rad_dy);
        let rule = self.rule(x0 + rw, top, bw, t)?;
        out.items.push(rule);
        out.put(body, x0 + rw, 0.0);
        out.w = x0 + rw + bw;
        Ok(out)
    }

    fn left_right(
        &mut self,
        left: Option<char>,
        body: &[Node],
        right: Option<char>,
        st: St,
    ) -> R<LBox> {
        // The body is laid out once: `\middle`s wait as placeholders until the
        // rest of it sets their size, then everything is spaced together.
        let (mut pieces, middles) = self.pieces(body, st, true)?;
        let (h, d) = pieces
            .iter()
            .fold((0.0f32, 0.0f32), |(h, d), piece| match piece {
                Piece::Atom(atom, _) => (h.max(atom.b.h), d.max(atom.b.d)),
                Piece::Kern(_) => (h, d),
            });
        let target = self.delimiter_target(h, d, st);
        for (ix, delim, st) in middles {
            pieces[ix] = Piece::Atom(
                Atom::new(Class::Ord, self.delimiter(delim, target, st)?),
                st,
            );
        }
        let inner = self.assemble(pieces, false);
        self.fence(left, inner, right, target, st)
    }

    /// `inner` between two delimiters sized to `target`.
    fn fence(
        &mut self,
        left: Option<char>,
        inner: LBox,
        right: Option<char>,
        target: f32,
        st: St,
    ) -> R<LBox> {
        let l = self.delimiter(left, target, st)?;
        let r = self.delimiter(right, target, st)?;
        let mut out = LBox {
            w: l.w + inner.w + r.w,
            h: l.h.max(inner.h).max(r.h),
            d: l.d.max(inner.d).max(r.d),
            ..LBox::default()
        };
        let (lw, iw) = (l.w, inner.w);
        out.put(l, 0.0, 0.0);
        out.put(inner, lw, 0.0);
        out.put(r, lw + iw, 0.0);
        Ok(out)
    }

    fn accent(&mut self, ch: char, wide: bool, body: &[Node], st: St) -> R<LBox> {
        let c = self.font.c;
        let k = self.k(st);
        // Accented i and j drop their dots.
        let dotless;
        let body = match body {
            [
                Node::Char {
                    ch: base @ ('i' | 'j'),
                    class,
                    variant: variant @ (Variant::Normal | Variant::Italic),
                    at,
                },
            ] => {
                dotless = [Node::Char {
                    ch: if *base == 'i' { 'ı' } else { 'ȷ' },
                    class: *class,
                    variant: *variant,
                    at: *at,
                }];
                &dotless[..]
            }
            _ => body,
        };
        let base = self.list(body, st.cramp())?;
        let acc = if wide {
            self.stretch_char(ch, base.w, false, st)?
        } else {
            let glyph = self.lookup(ch, 0)?;
            self.glyph_box(glyph, st)?
        };
        let combining = matches!(ch, '\u{300}'..='\u{36F}' | '\u{20D0}'..='\u{20FF}');
        let shift = if combining {
            // Combining marks are drawn to sit on x-height; taller bases lift them.
            (base.h - c.accent_base_height * k).max(0.0)
        } else {
            // Arrows: ink bottom a small gap above the base.
            base.h + c.stretch_stack_gap_below_min * k + acc.d
        };
        let base_attach = base
            .glyph
            .and_then(|g| Some(self.face(g).top_accent(g.id)? * self.kg(st, g)))
            .unwrap_or((base.w + base.italic) / 2.0);
        let acc_attach = match acc.glyph {
            Some(g) => {
                let (face, kg) = (self.face(g), self.kg(st, g));
                face.top_accent(g.id).map_or_else(
                    || {
                        let m = face.metrics(g.id);
                        (m.x_min + m.x_max) / 2.0 * kg
                    },
                    |a| a * kg,
                )
            }
            None => acc.w / 2.0,
        };
        let mut out = LBox {
            w: base.w,
            h: base.h.max(shift + acc.h),
            d: base.d,
            italic: base.italic,
            ..LBox::default()
        };
        out.put(base, 0.0, 0.0);
        out.put(acc, base_attach - acc_attach, -shift);
        Ok(out)
    }

    fn line(&mut self, over: bool, body: &[Node], st: St) -> R<LBox> {
        let c = self.font.c;
        let k = self.k(st);
        let body = self.list(body, if over { st.cramp() } else { st })?;
        let mut out = LBox {
            w: body.w,
            h: body.h,
            d: body.d,
            ..LBox::default()
        };
        let rule = if over {
            let (gap, t) = (c.overbar_vertical_gap * k, c.overbar_rule_thickness * k);
            out.h = body.h + gap + t + c.overbar_extra_ascender * k;
            self.rule(0.0, -(body.h + gap + t), body.w, t)?
        } else {
            let (gap, t) = (c.underbar_vertical_gap * k, c.underbar_rule_thickness * k);
            out.d = body.d + gap + t + c.underbar_extra_descender * k;
            self.rule(0.0, body.d + gap, body.w, t)?
        };
        out.put(body, 0.0, 0.0);
        out.items.push(rule);
        Ok(out)
    }

    fn brace(&mut self, over: bool, body: &[Node], st: St) -> R<LBox> {
        let c = self.font.c;
        let k = self.k(st);
        let base = self.list(body, st)?;
        let brace = self.stretch_char(if over { '⏞' } else { '⏟' }, base.w, false, st)?;
        let mut out = LBox {
            w: base.w.max(brace.w),
            h: base.h,
            d: base.d,
            ..LBox::default()
        };
        let x = (out.w - brace.w) / 2.0;
        let bx = (out.w - base.w) / 2.0;
        let y = if over {
            let y = -(base.h + c.stretch_stack_gap_below_min * k) - brace.d;
            out.h = -y + brace.h;
            y
        } else {
            let y = base.d + c.stretch_stack_gap_above_min * k + brace.h;
            out.d = y + brace.d;
            y
        };
        out.put(base, bx, 0.0);
        out.put(brace, x, y);
        Ok(out)
    }

    fn xarrow(&mut self, ch: char, over: &[Node], under: Option<&[Node]>, st: St) -> R<LBox> {
        let c = self.font.c;
        let k = self.k(st);
        let em = self.font_size(st);
        let over = self.list(over, st.sup())?;
        let under = under.map(|u| self.list(u, st.sub())).transpose()?;
        let label = over.w.max(under.as_ref().map_or(0.0, |u| u.w));
        let arrow = self.stretch_char(ch, (label + 0.6 * em).max(1.5 * em), false, st)?;
        let w = arrow.w.max(label);
        let mut out = LBox {
            w,
            h: arrow.h,
            d: arrow.d,
            ..LBox::default()
        };
        let y = -(arrow.h + c.stretch_stack_gap_above_min * k + over.d);
        out.h = out.h.max(-y + over.h);
        let ow = over.w;
        out.put(over, (w - ow) / 2.0, y);
        if let Some(u) = under {
            let y = arrow.d + c.stretch_stack_gap_below_min * k + u.h;
            out.d = out.d.max(y + u.d);
            let uw = u.w;
            out.put(u, (w - uw) / 2.0, y);
        }
        let aw = arrow.w;
        out.put(arrow, (w - aw) / 2.0, 0.0);
        Ok(out)
    }

    fn text(&mut self, text: &str, variant: Variant, at: u32, st: St) -> R<LBox> {
        let k = self.k(st);
        let size = self.font_size(st);
        let mut out = LBox::default();
        let mut x = 0.0;
        for ch in text.chars() {
            match ch {
                ' ' | '\u{a0}' => x += self.font.space() * k,
                '\u{2003}' => x += size,
                _ => {
                    let glyph = self.char_glyph(ch, variant, at)?;
                    let b = self.glyph_box(glyph, st)?;
                    out.h = out.h.max(b.h);
                    out.d = out.d.max(b.d);
                    let w = b.w;
                    out.put(b, x, 0.0);
                    x += w;
                }
            }
        }
        out.w = x;
        Ok(out)
    }

    fn boxed(&mut self, body: &[Node], st: St) -> R<LBox> {
        let t = self.font.c.fraction_rule_thickness * self.k(st);
        let pad = 0.3 * self.font_size(st);
        let inner = self.list(body, st)?;
        let (w, h, d) = (
            inner.w + 2.0 * (pad + t),
            inner.h + pad + t,
            inner.d + pad + t,
        );
        let mut out = LBox {
            w,
            h,
            d,
            ..LBox::default()
        };
        out.put(inner, pad + t, 0.0);
        for rule in [
            (0.0, -h, w, t),
            (0.0, d - t, w, t),
            (0.0, -h, t, h + d),
            (w - t, -h, t, h + d),
        ] {
            let rule = self.rule(rule.0, rule.1, rule.2, rule.3)?;
            out.items.push(rule);
        }
        Ok(out)
    }

    /// `\not`: a slash overlaid on the atom's center.
    fn negated(&mut self, mut b: LBox, st: St) -> R<LBox> {
        let glyph = self
            .find('\u{338}')
            .map_or_else(|| self.lookup('/', 0), Ok)?;
        let slash = self.glyph_box(glyph, st)?;
        let m = self.face(glyph).metrics(glyph.id);
        let ink_center = (m.x_min + m.x_max) / 2.0 * self.kg(st, glyph);
        b.h = b.h.max(slash.h);
        b.d = b.d.max(slash.d);
        let x = b.w / 2.0 - ink_center;
        b.put(slash, x, 0.0);
        b.glyph = None;
        Ok(b)
    }

    fn array(&mut self, a: &Array, st: St) -> R<LBox> {
        let em = self.font_size(st);
        let (cell_st, stretch, jot) = match a.kind {
            ArrayKind::Matrix | ArrayKind::Array => (St::new(Style::Text), 1.0, 0.0),
            ArrayKind::Small => (St::new(Style::Script), 0.6, 0.0),
            ArrayKind::Cases { display } => (
                St::new(if display { Style::Display } else { Style::Text }),
                1.2,
                0.0,
            ),
            ArrayKind::Aligned | ArrayKind::Gathered => (St::new(Style::Display), 1.0, 0.3),
        };
        let ncols = a.rows.iter().map(Vec::len).max().unwrap_or(0);
        let align = |j: usize| match a.kind {
            ArrayKind::Matrix | ArrayKind::Small | ArrayKind::Gathered => Align::Center,
            ArrayKind::Cases { .. } => Align::Left,
            ArrayKind::Aligned => {
                if j.is_multiple_of(2) {
                    Align::Right
                } else {
                    Align::Left
                }
            }
            ArrayKind::Array => a.cols.get(j).copied().unwrap_or(Align::Center),
        };
        let gap_after = |j: usize| match a.kind {
            ArrayKind::Small => 0.333 * em,
            ArrayKind::Aligned if j.is_multiple_of(2) => 0.0,
            ArrayKind::Aligned => 2.0 * em,
            _ => em,
        };
        let fenced = a.left.is_some() || a.right.is_some();
        let outer = match a.kind {
            ArrayKind::Array => 0.5 * em,
            // TeX sets matrix entries flush against the brackets, leaning on
            // Computer Modern's roomy side bearings; slimmer fonts' brackets
            // crowd them without a little air.
            ArrayKind::Matrix | ArrayKind::Small if fenced => 0.1 * em,
            _ => 0.0,
        };
        // Every row is at least a strut tall (`\arraystretch` × 1.2em baselines).
        let skip = 1.2 * stretch * self.font_size(cell_st);
        let (strut_h, strut_d) = (0.7 * skip, 0.3 * skip);
        let mut cells: Vec<Vec<LBox>> = Vec::with_capacity(a.rows.len());
        let mut widths = vec![0.0f32; ncols];
        let mut heights = Vec::with_capacity(a.rows.len());
        for row in &a.rows {
            let mut boxes = Vec::with_capacity(row.len());
            let (mut h, mut d) = (strut_h, strut_d);
            for (j, cell) in row.iter().enumerate() {
                // Empty cells (`&&&…`) emit nothing but still cost a layout.
                self.work(1)?;
                let lead = a.kind == ArrayKind::Aligned && j % 2 == 1;
                let b = self.list_lead(cell, cell_st, lead)?;
                widths[j] = widths[j].max(b.w);
                h = h.max(b.h);
                d = d.max(b.d);
                boxes.push(b);
            }
            heights.push((h, d));
            cells.push(boxes);
        }
        let mut xs = Vec::with_capacity(ncols);
        let mut x = outer;
        for (j, w) in widths.iter().enumerate() {
            xs.push(x);
            x += w;
            if j + 1 < ncols {
                x += gap_after(j);
            }
        }
        let total_w = x + outer;
        // TeX's `\lineskip`: rows whose content overflows the struts that
        // space them (scripts above and below) keep 1pt apart instead of touching.
        let overflows = |i: usize| {
            let (_, d) = heights[i];
            let (h, _) = heights.get(i + 1).copied().unwrap_or((strut_h, strut_d));
            d > strut_d || h > strut_h
        };
        let row_gap = |i: usize| {
            a.gaps.get(i).copied().unwrap_or(0.0) * em
                + jot * em
                + if overflows(i) { 0.1 * em } else { 0.0 }
        };
        let total_h: f32 = heights.iter().map(|(h, d)| h + d).sum::<f32>()
            + (0..heights.len().saturating_sub(1))
                .map(row_gap)
                .sum::<f32>();
        let axis = self.font.c.axis_height * self.k(st);
        let top = -axis - total_h / 2.0;
        let mut out = LBox {
            w: total_w,
            h: axis + total_h / 2.0,
            d: total_h / 2.0 - axis,
            ..LBox::default()
        };
        let mut y = top;
        for (i, row) in cells.into_iter().enumerate() {
            let (h, d) = heights[i];
            let baseline = y + h;
            for (j, b) in row.into_iter().enumerate() {
                let slack = widths[j] - b.w;
                let dx = match align(j) {
                    Align::Left => 0.0,
                    Align::Center => slack / 2.0,
                    Align::Right => slack,
                };
                out.put(b, xs[j] + dx, baseline);
            }
            y = baseline + d + row_gap(i);
        }
        Ok(out)
    }
}

/// TeX's inter-atom spacing in mu (Appendix G table); entries marked as
/// non-script-only vanish in script styles.
fn spacing(l: Class, r: Class, small: bool) -> f32 {
    use Class::*;
    // 1 thin, 2 medium, 3 thick; negative: only in display/text style.
    let code: i8 = match (l, r) {
        (Ord, Op) | (Op, Ord) | (Op, Op) | (Close, Op) | (Inner, Op) => 1,
        (Ord, Bin) | (Op, Bin) | (Close, Bin) | (Inner, Bin) => -2,
        (Bin, Ord | Op | Open | Inner) => -2,
        (Ord | Op | Close | Inner, Rel) => -3,
        (Rel, Ord | Op | Open | Inner) => -3,
        (Ord | Op | Close, Inner) => -1,
        (Punct, Ord | Op | Rel | Open | Close | Punct | Inner) => -1,
        (Inner, Ord | Open | Punct | Inner) => -1,
        _ => 0,
    };
    if code < 0 && small {
        return 0.0;
    }
    [0.0, 3.0, 4.0, 5.0][code.unsigned_abs() as usize]
}

/// Look-alikes for symbols some math fonts lack, tried after the whole chain.
fn equivalents(ch: char) -> &'static [char] {
    match ch {
        '∖' => &['⧵', '\\'],
        '⟂' => &['⊥'],
        '∣' => &['|'],
        '∥' => &['‖'],
        '⋅' => &['·'],
        '∗' => &['*'],
        '′' => &['\''],
        '⟶' => &['→'],
        '⟵' => &['←'],
        '⟷' => &['↔'],
        '⟹' => &['⇒'],
        '⟸' => &['⇐'],
        '⟺' => &['⇔'],
        '⟼' => &['↦'],
        _ => &[],
    }
}
