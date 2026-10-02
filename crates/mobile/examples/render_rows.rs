//! Reference rasterizer for the mobile layout.
//!
//! Runs the real pipeline (`TranscriptView`: rows → measured display lists)
//! on a scenario and paints the display lists to a PNG with the bundled Geist
//! faces and the app palette, so a layout can be *looked at* on a machine with
//! no simulator. It is not the iOS renderer: glyphs are outlines scaled to the
//! Rust-measured run widths (no kerning / ligature shaping), SF Symbols are
//! small stand-in drawings, spinners are static, scrollers are clipped but
//! not edge-faded. Positions, sizes, wrapping and truncation are the core's.
//!
//! ```text
//! cargo run -p zeron-mobile --example render_rows -- \
//!     --scenario workflows --width 390 --out /tmp/x.png \
//!     [--dark] [--act wf.toggle:run-live,goal.toggle] [--rows 5..12] [--hits]
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tiny_skia::{
    Color, FillRule, LineCap, Mask, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};
use zeron_client::demo_workflows::{self as demo, PhaseSpec, RunSpec};
use zeron_doc::parts::MessagePart;
use zeron_doc::schema::{MessageRole, SessionMessageEntry};
use zeron_mobile::layout::display::{BoxStyle, ColorRole, FadeEdge, RowDisplay, WidgetKind};
use zeron_mobile::layout::{
    FaceData, FaceRole, LayoutFrame, LayoutListener, PlatformMeasurer, TextSystem, TranscriptInput, TranscriptView,
};
use zeron_proto::{TodoItem, TodoStatus, WorkflowStatus};

const SCALE: f32 = 3.0;
const NOW: i64 = 1_800_000_000_000;

struct Ready(Mutex<u64>, Condvar);
impl LayoutListener for Ready {
    fn frame_ready(&self, revision: u64) {
        *self.0.lock().unwrap() = revision;
        self.1.notify_all();
    }
}

/// Glyphs the faces lack are measured as a fixed advance (as the tests do).
struct Fixed;
impl PlatformMeasurer for Fixed {
    fn measure(&self, _: FaceRole, size: f32, _: bool, text: String) -> f32 {
        text.chars().count() as f32 * size * 0.9
    }
    fn measure_run(&self, _: FaceRole, size: f32, _: bool, text: String) -> Vec<f32> {
        text.chars().map(|_| size * 0.9).collect()
    }
}

fn font_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/assets/fonts")
}

const FACES: [(FaceRole, &str); 12] = [
    (FaceRole::Sans, "Geist"),
    (FaceRole::SansMedium, "Geist-Medium"),
    (FaceRole::SansSemibold, "Geist-SemiBold"),
    (FaceRole::SansBold, "Geist-Bold"),
    (FaceRole::SansItalic, "Geist-Italic"),
    (FaceRole::SansMediumItalic, "Geist-MediumItalic"),
    (FaceRole::SansSemiboldItalic, "Geist-SemiBoldItalic"),
    (FaceRole::SansBoldItalic, "Geist-BoldItalic"),
    (FaceRole::Mono, "GeistMono"),
    (FaceRole::MonoMedium, "GeistMono-Medium"),
    (FaceRole::MonoSemibold, "GeistMono-SemiBold"),
    (FaceRole::MonoItalic, "GeistMono-Italic"),
];

// MARK: - palette (apps/ios/Zeron/Design/Palette.swift)

fn rgba(hex: u32, a: f32) -> Color {
    Color::from_rgba8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, (a * 255.0) as u8)
}

fn dynamic(light: u32, dark: u32, a: f32, is_dark: bool) -> Color {
    rgba(if is_dark { dark } else { light }, a)
}

fn white_black(light_a: f32, dark_a: f32, is_dark: bool) -> Color {
    if is_dark { Color::from_rgba8(255, 255, 255, (dark_a * 255.0) as u8) } else { Color::from_rgba8(0, 0, 0, (light_a * 255.0) as u8) }
}

