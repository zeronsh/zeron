//! Vector paint for visuals: positioned text runs (measured here with the
//! bundled faces, drawn by the platform with the same bytes) and shapes.

use zeron_text::{OverflowWrap, PrepareOptions, Prepared, Span, WhiteSpace, WidthCache};
use zeron_visual::{Align, TextSize};

use super::{Rgba, VisualPaint, VisualRun, VisualShape};
use crate::layout::style::{Family, Typography, Weight, baseline};

/// Text measurement for one visual (its own faces and width cache).
pub(crate) struct TextCtx<'a> {
    pub typo: &'a mut Typography,
    pub cache: &'a mut WidthCache,
}

/// One styled stretch of a paragraph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Face {
    pub size: TextSize,
    pub weight: zeron_visual::Weight,
    pub mono: bool,
    pub italic: bool,
}

impl Face {
    pub fn new(size: TextSize, weight: zeron_visual::Weight) -> Self {
        Self {
            size,
            weight,
            mono: false,
            italic: false,
        }
    }
}

fn weight(w: zeron_visual::Weight) -> Weight {
    match w {
        zeron_visual::Weight::Regular => Weight::Regular,
        zeron_visual::Weight::Medium => Weight::Medium,
        zeron_visual::Weight::Semibold => Weight::Semibold,
        zeron_visual::Weight::Bold => Weight::Bold,
    }
}

/// A prepared paragraph: wraps at any width without re-measuring.
pub(crate) struct Para {
    pub p: Prepared,
    /// Line height and baseline within it.
    pub lh: f32,
    pub base: f32,
    pub colors: Vec<Rgba>,
}

impl TextCtx<'_> {
    /// Prepare `parts` as one paragraph; the line height is the largest
    /// face's.
    pub fn para(&mut self, parts: &[(&str, Face, Rgba)]) -> Para {
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut colors = Vec::new();
        let mut lh: f32 = 0.0;
        let mut base: f32 = 0.0;
        for (s, face, color) in parts {
            let (size, line) = face.size.metrics();
            let family = if face.mono {
                Family::Mono
            } else {
                Family::Sans
            };
            let style = self
                .typo
                .style(family, weight(face.weight), face.italic, size);
            let line = self.typo.px(line);
            if line >= lh {
                lh = line;
                base = baseline(line, style);
            }
            if s.is_empty() {
                continue;
            }
            let start = text.len();
            text.push_str(s);
            spans.push(Span::new(start..text.len(), style.id));
            colors.push(*color);
        }
        let p = zeron_text::prepare(
            &self.typo.book,
            self.cache,
            &text,
            &spans,
            &PrepareOptions {
                white_space: WhiteSpace::PreWrap,
                overflow_wrap: OverflowWrap::Anywhere,
                ..Default::default()
            },
        );
        Para {
            p,
            lh,
            base,
            colors,
        }
    }

    /// One line of plain text.
    pub fn line(&mut self, text: &str, face: Face, color: Rgba) -> Para {
        self.para(&[(text, face, color)])
    }

    pub fn line_height(&mut self, size: TextSize) -> f32 {
        self.typo.px(size.metrics().1)
    }
}

impl Para {
    /// Its width unwrapped.
    pub fn natural(&self) -> f32 {
        self.p.max_content_width()
    }
}

/// Accumulates a layer of runs and shapes.
#[derive(Default)]
pub(crate) struct Paint {
    pub text: String,
    utf16: u32,
    pub runs: Vec<VisualRun>,
    pub shapes: Vec<VisualShape>,
}

/// Where a paint stood, so what was added since can be moved.
#[derive(Clone, Copy)]
pub(crate) struct Mark {
    runs: usize,
    shapes: usize,
}

impl Paint {
    pub fn mark(&self) -> Mark {
        Mark {
            runs: self.runs.len(),
            shapes: self.shapes.len(),
        }
    }

