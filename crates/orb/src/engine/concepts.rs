// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Original low-density orb concepts: focus, gyroscope, and memory echoes.
//!
//! These modes deliberately reuse the engine's existing count/radius knobs and
//! emit dots only, keeping them on the cheapest GPUI paint path.

use crate::engine::{
    core::{Dot, Frame, frac, make_proj, radius_scale, with_unit_circle},
    profiles::{MAX_LANES, MAX_PARTICLES, MAX_SEGS, ModeOpts, count_usize},
};
use std::f32::consts::PI;

/// Iris-like streams converge on a small, steady focal core.
pub fn draw_focus_into(size: f32, t: f32, o: &ModeOpts, out: &mut Frame) {
    let center = size / 2.0;
    let extent = size * 0.38 * o.spread.unwrap_or(1.0);
    let lanes = count_usize(o.lanes, 6.0, 1, MAX_LANES as usize);
    let segs = count_usize(o.segs, 12.0, 1, MAX_SEGS as usize);
    let core_n = count_usize(o.particles, 5.0, 1, MAX_PARTICLES as usize);
    let rs = radius_scale(size, o.rs_pow.unwrap_or(0.6));
    let r_base = o.r_base.unwrap_or(0.9);
    let r_depth = o.r_depth.unwrap_or(1.25);
    let total = lanes.saturating_mul(segs).saturating_add(core_n);
    out.dots.reserve(total);

    // Close and reopen a six-blade aperture without collapsing the samples
    // into a star. Every blade keeps a continuous curved silhouette; only its
    // inner edge moves, so the concept remains readable in a frozen frame.
    let openness = 0.5 + 0.5 * (t * 0.9).sin();
    let inner = 0.12 + 0.2 * openness;
    let rotation = t * 0.11;
    let inv_segs = 1.0 / segs as f32;
    with_unit_circle(lanes, |directions| {
        for (lane, direction) in directions.iter().enumerate() {
            for i in 0..segs {
                let u = (i as f32 + 0.5) * inv_segs;
                let radial = inner + (1.0 - inner) * u;
                let curl = (0.88 - 0.24 * openness) * (1.0 - u).powf(1.25);
                let twist = rotation + curl;
                let (st, ct) = twist.sin_cos();
                let dx = direction[0] * ct - direction[1] * st;
                let dy = direction[0] * st + direction[1] * ct;
                let inner_weight = 1.0 - u;
                let shimmer = 0.96 + 0.05 * (t * 1.4 + lane as f32 * 0.8).sin();
                out.dots.push(
                    Dot::new(
                        center + dx * extent * radial,
                        center + dy * extent * radial,
                        0.0,
                        (r_base + r_depth * inner_weight) * shimmer * rs,
                        0.12 + 0.52 * u,
                    )
                    .with_a(0.48 + 0.52 * (PI * u).sin()),
                );
            }
        }
    });

    // A tiny breathing nucleus makes the destination legible at inline size.
    let core_radius = size * (0.035 + 0.004 * (t * 1.5).sin());
    with_unit_circle(core_n, |circle| {
        for p in circle {
            out.dots.push(Dot::new(
                center + p[0] * core_radius,
                center + p[1] * core_radius,
                0.0,
                (r_base + r_depth * 1.15) * rs,
                0.08,
            ));
        }
    });
}

