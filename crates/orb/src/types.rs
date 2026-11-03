// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Public types shared by the animation engine and the GPUI widget.

/// The twelve shipped states — each a hand-tuned animation.
///
/// Marked `#[non_exhaustive]`: matching on this from another crate needs a
/// wildcard arm, so shipping a tenth state is not a breaking change. Prefer
/// [`OrbState::label`] / [`OrbState::as_str`] over matching where you can.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OrbState {
    /// Particles on tilted orbits.
    #[default]
    Working,
    /// A scan meridian sweeps a dotted globe.
    Searching,
    /// Bands scramble in quarter turns, then click back.
    Solving,
    /// A waveform rolls through latitude rings.
    Listening,
    /// A constellation wires itself, packets running the edges.
    Connecting,
    /// Three strands plait around the sphere.
    Weaving,
    /// An undulating multi-band sash.
    Composing,
    /// A face-on ring slowly morphing.
    Breathing,
    /// A dotted outline morphs circle → triangle → square.
    Shaping,
    /// An iris of particles converges and relaxes around a focal point.
    Focusing,
    /// Counter-rotating great circles form a reasoning gyroscope.
    Reasoning,
    /// Concentric memory echoes travel out from a steady core.
    Recalling,
}

impl OrbState {
    /// All states in playground / gallery order.
    ///
    /// This is a slice so adding future states does not change its public type.
    pub const ALL_STATES: &'static [OrbState] = &[
        OrbState::Working,
        OrbState::Searching,
        OrbState::Solving,
        OrbState::Listening,
        OrbState::Connecting,
        OrbState::Weaving,
        OrbState::Composing,
        OrbState::Breathing,
        OrbState::Shaping,
        OrbState::Focusing,
        OrbState::Reasoning,
        OrbState::Recalling,
    ];

    /// The original nine-state gallery.
    ///
    /// Kept at its original array type for source compatibility. New code
    /// should iterate [`Self::ALL_STATES`].
    #[deprecated(since = "0.2.0", note = "use OrbState::ALL_STATES")]
    pub const ALL: [OrbState; 9] = [
        OrbState::Working,
        OrbState::Searching,
        OrbState::Solving,
        OrbState::Listening,
        OrbState::Connecting,
        OrbState::Weaving,
        OrbState::Composing,
        OrbState::Breathing,
        OrbState::Shaping,
    ];

    /// Human-readable status label (matches upstream aria defaults).
    pub fn label(self) -> &'static str {
        match self {
            OrbState::Working => "Working…",
            OrbState::Searching => "Searching…",
            OrbState::Solving => "Solving…",
            OrbState::Listening => "Listening…",
            OrbState::Connecting => "Connecting…",
            OrbState::Weaving => "Weaving…",
            OrbState::Composing => "Composing…",
            OrbState::Breathing => "Thinking…",
            OrbState::Shaping => "Shaping…",
            OrbState::Focusing => "Focusing…",
            OrbState::Reasoning => "Reasoning…",
            OrbState::Recalling => "Recalling…",
        }
    }

    /// Stable snake_case name (matches the web package `state` prop).
    pub fn as_str(self) -> &'static str {
        match self {
            OrbState::Working => "working",
            OrbState::Searching => "searching",
            OrbState::Solving => "solving",
            OrbState::Listening => "listening",
            OrbState::Connecting => "connecting",
            OrbState::Weaving => "weaving",
            OrbState::Composing => "composing",
            OrbState::Breathing => "breathing",
            OrbState::Shaping => "shaping",
            OrbState::Focusing => "focusing",
            OrbState::Reasoning => "reasoning",
            OrbState::Recalling => "recalling",
        }
    }
}

/// Rendered size in logical pixels. Four tuned presets ship: inline (20),
/// avatar (64), large (96), and hero (128). Larger sizes add detail gradually
/// instead of merely stretching the 64 px artwork.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OrbSize {
    /// Chat-avatar scale (64 logical px).
    #[default]
    Avatar,
    /// Inline-text scale (20 logical px).
    Inline,
    /// Prominent status / card scale (96 logical px).
    Large,
    /// Hero / empty-state scale (128 logical px).
    Hero,
}

impl OrbSize {
    /// All sizes in compact-to-prominent gallery order.
    pub const ALL_SIZES: &'static [OrbSize] = &[
        OrbSize::Inline,
        OrbSize::Avatar,
        OrbSize::Large,
        OrbSize::Hero,
    ];

    /// Logical pixel edge length of the orb.
    pub fn pixels(self) -> f32 {
        match self {
            OrbSize::Inline => 20.0,
            OrbSize::Avatar => 64.0,
            OrbSize::Large => 96.0,
            OrbSize::Hero => 128.0,
        }
    }

    /// Stable lowercase name for controls and command-line arguments.
    pub fn as_str(self) -> &'static str {
        match self {
            OrbSize::Inline => "inline",
            OrbSize::Avatar => "avatar",
            OrbSize::Large => "large",
            OrbSize::Hero => "hero",
        }
    }

    /// Compact playground label.
    pub fn label(self) -> &'static str {
        match self {
            OrbSize::Inline => "20 · inline",
            OrbSize::Avatar => "64 · avatar",
            OrbSize::Large => "96 · large",
            OrbSize::Hero => "128 · hero",
        }
    }
}

/// Theme mode for monochrome ink on transparent canvas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OrbTheme {
    /// Resolve from the window appearance (GPUI `WindowAppearance`).
    #[default]
    Auto,
    /// Light ink for dark backgrounds.
    Dark,
    /// Dark ink for light backgrounds.
    Light,
}

/// Internal mode keys — one painter per key. Grows in lockstep with
/// [`OrbState`], hence `#[non_exhaustive]`.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModeKey {
    Orbits,
    Globe,
    Rubik,
    Wave,
    Web,
    Braid,
    Ribbon,
    Ring,
    Morph,
    Focus,
    Gyroscope,
    Echo,
}

impl ModeKey {
    pub fn from_state(state: OrbState) -> Self {
        match state {
            OrbState::Working => ModeKey::Orbits,
            OrbState::Searching => ModeKey::Globe,
            OrbState::Solving => ModeKey::Rubik,
            OrbState::Listening => ModeKey::Wave,
            OrbState::Connecting => ModeKey::Web,
            OrbState::Weaving => ModeKey::Braid,
            OrbState::Composing => ModeKey::Ribbon,
            OrbState::Breathing => ModeKey::Ring,
            OrbState::Shaping => ModeKey::Morph,
            OrbState::Focusing => ModeKey::Focus,
            OrbState::Reasoning => ModeKey::Gyroscope,
            OrbState::Recalling => ModeKey::Echo,
        }
    }
}
