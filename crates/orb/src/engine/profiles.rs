// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Density profiles + the multiplier machinery that scales them.

/// Hard ceilings for count-like knobs. Preset values sit well below these;
/// they only bite adversarial or mistaken power-user inputs.
pub const MAX_LATTICE_RINGS: f32 = 128.0;
pub const MAX_LON_DENSITY: f32 = 256.0;
pub const MAX_ORBIT_N: f32 = 64.0;
pub const MAX_GHOST_N: f32 = 512.0;
pub const MAX_PARTICLES: f32 = 16.0;
pub const MAX_NODE_N: f32 = 128.0;
pub const MAX_SIGNALS: f32 = 64.0;
pub const MAX_STRAND_N: f32 = 256.0;
pub const MAX_LANES: f32 = 32.0;
pub const MAX_SEGS: f32 = 512.0;
pub const MAX_MOVE_COUNT: f32 = 64.0;
pub const MAX_ICON_D: f32 = 8.0;
/// Max dots the Morph painter will emit after `icon_d` is applied.
pub const MAX_MORPH_DOTS: usize = 512;
/// Logical size (px) accepted by [`crate::engine::draw_mode_into`].
pub const MIN_SIZE: f32 = 1.0;
pub const MAX_SIZE: f32 = 1024.0;

/// Free-form numeric knobs for mode painters (mirrors upstream `ModeOpts`).
///
/// **Power-user / advanced API.** The normal path is the host widget /
/// [`crate::resolve_preset`], which only ever pass hand-tuned finite values.
///
/// If you build a [`ModeOpts`] yourself and pass it to
/// [`crate::engine::draw_mode`] / [`crate::engine::draw_mode_into`], every count-like field is
/// clamped by [`sanitize_mode_opts`] before geometry runs:
///
/// | Field family | Range after sanitize |
/// |--------------|----------------------|
/// | lattice rings / density | 1…128 / 1…256 |
/// | `orbit_n`, `ghost_n`, `particles` | 0…64 / 0…512 / 0…16 |
/// | `node_n`, `signals` | 0…128 / 0…64 |
/// | `strand_n`, `lanes`, `segs` | 0…256 / 1…32 / 1…512 |
/// | `move_count` | 0…64 |
/// | `icon_d` | 0.02…8 |
///
/// Non-finite values fall back to the field default used by the painter.
/// New fields may be added without a major version bump.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct ModeOpts {
    // lattice
    pub lat_rings: Option<f32>,
    pub lon_density: Option<f32>,
    pub rings: Option<f32>,
    pub r_base: Option<f32>,
    pub r_depth: Option<f32>,
    pub r_boost: Option<f32>,
    pub r_active: Option<f32>,
    pub ink_far: Option<f32>,
    pub ink_span: Option<f32>,
    pub rs_pow: Option<f32>,
    pub r_min: Option<f32>,
    pub move_count: Option<f32>,
    pub scan_mul: Option<f32>,
    pub dim_base: Option<f32>,
    // orbits
    pub orbit_n: Option<f32>,
    pub ghost_n: Option<f32>,
    pub ghost_r: Option<f32>,
    pub ghost_a: Option<f32>,
    pub particles: Option<f32>,
    pub part_r: Option<f32>,
    pub part_r_depth: Option<f32>,
    // web
    pub node_n: Option<f32>,
    pub thr: Option<f32>,
    pub signals: Option<f32>,
    pub node_r: Option<f32>,
    pub node_r_depth: Option<f32>,
    pub line_w: Option<f32>,
    // braid
    pub strand_n: Option<f32>,
    pub turns: Option<f32>,
    // ribbon / ring
    pub lanes: Option<f32>,
    pub segs: Option<f32>,
    pub face_on: Option<f32>,
    pub spin: Option<f32>,
    pub band_mul: Option<f32>,
    pub wob_mul: Option<f32>,
    // morph
    pub r_dot: Option<f32>,
    pub icon_d: Option<f32>,
    pub spread: Option<f32>,
    pub r_size_mul: Option<f32>,
}

/// Clamp a free-form float option into a finite range, or leave `None`.
fn clamp_opt(v: Option<f32>, min: f32, max: f32) -> Option<f32> {
    v.and_then(|x| {
        if x.is_finite() {
            Some(x.clamp(min, max))
        } else {
            None
        }
    })
}

