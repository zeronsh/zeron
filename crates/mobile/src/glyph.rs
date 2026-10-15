//! The desktop's voice glyph for platform canvases. Which dots light, how
//! much and in what colour come from `zeron-orb`'s glyph engine, exactly as
//! the GPUI stage runs it; the platform only fills circles.
//!
//! A frame is a flat `f32` list, [`STRIDE`] values a dot:
//! `[x, y, size, r, g, b, alpha]` — the dot's
//! centre and diameter in logical points from the box's corner, its fill,
//! Fill each as a circle onto a transparent canvas: light is told by colour,
//! never by glow, so a frame is cheap to paint.
use std::sync::{Arc, Mutex};
use std::time::Instant;
use zeron_orb::glyph::{
    BANDS, Beam, Cell, Field, Grid, Inks, Mode, Outline, Rhythm, Rings, Skip, Voice, look,
};

use crate::orb::VoiceOrb;

/// Values per dot in a frame.
pub const STRIDE: usize = 7;

/// What a glyph is drawn as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum GlyphShape {
    /// A disc of dots, its rings round from the centre: a control.
    Disc,
    /// A call's stage: a disc of dots to its edge, its rim easing into what
    /// holds it.
    Stage,
    /// A dictation's track: a strip that fills its box and rises from its
    /// bottom as the voice does, the desktop composer's own beam.
    Beam,
}

/// Room left for something drawn over a screen glyph, in points from its
/// box's corner: no dots under a control, a quiet patch under words.
#[derive(Debug, Clone, uniffi::Record)]
pub struct GlyphRoom {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Its corners' radius; a round control's is half its width.
    pub radius: f32,
    /// Words, over which the grid stays, quieter.
    pub text: bool,
}

/// One animated glyph. Calls are cheap and safe from any thread; drive
/// [`GlyphRenderer::next_frame`] from the display link.
#[derive(uniffi::Object)]
pub struct GlyphRenderer {
    inner: Mutex<Inner>,
}

struct Inner {
    shape: GlyphShape,
    field: Field,
    beam: Beam,
    mode: Mode,
    user: Rhythm,
    agent: Rhythm,
    /// When Codex's voice was last heard playing.
    playing: Option<Instant>,
}

fn mode(orb: VoiceOrb) -> Mode {
    match orb {
        VoiceOrb::Connecting => Mode::Connecting,
        VoiceOrb::Listening | VoiceOrb::Speaking => Mode::Listening,
        VoiceOrb::Working => Mode::Thinking,
        VoiceOrb::AwaitingInput => Mode::Asking,
        VoiceOrb::Transcribing => Mode::Processing,
        VoiceOrb::Idle | VoiceOrb::Muted => Mode::Muted,
    }
}

#[uniffi::export]
impl GlyphRenderer {
    #[uniffi::constructor]
    pub fn new(shape: GlyphShape, orb: VoiceOrb) -> Arc<Self> {
        let field = match shape {
            GlyphShape::Disc | GlyphShape::Beam => Field::new(Rings::Round),
            GlyphShape::Stage => Field::new(Rings::Round).soft_rimmed(),
        };
        Arc::new(Self {
            inner: Mutex::new(Inner {
                shape,
                field,
                beam: Beam::default(),
                mode: mode(orb),
                user: Rhythm::default(),
                agent: Rhythm::default(),
                playing: None,
            }),
        })
    }

    pub fn set_orb(&self, orb: VoiceOrb) {
        let inner = &mut *self.inner.lock().unwrap();
        inner.mode = mode(orb);
        // The passage starts now, so the glyph keeps moving until it is
        // over, even into a still mode.
        inner.field.shift(inner.mode);
        inner.beam.shift(inner.mode);
    }

    /// The call's peaks, 0–1: each voice is split into its bands by the
    /// rhythm of its loudness, as the desktop does for Codex's.
    pub fn set_audio_levels(&self, microphone: f32, speaker: f32) {
        let inner = &mut *self.inner.lock().unwrap();
        let now = Instant::now();
        // While Codex plays, the microphone mostly hears Codex through the
        // speaker: the user's voice is not read from it (as on desktop).
        if speaker > 0.02 {
            inner.playing = Some(now);
        }
        let playing = inner.playing.is_some_and(|at| {
            now.saturating_duration_since(at) < std::time::Duration::from_millis(250)
        });
        inner.user.hear(if playing { 0.0 } else { microphone }, now);
        inner.agent.hear(speaker, now);
    }

