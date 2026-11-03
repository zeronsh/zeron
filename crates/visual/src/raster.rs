//! Software rendering for `ui.scene`: a depth-buffered rasterizer that turns a
//! scene into one image the desktop paints.
//!
//! Sorting whole faces back to front (the painter's algorithm) cannot order a
//! wall against the furniture beside it, and painting every face as its own
//! vector path made orbiting slow. Here every sample keeps its depth, so
//! surfaces hide each other exactly; faces crossing the near plane are clipped
//! rather than dropped; curved solids shade smoothly; silhouettes and folds get
//! a fine outline; and edges are supersampled once the camera rests. The image
//! is straight-alpha BGRA, transparent wherever nothing was drawn, so a scene
//! still sits on glass and uses the theme's colors.

use crate::scene::{Camera, Scene, V3, View, add, dot, normalize, scale, sub};
use crate::style::{Color, TextSize};

/// How carefully to draw: `Draft` while the camera moves (one sample per
/// pixel), `Final` once it rests (supersampled within a budget).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    Draft,
    Final,
}

/// A rendered scene.
#[derive(Debug, Clone, Default)]
pub struct Raster {
    /// Device pixels.
    pub width: u32,
    pub height: u32,
    /// Straight-alpha BGRA, top row first.
    pub bgra: Vec<u8>,
    /// Hover hints: canvas position (logical pixels), radius, text.
    pub hints: Vec<([f32; 2], f32, String)>,
    /// Labels for the desktop to set as text over the image.
    pub labels: Vec<RasterLabel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RasterLabel {
    pub at: [f32; 2],
    pub text: String,
    pub color: Color,
    pub size: TextSize,
}

/// Samples a `Final` render may use; past it, it draws one per pixel.
const SAMPLE_BUDGET: f32 = 6_000_000.0;
/// Where two neighbouring samples' depths differ by more than this fraction,
/// a fold or silhouette is outlined.
const EDGE_DEPTH: f32 = 0.02;
/// How much an outline darkens the surface it crosses.
const EDGE_SHADE: f32 = 0.78;

#[derive(Clone, Copy)]
struct Vertex {
    /// View space.
    p: V3,
    /// Lit color, straight sRGB with alpha.
    c: [f32; 4],
}

/// The sample grid: `k` samples per logical pixel.
struct Grid {
    width: usize,
    height: usize,
    k: f32,
}

/// Rows `y0..y1` of the sample grid, rendered by one thread.
struct Band<'a> {
    y0: usize,
    y1: usize,
    width: usize,
    /// Premultiplied sRGB.
    color: &'a mut [[f32; 4]],
    /// View-space depth of the nearest surface; infinity where empty.
    depth: &'a mut [f32],
}

/// A dot in sample space.
struct Dot {
    x: f32,
    y: f32,
    z: f32,
    r: f32,
    c: [f32; 4],
}

