//! Port of Ghostty's src/font/sprite/canvas.zig: an alpha-only drawing surface
//! with padding around the cell, so a glyph may spill past its cell edges.
//! z2d is replaced by tiny-skia; coordinates passed in are cell-relative and
//! the padding offset is applied here.

use tiny_skia::{
    FillRule, LineCap, Paint, Path, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform,
};

/// Pixel coverage. Ghostty's `Color`/`Shade`: anything 0…255.
pub type Color = u8;
pub const ON: Color = 0xff;
pub const OFF: Color = 0x00;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point<T> {
    pub x: T,
    pub y: T,
}

pub const fn pt<T>(x: T, y: T) -> Point<T> {
    Point { x, y }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect<T> {
    pub x: T,
    pub y: T,
    pub width: T,
    pub height: T,
}

pub struct Canvas {
    /// RGBA, but only alpha is meaningful (all channels carry the coverage).
    pixmap: Pixmap,
    pub padding_x: u32,
    pub padding_y: u32,
    /// Margins zeroed on output: a glyph sets them to its padding to clip
    /// itself to the cell.
    pub clip_top: u32,
    pub clip_left: u32,
    pub clip_right: u32,
    pub clip_bottom: u32,
}

impl Canvas {
    pub fn new(width: u32, height: u32, padding_x: u32, padding_y: u32) -> Canvas {
        Canvas {
            pixmap: Pixmap::new(
                (width + 2 * padding_x).max(1),
                (height + 2 * padding_y).max(1),
            )
            .expect("canvas size"),
            padding_x,
            padding_y,
            clip_top: 0,
            clip_left: 0,
            clip_right: 0,
            clip_bottom: 0,
        }
    }

    /// Full surface size, padding included.
    pub fn surface_width(&self) -> u32 {
        self.pixmap.width()
    }

    pub fn surface_height(&self) -> u32 {
        self.pixmap.height()
    }

    fn transform(&self) -> Transform {
        Transform::from_translate(self.padding_x as f32, self.padding_y as f32)
    }

    /// Set one pixel (cell coordinates); outside the surface is ignored.
    pub fn pixel(&mut self, x: i32, y: i32, color: Color) {
        let x = x + self.padding_x as i32;
        let y = y + self.padding_y as i32;
        if x < 0 || y < 0 || x >= self.pixmap.width() as i32 || y >= self.pixmap.height() as i32 {
            return;
        }
        let ix = y as usize * self.pixmap.width() as usize + x as usize;
        self.pixmap.pixels_mut()[ix] =
            PremultipliedColorU8::from_rgba(color, color, color, color).unwrap();
    }

    /// Fill a rectangle, replacing what is there. The main primitive: lines
    /// are skinny rectangles.
    pub fn rect(&mut self, r: Rect<i32>, color: Color) {
        for y in r.y..r.y + r.height {
            for x in r.x..r.x + r.width {
                self.pixel(x, y, color);
            }
        }
    }

    /// `rect` from two corners, in any order.
    pub fn box_(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, color: Color) {
        let (l, r) = (x0.min(x1), x0.max(x1));
        let (t, b) = (y0.min(y1), y0.max(y1));
        self.rect(
            Rect {
                x: l,
                y: t,
                width: r - l,
                height: b - t,
            },
            color,
        );
    }

    pub fn quad(&mut self, p: [Point<f64>; 4], color: Color) {
        let mut pb = PathBuilder::new();
        pb.move_to(p[0].x as f32, p[0].y as f32);
        for q in &p[1..] {
            pb.line_to(q.x as f32, q.y as f32);
        }
        pb.close();
        if let Some(path) = pb.finish() {
            self.fill_path(&path, color);
        }
    }

    pub fn triangle(&mut self, p: [Point<f64>; 3], color: Color) {
        let mut pb = PathBuilder::new();
        pb.move_to(p[0].x as f32, p[0].y as f32);
        pb.line_to(p[1].x as f32, p[1].y as f32);
        pb.line_to(p[2].x as f32, p[2].y as f32);
        pb.close();
        if let Some(path) = pb.finish() {
            self.fill_path(&path, color);
        }
    }

    /// Stroke a line with butt caps.
    pub fn line(&mut self, p0: Point<f64>, p1: Point<f64>, thickness: f64, color: Color) {
        let mut pb = PathBuilder::new();
        pb.move_to(p0.x as f32, p0.y as f32);
        pb.line_to(p1.x as f32, p1.y as f32);
        if let Some(path) = pb.finish() {
            self.stroke_path(&path, &stroke(thickness), color);
        }
    }

    /// Fill a cell-coordinate path (non-zero rule, anti-aliased, src-over).
    pub fn fill_path(&mut self, path: &Path, color: Color) {
        let transform = self.transform();
        self.pixmap
            .fill_path(path, &paint(color), FillRule::Winding, transform, None);
    }

    /// Stroke a cell-coordinate path (anti-aliased, src-over). Filled as the
    /// stroke's outline like z2d: tiny-skia draws strokes ≤1px as faint
    /// hairlines instead.
    pub fn stroke_path(&mut self, path: &Path, stroke: &Stroke, color: Color) {
        if let Some(outline) = path.stroke(stroke, 1.0) {
            self.fill_path(&outline, color);
        }
    }

    pub fn invert(&mut self) {
        for px in self.pixmap.pixels_mut() {
            let a = 255 - px.alpha();
            *px = PremultipliedColorU8::from_rgba(a, a, a, a).unwrap();
        }
    }

    pub fn flip_horizontal(&mut self) {
        let w = self.pixmap.width() as usize;
        for row in self.pixmap.pixels_mut().chunks_exact_mut(w) {
            row.reverse();
        }
        std::mem::swap(&mut self.clip_left, &mut self.clip_right);
    }

    pub fn flip_vertical(&mut self) {
        let w = self.pixmap.width() as usize;
        let h = self.pixmap.height() as usize;
        let px = self.pixmap.pixels_mut();
        for y in 0..h / 2 {
            for x in 0..w {
                px.swap(y * w + x, (h - 1 - y) * w + x);
            }
        }
        std::mem::swap(&mut self.clip_top, &mut self.clip_bottom);
    }

    /// Coverage of one surface pixel (surface coordinates, padding included).
    pub fn alpha_at(&self, x: u32, y: u32) -> u8 {
        self.pixmap.pixel(x, y).map_or(0, |p| p.alpha())
    }

    /// The whole padded surface as alpha bytes, row-major, with the clip
    /// margins zeroed. Its top-left sits at (-padding_x, -padding_y) relative
    /// to the cell.
    pub fn into_alpha(self) -> Vec<u8> {
        let (w, h) = (self.pixmap.width(), self.pixmap.height());
        let mut out: Vec<u8> = self.pixmap.pixels().iter().map(|p| p.alpha()).collect();
        for y in 0..h {
            for x in 0..w {
                let clipped = x < self.clip_left
                    || x >= w.saturating_sub(self.clip_right)
                    || y < self.clip_top
                    || y >= h.saturating_sub(self.clip_bottom);
                if clipped {
                    out[(y * w + x) as usize] = 0;
                }
            }
        }
        out
    }
}

fn paint(color: Color) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(255, 255, 255, color);
    paint.anti_alias = true;
    paint
}

/// z2d's stroke defaults: butt caps, miter joins with limit 10.
pub fn stroke(width: f64) -> Stroke {
    Stroke {
        width: width as f32,
        line_cap: LineCap::Butt,
        miter_limit: 10.0,
        ..Stroke::default()
    }
}
