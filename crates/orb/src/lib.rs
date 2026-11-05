// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Thinking orbs — dotted loading indicators for AI & agent UIs, ported from
//! gpui-thinking-orbs (MIT), itself a port of Jakub Antalik's thinking-orbs.
//!
//! Twelve hand-tuned animated states and four size presets, emitted as
//! monochrome geometry ([`engine::Frame`]). Nothing here paints: the desktop
//! widget renders frames through GPUI and the mobile apps through their native
//! 2-D APIs, so every platform shows the same artwork.

mod animator;
mod motion;
mod presets;
mod types;

pub mod engine;

pub use animator::{OrbAnimator, crossfade};
pub use motion::{AnimationClock, AudioResponse};
pub use presets::{Resolved, resolve_preset};
pub use types::{ModeKey, OrbSize, OrbState, OrbTheme};

/// Reduced motion freezes every orb on this representative frame.
pub const REDUCED_MOTION_T: f32 = 0.6;