impl Scene {
    /// Draw the scene for a canvas `width` by `height` logical pixels at
    /// `pixel_scale` device pixels each. `rgba` resolves a scene color to
    /// straight sRGB components in 0..1. Rows render in parallel.
    pub fn render(
        &self,
        camera: Camera,
        width: f32,
        height: f32,
        pixel_scale: f32,
        quality: Quality,
        rgba: &dyn Fn(Color) -> [f32; 4],
    ) -> Raster {
        let device_w = (width * pixel_scale).ceil().max(0.0) as usize;
        let device_h = (height * pixel_scale).ceil().max(0.0) as usize;
        if device_w == 0 || device_h == 0 {
            return Raster::default();
        }
        let Some(view) = self.view_for(camera, width, height) else {
            return Raster {
                width: device_w as u32,
                height: device_h as u32,
                bgra: vec![0; device_w * device_h * 4],
                ..Default::default()
            };
        };
        let ss = match quality {
            Quality::Draft => 1,
            Quality::Final if (device_w * device_h * 4) as f32 <= SAMPLE_BUDGET => 2,
            Quality::Final => 1,
        };
        let grid = Grid {
            width: device_w * ss,
            height: device_h * ss,
            k: pixel_scale * ss as f32,
        };

        // Everything in sample space first (cheap, single-threaded); the
        // per-sample work after splits across cores.
        let mut opaque: Vec<[Sample; 3]> = Vec::new();
        let mut translucent: Vec<(f32, Vec<[Sample; 3]>)> = Vec::new();
        for face in &self.faces {
            let Some(polygon) = shade_face(face, &view, rgba) else {
                continue;
            };
            let samples: Vec<Sample> = polygon.iter().map(|v| to_sample(&grid, &view, v)).collect();
            let tris = (1..samples.len() - 1).map(|i| [samples[0], samples[i], samples[i + 1]]);
            if polygon.iter().any(|v| v.c[3] < 0.999) {
                let depth = polygon.iter().map(|v| v.p[2]).sum::<f32>() / polygon.len() as f32;
                translucent.push((depth, tris.collect()));
            } else {
                opaque.extend(tris);
            }
        }
        // See-through surfaces blend far to near over what they cover.
        translucent.sort_by(|a, b| b.0.total_cmp(&a.0));
        let translucent: Vec<[Sample; 3]> = translucent.into_iter().flat_map(|(_, t)| t).collect();
        // Lines and points ride just in front of the surface they lie on.
        let bias = view.radius * 2e-3;
        let mut lines: Vec<[Sample; 3]> = Vec::new();
        for line in &self.lines {
            line_tris(
                &grid,
                &view,
                line.a,
                line.b,
                line.width,
                rgba(line.color),
                &mut lines,
            );
        }
        let (hints, labels) = self.annotations(camera, width, height);
        let mut dots = Vec::new();
        for point in &self.points {
            let p = view.view(point.at);
            if p[2] < view.near {
                continue;
            }
            let at = view.screen(p);
            dots.push(Dot {
                x: at[0] * grid.k,
                y: at[1] * grid.k,
                z: p[2] - bias,
                r: point.radius * grid.k,
                c: rgba(point.color),
            });
        }
        let mut color = vec![[0.0f32; 4]; grid.width * grid.height];
        let mut depth = vec![f32::INFINITY; grid.width * grid.height];
        // Opaque surfaces, writing depth.
        for_bands(&grid, ss, &mut color, &mut depth, |band| {
            for tri in &opaque {
                fill_triangle(band, *tri, true, 0.0);
            }
        });
        outline(&grid, ss, &mut color, &depth);
        // Then everything that blends without hiding what is behind it.
        for_bands(&grid, ss, &mut color, &mut depth, |band| {
            for tri in &translucent {
                fill_triangle(band, *tri, false, 0.0);
            }
            for tri in &lines {
                fill_triangle(band, *tri, false, bias);
            }
            for dot in &dots {
                draw_dot(band, dot);
            }
        });

        Raster {
            width: device_w as u32,
            height: device_h as u32,
            bgra: resolve(&grid, &color, device_w, device_h, ss),
            hints,
            labels,
        }
    }
}

impl Scene {
    /// Hover hints and labels at their canvas positions, for the desktop to
    /// draw as text over whichever renderer made the image.
    #[allow(clippy::type_complexity)]
    pub fn annotations(
        &self,
        camera: Camera,
        width: f32,
        height: f32,
    ) -> (Vec<([f32; 2], f32, String)>, Vec<RasterLabel>) {
        let Some(view) = self.view_for(camera, width, height) else {
            return Default::default();
        };
        let hints = self
            .points
            .iter()
            .filter_map(|point| {
                let p = view.view(point.at);
                let hint = point.hint.as_ref()?;
                (p[2] >= view.near).then(|| (view.screen(p), point.radius.max(4.0), hint.clone()))
            })
            .collect();
        let labels = self
            .labels
            .iter()
            .filter_map(|label| {
                let p = view.view(label.at);
                (p[2] >= view.near).then(|| RasterLabel {
                    at: view.screen(p),
                    text: label.text.clone(),
                    color: label.color,
                    size: label.size,
                })
            })
            .collect();
        (hints, labels)
    }
}