fn palette(role: ColorRole, d: bool) -> Color {
    use ColorRole::*;
    let dy = |l, k| dynamic(l, k, 1.0, d);
    match role {
        Text => dy(0x27272C, 0xE8E8EA),
        TextSecondary => dy(0x62626A, 0xA9A9AE),
        TextTertiary => dy(0x97979F, 0x6B6B72),
        Link | Accent => dy(0x5B43E8, 0x8B7CF6),
        Danger => dy(0xDC2626, 0xF87171),
        Success => dy(0x15803D, 0x34D399),
        Warning => dy(0xA16207, 0xFACC15),
        InlineCodeText => dy(0x3F3F46, 0xDCDCE0),
        InlineCodeBackground => dy(0xE9E9ED, 0x1A1A1E),
        CodeText => dy(0x303035, 0xE8E8EA),
        CodeBackground => dy(0xFAFAFB, 0x0B0B0D),
        CodeBorder => dy(0xE4E4E8, 0x1F1F23),
        QuoteBar => dynamic(0x5B43E8, 0x8B7CF6, 0.45, d),
        Rule => dy(0xE2E2E6, 0x1E1E22),
        TableBorder => dy(0xE2E2E6, 0x232327),
        TableHeaderBackground => dy(0xF3F3F5, 0x121215),
        UserBubble => dy(0xFFFFFF, 0x19191C),
        ChipBackground => dy(0xE7E7EB, 0x1C1C20),
        SyntaxKeyword => dy(0x5B43E8, 0x8B7CF6),
        SyntaxString => dy(0x15803D, 0x34D399),
        SyntaxComment => dy(0x6B7280, 0x92929A),
        SyntaxNumber | SyntaxConstant => dy(0xA16207, 0xFACC15),
        SyntaxFunction => dy(0x2563EB, 0x60A5FA),
        SyntaxType => dy(0x7E22CE, 0xC084FC),
        SyntaxVariable => dy(0x303035, 0xE8E8EA),
        SyntaxProperty | SyntaxEscape => dy(0x0E7490, 0x22D3EE),
        SyntaxOperator | SyntaxPunctuation => dy(0x52525B, 0xA1A1AA),
        SyntaxTag => dy(0xBE185D, 0xF472B6),
        SyntaxAttribute => dy(0xB91C1C, 0xF87171),
        TextFaint => dy(0x797981, 0x85858A),
        TextSoft => dynamic(0x303035, 0xE8E8EA, 0.85, d),
        ToolRail => white_black(0.162, 0.12, d),
        ToolBadge => white_black(0.06, 0.06, d),
        ToolWell => if d { Color::from_rgba8(0, 0, 0, 41) } else { Color::from_rgba8(255, 255, 255, 41) },
        AgentCard => white_black(0.03, 0.03, d),
        AgentCardBorder => white_black(0.0945, 0.07, d),
        AgentTile => white_black(0.08, 0.08, d),
        DiffAddWash => dynamic(0x15803D, 0x34D399, 0.055, d),
        DiffDelWash => dynamic(0xDC2626, 0xF87171, 0.055, d),
        DiffAddBar => dynamic(0x15803D, 0x34D399, 0.55, d),
        DiffDelBar => dynamic(0xDC2626, 0xF87171, 0.55, d),
        DiffHunk => dynamic(0x5B43E8, 0x8B7CF6, 0.07, d),
    }
}

fn paint(color: Color) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(color);
    p.anti_alias = true;
    p
}

// MARK: - drawing

struct Fonts {
    faces: HashMap<u16, (Vec<u8>, f32)>,
}

struct Canvas<'a> {
    pm: &'a mut Pixmap,
    dark: bool,
    fonts: &'a Fonts,
    /// Offset of the current row's origin, in points.
    ox: f32,
    oy: f32,
    hits: bool,
}

