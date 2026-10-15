//! The voice glyph's engine ([`glyph`]) and the state a call shows, shared by
//! the desktop and the mobile apps so every surface reads a call the same way.
//! Nothing here paints: the desktop draws its dots through GPUI, the phone
//! through Core Graphics, from the same frames.

pub mod glyph;

/// What a call is doing, as its glyph shows it.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OrbState {
    /// Codex works on the user's behalf.
    #[default]
    Working,
    /// Codex looks something up.
    Searching,
    /// Codex waits on the user's answer.
    Solving,
    /// The user has the floor.
    Listening,
    /// The call is being placed.
    Connecting,
    /// Codex speaks.
    Composing,
    /// The call rests: muted, or not yet heard.
    Breathing,
}
