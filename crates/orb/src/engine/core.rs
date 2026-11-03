// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Shared primitives for the dotted 3D thought-orbs.
//! Ported from thinking-orbs / inkform (HalftoneSphere lineage).

use std::{cell::RefCell, f32::consts::PI};

type Vec3 = [f32; 3];
type Vec2 = [f32; 2];

thread_local! {
    /// Per-thread topology caches, indexed directly by sample count. Public
    /// draw inputs are capped before reaching these helpers, so the sparse
    /// vectors stay small while lookup remains O(1).
    static FIBONACCI_DIRECTIONS: RefCell<Vec<Option<Box<[Vec3]>>>> = const { RefCell::new(Vec::new()) };
    static UNIT_CIRCLES: RefCell<Vec<Option<Box<[Vec2]>>>> = const { RefCell::new(Vec::new()) };
}

/// A projected, depth-shaded disk.
#[derive(Clone, Copy, Debug)]
pub struct Dot {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub r: f32,
    /// Ink value: 0 = darkest ink on paper. Mirrored on dark themes.
    pub white: f32,
    pub a: f32,
}

impl Dot {
    pub fn new(x: f32, y: f32, z: f32, r: f32, white: f32) -> Self {
        Self {
            x,
            y,
            z,
            r,
            white,
            a: 1.0,
        }
    }

    pub fn with_a(mut self, a: f32) -> Self {
        self.a = a;
        self
    }
}

/// A stroked edge between two projected points (the `connecting` web).
#[derive(Clone, Copy, Debug)]
pub struct Line {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    pub white: f32,
    pub a: f32,
    pub w: f32,
}

/// Frame geometry produced by a mode painter (backend-agnostic).
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub dots: Vec<Dot>,
    pub lines: Vec<Line>,
}

impl Frame {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop all geometry but keep the allocated capacity, so a steady-state
    /// animation loop reuses one pair of buffers forever.
    #[inline]
    pub fn clear(&mut self) {
        self.dots.clear();
        self.lines.clear();
    }
}

/// Spin + tilt + orthographic projection, precomputed once per frame.
///
/// This used to be a `Box<dyn Fn>`, which meant a heap allocation on every
/// `draw_mode` call and an indirect call per dot. As a plain struct it is
/// stack-allocated and `project` inlines into the dot loops.
#[derive(Clone, Copy, Debug)]
pub struct Proj {
    st: f32,
    ct: f32,
    sy: f32,
    cyw: f32,
    cx: f32,
    cy: f32,
    scale: f32,
}

impl Proj {
    #[inline]
    pub fn project(&self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        let x1 = x * self.cyw + z * self.sy;
        let z1 = -x * self.sy + z * self.cyw;
        let y1 = y * self.ct - z1 * self.st;
        let z2 = y * self.st + z1 * self.ct;
        (self.cx + x1 * self.scale, self.cy - y1 * self.scale, z2)
    }
}

#[inline]
pub fn lerp(a: f32, b: f32, f: f32) -> f32 {
    a + (b - a) * f
}

#[inline]
pub fn frac(x: f32) -> f32 {
    x - x.floor()
}

/// Deterministic hash in [0, 1).
///
/// The magic numbers are the canonical GLSL `fract(sin(dot(...)) * 43758.5453)`
/// constants, kept verbatim from upstream so the generated dot layouts match.
/// They carry more digits than `f32` can hold; that is intentional — they are
/// quoted as written, and truncating them would be a gratuitous divergence.
#[inline]
#[allow(clippy::excessive_precision)]
pub fn hash_d(a: f32, b: f32) -> f32 {
    let h = (a * 12.9898 + b * 78.233).sin() * 43758.5453;
    h - h.floor()
}

