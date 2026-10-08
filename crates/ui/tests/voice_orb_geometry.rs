// MIT: extracted with the orb; see THIRD_PARTY_NOTICES.md.
//! Smoke tests + microbench for mode painters.

use zeron_ui::orb::{
    ModeKey, OrbSize, OrbState,
    engine::{MAX_ICON_D, MAX_MORPH_DOTS, MAX_NODE_N, ModeOpts, draw_mode, sanitize_mode_opts},
    resolve_preset,
};

#[test]
fn all_states_emit_geometry() {
    for &state in OrbState::ALL_STATES {
        for &size in OrbSize::ALL_SIZES {
            let resolved = resolve_preset(state, size);
            let frame = draw_mode(resolved.mode, size.pixels(), 1.25, &resolved.opts);
            assert!(
                !frame.dots.is_empty(),
                "{:?}/{:?} produced zero dots",
                state,
                size
            );
            for d in &frame.dots {
                assert!(d.r.is_finite() && d.r >= 0.0);
                assert!(d.x.is_finite() && d.y.is_finite() && d.z.is_finite());
                assert!(d.a >= 0.0 && d.a <= 1.0 + 1e-3);
            }
        }
    }
}

#[test]
fn connecting_emits_lines() {
    let resolved = resolve_preset(OrbState::Connecting, OrbSize::Avatar);
    let frame = draw_mode(resolved.mode, 64.0, 2.0, &resolved.opts);
    assert!(!frame.lines.is_empty(), "connecting should wire edges");
    assert!(!frame.dots.is_empty());
}

#[test]
fn reduced_motion_frame_is_deterministic() {
    let resolved = resolve_preset(OrbState::Working, OrbSize::Avatar);
    let a = draw_mode(resolved.mode, 64.0, 0.6, &resolved.opts);
    let b = draw_mode(resolved.mode, 64.0, 0.6, &resolved.opts);
    assert_eq!(a.dots.len(), b.dots.len());
    for (da, db) in a.dots.iter().zip(b.dots.iter()) {
        assert!((da.x - db.x).abs() < 1e-4);
        assert!((da.y - db.y).abs() < 1e-4);
    }
}

#[test]
fn solve_cycle_survives_float_boundary_move_counts() {
    // `move_count` is a public `ModeOpts` knob. Certain values put the slot
    // boundary exactly on a float rounding edge, where `tc / slot_dur` reaches
    // `2 * count` even though the `tc < 2 * count * slot_dur` guard passed —
    // which used to underflow `2*count-1-slot` to `usize::MAX` and panic.
    //
    // Sweep enough move counts and adjacent float values to exercise rounding
    // at both forward and reverse slot boundaries.
    const SLOT_DUR: f32 = 0.58;
    let resolved = resolve_preset(OrbState::Solving, OrbSize::Avatar);
    for count in 1..40u32 {
        let mut opts = resolved.opts.clone();
        opts.move_count = Some(count as f32);
        for c in 1..=count {
            // Step onto the largest float strictly below each slot boundary.
            let boundary = 2.0 * c as f32 * SLOT_DUR;
            for t in [
                f32::from_bits(boundary.to_bits() - 1),
                boundary,
                f32::from_bits(boundary.to_bits() + 1),
            ] {
                let frame = draw_mode(resolved.mode, 64.0, t, &opts);
                assert!(!frame.dots.is_empty());
            }
        }
    }
}

#[test]
fn new_concepts_are_distinct_low_density_modes() {
    let concepts = [
        (OrbState::Focusing, ModeKey::Focus, 77, 23),
        (OrbState::Reasoning, ModeKey::Gyroscope, 90, 32),
        (OrbState::Recalling, ModeKey::Echo, 75, 21),
    ];

    for (state, mode, avatar_dots, inline_dots) in concepts {
        assert_eq!(ModeKey::from_state(state), mode);
        assert!(OrbState::ALL_STATES.contains(&state));
        for (size, expected) in [
            (OrbSize::Avatar, avatar_dots),
            (OrbSize::Inline, inline_dots),
        ] {
            let resolved = resolve_preset(state, size);
            let frame = draw_mode(resolved.mode, size.pixels(), 0.6, &resolved.opts);
            assert_eq!(frame.dots.len(), expected, "{state:?}/{size:?}");
            assert!(
                frame.lines.is_empty(),
                "new concepts stay on the dot-only paint path"
            );
            assert!(
                frame.dots.len() < 100,
                "new concepts must remain low density"
            );
        }
    }
}

