// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Orbits: particles on tilted orbits — the "working" state.

use crate::engine::{
    core::{Dot, Frame, hash_d, make_proj, radius_scale, with_unit_circle},
    profiles::{MAX_GHOST_N, MAX_ORBIT_N, MAX_PARTICLES, ModeOpts, count_usize},
};
use std::f32::consts::PI;

pub fn draw_orbits_into(size: f32, t: f32, o: &ModeOpts, out: &mut Frame) {
    let cx = size / 2.0;
    let cy = size / 2.0;
    let r = (size / 2.0) * 0.82;
    let pt = make_proj(t * 0.12, 0.3, cx, cy, 1.0);
    let rs = radius_scale(size, o.rs_pow.unwrap_or(0.6));

    let orbit_n = count_usize(o.orbit_n, 12.0, 0, MAX_ORBIT_N as usize);
    let ghost_n = count_usize(o.ghost_n, 40.0, 0, MAX_GHOST_N as usize);
    let particles = count_usize(o.particles, 3.0, 0, MAX_PARTICLES as usize);

    let dots = &mut out.dots;
    let per_orbit = ghost_n.saturating_add(particles);
    dots.reserve(orbit_n.saturating_mul(per_orbit));
    let ghost_r = o.ghost_r.unwrap_or(0.9) * rs;
    let ghost_a = o.ghost_a.unwrap_or(0.5);
    let part_r = o.part_r.unwrap_or(1.2);
    let part_r_depth = o.part_r_depth.unwrap_or(1.6);

    for orb in 0..orbit_n {
        let orb_f = orb as f32;
        let h1 = hash_d(orb_f, 1.7);
        let h2 = hash_d(orb_f, 5.2);
        let h3 = hash_d(orb_f, 8.9);
        let ro = r * (0.45 + 0.52 * h1);
        let th = h1 * 2.0 * PI;
        let phi = (2.0 * h2 - 1.0).acos();
        // Build a stable plane basis by crossing the normal with whichever
        // world axis is least parallel to it.
        let nx = phi.sin() * th.cos();
        let ny = phi.cos();
        let nz = phi.sin() * th.sin();
        let (mut ux, mut uy, mut uz) = if nz.abs() < 0.9 {
            (-ny, nx, 0.0)
        } else {
            (0.0, -nz, ny)
        };
        let ul = (ux * ux + uy * uy + uz * uz).sqrt().max(1e-6);
        ux /= ul;
        uy /= ul;
        uz /= ul;
        let vx = ny * uz - nz * uy;
        let vy = nz * ux - nx * uz;
        let vz = nx * uy - ny * ux;
        let speed = (0.25 + 0.55 * h3) * if h3 > 0.5 { 1.0 } else { -1.0 };
        let inv_ro = 1.0 / ro;

        // Ghost topology is invariant; reuse its unit-circle samples instead
        // of evaluating 2 × orbit_n × ghost_n trig functions every frame.
        with_unit_circle(ghost_n, |circle| {
            for p in circle {
                let (ca, sa) = (p[0], p[1]);
                let (px, py, z) = pt.project(
                    (ux * ca + vx * sa) * ro,
                    (uy * ca + vy * sa) * ro,
                    (uz * ca + vz * sa) * ro,
                );
                let depth = (z * inv_ro + 1.0) / 2.0;
                dots.push(Dot::new(px, py, z, ghost_r, 0.72).with_a(ghost_a * (0.4 + 0.6 * depth)));
            }
        });
        // the particles doing the work
        for m in 0..particles {
            let a = t * speed + (m as f32 / particles as f32) * 2.0 * PI + h2 * 6.0;
            let (ca, sa) = (a.cos(), a.sin());
            let (px, py, z) = pt.project(
                (ux * ca + vx * sa) * ro,
                (uy * ca + vy * sa) * ro,
                (uz * ca + vz * sa) * ro,
            );
            let depth = (z * inv_ro + 1.0) / 2.0;
            dots.push(Dot::new(
                px,
                py,
                z,
                (part_r + part_r_depth * depth) * rs,
                0.3 - 0.22 * depth,
            ));
        }
    }
}