/// Value noise on a 2D lattice — smooth, deterministic, cheap.
pub fn vnoise(x: f32, y: f32) -> f32 {
    let xi = x.floor();
    let yi = y.floor();
    let mut fx = x - xi;
    let mut fy = y - yi;
    fx = fx * fx * (3.0 - 2.0 * fx);
    fy = fy * fy * (3.0 - 2.0 * fy);
    let a = hash_d(xi, yi);
    let b = hash_d(xi + 1.0, yi);
    let c = hash_d(xi, yi + 1.0);
    let d = hash_d(xi + 1.0, yi + 1.0);
    a + (b - a) * fx + (c - a) * fy + (a - b - c + d) * fx * fy
}

/// Stable directions on a unit sphere (Fibonacci lattice).
pub fn fib_dir(i: usize, n: usize) -> (f32, f32, f32) {
    let golden = PI * (3.0 - 5.0_f32.sqrt());
    let y = 1.0 - (2.0 * (i as f32 + 0.5)) / n as f32;
    let rad = (1.0 - y * y).sqrt();
    let a = i as f32 * golden;
    (rad * a.cos(), y, rad * a.sin())
}

/// Borrow the stable Fibonacci sphere for `n` points from a thread-local cache.
/// Profiles reuse the same handful of sanitized counts every frame, so their
/// square roots and trigonometry are normally paid only once.
pub fn with_fib_dirs<R>(n: usize, f: impl FnOnce(&[Vec3]) -> R) -> R {
    FIBONACCI_DIRECTIONS.with(|cell| {
        let mut cache = cell.borrow_mut();
        if cache.len() <= n {
            cache.resize_with(n + 1, || None);
        }
        let directions = cache[n].get_or_insert_with(|| {
            (0..n)
                .map(|i| {
                    let (x, y, z) = fib_dir(i, n);
                    [x, y, z]
                })
                .collect::<Vec<_>>()
                .into_boxed_slice()
        });
        f(directions)
    })
}

/// Borrow evenly spaced `(cos θ, sin θ)` samples for a closed circle.
/// Circle topology occurs in almost every mode and is invariant across frames.
pub fn with_unit_circle<R>(n: usize, f: impl FnOnce(&[Vec2]) -> R) -> R {
    UNIT_CIRCLES.with(|cell| {
        let mut cache = cell.borrow_mut();
        if cache.len() <= n {
            cache.resize_with(n + 1, || None);
        }
        let circle = cache[n].get_or_insert_with(|| {
            let step = 2.0 * PI / n.max(1) as f32;
            (0..n)
                .map(|i| {
                    let a = i as f32 * step;
                    [a.cos(), a.sin()]
                })
                .collect::<Vec<_>>()
                .into_boxed_slice()
        });
        f(circle)
    })
}

/// Shortest signed angular distance, wrapped to (-π, π].
#[inline]
pub fn angle_delta(a: f32, b: f32) -> f32 {
    (a - b).sin().atan2((a - b).cos())
}

/// Shared spin + tilt + orthographic projection.
pub fn make_proj(yaw: f32, tilt: f32, cx: f32, cy: f32, scale: f32) -> Proj {
    Proj {
        st: tilt.sin(),
        ct: tilt.cos(),
        sy: yaw.sin(),
        cyw: yaw.cos(),
        cx,
        cy,
        scale,
    }
}

/// Dot radii were tuned for a 300pt frame; sub-linear scaling keeps small
/// spinners legible. Lower pow = radii shrink less with size.
#[inline]
pub fn radius_scale(size: f32, pow: f32) -> f32 {
    (size / 300.0).powf(pow)
}

/// Z-sort far→near so nearer dots paint over farther ones.
///
/// `sort_unstable_by` is used deliberately: it sorts in place (the stable sort
/// allocates a scratch buffer every call) and ties only happen between dots at
/// identical depth, where paint order is visually irrelevant.
pub fn sort_dots(dots: &mut [Dot]) {
    dots.sort_unstable_by(|a, b| a.z.partial_cmp(&b.z).unwrap_or(std::cmp::Ordering::Equal));
}