impl Canvas<'_> {
    fn t(&self) -> Transform {
        Transform::from_scale(SCALE, SCALE).post_translate(0.0, 0.0).pre_translate(self.ox, self.oy)
    }

    fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
        let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
        let mut b = PathBuilder::new();
        b.move_to(x + r, y);
        b.line_to(x + w - r, y);
        b.quad_to(x + w, y, x + w, y + r);
        b.line_to(x + w, y + h - r);
        b.quad_to(x + w, y + h, x + w - r, y + h);
        b.line_to(x + r, y + h);
        b.quad_to(x, y + h, x, y + h - r);
        b.line_to(x, y + r);
        b.quad_to(x, y, x + r, y);
        b.close();
        b.finish()
    }

    fn fill_rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color, clip: Option<&Mask>) {
        if let Some(path) = Self::rrect(x, y, w, h, r) {
            self.pm.fill_path(&path, &paint(color), FillRule::Winding, self.t(), clip);
        }
    }

    fn stroke_rrect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, color: Color, width: f32, clip: Option<&Mask>) {
        if let Some(path) = Self::rrect(x + width / 2.0, y + width / 2.0, w - width, h - width, (r - width / 2.0).max(0.0)) {
            let stroke = Stroke { width, ..Default::default() };
            self.pm.stroke_path(&path, &paint(color), &stroke, self.t(), clip);
        }
    }

    fn line(&mut self, pts: &[(f32, f32)], color: Color, width: f32, clip: Option<&Mask>) {
        let mut b = PathBuilder::new();
        b.move_to(pts[0].0, pts[0].1);
        for p in &pts[1..] {
            b.line_to(p.0, p.1);
        }
        if let Some(path) = b.finish() {
            let stroke = Stroke { width, line_cap: LineCap::Round, ..Default::default() };
            self.pm.stroke_path(&path, &paint(color), &stroke, self.t(), clip);
        }
    }

    fn circle(&mut self, cx: f32, cy: f32, r: f32, color: Color, stroke: Option<f32>, clip: Option<&Mask>) {
        let mut b = PathBuilder::new();
        b.push_circle(cx, cy, r);
        if let Some(path) = b.finish() {
            match stroke {
                Some(w) => self.pm.stroke_path(&path, &paint(color), &Stroke { width: w, ..Default::default() }, self.t(), clip),
                None => self.pm.fill_path(&path, &paint(color), FillRule::Winding, self.t(), clip),
            }
        }
    }

    fn run(&mut self, display: &RowDisplay, run: &zeron_mobile::layout::display::TextRun, styles: &HashMap<u16, StyleInfo>, ox: f32, oy: f32, clip: Option<&Mask>, color_override: Option<Color>) {
        let Some(style) = styles.get(&run.style) else { return };
        let Some((data, _)) = self.fonts.faces.get(&(style.face as u16)) else { return };
        let Ok(face) = ttf_parser::Face::parse(data, 0) else { return };
        let units = display.text.encode_utf16().collect::<Vec<_>>();
        let s = String::from_utf16_lossy(&units[run.start as usize..(run.start + run.len) as usize]);
        let upem = face.units_per_em() as f32;
        let k = style.size / upem;
        // Natural advance, then spread to the Rust-measured width.
        let adv: Vec<f32> = s.chars().map(|c| face.glyph_index(c).and_then(|g| face.glyph_hor_advance(g)).map_or(style.size * 0.6, |a| a as f32 * k)).collect();
        let natural: f32 = adv.iter().sum();
        let fit = if natural > 0.0 && run.width > 0.0 { (run.width / natural).clamp(0.85, 1.15) } else { 1.0 };
        let color = color_override.unwrap_or_else(|| palette(run.color, self.dark));
        let mut x = run.x + ox + self.ox;
        for (c, a) in s.chars().zip(&adv) {
            if let Some(g) = face.glyph_index(c) {
                let mut sink = Outline(PathBuilder::new());
                if face.outline_glyph(g, &mut sink).is_some()
                    && let Some(path) = sink.0.finish()
                {
                    let tr = Transform::from_scale(SCALE, SCALE).pre_translate(x, run.baseline + oy + self.oy).pre_scale(k, -k);
                    self.pm.fill_path(&path, &paint(color), FillRule::Winding, tr, clip);
                }
            }
            x += a * fit;
        }
        if run.decoration != zeron_mobile::layout::display::Decoration::None {
            let y = match run.decoration {
                zeron_mobile::layout::display::Decoration::Underline => run.baseline + 2.0,
                _ => run.baseline - style.size * 0.3,
            } + oy + self.oy;
            let t = Transform::from_scale(SCALE, SCALE);
            if let Some(r) = Rect::from_xywh(run.x + ox + self.ox, y, run.width, 0.8) {
                self.pm.fill_rect(r, &paint(color), t, clip);
            }
        }
    }

    /// Stand-ins for SF Symbols and the app's tool assets: recognisable, not exact.
    fn icon(&mut self, name: &str, color: Color, x: f32, y: f32, w: f32, h: f32, clip: Option<&Mask>) {
        let s = w.min(h);
        let (cx, cy) = (x + w / 2.0, y + h / 2.0);
        let r = s / 2.0;
        let lw = (s * 0.11).max(1.0);
        match name {
            "chevron.down" => self.line(&[(cx - r * 0.5, cy - r * 0.25), (cx, cy + r * 0.3), (cx + r * 0.5, cy - r * 0.25)], color, lw, clip),
            "chevron.up" => self.line(&[(cx - r * 0.5, cy + r * 0.25), (cx, cy - r * 0.3), (cx + r * 0.5, cy + r * 0.25)], color, lw, clip),
            "chevron.right" => self.line(&[(cx - r * 0.25, cy - r * 0.5), (cx + r * 0.3, cy), (cx - r * 0.25, cy + r * 0.5)], color, lw, clip),
            "checkmark" => self.line(&[(cx - r * 0.55, cy), (cx - r * 0.1, cy + r * 0.45), (cx + r * 0.6, cy - r * 0.45)], color, lw * 1.2, clip),
            "xmark" => {
                self.line(&[(cx - r * 0.45, cy - r * 0.45), (cx + r * 0.45, cy + r * 0.45)], color, lw, clip);
                self.line(&[(cx + r * 0.45, cy - r * 0.45), (cx - r * 0.45, cy + r * 0.45)], color, lw, clip);
            }
            "minus" => self.line(&[(cx - r * 0.5, cy), (cx + r * 0.5, cy)], color, lw, clip),
            "pause.fill" => {
                self.fill_rrect(cx - r * 0.45, cy - r * 0.5, r * 0.3, r, 1.0, color, clip);
                self.fill_rrect(cx + r * 0.15, cy - r * 0.5, r * 0.3, r, 1.0, color, clip);
            }
            "play.fill" => {
                let mut b = PathBuilder::new();
                b.move_to(cx - r * 0.35, cy - r * 0.55);
                b.line_to(cx + r * 0.55, cy);
                b.line_to(cx - r * 0.35, cy + r * 0.55);
                b.close();
                if let Some(p) = b.finish() {
                    self.pm.fill_path(&p, &paint(color), FillRule::Winding, self.t(), clip);
                }
            }
            "target" => {
                self.circle(cx, cy, r * 0.85, color, Some(lw), clip);
                self.circle(cx, cy, r * 0.5, color, Some(lw), clip);
                self.circle(cx, cy, r * 0.15, color, None, clip);
            }
            "arrow.triangle.branch" => {
                self.line(&[(cx - r * 0.5, cy - r * 0.6), (cx - r * 0.5, cy + r * 0.6)], color, lw, clip);
                self.line(&[(cx - r * 0.5, cy), (cx + r * 0.5, cy - r * 0.25), (cx + r * 0.5, cy - r * 0.6)], color, lw, clip);
                self.circle(cx - r * 0.5, cy - r * 0.6, r * 0.2, color, None, clip);
                self.circle(cx - r * 0.5, cy + r * 0.6, r * 0.2, color, None, clip);
                self.circle(cx + r * 0.5, cy - r * 0.6, r * 0.2, color, None, clip);
            }
            "tool-checklist" => {
                for i in 0..3 {
                    let yy = cy - r * 0.55 + i as f32 * r * 0.55;
                    self.line(&[(cx - r * 0.7, yy), (cx - r * 0.45, yy)], color, lw, clip);
                    self.line(&[(cx - r * 0.1, yy), (cx + r * 0.7, yy)], color, lw, clip);
                }
            }
            "doc.text" | "doc" => {
                self.stroke_rrect(cx - r * 0.6, cy - r * 0.8, r * 1.2, r * 1.6, 2.0, color, lw, clip);
                if name == "doc.text" {
                    for i in 0..3 {
                        let yy = cy - r * 0.3 + i as f32 * r * 0.35;
                        self.line(&[(cx - r * 0.3, yy), (cx + r * 0.3, yy)], color, lw * 0.8, clip);
                    }
                }
            }
            "tablecells" => {
                self.stroke_rrect(cx - r * 0.8, cy - r * 0.7, r * 1.6, r * 1.4, 2.0, color, lw, clip);
                self.line(&[(cx - r * 0.8, cy), (cx + r * 0.8, cy)], color, lw * 0.8, clip);
                self.line(&[(cx, cy - r * 0.7), (cx, cy + r * 0.7)], color, lw * 0.8, clip);
            }
            "chart.bar" => {
                for (i, hh) in [0.5, 0.9, 0.7].iter().enumerate() {
                    self.fill_rrect(cx - r * 0.7 + i as f32 * r * 0.5, cy + r * 0.6 - r * 1.2 * hh, r * 0.34, r * 1.2 * hh, 1.0, color, clip);
                }
            }
            "questionmark.bubble" | "exclamationmark.triangle" | "checkmark.circle" | "pause.circle" | "arrow.right.circle" => {
                self.circle(cx, cy, r * 0.85, color, Some(lw), clip);
                match name {
                    "checkmark.circle" => self.line(&[(cx - r * 0.35, cy), (cx - r * 0.05, cy + r * 0.3), (cx + r * 0.4, cy - r * 0.3)], color, lw, clip),
                    "pause.circle" => {
                        self.line(&[(cx - r * 0.2, cy - r * 0.3), (cx - r * 0.2, cy + r * 0.3)], color, lw, clip);
                        self.line(&[(cx + r * 0.2, cy - r * 0.3), (cx + r * 0.2, cy + r * 0.3)], color, lw, clip);
                    }
                    "arrow.right.circle" => self.line(&[(cx - r * 0.35, cy), (cx + r * 0.35, cy), (cx + r * 0.1, cy - r * 0.25)], color, lw, clip),
                    "exclamationmark.triangle" => {
                        self.line(&[(cx, cy - r * 0.4), (cx, cy + r * 0.1)], color, lw, clip);
                        self.circle(cx, cy + r * 0.4, lw * 0.6, color, None, clip);
                    }
                    _ => {
                        self.line(&[(cx, cy - r * 0.15), (cx, cy + r * 0.1)], color, lw, clip);
                        self.circle(cx, cy + r * 0.4, lw * 0.6, color, None, clip);
                    }
                }
            }
            "arrow.clockwise" => self.circle(cx, cy, r * 0.6, color, Some(lw), clip),
            _ => self.circle(cx, cy, r * 0.5, color, Some(lw), clip),
        }
    }
}

