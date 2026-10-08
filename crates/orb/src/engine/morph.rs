// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Morph: dotted outline cycling circle → triangle → square — "shaping".

use crate::engine::{
    core::{Dot, Frame},
    profiles::{MAX_MORPH_DOTS, ModeOpts},
};
use std::f32::consts::PI;

/// Outline resolution used to re-parameterise the blended shape by arc length.
const M_SAMPLES: usize = 160;
/// Largest vertex count across the shipped polygons (square, incl. the
/// repeated closing vertex).
const MAX_VERTS: usize = 8;

const TRIANGLE: [(f32, f32); 3] = [(0.0, -0.26), (0.24, 0.16), (-0.24, 0.16)];
const SQUARE: [(f32, f32); 4] = [(0.0, -0.2), (0.2, -0.2), (0.2, 0.2), (-0.2, 0.2)];

fn smooth_e(x: f32) -> f32 {
    x * x * (3.0 - 2.0 * x)
}

fn circle_path(f: f32) -> (f32, f32) {
    let a = -PI / 2.0 + f * 2.0 * PI;
    (a.cos() * 0.24, a.sin() * 0.24)
}

/// Arc-length parameterisation of a closed polygon.
///
/// Used to be a `Box<dyn Fn>` built three times per frame; now it is a plain
/// stack value with fixed-capacity length table.
struct PolyPath {
    verts: &'static [(f32, f32)],
    lengths: [f32; MAX_VERTS],
    total: f32,
}

impl PolyPath {
    fn new(verts: &'static [(f32, f32)]) -> Self {
        debug_assert!(verts.len() <= MAX_VERTS);
        let mut lengths = [0.0; MAX_VERTS];
        let mut total = 0.0;
        let v = verts.len();
        for i in 0..v {
            let a = verts[i];
            let b = verts[(i + 1) % v];
            let l = (b.0 - a.0).hypot(b.1 - a.1);
            lengths[i] = l;
            total += l;
        }
        Self {
            verts,
            lengths,
            total,
        }
    }

    fn at(&self, f: f32) -> (f32, f32) {
        let v = self.verts.len();
        let mut target = f * self.total;
        let mut i = 0;
        // Bounds check first: `i` can never leave `0..v`.
        while i < v - 1 && target > self.lengths[i] {
            target -= self.lengths[i];
            i += 1;
        }
        let a = self.verts[i];
        let b = self.verts[(i + 1) % v];
        let ff = if self.lengths[i] > 0.0 {
            (target / self.lengths[i]).min(1.0)
        } else {
            0.0
        };
        (a.0 + (b.0 - a.0) * ff, a.1 + (b.1 - a.1) * ff)
    }
}

enum Shape {
    Circle,
    Poly(PolyPath),
}

impl Shape {
    fn at(&self, f: f32) -> (f32, f32) {
        match self {
            Shape::Circle => circle_path(f),
            Shape::Poly(p) => p.at(f),
        }
    }
}

fn morph_n(d: f32) -> usize {
    let n = (34.0 * d).round().max(6.0);
    (n as usize).clamp(6, MAX_MORPH_DOTS)
}

const HOLD: f32 = 1.4;
const MORPH: f32 = 0.9;
const SEG: f32 = HOLD + MORPH;

pub fn draw_morph_into(size: f32, t: f32, o: &ModeOpts, out: &mut Frame) {
    let cycle = [
        Shape::Circle,
        Shape::Poly(PolyPath::new(&TRIANGLE)),
        Shape::Poly(PolyPath::new(&SQUARE)),
    ];
    let k_len = cycle.len();
    let tc = t.rem_euclid(SEG * k_len as f32);
    let k = ((tc / SEG).floor() as usize).min(k_len - 1);
    let local = tc - k as f32 * SEG;
    let m = if local > HOLD {
        smooth_e((local - HOLD) / MORPH)
    } else {
        0.0
    };
    let sprd = o.spread.unwrap_or(1.0);

    let p_a = &cycle[k];
    let p_b = &cycle[(k + 1) % k_len];

    // Fixed-size stack scratch — no per-frame heap traffic.
    let mut pts = [(0.0f32, 0.0f32); M_SAMPLES];
    let mut lengths = [0.0f32; M_SAMPLES];
    let inv_samples = 1.0 / M_SAMPLES as f32;
    for (i, p) in pts.iter_mut().enumerate() {
        let f = i as f32 * inv_samples;
        let a = p_a.at(f);
        let b = p_b.at(f);
        *p = (
            (a.0 + (b.0 - a.0) * m) * sprd,
            (a.1 + (b.1 - a.1) * m) * sprd,
        );
    }
    let mut total = 0.0f32;
    for i in 0..M_SAMPLES {
        let a = pts[i];
        let b = pts[(i + 1) % M_SAMPLES];
        let l = (b.0 - a.0).hypot(b.1 - a.1);
        lengths[i] = l;
        total += l;
    }

    let n = morph_n(o.icon_d.unwrap_or(1.0));
    let re = o.r_dot.unwrap_or(0.021) * 1.35 * sprd;
    let pulse = 1.0 + 0.018 * (t * 2.35).sin();

    let dots = &mut out.dots;
    dots.reserve(n);
    let c2 = size / 2.0;
    let radius = (re * size).max(0.35);
    let mut seg = 0usize;
    let mut acc = 0.0f32;
    let inv_n = 1.0 / n as f32;
    for k2 in 0..n {
        let target = k2 as f32 * inv_n * total;
        // Bounds check first: `seg` can never leave `0..M_SAMPLES`.
        while seg < M_SAMPLES - 1 && acc + lengths[seg] < target {
            acc += lengths[seg];
            seg += 1;
        }
        let a = pts[seg];
        let b = pts[(seg + 1) % M_SAMPLES];
        let f = if lengths[seg] > 0.0 {
            ((target - acc) / lengths[seg]).min(1.0)
        } else {
            0.0
        };
        let x = (a.0 + (b.0 - a.0) * f) * pulse;
        let y = (a.1 + (b.1 - a.1) * f) * pulse;
        dots.push(Dot::new(c2 + x * size, c2 + y * size, 0.0, radius, 0.1));
    }
}