/// Return a safe copy of `opts` with every count-like / radius-like knob
/// finite and within the hard ceilings above.
///
/// Called automatically by [`crate::engine::draw_mode_into`]. Safe to call yourself
/// when composing custom profiles.
pub fn sanitize_mode_opts(opts: &ModeOpts) -> ModeOpts {
    ModeOpts {
        lat_rings: clamp_opt(opts.lat_rings, 1.0, MAX_LATTICE_RINGS),
        lon_density: clamp_opt(opts.lon_density, 1.0, MAX_LON_DENSITY),
        rings: clamp_opt(opts.rings, 1.0, MAX_LATTICE_RINGS),
        r_base: clamp_opt(opts.r_base, 0.0, 64.0),
        r_depth: clamp_opt(opts.r_depth, 0.0, 64.0),
        r_boost: clamp_opt(opts.r_boost, 0.0, 64.0),
        r_active: clamp_opt(opts.r_active, 0.0, 64.0),
        ink_far: clamp_opt(opts.ink_far, 0.0, 1.0),
        ink_span: clamp_opt(opts.ink_span, 0.0, 2.0),
        rs_pow: clamp_opt(opts.rs_pow, 0.05, 4.0),
        r_min: clamp_opt(opts.r_min, 0.0, 32.0),
        move_count: clamp_opt(opts.move_count, 0.0, MAX_MOVE_COUNT),
        scan_mul: clamp_opt(opts.scan_mul, 0.0, 16.0),
        dim_base: clamp_opt(opts.dim_base, 0.0, 1.0),
        orbit_n: clamp_opt(opts.orbit_n, 0.0, MAX_ORBIT_N),
        ghost_n: clamp_opt(opts.ghost_n, 0.0, MAX_GHOST_N),
        ghost_r: clamp_opt(opts.ghost_r, 0.0, 64.0),
        ghost_a: clamp_opt(opts.ghost_a, 0.0, 1.0),
        particles: clamp_opt(opts.particles, 0.0, MAX_PARTICLES),
        part_r: clamp_opt(opts.part_r, 0.0, 64.0),
        part_r_depth: clamp_opt(opts.part_r_depth, 0.0, 64.0),
        node_n: clamp_opt(opts.node_n, 0.0, MAX_NODE_N),
        thr: clamp_opt(opts.thr, 0.0, 4.0),
        signals: clamp_opt(opts.signals, 0.0, MAX_SIGNALS),
        node_r: clamp_opt(opts.node_r, 0.0, 64.0),
        node_r_depth: clamp_opt(opts.node_r_depth, 0.0, 64.0),
        line_w: clamp_opt(opts.line_w, 0.0, 32.0),
        strand_n: clamp_opt(opts.strand_n, 0.0, MAX_STRAND_N),
        turns: clamp_opt(opts.turns, 0.0, 32.0),
        lanes: clamp_opt(opts.lanes, 1.0, MAX_LANES),
        segs: clamp_opt(opts.segs, 1.0, MAX_SEGS),
        face_on: clamp_opt(opts.face_on, 0.0, 1.0),
        spin: clamp_opt(opts.spin, -16.0, 16.0),
        band_mul: clamp_opt(opts.band_mul, 0.05, 8.0),
        wob_mul: clamp_opt(opts.wob_mul, 0.0, 8.0),
        r_dot: clamp_opt(opts.r_dot, 0.0, 1.0),
        icon_d: clamp_opt(opts.icon_d, 0.02, MAX_ICON_D),
        spread: clamp_opt(opts.spread, 0.05, 4.0),
        r_size_mul: clamp_opt(opts.r_size_mul, 0.0, 64.0),
    }
}

/// Clamp the logical paint size in pixels.
pub fn sanitize_size(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(MIN_SIZE, MAX_SIZE)
    } else {
        64.0
    }
}

/// Interpret a count-like option as `usize` after sanitization has run.
///
/// Prefer calling this on already-sanitized opts. Non-finite / missing use
/// `default`; the result is always in `min..=max`.
#[inline]
pub fn count_usize(v: Option<f32>, default: f32, min: usize, max: usize) -> usize {
    let x = match v {
        Some(x) if x.is_finite() => x,
        _ => default,
    };
    let lo = min as f32;
    let hi = max as f32;
    (x.round().clamp(lo, hi)) as usize
}

/// Scale total dot density. 2-D lattices (rings × dots-per-ring) take √scale
/// each side so the TOTAL scales by `scale`. Flat lists scale linearly.
pub fn scale_counts(opts: &ModeOpts, scale: f32) -> ModeOpts {
    let mut out = opts.clone();
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    let rt = scale.sqrt();

    // Upstream COUNT_PAIRS: latRings×lonDensity first (owns lonDensity),
    // then rings×lonDensity only if latRings was absent.
    if let (Some(va), Some(vb)) = (out.lat_rings, out.lon_density) {
        out.lat_rings = Some((va * rt).round().clamp(2.0, MAX_LATTICE_RINGS));
        out.lon_density = Some((vb * rt).round().clamp(2.0, MAX_LON_DENSITY));
    } else if let (Some(va), Some(vb)) = (out.rings, out.lon_density) {
        out.rings = Some((va * rt).round().clamp(2.0, MAX_LATTICE_RINGS));
        out.lon_density = Some((vb * rt).round().clamp(2.0, MAX_LON_DENSITY));
    }
    // lanes × segs pair
    if let (Some(va), Some(vb)) = (out.lanes, out.segs) {
        out.lanes = Some((va * rt).round().clamp(2.0, MAX_LANES));
        out.segs = Some((vb * rt).round().clamp(2.0, MAX_SEGS));
    }

    // linear count keys (0 means opt-out — do not resurrect)
    let scale_key = |v: Option<f32>, max: f32| match v {
        Some(0.0) => Some(0.0),
        Some(v) if v.is_finite() => Some((v * scale).round().clamp(1.0, max)),
        Some(_) => None,
        None => None,
    };
    out.orbit_n = scale_key(out.orbit_n, MAX_ORBIT_N);
    out.ghost_n = scale_key(out.ghost_n, MAX_GHOST_N);
    out.node_n = scale_key(out.node_n, MAX_NODE_N);
    out.strand_n = scale_key(out.strand_n, MAX_STRAND_N);
    out.signals = scale_key(out.signals, MAX_SIGNALS);

    if let Some(v) = out.icon_d {
        if v.is_finite() {
            out.icon_d = Some((v * scale).clamp(0.02, MAX_ICON_D));
        } else {
            out.icon_d = None;
        }
    }
    out
}

