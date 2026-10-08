// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Thinking orbs — dotted loading indicators for AI & agent UIs, ported from
//! gpui-thinking-orbs (MIT), itself a port of Jakub Antalik's thinking-orbs.
//!
//! The animation engine and playback live in `zeron-orb`, shared with the
//! mobile apps; this module is its GPUI widget and paint path.
//!
//! ```ignore
//! cx.new(|_| Orb::new().state(OrbState::Searching).size(OrbSize::Avatar))
//! ```
//!
//! Style flows through the environment like the rest of bezel: [`OrbTheme::Auto`]
//! resolves ink against the installed [`theme::Appearance`] at paint time.

mod orb;
mod paint;

pub use orb::{DEFAULT_TARGET_FPS, Orb, orb_element};
pub use zeron_orb::{ModeKey, OrbSize, OrbState, OrbTheme, Resolved, engine, resolve_preset};