#[test]
fn new_concepts_animate_and_keep_their_depth_identity() {
    for &state in &[OrbState::Focusing, OrbState::Reasoning, OrbState::Recalling] {
        let resolved = resolve_preset(state, OrbSize::Avatar);
        let a = draw_mode(resolved.mode, 64.0, 0.2, &resolved.opts);
        let b = draw_mode(resolved.mode, 64.0, 1.2, &resolved.opts);
        assert!(
            a.dots
                .iter()
                .zip(&b.dots)
                .any(|(a, b)| (a.x - b.x).abs() > 0.01 || (a.y - b.y).abs() > 0.01),
            "{state:?} did not animate"
        );

        let has_depth = a.dots.iter().any(|dot| dot.z.abs() > 0.01);
        assert_eq!(has_depth, state == OrbState::Reasoning);
    }
}

#[test]
fn larger_sizes_add_detail_without_changing_the_legacy_presets() {
    assert_eq!(OrbSize::ALL_SIZES.len(), 4);
    assert_eq!(OrbSize::Large.pixels(), 96.0);
    assert_eq!(OrbSize::Hero.pixels(), 128.0);

    for &state in OrbState::ALL_STATES {
        let avatar = resolve_preset(state, OrbSize::Avatar);
        let large = resolve_preset(state, OrbSize::Large);
        let hero = resolve_preset(state, OrbSize::Hero);
        let avatar_frame = draw_mode(avatar.mode, 64.0, 0.6, &avatar.opts);
        let large_frame = draw_mode(large.mode, 96.0, 0.6, &large.opts);
        let hero_frame = draw_mode(hero.mode, 128.0, 0.6, &hero.opts);

        assert_eq!(avatar.mode, large.mode);
        assert_eq!(avatar.mode, hero.mode);
        assert!(
            large_frame.dots.len() >= avatar_frame.dots.len(),
            "{state:?}/large"
        );
        assert!(
            hero_frame.dots.len() >= large_frame.dots.len(),
            "{state:?}/hero"
        );
        assert!(large.speed <= avatar.speed);
        assert!(hero.speed <= large.speed);
    }
}

#[test]
fn legacy_all_constant_keeps_its_original_array_shape() {
    #[allow(deprecated)]
    let original_nine: [OrbState; 9] = OrbState::ALL;
    assert_eq!(original_nine.len(), 9);
    assert_eq!(OrbState::ALL_STATES.len(), 12);
}

#[test]
fn bench_geometry_cost() {
    use std::time::Instant;
    for &state in OrbState::ALL_STATES {
        for &size in OrbSize::ALL_SIZES {
            let resolved = resolve_preset(state, size);
            for i in 0..20 {
                let _ = draw_mode(
                    resolved.mode,
                    size.pixels(),
                    i as f32 * 0.016,
                    &resolved.opts,
                );
            }
            let n = 1000usize;
            let t0 = Instant::now();
            let mut total_dots = 0usize;
            let mut total_lines = 0usize;
            for i in 0..n {
                let f = draw_mode(
                    resolved.mode,
                    size.pixels(),
                    i as f32 * 0.016,
                    &resolved.opts,
                );
                total_dots += f.dots.len();
                total_lines += f.lines.len();
            }
            let elapsed = t0.elapsed();
            let per = elapsed / n as u32;
            let avg_dots = total_dots / n;
            let avg_lines = total_lines / n;
            eprintln!(
                "{:>12}/{:<6}  {:>5} dots  {:>4} lines  {:>8.2?} /frame  (~{:.0}k geom-fps)",
                state.as_str(),
                size.pixels() as u32,
                avg_dots,
                avg_lines,
                per,
                1.0 / per.as_secs_f64() / 1000.0
            );
        }
    }
}

/// Strokes are painted at their exact colour, one `Path` per segment. Batching
/// them into shared paths was tried and reverted for visibly brightening the
/// web (see `paint.rs`), so what matters now is that every alpha the engine
/// emits is in range and finite — a stroke painter has no clamping to hide
/// behind.
#[test]
fn stroke_colours_are_paintable() {
    let resolved = resolve_preset(OrbState::Connecting, OrbSize::Avatar);
    let mut seen = 0usize;
    for i in 0..400 {
        let frame = draw_mode(resolved.mode, 64.0, i as f32 * 0.05, &resolved.opts);
        for l in &frame.lines {
            assert!(
                l.a.is_finite() && (0.0..=1.0).contains(&l.a),
                "alpha {}",
                l.a
            );
            assert!(
                l.white.is_finite() && (0.0..=1.0).contains(&l.white),
                "white {}",
                l.white
            );
            assert!(l.w.is_finite() && l.w > 0.0, "stroke width {}", l.w);
            assert!(l.x1.is_finite() && l.y1.is_finite());
            assert!(l.x2.is_finite() && l.y2.is_finite());
            seen += 1;
        }
    }
    assert!(seen > 0, "connecting emitted no strokes across 400 frames");
}