/// Three inference loops carry thoughts in alternating directions.
pub fn draw_gyroscope_into(size: f32, t: f32, o: &ModeOpts, out: &mut Frame) {
    let center = size / 2.0;
    let radius = size * 0.385 * o.spread.unwrap_or(1.0);
    let rings = count_usize(o.lanes, 3.0, 1, MAX_LANES as usize).min(6);
    let segs = count_usize(o.segs, 24.0, 3, MAX_SEGS as usize);
    let rs = radius_scale(size, o.rs_pow.unwrap_or(0.6));
    let r_base = o.r_base.unwrap_or(0.8);
    let r_depth = o.r_depth.unwrap_or(1.5);
    let proj = make_proj(t * 0.055, 0.18, center, center, radius);
    out.dots
        .reserve(rings.saturating_mul(segs.saturating_add(2)));

    with_unit_circle(segs, |circle| {
        for ring in 0..rings {
            let spread = if rings > 1 {
                ring as f32 / (rings - 1) as f32 - 0.5
            } else {
                0.0
            };
            let tilt_x = spread * 1.55 + 0.12 * (t * 0.16 + ring as f32).sin();
            let tilt_y = ring as f32 * PI / rings as f32 + 0.32;
            let (sx, cx) = tilt_x.sin_cos();
            let (sy, cy) = tilt_y.sin_cos();
            let direction = if ring % 2 == 0 { 1.0 } else { -1.0 };
            let orient = |ca: f32, sa: f32| {
                let y1 = sa * cx;
                let z1 = sa * sx;
                (ca * cy + z1 * sy, y1, -ca * sy + z1 * cy)
            };

            for p in circle {
                let (x, y, z) = orient(p[0], p[1]);
                let (px, py, depth_z) = proj.project(x, y, z);
                let depth = (depth_z + 1.0) * 0.5;
                out.dots.push(
                    Dot::new(
                        px,
                        py,
                        depth_z,
                        (r_base * 0.9 + r_depth * 0.5 * depth) * rs,
                        0.62 - 0.34 * depth,
                    )
                    .with_a(0.34 + 0.4 * depth),
                );
            }

            // A leading thought and a softer echo make direction and velocity
            // legible without turning the complete track into visual noise.
            let travel =
                t * (0.72 + ring as f32 * 0.08) * direction + ring as f32 * 2.0 * PI / rings as f32;
            for (trail, offset) in [0.0, -0.22 * direction].into_iter().enumerate() {
                let (sa, ca) = (travel + offset).sin_cos();
                let (x, y, z) = orient(ca, sa);
                let (px, py, depth_z) = proj.project(x, y, z);
                let depth = (depth_z + 1.0) * 0.5;
                let strength = if trail == 0 { 1.0 } else { 0.48 };
                out.dots.push(
                    Dot::new(
                        px,
                        py,
                        depth_z + 0.002,
                        (r_base + r_depth * depth + 1.65 * strength) * rs,
                        0.44 - 0.42 * depth - 0.2 * strength,
                    )
                    .with_a((0.55 + 0.45 * depth) * strength),
                );
            }
        }
    });
}

/// Concentric echoes emerge from a stable core and dissolve at the boundary.
pub fn draw_echo_into(size: f32, t: f32, o: &ModeOpts, out: &mut Frame) {
    let center = size / 2.0;
    let extent = size * 0.4 * o.spread.unwrap_or(1.0);
    let rings = count_usize(o.lanes, 4.0, 1, MAX_LANES as usize);
    let segs = count_usize(o.segs, 18.0, 3, MAX_SEGS as usize);
    let core_n = count_usize(o.particles, 3.0, 1, MAX_PARTICLES as usize);
    let rs = radius_scale(size, o.rs_pow.unwrap_or(0.6));
    let r_base = o.r_base.unwrap_or(0.85);
    let r_depth = o.r_depth.unwrap_or(1.05);
    out.dots
        .reserve(rings.saturating_mul(segs).saturating_add(core_n));

    with_unit_circle(segs, |circle| {
        for ring in 0..rings {
            let phase = frac(t * 0.14 + ring as f32 / rings as f32);
            let radius = extent * (0.13 + 0.87 * phase);
            let life = (PI * phase).sin().max(0.0);
            let turn = ring as f32 * 0.37 - t * 0.1;
            let (st, ct) = turn.sin_cos();
            for p in circle {
                let x = p[0] * ct - p[1] * st;
                let y = p[0] * st + p[1] * ct;
                out.dots.push(
                    Dot::new(
                        center + x * radius,
                        center + y * radius,
                        0.0,
                        (r_base + r_depth * (1.0 - phase)) * rs,
                        0.18 + 0.52 * phase,
                    )
                    .with_a(life.powf(0.58)),
                );
            }
        }
    });

    let core_radius = size * 0.025;
    with_unit_circle(core_n, |circle| {
        for p in circle {
            out.dots.push(Dot::new(
                center + p[0] * core_radius,
                center + p[1] * core_radius,
                0.0,
                (r_base + r_depth * 1.4) * rs,
                0.06,
            ));
        }
    });
}
