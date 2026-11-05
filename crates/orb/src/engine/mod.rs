// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Backend-agnostic animation engine (ported from thinking-orbs).

mod braid;
mod concepts;
mod core;
mod lattice;
mod morph;
mod orbits;
mod profiles;
mod ribbon;
mod web;

pub use core::{Dot, Frame, Line, Proj, make_proj, sort_dots};
pub use profiles::{
    MAX_ICON_D, MAX_MORPH_DOTS, MAX_NODE_N, MAX_SIZE, MIN_SIZE, ModeOpts, base_profile,
    sanitize_mode_opts, sanitize_size, scale_counts, scale_radii,
};

use crate::types::ModeKey;
use braid::draw_braid_into;
use concepts::{draw_echo_into, draw_focus_into, draw_gyroscope_into};
use lattice::{draw_globe_into, draw_rubik_into, draw_wave_into};
use morph::draw_morph_into;
use orbits::draw_orbits_into;
use ribbon::draw_ribbon_into;
use web::draw_web_into;

/// Paint one frame for the given mode into an existing [`Frame`], reusing its
/// buffers.
///
/// This is the allocation-free entry point for the **geometry** buffers: `out`
/// is cleared but keeps its capacity, so a steady-state animation loop performs
/// no heap growth after the first frame (the Solving mode reuses thread-local
/// scratch for its move tables). Prefer this over [`draw_mode`] in render loops.
///
/// `size` and every count-like field of `opts` are sanitized (finite + hard
/// ceilings) before geometry runs — see [`sanitize_mode_opts`].
pub fn draw_mode_into(mode: ModeKey, size: f32, t: f32, opts: &ModeOpts, out: &mut Frame) {
    let size = sanitize_size(size);
    let opts = sanitize_mode_opts(opts);
    let t = if t.is_finite() { t } else { 0.0 };
    draw_mode_into_resolved(mode, size, t, &opts, out);
}

/// Fast path for the widget's already-sanitized, size-bounded presets.
///
/// Only for options from [`crate::resolve_preset`], which are already finite
/// and bounded: it skips the 40-field clone/sanitize pass on every widget
/// tick. Arbitrary options must go through [`draw_mode_into`].
#[doc(hidden)]
pub fn draw_mode_into_resolved(mode: ModeKey, size: f32, t: f32, opts: &ModeOpts, out: &mut Frame) {
    out.clear();
    match mode {
        ModeKey::Orbits => draw_orbits_into(size, t, opts, out),
        ModeKey::Globe => draw_globe_into(size, t, opts, out),
        ModeKey::Rubik => draw_rubik_into(size, t, opts, out),
        ModeKey::Wave => draw_wave_into(size, t, opts, out),
        ModeKey::Web => draw_web_into(size, t, opts, out),
        ModeKey::Braid => draw_braid_into(size, t, opts, out),
        ModeKey::Ribbon | ModeKey::Ring => draw_ribbon_into(size, t, opts, out),
        ModeKey::Morph => draw_morph_into(size, t, opts, out),
        ModeKey::Focus => draw_focus_into(size, t, opts, out),
        ModeKey::Gyroscope => draw_gyroscope_into(size, t, opts, out),
        ModeKey::Echo => draw_echo_into(size, t, opts, out),
    }
    // Face-on modes intentionally emit z=0 in painter order, so a comparison
    // sort cannot change their result. Avoid it entirely.
    if !matches!(mode, ModeKey::Focus | ModeKey::Echo) {
        sort_dots(&mut out.dots);
    }
}

/// Paint one frame for the given mode into a fresh [`Frame`].
///
/// Convenience wrapper around [`draw_mode_into`] for one-off calls and tests;
/// it allocates. Render loops should use [`draw_mode_into`] instead.
pub fn draw_mode(mode: ModeKey, size: f32, t: f32, opts: &ModeOpts) -> Frame {
    let mut frame = Frame::new();
    draw_mode_into(mode, size, t, opts, &mut frame);
    frame
}
