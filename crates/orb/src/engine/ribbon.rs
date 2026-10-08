// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Ribbon / ring: undulating sash or face-on breathing ring.

use crate::engine::{
    core::{Dot, Frame, make_proj, radius_scale, with_fib_dirs},
    profiles::{MAX_GHOST_N, MAX_LANES, MAX_SEGS, ModeOpts, count_usize},
};
use std::f32::consts::PI;

pub fn draw_ribbon_into(size: f32, t: f32, o: &ModeOpts, out: &mut Frame) {
    let cx = size / 2.0;
    let cy = size / 2.0;
    let r = (size / 2.0) * 0.78;
    let spin = o.spin.unwrap_or(1.0);
    let cam_tilt = 0.3;
    let pt = make_proj(t * 0.1 * spin, cam_tilt, cx, cy, 1.0);
    let rs = radius_scale(size, o.rs_pow.unwrap_or(0.6));
    let face_on = o.face_on.unwrap_or(0.0) != 0.0;

    let dots = &mut out.dots;
    let ghost_n = count_usize(o.ghost_n, 150.0, 0, MAX_GHOST_N as usize);
    let inv_r = 1.0 / r;
    dots.reserve(ghost_n);
    with_fib_dirs(ghost_n, |directions| {
        for d in directions {
            let (px, py, z) = pt.project(d[0] * r, d[1] * r, d[2] * r);
            let depth = (z * inv_r + 1.0) / 2.0;
            dots.push(Dot::new(px, py, z, 0.8 * rs, 0.78).with_a(0.1 + 0.22 * depth));
        }
    });

    let ya = t * 0.24 * spin;
    let ta = if face_on {
        -cam_tilt
    } else {
        0.55 + 0.3 * (t * 0.18).sin() * spin
    };
    let ux = ya.cos();
    let uy = 0.0;
    let uz = ya.sin();
    let (sta, cta) = (ta.sin(), ta.cos());
    let vx = -uz * sta;
    let vy = cta;
    let vz = ux * sta;
    // plane normal n = u × v
    let nx = uy * vz - uz * vy;
    let ny = uz * vx - ux * vz;
    let nz = ux * vy - uy * vx;

    let wob_mul = o.wob_mul.unwrap_or(1.0);
    let wob_amp = 0.23 * wob_mul;
    let base_r = if face_on {
        r / (1.0 + 0.85 * wob_amp)
    } else {
        r
    };

    let base_lanes = o.lanes.unwrap_or(5.0).clamp(1.0, MAX_LANES);
    let segs = count_usize(o.segs, 88.0, 1, MAX_SEGS as usize);
    let lanes = (base_lanes * o.band_mul.unwrap_or(1.0))
        .round()
        .clamp(1.0, MAX_LANES) as usize;
    let r_base = o.r_base.unwrap_or(1.1);
    let r_depth = o.r_depth.unwrap_or(1.7);
    let seg_step = 2.0 * PI / segs as f32;
    let half = (lanes as f32 - 1.0) / 2.0;
    let inv_half = 1.0 / half.max(1.0);
    dots.reserve(lanes.saturating_mul(segs));

    for w in 0..lanes {
        let centered = w as f32 - half;
        let lane_off = centered * 0.075;
        let edge = centered.abs() * inv_half;
        let edge_r = 1.0 - 0.25 * edge;
        let edge_ink = 0.18 * edge;
        for k in 0..segs {
            let a = k as f32 * seg_step;
            let (ca, sa) = (a.cos(), a.sin());
            let breath = if face_on {
                0.72 + 0.28 * (t * 0.48).sin()
            } else {
                1.0
            };
            let wob = (0.16 * (a * 3.0 - t * 1.7 + w as f32 * 0.22).sin()
                + 0.07 * (a * 5.0 + t * 1.1).sin())
                * wob_mul
                * breath;
            let radial = if face_on { 1.0 + wob } else { 1.0 };
            let off = if face_on { lane_off } else { lane_off + wob };
            let x = ux * ca + vx * sa + nx * off;
            let y = uy * ca + vy * sa + ny * off;
            let z = uz * ca + vz * sa + nz * off;
            let l = (x * x + y * y + z * z).sqrt();
            let inv_l = if l > 1e-6 { 1.0 / l } else { 0.0 };
            let rr = base_r * radial * inv_l;
            let (px, py, zr) = pt.project(x * rr, y * rr, z * rr);
            let depth = (zr * inv_r + 1.0) / 2.0;
            dots.push(
                Dot::new(
                    px,
                    py,
                    zr,
                    (r_base + r_depth * depth) * edge_r * rs,
                    0.52 - 0.44 * depth + edge_ink,
                )
                .with_a(0.4 + 0.6 * depth),
            );
        }
    }
}
