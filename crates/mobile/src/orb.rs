//! The desktop's thinking orb for platform canvases. Geometry, clock, audio
//! response and state crossfades come from `zeron-orb`, exactly as the GPUI
//! widget runs them; the platform only strokes lines and fills disks.
//!
//! Frames are flat `f32` lists so a 30 fps loop crosses the FFI with two
//! buffers instead of hundreds of records:
//! - `lines`: `[x1, y1, x2, y2, width, white, alpha]` per segment;
//! - `dots`: `[x, y, radius, white, alpha]` per disk, back to front.
//!
//! Coordinates are logical points in a `size × size` box. `white` is ink
//! (0 = darkest on paper); dark appearances paint `1 - white`. Paint lines,
//! then dots, onto a transparent canvas.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use zeron_orb::{OrbAnimator, OrbSize, OrbState, engine::Frame};

/// The four tuned size presets (inline 20, avatar 64, large 96, hero 128).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OrbPreset {
    Inline,
    Avatar,
    Large,
    Hero,
}

impl From<OrbPreset> for OrbSize {
    fn from(preset: OrbPreset) -> Self {
        match preset {
            OrbPreset::Inline => OrbSize::Inline,
            OrbPreset::Avatar => OrbSize::Avatar,
            OrbPreset::Large => OrbSize::Large,
            OrbPreset::Hero => OrbSize::Hero,
        }
    }
}

/// What the voice orb is showing, in call terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum VoiceOrb {
    /// No call: the calm form of a muted orchestrator.
    Idle,
    Connecting,
    Listening,
    Speaking,
    Working,
    AwaitingInput,
    Muted,
}

impl From<VoiceOrb> for OrbState {
    fn from(orb: VoiceOrb) -> Self {
        match orb {
            VoiceOrb::Idle | VoiceOrb::Muted => OrbState::Breathing,
            VoiceOrb::Connecting => OrbState::Connecting,
            VoiceOrb::Listening => OrbState::Listening,
            VoiceOrb::Speaking => OrbState::Composing,
            VoiceOrb::Working => OrbState::Working,
            VoiceOrb::AwaitingInput => OrbState::Solving,
        }
    }
}

impl VoiceOrb {
    pub(crate) fn from_state(state: OrbState) -> Self {
        match state {
            OrbState::Connecting => VoiceOrb::Connecting,
            OrbState::Listening => VoiceOrb::Listening,
            OrbState::Composing => VoiceOrb::Speaking,
            OrbState::Working => VoiceOrb::Working,
            OrbState::Solving => VoiceOrb::AwaitingInput,
            _ => VoiceOrb::Muted,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct OrbFrame {
    /// Edge length of the artwork box, in logical points.
    pub size: f32,
    pub lines: Vec<f32>,
    pub dots: Vec<f32>,
}

/// One animated orb. Calls are cheap and safe from any thread; drive
/// [`OrbRenderer::next_frame`] from the display link.
#[derive(uniffi::Object)]
pub struct OrbRenderer {
    inner: Mutex<Inner>,
}

struct Inner {
    animator: OrbAnimator,
    frame: Frame,
}

/// Below this the desktop paint path skips a primitive too.
const MIN_ALPHA: f32 = 0.02;

#[uniffi::export]
impl OrbRenderer {
    /// Crossfades state changes over 300 ms, like the desktop voice orbs.
    #[uniffi::constructor]
    pub fn new(preset: OrbPreset, orb: VoiceOrb) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                animator: OrbAnimator::new(orb.into(), preset.into())
                    .with_transition(Duration::from_millis(300)),
                frame: Frame::new(),
            }),
        })
    }

    pub fn set_orb(&self, orb: VoiceOrb) {
        let inner = &mut *self.inner.lock().unwrap();
        inner.animator.set_state(orb.into(), &inner.frame);
    }

    /// Normalized 0…1 peaks; they speed the motion up, never the geometry.
    pub fn set_audio_levels(&self, microphone: f32, speaker: f32) {
        self.inner
            .lock()
            .unwrap()
            .animator
            .set_audio_levels(microphone, speaker, Instant::now());
    }

    /// Advance to now and return the frame to paint. Time only accumulates
    /// while `animating`; reduced motion shows the static representative frame.
    pub fn next_frame(&self, animating: bool, reduced_motion: bool) -> OrbFrame {
        let inner = &mut *self.inner.lock().unwrap();
        inner
            .animator
            .tick(Instant::now(), animating && !reduced_motion, reduced_motion);
        inner.animator.draw(&mut inner.frame);
        let r_min = inner.animator.r_min();
        let mut lines = Vec::with_capacity(inner.frame.lines.len() * 7);
        for l in inner.frame.lines.iter().filter(|l| l.a >= MIN_ALPHA) {
            lines.extend_from_slice(&[l.x1, l.y1, l.x2, l.y2, l.w, l.white, l.a]);
        }
        let mut dots = Vec::with_capacity(inner.frame.dots.len() * 5);
        for d in inner.frame.dots.iter().filter(|d| d.a >= MIN_ALPHA) {
            dots.extend_from_slice(&[d.x, d.y, d.r.max(r_min), d.white, d.a]);
        }
        OrbFrame {
            size: inner.animator.pixels(),
            lines,
            dots,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_voice_orb_paints_finite_geometry_inside_its_box() {
        for orb in [
            VoiceOrb::Idle,
            VoiceOrb::Connecting,
            VoiceOrb::Listening,
            VoiceOrb::Speaking,
            VoiceOrb::Working,
            VoiceOrb::AwaitingInput,
            VoiceOrb::Muted,
        ] {
            let renderer = OrbRenderer::new(OrbPreset::Hero, orb);
            renderer.set_audio_levels(0.5, 0.8);
            let frame = renderer.next_frame(true, false);
            assert_eq!(frame.size, 128.0);
            assert!(!frame.dots.is_empty(), "{orb:?}");
            assert_eq!(frame.dots.len() % 5, 0);
            assert_eq!(frame.lines.len() % 7, 0);
            assert!(frame.dots.iter().chain(&frame.lines).all(|v| v.is_finite()));
            assert_eq!(
                VoiceOrb::from_state(orb.into()),
                match orb {
                    VoiceOrb::Idle => VoiceOrb::Muted,
                    orb => orb,
                }
            );
        }
    }

    #[test]
    fn reduced_motion_is_a_still_frame() {
        let renderer = OrbRenderer::new(OrbPreset::Avatar, VoiceOrb::Listening);
        let first = renderer.next_frame(true, true);
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(first.dots, renderer.next_frame(true, true).dots);
    }
}