/// How many threads share a render.
fn threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 16)
}

/// Run `work` on horizontal bands of the grid, one per thread. Bands start on
/// whole output rows (`ss` sample rows), so supersampled pixels never straddle
/// two bands.
fn for_bands(
    grid: &Grid,
    ss: usize,
    color: &mut [[f32; 4]],
    depth: &mut [f32],
    work: impl Fn(&mut Band) + Sync,
) {
    let rows = grid.height / ss;
    let per = rows.div_ceil(threads()).max(1) * ss;
    let width = grid.width;
    std::thread::scope(|scope| {
        let mut color = color;
        let mut depth = depth;
        let mut y0 = 0;
        let work = &work;
        while y0 < grid.height {
            let y1 = (y0 + per).min(grid.height);
            let (band_color, rest_color) = color.split_at_mut((y1 - y0) * width);
            let (band_depth, rest_depth) = depth.split_at_mut((y1 - y0) * width);
            color = rest_color;
            depth = rest_depth;
            let mut band = Band {
                y0,
                y1,
                width,
                color: band_color,
                depth: band_depth,
            };
            scope.spawn(move || work(&mut band));
            y0 = y1;
        }
    });
}

/// A face as lit view-space vertices, clipped to the near plane; `None` when
/// it is behind the camera or turned away.
fn shade_face(
    face: &crate::scene::Face,
    view: &View,
    rgba: &dyn Fn(Color) -> [f32; 4],
) -> Option<Vec<Vertex>> {
    let corners = &face.v[..face.n as usize];
    let centroid = scale(
        corners.iter().fold([0.0; 3], |acc, v| add(acc, *v)),
        1.0 / corners.len() as f32,
    );
    let toward_eye = dot(face.normal, sub(view.eye, centroid));
    if !face.two_sided && toward_eye < 0.0 {
        return None;
    }
    // The lit side of a two-sided surface is the one the camera sees.
    let flip = if toward_eye < 0.0 { -1.0 } else { 1.0 };
    let mut base = rgba(face.color);
    // Without the GPU's refraction, glass is a see-through tint.
    base[3] *= 1.0 - 0.7 * face.material.glass;
    let glow = face.material.glow;
    let polygon: Vec<Vertex> = corners
        .iter()
        .enumerate()
        .map(|(i, corner)| {
            let normal = match &face.normals {
                Some(normals) => normals[i],
                None => face.normal,
            };
            Vertex {
                p: view.view(*corner),
                c: lit(base, scale(normal, flip), view.light, glow),
            }
        })
        .collect();
    let clipped = clip_near(&polygon, view.near);
    (clipped.len() >= 3).then_some(clipped)
}

/// A soft studio light: sky-to-ground ambient, a key light from the upper
/// left, and a dim fill from the opposite side. Lit in linear light.
fn lit(base: [f32; 4], normal: V3, key: V3, glow: f32) -> [f32; 4] {
    let n = normalize(normal);
    let sky = 0.5 + 0.5 * n[1];
    let ambient = 0.34 + 0.2 * sky;
    let key_light = dot(n, key).max(0.0) * 0.6;
    let fill = dot(n, scale(key, -1.0)).max(0.0) * 0.14;
    let light = ambient + key_light + fill + glow;
    let mut out = base;
    for channel in &mut out[..3] {
        *channel = to_srgb((to_linear(*channel) * light).min(1.0));
    }
    out
}

