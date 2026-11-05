// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Web: a constellation wires itself — the "connecting" state.

use crate::engine::{
    core::{Dot, Frame, Line, frac, hash_d, lerp, make_proj, radius_scale, vnoise, with_fib_dirs},
    profiles::{MAX_NODE_N, MAX_SIGNALS, ModeOpts, count_usize},
};
use std::cell::RefCell;

thread_local! {
    /// Reused between frames so the steady-state animation loop never
    /// allocates. Holds `(unit_x, unit_y, unit_z, proj_x, proj_y, proj_z)`
    /// per node: projecting once up front turns the O(n²) edge scan from
    /// ~2n² projections into n.
    static NODES: RefCell<Vec<[f32; 6]>> = const { RefCell::new(Vec::new()) };
    static EDGES: RefCell<Vec<(usize, usize)>> = const { RefCell::new(Vec::new()) };
}

pub fn draw_web_into(size: f32, t: f32, o: &ModeOpts, out: &mut Frame) {
    let cx = size / 2.0;
    let cy = size / 2.0;
    let r = (size / 2.0) * 0.8 * o.spread.unwrap_or(1.0);
    let pt = make_proj(t * 0.12, 0.32, cx, cy, r);
    let rs = radius_scale(size, o.rs_pow.unwrap_or(0.6));

    let node_n = count_usize(o.node_n, 30.0, 0, MAX_NODE_N as usize);
    let thr = o.thr.unwrap_or(0.72);
    let node_r = o.node_r.unwrap_or(1.4);
    let node_r_depth = o.node_r_depth.unwrap_or(1.8);
    let line_w = (o.line_w.unwrap_or(0.8) * rs).max(0.6);
    let inv_thr = if thr > 0.0 { 1.0 / thr } else { 0.0 };

    NODES.with(|cell| {
        EDGES.with(|edges_cell| {
            let mut nodes = cell.borrow_mut();
            let mut edges = edges_cell.borrow_mut();
            nodes.clear();
            edges.clear();
            nodes.reserve(node_n);

            with_fib_dirs(node_n, |directions| {
                for (i, d) in directions.iter().enumerate() {
                    let i_f = i as f32;
                    let x = d[0] + 0.3 * (vnoise(i_f * 0.31 + 9.0, t * 0.24) - 0.5) * 2.0;
                    let y = d[1] + 0.3 * (vnoise(i_f * 0.53 + 27.0, t * 0.21) - 0.5) * 2.0;
                    let z = d[2] + 0.3 * (vnoise(i_f * 0.77 + 55.0, t * 0.27) - 0.5) * 2.0;
                    let l = (x * x + y * y + z * z).sqrt();
                    let inv_l = if l > 1e-6 { 1.0 / l } else { 0.0 };
                    let (ux, uy, uz) = (x * inv_l, y * inv_l, z * inv_l);
                    let (px, py, pz) = pt.project(ux, uy, uz);
                    nodes.push([ux, uy, uz, px, py, pz]);
                }
            });

            for i in 0..node_n {
                let a = nodes[i];
                for j in (i + 1)..node_n {
                    let b = nodes[j];
                    let dx = a[0] - b[0];
                    let dy = a[1] - b[1];
                    let dz = a[2] - b[2];
                    let d2 = dx * dx + dy * dy + dz * dz;
                    // Compare squared distances — avoids a sqrt for every
                    // rejected pair, and most pairs are rejected.
                    if d2 >= thr * thr {
                        continue;
                    }
                    let dist = d2.sqrt();
                    let depth = ((a[5] + b[5]) / 2.0 + 1.0) / 2.0;
                    out.lines.push(Line {
                        x1: a[3],
                        y1: a[4],
                        x2: b[3],
                        y2: b[4],
                        white: 0.42,
                        a: (1.0 - dist * inv_thr) * (0.3 + 0.55 * depth),
                        w: line_w,
                    });
                    edges.push((i, j));
                }
            }

            for (i, n) in nodes.iter().enumerate() {
                let depth = (n[5] + 1.0) / 2.0;
                let pulse = 1.0 + 0.25 * (t * 1.4 + i as f32 * 2.7).sin();
                out.dots.push(Dot::new(
                    n[3],
                    n[4],
                    n[5],
                    (node_r + node_r_depth * depth) * pulse * rs,
                    0.55 - 0.45 * depth,
                ));
            }

            let signals = count_usize(o.signals, 5.0, 0, MAX_SIGNALS as usize);
            for s in 0..signals {
                if edges.is_empty() {
                    break;
                }
                let s_f = s as f32;
                let journey = t * 0.48 + s_f * 3.73;
                let cycle = journey.floor();
                let edge_index = (hash_d(cycle, s_f * 4.7 + 1.3) * edges.len() as f32)
                    .floor()
                    .min((edges.len() - 1) as f32) as usize;
                let (a, b) = edges[edge_index];
                let f = frac(journey);
                let px = lerp(nodes[a][3], nodes[b][3], f);
                let py = lerp(nodes[a][4], nodes[b][4], f);
                let zr = lerp(nodes[a][5], nodes[b][5], f);
                let depth = (zr + 1.0) / 2.0;
                let life = (std::f32::consts::PI * f).sin().max(0.0);
                out.dots.push(
                    Dot::new(px, py, zr, (node_r * 1.5 + node_r_depth * depth) * rs, 0.05)
                        .with_a(life * (0.5 + 0.5 * depth)),
                );
            }
        });
    });
}