    /// Move what was added between two marks.
    pub fn shift_between(&mut self, from: Mark, to: Mark, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        for run in &mut self.runs[from.runs..to.runs] {
            run.x += dx;
            run.baseline += dy;
        }
        for shape in &mut self.shapes[from.shapes..to.shapes] {
            match shape {
                VisualShape::Rect { x, y, .. } => {
                    *x += dx;
                    *y += dy;
                }
                VisualShape::Path { points, .. } => {
                    for xy in points.chunks_exact_mut(2) {
                        xy[0] += dx;
                        xy[1] += dy;
                    }
                }
            }
        }
    }

    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: Rgba) {
        if w <= 0.0 || h <= 0.0 || color.a <= 0.0 {
            return;
        }
        self.shapes.push(VisualShape::Rect {
            x,
            y,
            w,
            h,
            radius,
            color,
        });
    }

    pub fn fill(&mut self, points: impl IntoIterator<Item = [f32; 2]>, color: Rgba) {
        let points: Vec<f32> = points.into_iter().flatten().collect();
        if points.len() < 6 || color.a <= 0.0 {
            return;
        }
        self.shapes.push(VisualShape::Path {
            points,
            closed: true,
            fill: Some(color),
            stroke: None,
            line_width: 0.0,
            dash: None,
        });
    }

    pub fn stroke(
        &mut self,
        points: impl IntoIterator<Item = [f32; 2]>,
        closed: bool,
        width: f32,
        dash: Option<f32>,
        color: Rgba,
    ) {
        let points: Vec<f32> = points.into_iter().flatten().collect();
        if points.len() < 4 || color.a <= 0.0 {
            return;
        }
        self.shapes.push(VisualShape::Path {
            points,
            closed,
            fill: None,
            stroke: Some(color),
            line_width: width,
            dash,
        });
    }

    /// A filled dot (chart points).
    pub fn dot(&mut self, [cx, cy]: [f32; 2], radius: f32, color: Rgba) {
        self.rect(
            cx - radius,
            cy - radius,
            radius * 2.0,
            radius * 2.0,
            radius,
            color,
        );
    }

    /// Wrap `para` at `width` from (x, y); returns its height. `truncate`
    /// keeps the first line, ending in an ellipsis when more follows.
    #[allow(clippy::too_many_arguments)]
    pub fn para(
        &mut self,
        ctx: &mut TextCtx,
        para: &Para,
        x: f32,
        y: f32,
        width: f32,
        align: Align,
        truncate: bool,
    ) -> f32 {
        let width = width.max(1.0);
        if truncate {
            let lines = para.p.lines(width);
            if lines.len() <= 1 {
                self.lines(para, &lines, x, y, width, align);
                return para.lh;
            }
            // Room for the ellipsis in the last span's face.
            let last_style = para.p.spans().last().map(|s| s.style);
            let ellipsis_width = last_style.map_or(0.0, |style| {
                let p = zeron_text::prepare(
                    &ctx.typo.book,
                    ctx.cache,
                    "…",
                    &[Span::new(0.."…".len(), style)],
                    &PrepareOptions::default(),
                );
                p.max_content_width()
            });
            let lines = para.p.lines((width - ellipsis_width).max(1.0));
            let first = &lines[..1];
            let end = self.lines(para, first, x, y, width - ellipsis_width, Align::Start);
            if let Some(style) = last_style {
                let start = self.push_text("…");
                let color = para.colors.last().copied().unwrap_or_default();
                self.runs.push(VisualRun {
                    start,
                    len: 1,
                    x: end,
                    baseline: y + para.base,
                    width: ellipsis_width,
                    style: style.0,
                    color,
                });
            }
            return para.lh;
        }
        let lines = para.p.lines(width);
        self.lines(para, &lines, x, y, width, align);
        lines.len().max(1) as f32 * para.lh
    }

    /// Draw laid-out lines; returns where the last one ended.
    fn lines(
        &mut self,
        para: &Para,
        lines: &[zeron_text::Line],
        x: f32,
        y: f32,
        width: f32,
        align: Align,
    ) -> f32 {
        if lines.is_empty() {
            return x;
        }
        let base16 = self.push_text(para.p.text());
        let spans = para.p.spans();
        let mut end = x;
        for (i, line) in lines.iter().enumerate() {
            let offset = match align {
                Align::Start => 0.0,
                Align::Center => ((width - line.width) / 2.0).max(0.0),
                Align::End => (width - line.width).max(0.0),
            };
            let baseline = y + i as f32 * para.lh + para.base;
            for f in &line.fragments {
                if f.utf16.is_empty() {
                    continue;
                }
                self.runs.push(VisualRun {
                    start: base16 + f.utf16.start as u32,
                    len: (f.utf16.end - f.utf16.start) as u32,
                    x: x + offset + f.x,
                    baseline,
                    width: f.width,
                    style: spans[f.span].style.0,
                    color: para.colors.get(f.span).copied().unwrap_or_default(),
                });
            }
            end = x + offset + line.width;
        }
        end
    }

    /// One line anchored at `at`: `align` places it horizontally around x,
    /// `vertical` places its line box below (Start), around (Center) or
    /// above (End) y. Returns its width.
    #[allow(clippy::too_many_arguments)]
    pub fn label(
        &mut self,
        ctx: &mut TextCtx,
        text: &str,
        face: Face,
        color: Rgba,
        [x, y]: [f32; 2],
        align: Align,
        vertical: Align,
    ) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        let para = ctx.line(text, face, color);
        let width = para.natural();
        let left = match align {
            Align::Start => x,
            Align::Center => x - width / 2.0,
            Align::End => x - width,
        };
        let top = match vertical {
            Align::Start => y,
            Align::Center => y - para.lh / 2.0,
            Align::End => y - para.lh,
        };
        let lines = para.p.lines(1.0e7);
        self.lines(&para, &lines, left, top, width, Align::Start);
        width
    }

    fn push_text(&mut self, s: &str) -> u32 {
        let start = self.utf16;
        self.text.push_str(s);
        self.utf16 += s.encode_utf16().count() as u32;
        start
    }

    /// A copy for the platform (the layout keeps its own).
    pub fn snapshot(&self) -> VisualPaint {
        VisualPaint {
            text: self.text.clone(),
            runs: self.runs.clone(),
            shapes: self.shapes.clone(),
        }
    }

    pub fn finish(self) -> VisualPaint {
        VisualPaint {
            text: self.text,
            runs: self.runs,
            shapes: self.shapes,
        }
    }
}