struct Outline(PathBuilder);
impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.0.close();
    }
}

struct StyleInfo {
    face: FaceRole,
    size: f32,
}

fn clip_mask(w: u32, h: u32, x: f32, y: f32, cw: f32, ch: f32) -> Option<Mask> {
    let mut m = Mask::new(w, h)?;
    let path = PathBuilder::from_rect(Rect::from_xywh(x * SCALE, y * SCALE, cw * SCALE, ch * SCALE)?);
    m.fill_path(&path, FillRule::Winding, true, Transform::identity());
    Some(m)
}

fn draw_row(cv: &mut Canvas, d: &RowDisplay, styles: &HashMap<u16, StyleInfo>, width_px: u32, height_px: u32) {
    let dark = cv.dark;
    // Boxes under text, per layer (scrollers clip to their rect).
    let scroller_clip = |i: Option<u32>, cv: &Canvas| -> Option<(Mask, f32, f32)> {
        let s = d.scrollers.get(i? as usize)?;
        let m = clip_mask(width_px, height_px, cv.ox + s.x, cv.oy + s.y, s.w, s.h)?;
        Some((m, cv.ox + s.x, cv.oy + s.y))
    };
    for b in &d.boxes {
        let sc = scroller_clip(b.scroller, cv);
        let (ox, oy, clip) = match &sc {
            Some((m, x, y)) => (*x - cv.ox, *y - cv.oy, Some(m)),
            None => (0.0, 0.0, None),
        };
        let color = palette(b.color, dark);
        match b.style {
            BoxStyle::Fill => cv.fill_rrect(b.x + ox, b.y + oy, b.w, b.h, b.radius, color, clip),
            BoxStyle::Hairline => cv.stroke_rrect(b.x + ox, b.y + oy, b.w, b.h, b.radius, color, 1.0 / SCALE, clip),
        }
    }
    // Text: runs inside a trailing fade are drawn to a layer and ramped out.
    let faded: Vec<(usize, usize)> = d
        .runs
        .iter()
        .enumerate()
        .filter_map(|(ri, r)| d.fades.iter().position(|f| f.scroller == r.scroller && r.baseline > f.y && r.baseline <= f.y + f.h + 0.5).map(|fi| (ri, fi)))
        .collect();
    for (ri, r) in d.runs.iter().enumerate() {
        if faded.iter().any(|(i, _)| *i == ri) {
            continue;
        }
        let sc = scroller_clip(r.scroller, cv);
        let (ox, oy, clip) = match &sc {
            Some((m, x, y)) => (*x - cv.ox, *y - cv.oy, Some(m)),
            None => (0.0, 0.0, None),
        };
        cv.run(d, r, styles, ox, oy, clip, None);
    }
    for (fi, f) in d.fades.iter().enumerate() {
        let mut layer = Pixmap::new(width_px, height_px).unwrap();
        {
            let mut sub = Canvas { pm: &mut layer, dark, fonts: cv.fonts, ox: cv.ox, oy: cv.oy, hits: false };
            for (ri, _) in faded.iter().filter(|(_, i)| *i == fi) {
                sub.run(d, &d.runs[*ri], styles, 0.0, 0.0, None, None);
            }
        }
        // Alpha ramp across the fade rect; hard clip past a trailing fade's end.
        let (fx, fy, fw, fh) = ((cv.ox + f.x) * SCALE, (cv.oy + f.y) * SCALE, f.w * SCALE, f.h * SCALE);
        let data = layer.data_mut();
        for py in 0..height_px {
            for px in 0..width_px {
                let a = match f.edge {
                    FadeEdge::Trailing => {
                        let xx = px as f32;
                        if xx >= fx + fw { 0.0 } else if xx >= fx { 1.0 - (xx - fx) / fw } else { 1.0 }
                    }
                    FadeEdge::Bottom => {
                        let yy = py as f32;
                        if yy >= fy { (1.0 - (yy - fy) / fh * 0.92).clamp(0.0, 1.0) } else { 1.0 }
                    }
                };
                if a < 1.0 {
                    let i = ((py * width_px + px) * 4) as usize;
                    for c in 0..4 {
                        data[i + c] = (data[i + c] as f32 * a) as u8;
                    }
                }
            }
        }
        cv.pm.draw_pixmap(0, 0, layer.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    // Widgets.
    for w in &d.widgets {
        let sc = scroller_clip(w.scroller, cv);
        let (ox, oy, clip) = match &sc {
            Some((m, x, y)) => (*x - cv.ox, *y - cv.oy, Some(m)),
            None => (0.0, 0.0, None),
        };
        let (x, y) = (w.x + ox, w.y + oy);
        match &w.kind {
            WidgetKind::Icon { name, color } => cv.icon(name, palette(*color, dark), x, y, w.w, w.h, clip),
            WidgetKind::Spinner => {
                // The 3x3 glyph spinner, frozen mid-cycle.
                let cell = w.w / 3.0;
                for gy in 0..3 {
                    for gx in 0..3 {
                        let a = 0.25 + 0.75 * (((gx + gy) % 3) as f32 / 2.0);
                        let c = palette(ColorRole::TextSecondary, dark);
                        let c = Color::from_rgba(c.red(), c.green(), c.blue(), a).unwrap_or(c);
                        cv.fill_rrect(x + gx as f32 * cell + cell * 0.2, y + gy as f32 * cell + cell * 0.2, cell * 0.6, cell * 0.6, cell * 0.2, c, clip);
                    }
                }
            }
            WidgetKind::Action { .. } if cv.hits => {
                let c = Color::from_rgba8(255, 0, 90, 70);
                cv.fill_rrect(x, y, w.w, w.h, 4.0, c, clip);
                cv.stroke_rrect(x, y, w.w, w.h, 4.0, Color::from_rgba8(255, 0, 90, 200), 0.5, clip);
            }
            _ => {}
        }
    }
}

// MARK: - scenarios

fn text_entry(id: &str, role: MessageRole, body: &str) -> Arc<SessionMessageEntry> {
    Arc::new(SessionMessageEntry {
        origin: None,
        id: id.into(),
        role,
        parts: vec![MessagePart::Text { id: "t0".into(), text: body.into() }],
        created_at: 0,
        device_id: "d".into(),
        status: Some(zeron_doc::parts::MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    })
}

fn scenario(name: &str) -> TranscriptInput {
    let todo = |spec: &str| -> Arc<Vec<TodoItem>> {
        Arc::new(
            spec.chars()
                .enumerate()
                .map(|(i, c)| {
                    TodoItem::new(
                        format!("Step {i}: {}", ["read the failing test", "reproduce it locally", "fix the null check", "add a regression test", "run the whole suite", "open the pull request"][i % 6]),
                        match c {
                            'x' => TodoStatus::Completed,
                            '>' => TodoStatus::InProgress,
                            _ => TodoStatus::Pending,
                        },
                    )
                })
                .collect(),
        )
    };
    match name {
        "todo" => TranscriptInput {
            entries: vec![text_entry("u", MessageRole::User, "Fix the flaky upload test."), text_entry("a", MessageRole::Assistant, "On it. I will start by reading the failing test.")],
            todo: Some(todo("xx>...")),
            working: true,
            now_ms: NOW,
            ..Default::default()
        },
        "workflows" => TranscriptInput {
            entries: demo::transcript("host", NOW).into_iter().map(Arc::new).collect(),
            goal: Some(Arc::new(demo::demo_goal(NOW))),
            todo: Some(todo("xx>..")),
            workflows: Arc::new(demo::demo_runs(NOW)),
            now_ms: NOW,
            ..Default::default()
        },
        "stopped" => {
            let mut spec = RunSpec::new("run-live", "security-review", WorkflowStatus::Stopped, vec![PhaseSpec::new("scan", 3, 3, 0), PhaseSpec::new("review", 8, 4, 1), PhaseSpec::new("verify", 2, 0, 0)]);
            spec.stop_reason = Some(zeron_proto::WorkflowStopReason::Provider);
            spec.artifacts = true;
            let mut inp = scenario("workflows");
            inp.workflows = Arc::new(zeron_proto::WorkflowRunsState { revision: 1, runs: vec![demo::build_run(&spec)] });
            inp.entries.retain(|e| !e.id.starts_with("wf-run-done"));
            inp
        }
        "big" => {
            let spec = RunSpec::new("run-live", "full-repo-sweep", WorkflowStatus::Running, vec![PhaseSpec::new("sweep", 200, 120, 7), PhaseSpec::new("merge", 1, 0, 0)]);
            let mut inp = scenario("workflows");
            inp.workflows = Arc::new(zeron_proto::WorkflowRunsState { revision: 1, runs: vec![demo::build_run(&spec)] });
            inp.entries.retain(|e| !e.id.starts_with("wf-run-done"));
            inp
        }
        other => panic!("unknown scenario {other:?}: todo | workflows | stopped | big"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let width: f32 = get("--width").and_then(|v| v.parse().ok()).unwrap_or(390.0);
    let out = get("--out").unwrap_or_else(|| "/tmp/render_rows.png".into());
    let dark = args.iter().any(|a| a == "--dark");
    let hits = args.iter().any(|a| a == "--hits");
    let name = get("--scenario").unwrap_or_else(|| "workflows".into());

    let mut faces = Vec::new();
    let mut font_data: HashMap<u16, (Vec<u8>, f32)> = HashMap::new();
    for (role, file) in FACES {
        let bytes = std::fs::read(font_dir().join(format!("{file}.ttf"))).unwrap_or_else(|e| panic!("font {file}: {e}"));
        font_data.insert(role as u16, (bytes.clone(), 0.0));
        faces.push(FaceData { role, bytes });
    }
    let ready = Arc::new(Ready(Mutex::new(0), Condvar::new()));
    let view = TranscriptView::new(TextSystem::new(faces, Some(Arc::new(Fixed))), ready.clone());
    view.set_viewport(width, 1.0);
    view.set_input(scenario(&name));
    for act in get("--act").unwrap_or_default().split(',').filter(|a| !a.is_empty()) {
        view.act(act.to_owned());
    }
    // Wait for the worker to go quiet.
    let mut last = 0;
    loop {
        let guard = ready.0.lock().unwrap();
        let (guard, timeout) = ready.1.wait_timeout(guard, Duration::from_millis(250)).unwrap();
        if timeout.timed_out() && *guard == last && last > 0 {
            break;
        }
        last = *guard;
    }
    let frame: Arc<LayoutFrame> = view.frame();
    let styles: HashMap<u16, StyleInfo> = frame.styles().into_iter().map(|s| (s.id, StyleInfo { face: s.face, size: s.size })).collect();

    let (first, last_row) = match get("--rows").and_then(|r| r.split_once("..").map(|(a, b)| (a.parse::<u32>().unwrap_or(0), b.parse::<u32>().unwrap_or(u32::MAX)))) {
        Some((a, b)) => (a, b.min(frame.row_count())),
        None => (0, frame.row_count()),
    };
    let top = frame.placement(first).map_or(0.0, |p| p.y);
    let bottom = frame.placement(last_row.saturating_sub(1)).map_or(frame.total_height(), |p| p.y + p.height);
    let height_pt = (bottom - top + 12.0).max(40.0);
    let (wpx, hpx) = ((width * SCALE) as u32, (height_pt * SCALE) as u32);
    let mut pm = Pixmap::new(wpx, hpx).expect("pixmap");
    pm.fill(palette_bg(dark));
    let fonts = Fonts { faces: font_data };
    // Style ids index the frame's table; faces are looked up by role.
    for i in first..last_row {
        let p = frame.placement(i).unwrap();
        let d = frame.display(i).unwrap();
        let mut cv = Canvas { pm: &mut pm, dark, fonts: &fonts, ox: 0.0, oy: p.y - top, hits };
        draw_row(&mut cv, &d, &styles, wpx, hpx);
    }
    pm.save_png(&out).expect("write png");
    eprintln!("{name} @{width}pt: {} rows, {:.0}pt tall -> {out}", last_row - first, height_pt);
}

fn palette_bg(dark: bool) -> Color {
    dynamic(0xF3F3F5, 0x060606, 1.0, dark)
}