    /// Whether the glyph still moves: a sound in it or a mode that
    /// animates on its own. A still glyph needs no display link.
    pub fn moving(&self) -> bool {
        let inner = self.inner.lock().unwrap();
        match inner.shape {
            GlyphShape::Beam => inner.beam.moving(inner.mode),
            GlyphShape::Disc | GlyphShape::Stage => inner.field.moving(inner.mode),
        }
    }

    /// Advance to now (only while `animating`) and return the dots for a box
    /// `width` × `height`, dots `dot` points across, the box's corners
    /// `corner` round, leaving `rooms`. `accent` is the user's three row
    /// tints, top first, as nine 0–1 components.
    #[allow(clippy::too_many_arguments)]
    pub fn next_frame(
        &self,
        width: f32,
        height: f32,
        dot: f32,
        corner: f32,
        rooms: Vec<GlyphRoom>,
        accent: Vec<f32>,
        animating: bool,
    ) -> Vec<f32> {
        let inner = &mut *self.inner.lock().unwrap();
        let voice = Voice {
            user: if inner.mode == Mode::Muted {
                [0.0; BANDS]
            } else {
                inner.user.bands()
            },
            agent: inner.agent.bands(),
        };
        let grid = Grid::dense(dot.max(0.5));
        let tint = |row: usize| {
            let at = |i: usize| accent.get(row * 3 + i).copied().unwrap_or(0.5);
            [at(0), at(1), at(2)]
        };
        let inks = Inks::new([tint(0), tint(1), tint(2)]);
        let (origin, cells) = match inner.shape {
            GlyphShape::Beam => {
                if animating {
                    inner.beam.hear(voice, Instant::now());
                }
                // The matrix covers the box and runs past it: the box's own
                // outline, round at every corner as the track's capsule is,
                // is where the dots fade out.
                let ((cols, rows), (mx, my)) = grid.cover(width, height);
                let area = (width / grid.pitch(), height / grid.pitch());
                let outline = if corner >= height / 2.0 - 0.5 {
                    Outline::Stadium
                } else {
                    Outline::Rounded(corner / grid.pitch())
                };
                let cells = inner
                    .beam
                    .cells_over(cols, rows, area, inner.mode, &inks, outline);
                ((mx, my), cells)
            }
            GlyphShape::Disc | GlyphShape::Stage => {
                if animating {
                    inner.field.hear(voice, Instant::now());
                }
                // Square, centred in the box: a disc, or a stage's field.
                let ((cols, rows), _) = grid.fit(width, height);
                let (cols, rows) = (cols.min(rows), cols.min(rows));
                let (w, h) = grid.extent(cols, rows);
                let (mx, my) = ((width - w) / 2.0, (height - h) / 2.0);
                inner.field.set_rings(Rings::Round);
                let skip: Vec<Skip> = rooms
                    .iter()
                    .map(|room| {
                        let skip = Skip::rect(
                            room.x - mx,
                            room.y - my,
                            room.width,
                            room.height,
                            room.radius,
                        );
                        if room.text { skip.soft() } else { skip }
                    })
                    .collect();
                let cells =
                    inner
                        .field
                        .cells(cols, rows, grid, inner.mode, &inks, Outline::Disc, &skip);
                ((mx, my), cells)
            }
        };
        frame(origin, grid, &cells)
    }
}