fn to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(c: f32) -> f32 {
    if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// Sutherland–Hodgman against `z = near`: keep what is in front.
fn clip_near(polygon: &[Vertex], near: f32) -> Vec<Vertex> {
    if polygon.iter().all(|v| v.p[2] >= near) {
        return polygon.to_vec();
    }
    let mut out = Vec::with_capacity(polygon.len() + 2);
    for i in 0..polygon.len() {
        let a = polygon[i];
        let b = polygon[(i + 1) % polygon.len()];
        let (a_in, b_in) = (a.p[2] >= near, b.p[2] >= near);
        if a_in {
            out.push(a);
        }
        if a_in != b_in {
            let t = (near - a.p[2]) / (b.p[2] - a.p[2]);
            out.push(Vertex {
                p: add(a.p, scale(sub(b.p, a.p), t)),
                c: lerp4(a.c, b.c, t),
            });
        }
    }
    out
}

fn lerp4(a: [f32; 4], b: [f32; 4], t: f32) -> [f32; 4] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// A vertex in sample space: x, y, view depth, color.
#[derive(Clone, Copy)]
struct Sample {
    x: f32,
    y: f32,
    z: f32,
    c: [f32; 4],
}

fn to_sample(grid: &Grid, view: &View, v: &Vertex) -> Sample {
    let [x, y] = view.screen(v.p);
    Sample {
        x: x * grid.k,
        y: y * grid.k,
        z: v.p[2],
        c: v.c,
    }
}

/// Rasterize one triangle into a band with perspective-correct depth and
/// color. `opaque` writes depth and replaces color; otherwise it blends.
fn fill_triangle(band: &mut Band, tri: [Sample; 3], opaque: bool, bias: f32) {
    let [a, b, c] = tri;
    let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    if area.abs() < 1e-8 {
        return;
    }
    let min_y = (a.y.min(b.y).min(c.y).floor().max(band.y0 as f32)) as usize;
    let max_y = (a.y.max(b.y).max(c.y).ceil() as isize).min(band.y1 as isize - 1);
    if max_y < min_y as isize {
        return;
    }
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as usize;
    let max_x = (a.x.max(b.x).max(c.x).ceil() as isize).min(band.width as isize - 1);
    if max_x < min_x as isize {
        return;
    }
    let inv_area = 1.0 / area;
    let inv_z = [1.0 / a.z, 1.0 / b.z, 1.0 / c.z];
    for y in min_y..=max_y as usize {
        let py = y as f32 + 0.5;
        let row = (y - band.y0) * band.width;
        for x in min_x..=max_x as usize {
            let px = x as f32 + 0.5;
            let w0 = ((b.x - px) * (c.y - py) - (b.y - py) * (c.x - px)) * inv_area;
            let w1 = ((c.x - px) * (a.y - py) - (c.y - py) * (a.x - px)) * inv_area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let (p0, p1, p2) = (w0 * inv_z[0], w1 * inv_z[1], w2 * inv_z[2]);
            let one_over_z = p0 + p1 + p2;
            let z = 1.0 / one_over_z - bias;
            let i = row + x;
            if z >= band.depth[i] {
                continue;
            }
            let color = [
                (p0 * a.c[0] + p1 * b.c[0] + p2 * c.c[0]) / one_over_z,
                (p0 * a.c[1] + p1 * b.c[1] + p2 * c.c[1]) / one_over_z,
                (p0 * a.c[2] + p1 * b.c[2] + p2 * c.c[2]) / one_over_z,
                (p0 * a.c[3] + p1 * b.c[3] + p2 * c.c[3]) / one_over_z,
            ];
            if opaque {
                band.depth[i] = z;
                band.color[i] = [color[0], color[1], color[2], 1.0];
            } else {
                blend(&mut band.color[i], color);
            }
        }
    }
}

/// Straight color over a premultiplied sample.
fn blend(under: &mut [f32; 4], over: [f32; 4]) {
    let a = over[3].clamp(0.0, 1.0);
    for ch in 0..3 {
        under[ch] = over[ch] * a + under[ch] * (1.0 - a);
    }
    under[3] = a + under[3] * (1.0 - a);
}

/// Darken samples on a silhouette or fold, where depth jumps between
/// neighbours, so shapes read clearly whatever the lighting.
fn outline(grid: &Grid, ss: usize, color: &mut [[f32; 4]], depth: &[f32]) {
    let (w, h) = (grid.width, grid.height);
    let rows = h / ss;
    let per = rows.div_ceil(threads()).max(1) * ss;
    std::thread::scope(|scope| {
        let mut rest = color;
        let mut y0 = 0;
        while y0 < h {
            let y1 = (y0 + per).min(h);
            let (band, tail) = rest.split_at_mut((y1 - y0) * w);
            rest = tail;
            scope.spawn(move || {
                for y in y0..y1 {
                    for x in 0..w {
                        let i = y * w + x;
                        let z = depth[i];
                        if !z.is_finite() {
                            continue;
                        }
                        let jump = |j: usize| {
                            let other = depth[j];
                            !other.is_finite() || (other - z).abs() > EDGE_DEPTH * z.min(other)
                        };
                        let edge = (x + 1 < w && jump(i + 1))
                            || (x > 0 && jump(i - 1))
                            || (y + 1 < h && jump(i + w))
                            || (y > 0 && jump(i - w));
                        if edge {
                            for ch in &mut band[(y - y0) * w + x][..3] {
                                *ch *= EDGE_SHADE;
                            }
                        }
                    }
                }
            });
            y0 = y1;
        }
    });
}

/// A line `width` logical pixels wide, as two triangles in sample space.
#[allow(clippy::too_many_arguments)]
fn line_tris(
    grid: &Grid,
    view: &View,
    a: V3,
    b: V3,
    width: f32,
    color: [f32; 4],
    out: &mut Vec<[Sample; 3]>,
) {
    let (mut a, mut b) = (view.view(a), view.view(b));
    if a[2] < view.near && b[2] < view.near {
        return;
    }
    // Clip the part behind the near plane.
    if a[2] < view.near || b[2] < view.near {
        let t = (view.near - a[2]) / (b[2] - a[2]);
        let cut = add(a, scale(sub(b, a), t));
        if a[2] < view.near {
            a = cut;
        } else {
            b = cut;
        }
    }
    let to = |p: V3| {
        let [x, y] = view.screen(p);
        (x * grid.k, y * grid.k, p[2])
    };
    let (ax, ay, az) = to(a);
    let (bx, by, bz) = to(b);
    let (dx, dy) = (bx - ax, by - ay);
    let len = dx.hypot(dy);
    if len < 1e-4 {
        return;
    }
    let half = (width * grid.k).max(1.0) / 2.0;
    let (nx, ny) = (-dy / len * half, dx / len * half);
    let corner = |x: f32, y: f32, z: f32| Sample { x, y, z, c: color };
    let quad = [
        corner(ax + nx, ay + ny, az),
        corner(bx + nx, by + ny, bz),
        corner(bx - nx, by - ny, bz),
        corner(ax - nx, ay - ny, az),
    ];
    out.push([quad[0], quad[1], quad[2]]);
    out.push([quad[0], quad[2], quad[3]]);
}

/// A round dot, depth-tested at its center.
fn draw_dot(band: &mut Band, dot: &Dot) {
    let min_y = (dot.y - dot.r).floor().max(band.y0 as f32) as usize;
    let max_y = ((dot.y + dot.r).ceil() as isize).min(band.y1 as isize - 1);
    let min_x = (dot.x - dot.r).floor().max(0.0) as usize;
    let max_x = ((dot.x + dot.r).ceil() as isize).min(band.width as isize - 1);
    if max_y < min_y as isize || max_x < min_x as isize {
        return;
    }
    for y in min_y..=max_y as usize {
        for x in min_x..=max_x as usize {
            let (px, py) = (x as f32 + 0.5 - dot.x, y as f32 + 0.5 - dot.y);
            if px * px + py * py > dot.r * dot.r {
                continue;
            }
            let i = (y - band.y0) * band.width + x;
            if dot.z < band.depth[i] {
                blend(&mut band.color[i], dot.c);
            }
        }
    }
}

/// Average each pixel's samples and hand back straight-alpha BGRA.
fn resolve(grid: &Grid, color: &[[f32; 4]], width: usize, height: usize, ss: usize) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    let n = (ss * ss) as f32;
    let per = height.div_ceil(threads()).max(1);
    std::thread::scope(|scope| {
        for (chunk, rows) in out.chunks_mut(per * width * 4).enumerate() {
            scope.spawn(move || {
                let y0 = chunk * per;
                for (dy, row) in rows.chunks_mut(width * 4).enumerate() {
                    let y = y0 + dy;
                    for x in 0..width {
                        let mut sum = [0.0f32; 4];
                        for sy in 0..ss {
                            let base = (y * ss + sy) * grid.width + x * ss;
                            for sample in &color[base..base + ss] {
                                for ch in 0..4 {
                                    sum[ch] += sample[ch];
                                }
                            }
                        }
                        let alpha = sum[3] / n;
                        if alpha <= 0.0 {
                            continue;
                        }
                        let straight = |ch: usize| {
                            ((sum[ch] / n / alpha).clamp(0.0, 1.0) * 255.0).round() as u8
                        };
                        let o = x * 4;
                        row[o] = straight(2);
                        row[o + 1] = straight(1);
                        row[o + 2] = straight(0);
                        row[o + 3] = (alpha.clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
            });
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Token;

    fn camera() -> Camera {
        Camera {
            yaw: 0.0,
            pitch: 0.0,
            zoom: 1.0,
        }
    }

    fn colors(color: Color) -> [f32; 4] {
        match color.token {
            Token::Chart(0) => [1.0, 0.0, 0.0, 1.0],
            Token::Chart(1) => [0.0, 0.0, 1.0, 1.0],
            _ => [0.5, 0.5, 0.5, 1.0],
        }
    }

    fn pixel(raster: &Raster, x: u32, y: u32) -> [u8; 4] {
        let o = ((y * raster.width + x) * 4) as usize;
        let p = &raster.bgra[o..o + 4];
        [p[2], p[1], p[0], p[3]]
    }

    /// A small red square in front of a large blue one: whatever order the
    /// faces come in, the near one covers the far one where they overlap.
    #[test]
    fn nearer_surfaces_hide_farther_ones_whatever_their_order() {
        for near_first in [true, false] {
            let mut s = Scene::default();
            let far = [
                [-2., -2., -1.],
                [2., -2., -1.],
                [2., 2., -1.],
                [-2., 2., -1.],
            ];
            let near = [
                [-0.5, -0.5, 1.],
                [0.5, -0.5, 1.],
                [0.5, 0.5, 1.],
                [-0.5, 0.5, 1.],
            ];
            let faces = [(near, 0), (far, 1)];
            let order: Vec<_> = if near_first {
                faces.to_vec()
            } else {
                faces.iter().rev().copied().collect()
            };
            for (quad, chart) in order {
                s.face(
                    &quad,
                    Color::token(Token::Chart(chart)),
                    Some([0., 0., 1.]),
                    true,
                )
                .unwrap();
            }
            let raster = s.render(camera(), 200.0, 200.0, 1.0, Quality::Draft, &colors);
            let center = pixel(&raster, 100, 100);
            assert!(
                center[0] > 100 && center[2] < 60,
                "near red wins: {center:?}"
            );
            let corner = pixel(&raster, 100, 30);
            assert!(
                corner[2] > 100 && corner[0] < 60,
                "far blue shows around it: {corner:?}"
            );
            // Nothing outside the geometry: transparent.
            assert_eq!(pixel(&raster, 1, 1)[3], 0);
        }
    }

    #[test]
    fn faces_crossing_the_camera_are_clipped_not_dropped() {
        let mut s = Scene::default();
        // A long floor that runs from far away to behind the camera.
        let floor = [
            [-1., -1., -40.],
            [1., -1., -40.],
            [1., -1., 40.],
            [-1., -1., 40.],
        ];
        s.face(
            &floor,
            Color::token(Token::Chart(0)),
            Some([0., 1., 0.]),
            true,
        )
        .unwrap();
        // A marker the camera frames itself on, near the middle.
        s.face(
            &[[-0.2, 0., 0.], [0.2, 0., 0.], [0., 0.3, 0.]],
            Color::token(Token::Chart(1)),
            Some([0., 0., 1.]),
            true,
        )
        .unwrap();
        let zoomed = Camera {
            zoom: 6.0,
            ..camera()
        };
        let raster = s.render(zoomed, 200.0, 200.0, 1.0, Quality::Draft, &colors);
        let drawn = raster.bgra.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(drawn > 0, "the floor still shows");
    }

    #[test]
    fn final_quality_supersamples_within_its_budget() {
        let mut s = Scene::default();
        s.face(
            &[[-1., -1., 0.], [1., -1., 0.], [0., 1., 0.]],
            Color::token(Token::Chart(0)),
            Some([0., 0., 1.]),
            true,
        )
        .unwrap();
        let draft = s.render(camera(), 100.0, 80.0, 2.0, Quality::Draft, &colors);
        let fine = s.render(camera(), 100.0, 80.0, 2.0, Quality::Final, &colors);
        assert_eq!((draft.width, draft.height), (200, 160));
        assert_eq!((fine.width, fine.height), (200, 160));
        // Supersampled edges have partial coverage; one sample cannot.
        let partial = |r: &Raster| r.bgra.chunks_exact(4).any(|p| p[3] > 0 && p[3] < 255);
        assert!(!partial(&draft));
        assert!(partial(&fine));
    }
}

#[cfg(test)]
mod timing {
    use super::*;
    use crate::style::Token;

    /// Not an assertion: prints how long a furnished room takes, so a change
    /// that slows rendering shows up in `cargo test -- --nocapture timing`.
    #[test]
    fn timing_a_furnished_room() {
        let mut s = Scene::default();
        let quad = |s: &mut Scene, q: [V3; 4], c: u8| {
            s.face(&q, Color::token(Token::Chart(c)), None, true)
                .unwrap();
        };
        quad(
            &mut s,
            [[-4., 0., -3.], [4., 0., -3.], [4., 0., 3.], [-4., 0., 3.]],
            0,
        );
        quad(
            &mut s,
            [[-4., 0., -3.], [4., 0., -3.], [4., 3., -3.], [-4., 3., -3.]],
            1,
        );
        quad(
            &mut s,
            [[-4., 0., -3.], [-4., 0., 3.], [-4., 3., 3.], [-4., 3., -3.]],
            2,
        );
        // Spheres stand in for plants and lamps: smooth, many faces.
        for k in 0..8 {
            let at = [-3.0 + k as f32 * 0.8, 0.6, 1.0];
            let (lon, lat) = (36, 18);
            for j in 0..lat {
                for i in 0..lon {
                    let p = |i: u32, j: u32| {
                        let t = std::f32::consts::TAU * i as f32 / lon as f32;
                        let f = std::f32::consts::PI * j as f32 / lat as f32;
                        [
                            at[0] + 0.4 * f.sin() * t.cos(),
                            at[1] + 0.4 * f.cos(),
                            at[2] + 0.4 * f.sin() * t.sin(),
                        ]
                    };
                    let q = [p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1)];
                    let n: Vec<V3> = q.iter().map(|c| sub(*c, at)).collect();
                    s.smooth_face(&q, &n, Color::token(Token::Chart(3)))
                        .unwrap();
                }
            }
        }
        let colors = |_: Color| [0.6, 0.6, 0.6, 1.0];
        let camera = Camera {
            yaw: 0.7,
            pitch: 0.4,
            zoom: 1.0,
        };
        for quality in [Quality::Draft, Quality::Final] {
            let start = std::time::Instant::now();
            let raster = s.render(camera, 736.0, 420.0, 2.0, quality, &colors);
            println!(
                "{quality:?}: {} faces, {}x{} in {:?}",
                s.face_count(),
                raster.width,
                raster.height,
                start.elapsed()
            );
        }
    }
}