/// Scale every key that sets a dot's rendered radius.
pub fn scale_radii(opts: &ModeOpts, scale: f32) -> ModeOpts {
    let mut out = opts.clone();
    let scale = if scale.is_finite() { scale } else { 1.0 };
    let mul = |v: Option<f32>| v.and_then(|x| x.is_finite().then_some(x * scale));
    out.r_base = mul(out.r_base);
    out.r_depth = mul(out.r_depth);
    out.r_active = mul(out.r_active);
    out.r_dot = mul(out.r_dot);
    out.ghost_r = mul(out.ghost_r);
    out.part_r = mul(out.part_r);
    out.part_r_depth = mul(out.part_r_depth);
    out.node_r = mul(out.node_r);
    out.node_r_depth = mul(out.node_r_depth);
    out.r_size_mul = Some(out.r_size_mul.unwrap_or(1.0) * scale);
    out
}

impl ModeOpts {
    /// The base profile for `mode`, then hand the mutable options over for the
    /// power-user tweaks a preset cannot express. The struct is
    /// `#[non_exhaustive]`, so this is the only way to build one outside the
    /// crate.
    pub fn fill(mode: crate::types::ModeKey, f: impl FnOnce(&mut Self)) -> Self {
        let mut opts = base_profile(mode);
        f(&mut opts);
        opts
    }
}

/// Base (fine) profiles per mode, before preset multipliers.
pub fn base_profile(mode: crate::types::ModeKey) -> ModeOpts {
    use crate::types::ModeKey::*;
    match mode {
        Globe => ModeOpts {
            lat_rings: Some(17.0),
            lon_density: Some(44.0),
            r_base: Some(0.6),
            r_depth: Some(1.7),
            r_boost: Some(1.0),
            ink_far: Some(0.62),
            ink_span: Some(0.54),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Orbits => ModeOpts {
            orbit_n: Some(12.0),
            ghost_n: Some(40.0),
            ghost_r: Some(0.9),
            ghost_a: Some(0.5),
            particles: Some(3.0),
            part_r: Some(1.2),
            part_r_depth: Some(1.6),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Rubik => ModeOpts {
            lat_rings: Some(15.0),
            lon_density: Some(40.0),
            move_count: Some(14.0),
            r_base: Some(0.6),
            r_depth: Some(1.7),
            r_active: Some(0.3),
            ink_far: Some(0.62),
            ink_span: Some(0.54),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Wave => ModeOpts {
            rings: Some(15.0),
            lon_density: Some(40.0),
            r_base: Some(0.6),
            r_depth: Some(1.7),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Web => ModeOpts {
            node_n: Some(30.0),
            thr: Some(0.72),
            signals: Some(5.0),
            node_r: Some(1.4),
            node_r_depth: Some(1.8),
            line_w: Some(0.8),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Braid => ModeOpts {
            strand_n: Some(52.0),
            turns: Some(3.0),
            ghost_n: Some(150.0),
            r_base: Some(1.2),
            r_depth: Some(1.8),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Ribbon => ModeOpts {
            lanes: Some(5.0),
            segs: Some(88.0),
            ghost_n: Some(150.0),
            r_base: Some(1.1),
            r_depth: Some(1.7),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Ring => ModeOpts {
            lanes: Some(5.0),
            segs: Some(88.0),
            ghost_n: Some(0.0),
            face_on: Some(1.0),
            r_base: Some(1.1),
            r_depth: Some(1.7),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Morph => ModeOpts {
            r_dot: Some(0.021),
            icon_d: Some(1.0),
            r_min: Some(0.25),
            ..Default::default()
        },
        Focus => ModeOpts {
            lanes: Some(6.0),
            segs: Some(12.0),
            particles: Some(5.0),
            r_base: Some(0.9),
            r_depth: Some(1.25),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Gyroscope => ModeOpts {
            lanes: Some(3.0),
            segs: Some(28.0),
            r_base: Some(0.8),
            r_depth: Some(1.5),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
        Echo => ModeOpts {
            lanes: Some(4.0),
            segs: Some(18.0),
            particles: Some(3.0),
            r_base: Some(0.85),
            r_depth: Some(1.05),
            rs_pow: Some(0.6),
            r_min: Some(0.3),
            ..Default::default()
        },
    }
}