/// Lays `cells` out on `grid` from `origin`, each as what to paint.
fn frame((mx, my): (f32, f32), grid: Grid, cells: &[Cell]) -> Vec<f32> {
    let mut frame = Vec::with_capacity(cells.len() * STRIDE);
    for cell in cells {
        let look = look(cell);
        frame.extend_from_slice(&[
            mx + cell.col * grid.pitch() + grid.dot / 2.0,
            my + cell.row * grid.pitch() + grid.dot / 2.0,
            grid.dot * look.size,
            look.fill[0],
            look.fill[1],
            look.fill[2],
            look.alpha,
        ]);
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCENT: [f32; 9] = [0.7, 0.6, 1.0, 0.5, 0.4, 0.9, 0.3, 0.2, 0.7];

    #[test]
    fn a_disc_fills_its_box_and_a_voice_lights_it() {
        let glyph = GlyphRenderer::new(GlyphShape::Disc, VoiceOrb::Speaking);
        let quiet = glyph.next_frame(64.0, 64.0, 6.0, 0.0, vec![], ACCENT.to_vec(), true);
        assert_eq!(quiet.len() % STRIDE, 0);
        assert!(
            quiet
                .chunks(STRIDE)
                .all(|dot| dot.iter().all(|v| v.is_finite()))
        );
        assert!(
            quiet.chunks(STRIDE).all(|dot| dot[6] < 0.5),
            "silent: every dot is quiet"
        );
        // Half a second of Codex speaking, heard as the display link would.
        for _ in 0..25 {
            glyph.set_audio_levels(0.0, 0.5);
            glyph.next_frame(64.0, 64.0, 6.0, 0.0, vec![], ACCENT.to_vec(), true);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let loud = glyph.next_frame(64.0, 64.0, 6.0, 0.0, vec![], ACCENT.to_vec(), true);
        assert!(
            loud.chunks(STRIDE).any(|dot| dot[6] > 0.6),
            "Codex lights it"
        );
    }

    #[test]
    fn a_stage_fills_its_disc_to_the_edge_with_a_quieter_rim() {
        let glyph = GlyphRenderer::new(GlyphShape::Stage, VoiceOrb::Muted);
        let frame = glyph.next_frame(200.0, 200.0, 4.0, 0.0, vec![], ACCENT.to_vec(), true);
        let at = |x: f32, y: f32| {
            frame
                .chunks(STRIDE)
                .filter(|dot| ((dot[0] - x).powi(2) + (dot[1] - y).powi(2)).sqrt() < 4.0)
                .map(|dot| dot[6])
                .fold(0.0f32, f32::max)
        };
        // A dot at the heart and one at the rim; none in the corners.
        assert!(at(100.0, 100.0) > 0.0 && at(3.0, 100.0) > 0.0);
        assert_eq!(at(3.0, 3.0), 0.0);
        assert!(at(3.0, 100.0) < at(100.0, 100.0), "the rim is quieter");
    }

    #[test]
    fn a_beam_spans_its_strip_and_rises_with_the_user_from_the_bottom() {
        let glyph = GlyphRenderer::new(GlyphShape::Beam, VoiceOrb::Listening);
        let frame = |glyph: &GlyphRenderer| {
            glyph.next_frame(240.0, 32.0, 2.4, 16.0, vec![], ACCENT.to_vec(), true)
        };
        let quiet = frame(&glyph);
        let xs = quiet.chunks(STRIDE).map(|dot| dot[0]);
        let (left, right) = xs.fold((f32::MAX, f32::MIN), |(l, r), x| (l.min(x), r.max(x)));
        assert!(left < 4.0 && right > 236.0, "the strip reaches both ends");
        // Its bottom corners are round: no dot in them.
        assert!(
            !quiet
                .chunks(STRIDE)
                .any(|dot| dot[0] < 3.0 && dot[1] > 29.0),
            "no dot in the round bottom corner"
        );
        for _ in 0..20 {
            glyph.set_audio_levels(0.6, 0.0);
            frame(&glyph);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let loud = frame(&glyph);
        let lit = |frame: &[f32], y_from: f32| {
            frame
                .chunks(STRIDE)
                .filter(|dot| dot[1] >= y_from && (dot[0] - 120.0).abs() < 6.0)
                .map(|dot| dot[6])
                .fold(0.0f32, f32::max)
        };
        assert!(
            lit(&loud, 24.0) > lit(&quiet, 24.0),
            "the voice lights the bottom"
        );
        let top = loud
            .chunks(STRIDE)
            .filter(|dot| dot[1] < 8.0 && (dot[0] - 120.0).abs() < 6.0)
            .map(|dot| dot[6])
            .fold(0.0f32, f32::max);
        assert!(lit(&loud, 12.0) >= top, "it rises from the bottom");
        assert!(glyph.moving());
    }
}