#[test]
fn adversarial_mode_opts_stay_bounded_and_finite() {
    // Power-user knobs used to accept Inf / huge finites and OOM or NaN.
    // draw_mode_into sanitizes first; geometry must stay finite and small.
    let cases: &[(ModeKey, ModeOpts)] = &[
        (
            ModeKey::Web,
            ModeOpts::fill(ModeKey::Web, |o| {
                o.node_n = Some(f32::INFINITY);
                o.signals = Some(1.0e12);
            }),
        ),
        (
            ModeKey::Globe,
            ModeOpts::fill(ModeKey::Globe, |o| {
                o.lat_rings = Some(0.0);
                o.lon_density = Some(5_000.0);
            }),
        ),
        (
            ModeKey::Rubik,
            ModeOpts::fill(ModeKey::Rubik, |o| {
                o.lat_rings = Some(0.0);
                o.move_count = Some(1.0e7);
            }),
        ),
        (
            ModeKey::Wave,
            ModeOpts::fill(ModeKey::Wave, |o| {
                o.rings = Some(0.0);
                o.lon_density = Some(f32::NAN);
            }),
        ),
        (
            ModeKey::Orbits,
            ModeOpts::fill(ModeKey::Orbits, |o| {
                o.orbit_n = Some(f32::INFINITY);
                o.ghost_n = Some(1.0e9);
                o.particles = Some(1.0e6);
            }),
        ),
        (
            ModeKey::Morph,
            ModeOpts::fill(ModeKey::Morph, |o| {
                o.icon_d = Some(1.0e6);
            }),
        ),
        (
            ModeKey::Ribbon,
            ModeOpts::fill(ModeKey::Ribbon, |o| {
                o.lanes = Some(1.0e6);
                o.segs = Some(1.0e6);
                o.band_mul = Some(1.0e6);
                o.ghost_n = Some(1.0e9);
            }),
        ),
        (
            ModeKey::Braid,
            ModeOpts::fill(ModeKey::Braid, |o| {
                o.strand_n = Some(1.0e9);
                o.ghost_n = Some(1.0e9);
            }),
        ),
    ];

    for (mode, opts) in cases {
        let frame = draw_mode(*mode, 64.0, 1.25, opts);
        assert!(
            frame.dots.len() <= 80_000,
            "{mode:?} emitted {} dots — sanitize failed",
            frame.dots.len()
        );
        for d in &frame.dots {
            assert!(d.x.is_finite() && d.y.is_finite() && d.z.is_finite() && d.r.is_finite());
        }
        for l in &frame.lines {
            assert!(l.x1.is_finite() && l.y1.is_finite() && l.x2.is_finite() && l.y2.is_finite());
        }
    }

    let morph = draw_mode(
        ModeKey::Morph,
        64.0,
        0.6,
        &ModeOpts::fill(ModeKey::Morph, |o| {
            o.icon_d = Some(MAX_ICON_D * 100.0);
        }),
    );
    assert!(morph.dots.len() <= MAX_MORPH_DOTS);

    let web = draw_mode(
        ModeKey::Web,
        64.0,
        1.0,
        &ModeOpts::fill(ModeKey::Web, |o| {
            o.node_n = Some(MAX_NODE_N * 10.0);
        }),
    );
    // node_n capped → dots include nodes + a few signal packets
    assert!(web.dots.len() < MAX_NODE_N as usize + 100);

    // The base mode is irrelevant here: the sanitizer is what is on trial.
    let clean = sanitize_mode_opts(&ModeOpts::fill(ModeKey::Web, |o| {
        o.node_n = Some(f32::INFINITY);
        o.lat_rings = Some(-3.0);
        o.icon_d = Some(f32::NAN);
    }));
    assert_eq!(clean.node_n, None); // non-finite → dropped
    assert_eq!(clean.lat_rings, Some(1.0)); // clamped up
    assert_eq!(clean.icon_d, None);
}
