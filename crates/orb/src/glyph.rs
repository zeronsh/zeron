//! The voice glyph: the transcript's working dots (round cells tinted per row
//! from the sunrise gradient, resting dim) grown into LED matrices a voice
//! lights band by band. Pure geometry and colour, painted by the desktop
//! through GPUI and by the phone through Core Graphics.
//!
//! - [`Field`] for a call: rings are the bands, lowest at the centre. Each
//!   sound starts there and is pushed out ring by ring, so the field reads as
//!   the voice travelling, not as a meter.
//! - [`Beam`] for an edge or a track: a strip that rises from its bottom as
//!   the voice does, the lowest band at its centre.
//!
//! Every matrix is sized at paint time to the box it fills ([`Grid::fit`]),
//! keeps the outline of what holds it ([`Outline`]), and leaves room for what
//! is drawn over it ([`Skip`]). Codex's voice takes the sunrise rows, the
//! user's the accent's, so the two are told apart by colour while moving
//! alike. [`look`] turns a dot into what to paint, the same everywhere.
use std::{collections::VecDeque, f32::consts::TAU, time::Instant};

/// The bands a voice is split into, lowest first.
pub const BANDS: usize = 5;
/// How loud each band is, 0–1, lowest first.
pub type Bands = [f32; BANDS];
/// A colour as linear-ish sRGB components, 0–1.
pub type Rgb = [f32; 3];

/// The two voices a glyph follows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Voice {
    pub user: Bands,
    pub agent: Bands,
}

/// A dot's opacity at rest: the transcript spinner's own.
pub const REST: f32 = 0.16;

/// Seconds a sound takes to cross one ring of a field: a ring a frame at the
/// glyph's 30 fps, the quickest a wave still shows every ring, so a voice
/// fills the field within a third of a second rather than lagging behind it.
const PUSH: f32 = 1.0 / 30.0;
/// A dissolving field: its reach in silence and what a full voice adds, and
/// the distances (in reaches) where it starts thinning and is gone.
const DISSOLVE: (f32, f32, f32, f32) = (0.5, 0.2, 1.4, 0.7);
/// The most pushes a sound takes to reach a field's edge.
const RINGS: f32 = 9.0;
/// Seconds a band takes to fall back after a sound, so a syllable's trough
/// does not blank the centre.
const RELEASE: f32 = 0.12;
/// How much of a field dot is its own band, the rest the voice's body.
const BAND_SHARE: f32 = 0.8;
/// Seconds per turn of the thinking sweep and per breath of the idle field.
const SWEEP_PERIOD: f32 = 1.6;
const BREATH_PERIOD: f32 = 3.2;
/// Seconds between a calling field's rings, and between an asking one's
/// knocks.
const RING_PERIOD: f32 = 1.4;
const KNOCK_PERIOD: f32 = 2.4;
/// Seconds a field takes to pass from one mode's pattern to the next.
const SHIFT: f32 = 0.8;
/// How much a field's slow shimmer lights its dots.
const SHIMMER: f32 = 0.09;
/// Dots dim over this many dots as they near what is drawn over a matrix.
const FEATHER: f32 = 3.0;
/// Dots dim over this many dots as they near a matrix's outline, so a
/// matrix touches what holds it and fades where it does.
const EDGE: f32 = 2.0;
/// Seconds a beam's bands take to fall back after a sound: longer than a
/// field's, so a dictation let go settles rather than drops.
const BEAM_RELEASE: f32 = 0.35;
/// Seconds a field's sense of how loud the call is takes to fall back, and
/// how much of its falloff a loud call lifts: a run of speech fills the
/// disc, a pause lets it draw back in.
const LOUD_RELEASE: f32 = 0.7;
const SWELL: f32 = 0.6;
/// Under text, the grid keeps this much of its rest presence.
const UNDER_TEXT: f32 = 0.3;
/// A dot's diameter at rest and fully lit, as shares of its slot: a
/// halftone, where light is ink coverage. At rest the dots are small and
/// the surface shows between them; lit, they swell past their slot and
/// touch, so a voice prints a solid shape with a graded edge rather than a
/// grid glowing evenly. The swell follows the square root of the level, as
/// a print's dot radius follows its coverage.
const DOT_REST: f32 = 0.58;
const DOT_SWELL: f32 = 1.18;
/// A pointer over a field lights the dots this far round it, in dots, and
/// this much at its heart.
pub const TOUCH_REACH: f32 = 7.0;
const TOUCH_LIGHT: f32 = 0.85;
/// The sunrise rows of the transcript's working spinner.
pub const SUNRISE: [Rgb; 3] = [rgb(0xB6D3EF), rgb(0xEDB185), rgb(0xF888A0)];

const fn rgb(hex: u32) -> Rgb {
    [
        ((hex >> 16) & 0xFF) as f32 / 255.0,
        ((hex >> 8) & 0xFF) as f32 / 255.0,
        (hex & 0xFF) as f32 / 255.0,
    ]
}

/// What a glyph shows when no one is speaking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The call or the microphone is opening: rings go out from the heart,
    /// as a call rings out.
    Connecting,
    /// Live and quiet: the heart breathes, ready for a voice.
    Listening,
    /// Codex works: a light runs round the heart, its trail behind it.
    Thinking,
    /// Codex waits on the user: the rim beats twice and rests, a knock.
    Asking,
    /// The capture ended and is being transcribed: a highlight crosses it.
    Processing,
    /// The microphone is off: the dots hold still and the user's voice is
    /// not shown.
    Muted,
}

/// The outline a matrix keeps: the dots outside it are left out, so a
/// matrix is as round as what holds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outline {
    Square,
    /// The largest disc in the matrix: a 5×5 loses its four corners.
    Disc,
    /// Round ends as high as the matrix: the shape of a pill.
    Stadium,
    /// Corners of this radius, in dots: the shape of a card or a screen.
    Rounded(f32),
    /// The bottom of something taller with corners of this radius, in dots:
    /// a strip along a composer's bottom edge, square at its top.
    Under(f32),
}

impl Outline {
    /// Whether the dot at `col`, `row` of a `cols` × `rows` matrix is kept:
    /// its centre lies inside the outline.
    pub fn keeps(self, col: f32, row: f32, cols: usize, rows: usize) -> bool {
        self.depth(col, row, cols, rows) > 0.0
    }

    /// How far inside the outline the dot at `col`, `row` sits, in dots,
    /// from its centre; negative outside. The outline runs through the
    /// outer dots' edges, so the matrix touches what holds it.
    pub fn depth(self, col: f32, row: f32, cols: usize, rows: usize) -> f32 {
        self.depth_in(col, row, cols, rows, (cols as f32, rows as f32))
    }

    /// [`Self::depth`] against a box `area` dots wide and tall, centred
    /// on the matrix: the box a covering matrix runs past.
    pub fn depth_in(self, col: f32, row: f32, cols: usize, rows: usize, area: (f32, f32)) -> f32 {
        let (hx, hy) = (area.0 / 2.0, area.1 / 2.0);
        let (x, y) = (col + 0.5 - cols as f32 / 2.0, row + 0.5 - rows as f32 / 2.0);
        // A box `hx` × `hy` about the centre with corners `radius` round;
        // `low` alone rounds the bottom corners and keeps the top square.
        let rounded = |radius: f32, low: bool| {
            let radius = radius.clamp(0.0, hx.min(hy));
            let qx = x.abs() - (hx - radius);
            let qy = if low { y } else { y.abs() } - (hy - radius);
            if low && y < 0.0 {
                // Above the middle: the plain box.
                return (hx - x.abs()).min(hy + y);
            }
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            -(outside + qx.max(qy).min(0.0) - radius)
        };
        match self {
            Self::Square => (hx - x.abs()).min(hy - y.abs()),
            Self::Disc => hx.min(hy) - (x * x + y * y).sqrt(),
            Self::Stadium => rounded(hx.min(hy), false),
            Self::Rounded(radius) => rounded(radius, false),
            Self::Under(radius) => rounded(radius, true),
        }
    }

    /// How present the dot at `col`, `row` is against the outline: full
    /// inside, fading over [`EDGE`] dots to nothing where it touches.
    pub fn edge(self, col: f32, row: f32, cols: usize, rows: usize) -> f32 {
        smooth(self.depth(col, row, cols, rows) / EDGE)
    }

    /// [`Self::edge`] against a box `area` dots wide and tall.
    pub fn edge_in(self, col: f32, row: f32, cols: usize, rows: usize, area: (f32, f32)) -> f32 {
        smooth(self.depth_in(col, row, cols, rows, area) / EDGE)
    }
}

/// A matrix's dots: how wide each is and the gap between them, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub dot: f32,
    pub gap: f32,
}

impl Grid {
    /// Dense, as an LED panel is: a gap of a third of a dot.
    pub fn dense(dot: f32) -> Self {
        Self {
            dot,
            gap: dot / 3.0,
        }
    }

    pub fn pitch(self) -> f32 {
        self.dot + self.gap
    }

    /// The size `cols` × `rows` dots take.
    pub fn extent(self, cols: usize, rows: usize) -> (f32, f32) {
        let span = |n: usize| n as f32 * self.pitch() - self.gap;
        (span(cols), span(rows))
    }

    /// The fewest dots that cover `width` × `height`, and the (negative)
    /// margin on each side once they are centred over it: the matrix runs
    /// past the box, so the box's outline is what shapes it.
    pub fn cover(self, width: f32, height: f32) -> ((usize, usize), (f32, f32)) {
        let count = |span: f32| (((span + self.gap) / self.pitch()).ceil() as usize).max(1);
        let (cols, rows) = (count(width), count(height));
        let (w, h) = self.extent(cols, rows);
        ((cols, rows), ((width - w) / 2.0, (height - h) / 2.0))
    }

    /// The most dots that fit `width` × `height`, and the margin left on
    /// each side once they are centred in it.
    pub fn fit(self, width: f32, height: f32) -> ((usize, usize), (f32, f32)) {
        let count = |span: f32| (((span + self.gap) / self.pitch()).floor() as usize).max(1);
        let (cols, rows) = (count(width), count(height));
        let (w, h) = self.extent(cols, rows);
        ((cols, rows), ((width - w) / 2.0, (height - h) / 2.0))
    }
}

/// Room a matrix leaves for something drawn over it, in points from the
/// matrix's first dot. A control is a hole: no dots under it, so its own
/// fill reads cleanly. Text is soft: the grid stays, quieter, and its light
/// gives way, so the words sit in a calm patch of the same surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Skip {
    shape: Room,
    soft: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Room {
    Rect {
        left: f32,
        top: f32,
        right: f32,
        bottom: f32,
        radius: f32,
    },
    Circle {
        x: f32,
        y: f32,
        radius: f32,
    },
}

impl Skip {
    /// A box at `left`, `top`, `width` × `height` with corners of `radius`.
    pub fn rect(left: f32, top: f32, width: f32, height: f32, radius: f32) -> Self {
        Self {
            shape: Room::Rect {
                left,
                top,
                right: left + width,
                bottom: top + height,
                radius,
            },
            soft: false,
        }
    }

    /// A round control of `diameter` centred at `x`, `y`.
    pub fn circle(x: f32, y: f32, diameter: f32) -> Self {
        Self {
            shape: Room::Circle {
                x,
                y,
                radius: diameter / 2.0,
            },
            soft: false,
        }
    }

    /// The same room `dx`, `dy` points away.
    pub fn shifted(mut self, dx: f32, dy: f32) -> Self {
        self.shape = match self.shape {
            Room::Rect {
                left,
                top,
                right,
                bottom,
                radius,
            } => Room::Rect {
                left: left + dx,
                top: top + dy,
                right: right + dx,
                bottom: bottom + dy,
                radius,
            },
            Room::Circle { x, y, radius } => Room::Circle {
                x: x + dx,
                y: y + dy,
                radius,
            },
        };
        self
    }

    /// The room is for text: the grid stays under it, quieter.
    pub fn soft(mut self) -> Self {
        self.soft = true;
        self
    }

    /// How far, in points, `x`, `y` is from this room: below zero inside.
    fn clearance(&self, x: f32, y: f32) -> f32 {
        match self.shape {
            Room::Rect {
                left,
                top,
                right,
                bottom,
                radius,
            } => {
                let (cx, cy) = ((left + right) / 2.0, (top + bottom) / 2.0);
                let (hx, hy) = ((right - left) / 2.0, (bottom - top) / 2.0);
                let radius = radius.min(hx).min(hy);
                let qx = (x - cx).abs() - (hx - radius);
                let qy = (y - cy).abs() - (hy - radius);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                outside + qx.max(qy).min(0.0) - radius
            }
            Room::Circle {
                x: cx,
                y: cy,
                radius,
            } => ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - radius,
        }
    }
}

/// How the rooms in `skip` bear on a dot centred at `x`, `y` on `grid`:
/// `None` when a hole leaves it out, else how much light it keeps and how
/// present it stays at rest.
fn rooms(skip: &[Skip], grid: Grid, x: f32, y: f32) -> Option<(f32, f32)> {
    let feather = FEATHER * grid.pitch();
    let (mut light, mut presence) = (1.0f32, 1.0f32);
    for room in skip {
        let clear = room.clearance(x, y);
        if !room.soft && clear < 0.0 {
            return None;
        }
        let near = smooth(clear / feather);
        light = light.min(near);
        if room.soft {
            presence = presence.min(UNDER_TEXT + (1.0 - UNDER_TEXT) * near);
        }
    }
    Some((light, presence))
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A pointer resting on a matrix, in points from its first dot: the dots
/// under it swell and light, as a halftone's do under a thumb, and the
/// light holds and fades after the pointer moves on (`strength`, 0–1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Touch {
    pub x: f32,
    pub y: f32,
    pub strength: f32,
}

impl Touch {
    /// How much the touch lights a dot centred at `x`, `y` on `grid`: full
    /// under the pointer, easing out over [`TOUCH_REACH`].
    fn light(&self, grid: Grid, x: f32, y: f32) -> f32 {
        let reach = TOUCH_REACH * grid.pitch();
        let away = ((x - self.x).powi(2) + (y - self.y).powi(2)).sqrt() / reach;
        TOUCH_LIGHT * self.strength.clamp(0.0, 1.0) * smooth(1.0 - away)
    }

    /// The strength `held` seconds after the pointer last moved: it holds,
    /// then fades, as a lit cell does behind a cursor.
    pub fn strength_after(held: f32) -> f32 {
        const HOLD: f32 = 0.4;
        const FADE: f32 = 0.8;
        1.0 - ((held - HOLD) / FADE).clamp(0.0, 1.0)
    }
}

/// The modes a glyph has shown, oldest first, and when each began: a glyph
/// passes from one pattern to the next over [`SHIFT`], and a mode that
/// arrives mid-passage joins the blend rather than cutting it, so quick
/// successive changes stay continuous.
#[derive(Clone, Debug)]
struct Passage {
    shown: Vec<(Mode, f32)>,
}

impl Passage {
    fn new() -> Self {
        Self {
            shown: vec![(Mode::Muted, f32::NEG_INFINITY)],
        }
    }

    /// The mode shown now, once fully arrived.
    fn mode(&self) -> Mode {
        self.shown.last().map_or(Mode::Muted, |(mode, _)| *mode)
    }

    /// Whether any mode has been shown yet.
    fn begun(&self) -> bool {
        self.shown.len() > 1 || self.shown[0].1.is_finite()
    }

    fn shift(&mut self, mode: Mode, time: f32) {
        if self.begun() && self.mode() == mode {
            return;
        }
        if !self.begun() {
            self.shown[0] = (mode, time - SHIFT);
            return;
        }
        // Layers fully covered by a later one are gone.
        let settled = self
            .shown
            .iter()
            .rposition(|(_, since)| time - since >= SHIFT)
            .unwrap_or(0);
        self.shown.drain(..settled);
        self.shown.push((mode, time));
    }

    /// How far the newest mode has arrived, eased: 1 once there.
    fn arrived(&self, time: f32) -> f32 {
        self.shown
            .last()
            .map_or(1.0, |(_, since)| smooth((time - since) / SHIFT))
    }

    /// `pattern` of every mode shown, each later one laid over the earlier
    /// as far as it has arrived.
    fn blend(&self, time: f32, pattern: impl Fn(Mode) -> f32) -> f32 {
        let mut value = pattern(self.shown[0].0);
        for (mode, since) in &self.shown[1..] {
            let weight = smooth((time - since) / SHIFT);
            value += (pattern(*mode) - value) * weight;
        }
        value
    }
}

/// The glyph's two sets of row tints, top row first.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inks {
    pub agent: [Rgb; 3],
    pub user: [Rgb; 3],
}

impl Inks {
    /// Codex in sunrise, the user in the accent's glyph rows `accent`,
    /// lifted: the accent's deep row is near the page's own darkness.
    pub fn new(accent: [Rgb; 3]) -> Self {
        Self {
            agent: SUNRISE,
            user: accent.map(|tint| mix(tint, [1.0; 3], 0.22)),
        }
    }
}

fn mix(from: Rgb, to: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [0, 1, 2].map(|i| from[i] + (to[i] - from[i]) * t)
}

/// The tint of the dot at `col`, `row`: one of the palette's three, scattered
/// over the grid at random and fixed to the dot, so the colours mingle as a
/// print's inks do and never move. A dot is ever one of its palette's
/// colours, never a blend between them.
fn tint(rows: [Rgb; 3], col: f32, row: f32) -> Rgb {
    // A small integer hash of the dot's place: even over the grid, the same
    // every frame.
    let mut h = (col as i32 as u32).wrapping_mul(0x9E37_79B1)
        ^ (row as i32 as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    rows[(h % 3) as usize]
}

/// Whose ink a dot takes when both voices light it: the louder's, whole.
fn ink(agent: Rgb, user: Rgb, share: f32) -> Rgb {
    if share >= 0.5 { user } else { agent }
}

/// A band's level `at` 0 (lowest) to 1 (highest), between bands.
fn band(bands: &Bands, at: f32) -> f32 {
    let at = at.clamp(0.0, 1.0) * (BANDS - 1) as f32;
    let low = at.floor() as usize;
    let high = (low + 1).min(BANDS - 1);
    bands[low] + (bands[high] - bands[low]) * (at - low as f32)
}

fn body(bands: &Bands) -> f32 {
    bands.iter().sum::<f32>() / BANDS as f32
}

/// A voice's level as a dot shows it: the hush below the first few percent
/// stays at rest, and the rest rises steeply, so a voice reads as dots
/// lighting up rather than a grid glowing evenly.
fn shape(level: f32) -> f32 {
    smooth((level - 0.06) / 0.84)
}

/// One dot: where it sits in dots, how lit it is (0 at rest, 1 full), its
/// colour, and how present it is at rest (lower under text).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub col: f32,
    pub row: f32,
    pub level: f32,
    pub rgb: Rgb,
    pub presence: f32,
}

/// What lit a dot: each voice's share and the mode's own motion.
fn lit(user: f32, agent: f32, idle: f32, inks: &Inks, (col, row): (f32, f32)) -> (f32, Rgb) {
    let (user, agent) = (shape(user), shape(agent));
    let voice = user.max(agent);
    let level = voice.max(idle).clamp(0.0, 1.0);
    let share = if voice > 0.02 {
        user / (user + agent)
    } else {
        0.0
    };
    (
        level,
        ink(tint(inks.agent, col, row), tint(inks.user, col, row), share),
    )
}

/// The level a mode gives a field's dot `out` from its heart (0) to its
/// reach (1), `turn` round it (0–1, clockwise from the top), at `time`. Each
/// pattern says what the call is doing, and all are smooth in space, as a
/// voice is, so one passes into the next without a seam. A dot only starts
/// to light above a few hundredths ([`shape`]): what must stay visible at
/// rest sits just over that, well under a voice.
fn calm(mode: Mode, out: f32, turn: f32, time: f32) -> f32 {
    let bell = |d: f32, width: f32| (-(d / width) * (d / width)).exp();
    // A slow shimmer drifting across the field, so it reads as a surface
    // that lives rather than a grid that waits; still while muted.
    let shimmer = if mode == Mode::Muted {
        0.0
    } else {
        let (x, y) = (out * (turn * TAU).sin(), out * (turn * TAU).cos());
        let drift = ((x * 4.1 + time * 0.37).sin() * (y * 3.7 - time * 0.29).cos() + 1.0) / 2.0;
        SHIMMER * drift * (1.0 - out * out).max(0.0)
    };
    shimmer
        + match mode {
            // Calling: rings leave the heart and fade as they spread.
            Mode::Connecting => (0..2)
                .map(|k| {
                    let at = (time / RING_PERIOD + k as f32 * 0.5).fract();
                    0.6 * (1.0 - at) * bell(out - at, 0.1)
                })
                .fold(0.0, f32::max),
            // Ready: the heart breathes, the swell easing out from it.
            // Ready: the heart breathes over a floor of light that never goes
            // out, so a live call at rest is never mistaken for a muted one.
            Mode::Listening => {
                let breath = 0.5 - 0.5 * ((time / BREATH_PERIOD - out * 0.35) * TAU).cos();
                (0.12 + 0.24 * breath) * (1.0 - out * out).max(0.0)
            }
            // Working: a light runs round the heart, its trail fading behind
            // it, and the heart itself breathes slowly under it.
            Mode::Thinking => {
                // Its head is soft: a dot ahead of it lights as it nears,
                // never in one frame.
                let behind = (time / SWEEP_PERIOD - turn).rem_euclid(1.0);
                let head = (-behind * 3.0).exp().max(bell(1.0 - behind, 0.06));
                let comet = 0.9 * head * bell(out - 0.6, 0.22);
                let heart = 0.12 + 0.08 * (0.5 - 0.5 * (time / BREATH_PERIOD * TAU).cos());
                comet + heart * bell(out, 0.32)
            }
            // Your turn: a ring at the rim holds its glow and beats twice,
            // then rests, like a knock at a door.
            Mode::Asking => {
                let at = (time / KNOCK_PERIOD).fract();
                let knock = bell(at - 0.08, 0.07).max(0.7 * bell(at - 0.28, 0.07));
                let ring = bell(out - 0.78, 0.16 + 0.06 * knock);
                (0.3 + 0.55 * knock) * ring + 0.06 * smooth((out - 0.15) / 0.7)
            }
            Mode::Processing | Mode::Muted => 0.0,
        }
}

/// Each band rises with the voice at once and falls back over [`RELEASE`].
fn hold(held: &mut Voice, voice: &Voice, dt: f32) {
    hold_over(held, voice, dt, RELEASE);
}

/// Keeps `voice` in `held`, each band falling back over `release` seconds.
fn hold_over(held: &mut Voice, voice: &Voice, dt: f32, release: f32) {
    let fall = (-dt / release).exp();
    for (held, heard) in [
        (&mut held.user, &voice.user),
        (&mut held.agent, &voice.agent),
    ] {
        for (held, heard) in held.iter_mut().zip(heard) {
            *held = heard.max(*held * fall);
        }
    }
}

/// Seconds since the last call, at most a quarter: a stalled frame does not
/// leap.
fn step(at: &mut Option<Instant>, now: Instant) -> f32 {
    at.replace(now)
        .map_or(0.0, |at| now.saturating_duration_since(at).as_secs_f32())
        .min(0.25)
}

/// How a field's rings run.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Rings {
    /// Round, from the centre: a call's disc.
    Round,
    /// Concentric with a rounded rectangle of the field's own size, its
    /// corners `radius` dots round: on a phone, rings the shape of the
    /// screen, its heart a line down the middle.
    Screen(f32),
}

/// A call's field: the last voices heard, newest first, one per [`PUSH`].
#[derive(Clone)]
pub struct Field {
    rings: Rings,
    /// How much of a sound is lost by the field's reach.
    falloff: f32,
    /// No edge: the grid thins out with distance from the heart, and a
    /// louder voice carries it further.
    dissolve: bool,
    /// The dots near the outline go quiet, so the field eases into what
    /// holds it rather than stopping at a hard ring.
    soft_rim: bool,
    /// How present the dots are at rest, against the surface they sit on:
    /// quieter where that surface is darker and the grid would show.
    rest: f32,
    heard: VecDeque<Voice>,
    held: Voice,
    /// How loud the call has been lately: it rises with a voice at once and
    /// falls back slowly, so sustained speech carries the field further out
    /// than a word does.
    loud: f32,
    at: Option<Instant>,
    owed: f32,
    time: f32,
    /// The modes shown, passing from each to the next.
    passage: Passage,
}

impl Field {
    pub fn new(rings: Rings) -> Self {
        Self {
            rings,
            falloff: 0.35,
            dissolve: false,
            soft_rim: false,
            rest: 1.0,
            heard: VecDeque::new(),
            held: Voice::default(),
            loud: 0.0,
            at: None,
            owed: 0.0,
            time: 0.0,
            passage: Passage::new(),
        }
    }

    /// Shows `mode` from now on, passing into it from the pattern shown over
    /// [`SHIFT`] rather than cutting to it.
    pub fn shift(&mut self, mode: Mode) {
        self.passage.shift(mode, self.time);
    }

    /// How far the field has passed into `mode`, eased: 1 once there.
    fn shifted(&self, mode: Mode) -> f32 {
        if self.passage.mode() != mode {
            return 1.0;
        }
        self.passage.arrived(self.time)
    }

    /// Loses `share` of a sound by the field's reach: near 1, only its heart
    /// lights.
    pub fn soft_rimmed(mut self) -> Self {
        self.soft_rim = true;
        self
    }

    pub fn dissolving(mut self) -> Self {
        self.dissolve = true;
        self
    }

    /// Keeps `share` of the dots' presence at rest: on a dark surface the
    /// unlit grid would otherwise read as a mesh.
    pub fn resting(mut self, share: f32) -> Self {
        self.rest = share.clamp(0.0, 1.0);
        self
    }

    pub fn falling(mut self, share: f32) -> Self {
        self.falloff = share.clamp(0.0, 1.0);
        self
    }

    /// Runs the rings `rings` from now on: a screen's follow its corners.
    pub fn set_rings(&mut self, rings: Rings) {
        self.rings = rings;
    }

    /// Hears `voice` at `now`; the field keeps what has not yet left it.
    pub fn hear(&mut self, voice: Voice, now: Instant) {
        let dt = step(&mut self.at, now);
        self.time += dt;
        self.owed += dt;
        hold(&mut self.held, &voice, dt);
        let voice = self.held;
        let level = body(&voice.user).max(body(&voice.agent));
        self.loud = level.max(self.loud * (-dt / LOUD_RELEASE).exp());
        if self.heard.is_empty() {
            self.heard.push_front(voice);
        }
        while self.owed >= PUSH {
            self.owed -= PUSH;
            self.heard.push_front(voice);
        }
        // The newest sound replaces the one being pushed out this step.
        self.heard[0] = voice;
        self.heard.truncate(RINGS as usize * 3);
    }

    /// The voice `ring` pushes out from the heart, between the two rings it
    /// falls between.
    fn heard_at(&self, ring: f32) -> Voice {
        let at = |i: usize| self.heard.get(i).copied().unwrap_or_default();
        let (near, t) = (ring.floor() as usize, ring.fract());
        let (a, b) = (at(near), at(near + 1));
        let blend = |a: &Bands, b: &Bands| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        Voice {
            user: blend(&a.user, &b.user),
            agent: blend(&a.agent, &b.agent),
        }
    }

    /// Whether anything is still moving: a sound in the field or a mode that
    /// animates on its own.
    pub fn moving(&self, mode: Mode) -> bool {
        mode != Mode::Muted && mode != Mode::Processing
            || self.shifted(mode) < 1.0
            || self
                .held
                .user
                .iter()
                .chain(&self.held.agent)
                .any(|level| *level > 0.01)
    }

    /// How far out a dot `x`, `y` dots from the centre of a `cols` × `rows`
    /// field sits, 0 at its heart, 1 at its reach.
    fn reach(&self, x: f32, y: f32, cols: f32, rows: f32) -> f32 {
        let (hx, hy) = (((cols - 1.0) / 2.0).max(1.0), ((rows - 1.0) / 2.0).max(1.0));
        match self.rings {
            Rings::Round => (x * x + y * y).sqrt() / hx.min(hy),
            Rings::Screen(radius) => {
                let radius = radius.min(hx).min(hy);
                let qx = x.abs() - (hx - radius);
                let qy = y.abs() - (hy - radius);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let edge = outside + qx.max(qy).min(0.0) - radius;
                1.0 + edge / hx.min(hy)
            }
        }
    }

    /// The field's dots across `cols` × `rows` on `grid`, inside `outline`
    /// and around `skip`, for `mode`; the user's voice is left out while
    /// muted.
    #[allow(clippy::too_many_arguments)]
    pub fn cells(
        &self,
        cols: usize,
        rows: usize,
        grid: Grid,
        mode: Mode,
        inks: &Inks,
        outline: Outline,
        skip: &[Skip],
    ) -> Vec<Cell> {
        self.cells_touched(cols, rows, grid, mode, inks, outline, skip, None)
    }

    /// [`Self::cells`] with a pointer `touch` resting on the field.
    #[allow(clippy::too_many_arguments)]
    pub fn cells_touched(
        &self,
        cols: usize,
        rows: usize,
        grid: Grid,
        mode: Mode,
        inks: &Inks,
        outline: Outline,
        skip: &[Skip],
        touch: Option<Touch>,
    ) -> Vec<Cell> {
        let (fc, fr) = (cols as f32, rows as f32);
        let (cx, cy) = ((fc - 1.0) / 2.0, (fr - 1.0) / 2.0);
        let rings = cx.max(cy).min(RINGS);
        // A dissolving field reaches further the louder it is heard.
        let reach = if self.dissolve {
            let loud = body(&self.held.user).max(body(&self.held.agent));
            DISSOLVE.0 + DISSOLVE.1 * shape(loud)
        } else {
            1.0
        };
        let shifted = self.shifted(mode);
        // Sustained speech carries further: the field loses less of a voice
        // by its reach the louder the call has lately been, so a run of
        // words fills the disc where a single one lights its heart.
        let falloff = self.falloff * (1.0 - SWELL * shape(self.loud));
        let mut cells = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            for col in 0..cols {
                let (c, r) = (col as f32, row as f32);
                let edge = outline.edge(c, r, cols, rows);
                if edge <= 0.0 {
                    continue;
                }
                let centre = (
                    c * grid.pitch() + grid.dot / 2.0,
                    r * grid.pitch() + grid.dot / 2.0,
                );
                let Some((room, presence)) = rooms(skip, grid, centre.0, centre.1) else {
                    continue;
                };
                let (x, y) = (c - cx, r - cy);
                let out = self.reach(x, y, fc, fr).max(0.0) / reach;
                // Past its reach a dissolving field thins out to nothing, so
                // it has no edge against what is behind it.
                let thin = if self.dissolve {
                    smooth((DISSOLVE.2 - out) / (DISSOLVE.2 - DISSOLVE.3))
                } else {
                    1.0
                };
                if thin <= 0.0 {
                    continue;
                }
                let rim = if self.soft_rim {
                    0.65 + 0.35 * smooth((1.0 - out * reach) / 0.2)
                } else {
                    1.0
                };
                // A sound reaches this ring this many pushes after the heart,
                // weakening as it spreads and dying out past the reach. A dot
                // between two rings hears both, so the field is as smooth
                // in space as the voice is in time.
                let voice = self.heard_at(out * rings);
                let fade = if out > 1.0 {
                    ((1.6 - out) / 0.6).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let spread = (1.0 - falloff * out.min(1.0).powi(2)) * fade * room;
                let carry = |bands: &Bands| {
                    (BAND_SHARE * band(bands, out) + (1.0 - BAND_SHARE) * body(bands)) * spread
                };
                let user = if mode == Mode::Muted {
                    0.0
                } else {
                    carry(&voice.user)
                };
                let agent = carry(&voice.agent);
                // The mode's own pattern, passing over from the last one's.
                let turn = (x / cx.max(1.0)).atan2(-y / cy.max(1.0)).rem_euclid(TAU) / TAU;
                let pattern = |mode| calm(mode, out.min(1.0), turn, self.time);
                let calm = if shifted < 1.0 {
                    self.passage.blend(self.time, pattern)
                } else {
                    pattern(mode)
                } * fade.max(0.3)
                    * room;
                let (level, rgb) = lit(user, agent, calm, inks, (c, r));
                // The pointer's light adds to the voice's and brings the
                // dots under it fully present, whatever their rest.
                let touched = touch.map_or(0.0, |touch| touch.light(grid, centre.0, centre.1));
                let presence = presence * thin * rim * edge * self.rest;
                cells.push(Cell {
                    col: c,
                    row: r,
                    level: (level + touched).min(1.0) * thin * edge,
                    rgb,
                    presence: presence.max(touched * thin * edge),
                });
            }
        }
        cells
    }
}

/// The level a mode gives a beam's column `x` across it (-1 to 1) at
/// `time`: a field's patterns, told along a line.
fn calm_along(mode: Mode, x: f32, time: f32) -> f32 {
    let bell = |d: f32, width: f32| (-(d / width) * (d / width)).exp();
    let out = x.abs();
    match mode {
        // Calling: pulses leave the centre for both ends.
        Mode::Connecting => (0..2)
            .map(|k| {
                let at = (time / RING_PERIOD + k as f32 * 0.5).fract();
                0.55 * (1.0 - 0.6 * at) * bell(out - at, 0.14)
            })
            .fold(0.0, f32::max),
        // Ready: the centre breathes.
        Mode::Listening => {
            let breath = 0.5 - 0.5 * ((time / BREATH_PERIOD - out * 0.35) * TAU).cos();
            0.1 + 0.12 * breath * (1.0 - out * out)
        }
        // Working: a light runs from side to side.
        Mode::Thinking => 0.55 * bell(x - (time / SWEEP_PERIOD * TAU).sin() * 0.7, 0.35),
        // Your turn: both ends beat twice, then rest.
        Mode::Asking => {
            let at = (time / KNOCK_PERIOD).fract();
            let knock = bell(at - 0.08, 0.05).max(0.7 * bell(at - 0.24, 0.05));
            0.04 + 0.45 * knock * smooth((out - 0.2) / 0.7)
        }
        // Transcribing: a highlight crosses, end to end.
        Mode::Processing => {
            0.55 * bell(x - ((time / SWEEP_PERIOD).fract() * 2.0 - 1.0) * 1.2, 0.35)
        }
        Mode::Muted => 0.04,
    }
}

/// An edge's beam: the voice as it is now, held through its troughs.
#[derive(Clone)]
pub struct Beam {
    held: Voice,
    at: Option<Instant>,
    time: f32,
    /// How present the dots are at rest, against the surface they sit on.
    rest: f32,
    /// The modes shown, passing from each to the next.
    passage: Passage,
}

impl Default for Beam {
    fn default() -> Self {
        Self {
            held: Voice::default(),
            at: None,
            time: 0.0,
            rest: 1.0,
            passage: Passage::new(),
        }
    }
}

impl Beam {
    pub fn hear(&mut self, voice: Voice, now: Instant) {
        let dt = step(&mut self.at, now);
        self.time += dt;
        hold_over(&mut self.held, &voice, dt, BEAM_RELEASE);
    }

    /// Shows `mode` from now on, passing into it over [`SHIFT`].
    pub fn shift(&mut self, mode: Mode) {
        self.passage.shift(mode, self.time);
    }

    /// Keeps `share` of the dots' presence at rest: over the page, with no
    /// surface of its own, the unlit beam would otherwise read as a strip.
    pub fn resting(mut self, share: f32) -> Self {
        self.rest = share.clamp(0.0, 1.0);
        self
    }

    /// Whether anything is still moving: a sound in the beam, a passage
    /// between modes, or a mode that animates on its own.
    pub fn moving(&self, mode: Mode) -> bool {
        mode != Mode::Muted
            || (self.passage.mode() == mode && self.passage.arrived(self.time) < 1.0)
            || self
                .held
                .user
                .iter()
                .chain(&self.held.agent)
                .any(|level| *level > 0.01)
    }

    /// The beam's dots across `cols` × `rows`, inside `outline`, rising from
    /// the bottom row: the lowest band at the centre, the highest at the
    /// ends. Quiet, it shows its mode's pattern ([`calm_along`]).
    pub fn cells(
        &self,
        cols: usize,
        rows: usize,
        mode: Mode,
        inks: &Inks,
        outline: Outline,
    ) -> Vec<Cell> {
        self.cells_over(cols, rows, (cols as f32, rows as f32), mode, inks, outline)
    }

    /// [`Self::cells`] for a matrix covering a box `area` dots wide and
    /// tall ([`Grid::cover`]): the box's outline, not the matrix's, is where
    /// the beam fades out, so it fills the box to its curves.
    pub fn cells_over(
        &self,
        cols: usize,
        rows: usize,
        area: (f32, f32),
        mode: Mode,
        inks: &Inks,
        outline: Outline,
    ) -> Vec<Cell> {
        let centre = (cols as f32 - 1.0) / 2.0;
        let mut cells = Vec::with_capacity(cols * rows);
        let shifted = if self.passage.mode() == mode {
            self.passage.arrived(self.time)
        } else {
            1.0
        };
        for col in 0..cols {
            let x = (col as f32 - centre) / centre.max(1.0);
            let out = x.abs();
            // Bells around the centre, narrower the higher the band.
            let reach = |bands: &Bands| {
                shape(band(bands, out) * 0.7 + body(bands) * 0.5) * (1.0 - out * out * 0.6)
            };
            let user = if mode == Mode::Muted {
                0.0
            } else {
                reach(&self.held.user)
            };
            let agent = reach(&self.held.agent);
            let calm = if shifted < 1.0 {
                self.passage
                    .blend(self.time, |mode| calm_along(mode, x, self.time))
            } else {
                calm_along(mode, x, self.time)
            };
            let height = user.max(agent).max(calm);
            let share = if user + agent > 0.02 {
                user / (user + agent)
            } else {
                0.0
            };
            for row in 0..rows {
                let edge = outline.edge_in(col as f32, row as f32, cols, rows, area);
                if edge <= 0.0 {
                    continue;
                }
                // Up from the bottom: the top of the column fades out.
                let up = (rows - 1 - row) as f32 / rows as f32;
                // A full voice reaches the top row: the strip is the beam's,
                // with no quiet band left above it.
                let level =
                    ((height * 1.15 - up) * rows as f32 * 0.6).clamp(0.0, 1.0) * height.sqrt();
                let (c, r) = (col as f32, row as f32);
                cells.push(Cell {
                    col: c,
                    row: r,
                    level: level * edge,
                    rgb: ink(tint(inks.agent, c, r), tint(inks.user, c, r), share),
                    presence: edge * self.rest,
                });
            }
        }
        cells
    }
}

/// What to paint for a dot, the same on every platform: a filled circle,
/// nothing else. Light is told by colour, not glow: a dot at rest is a quiet,
/// greyed version of its row's tint; lit, it is saturated and bright.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    /// The dot's colour and opacity.
    pub fill: Rgb,
    pub alpha: f32,
    /// Its diameter as a share of the grid's dot.
    pub size: f32,
}

/// How `cell` is painted: its ink as it is, one of its palette's colours,
/// at an opacity and a size that tell its light. Nothing whitens or greys
/// it, so a working Codex is the transcript's 3×3 glyph writ large.
pub fn look(cell: &Cell) -> Look {
    let level = cell.level.clamp(0.0, 1.0);
    Look {
        fill: cell.rgb,
        alpha: (REST + (1.0 - REST) * level) * (cell.presence + (1.0 - cell.presence) * level),
        size: DOT_REST + (DOT_SWELL - DOT_REST) * level.sqrt(),
    }
}

/// A voice of which only the loudness is known, split by how fast that
/// loudness moves: the slow bands carry the phrase, the middle one the
/// level itself, the fast ones each syllable's attack and its flutter —
/// heavy to fine, as a spectrum's bands run from low to high. Codex's voice
/// is only ever known this way; the user's is, wherever the microphone's own
/// spectrum is not to hand.
#[derive(Default)]
pub struct Rhythm {
    phrase: f32,
    syllable: f32,
    quick: f32,
    /// How much the level has lately swung about its syllable follower:
    /// the attack and flutter bands are read against it, so fast, even
    /// speech, whose swings are small, still moves them.
    swing: f32,
    at: Option<Instant>,
    bands: Bands,
}

/// The peak, in decibels, that reads as silence and the one that fills the
/// scale: full scale itself, so loud, continuous speech (whose peaks sit
/// well above -8 dB) still rises and falls instead of pinning at the top.
const SCALE: (f32, f32) = (-46.0, 0.0);
/// Seconds over which a phrase, a syllable and an attack are told apart.
const PACE: (f32, f32, f32) = (0.45, 0.14, 0.05);
/// Seconds over which the swing is averaged, the swing a plain word makes,
/// and how far the fast bands may be lifted when the swing is small.
const SWING: (f32, f32, f32) = (1.0, 0.1, 4.0);

impl Rhythm {
    /// Hears a peak, 0–1, at `now`.
    pub fn hear(&mut self, peak: f32, now: Instant) {
        let loud = 20.0 * peak.max(1e-6).log10();
        let level = ((loud - SCALE.0) / (SCALE.1 - SCALE.0)).clamp(0.0, 1.0);
        let dt = step(&mut self.at, now);
        for (follower, pace) in [
            (&mut self.phrase, PACE.0),
            (&mut self.syllable, PACE.1),
            (&mut self.quick, PACE.2),
        ] {
            *follower += (level - *follower) * (1.0 - (-dt / pace).exp());
        }
        // The phrase is the heart of a field: it rises with the voice at
        // once and only its fall is slow, so a word lights the heart as it
        // is said and the heart holds through the troughs between them.
        self.phrase = self.phrase.max(level);
        // A run of quick words swings less than slow ones: the fast bands are
        // read against the swing's own size, so they keep moving at speed
        // and do not blow up in silence.
        let swing = (level - self.syllable).abs();
        self.swing += (swing - self.swing) * (1.0 - (-dt / SWING.0).exp());
        let gain = if level > 0.1 {
            (SWING.1 / self.swing.max(1e-3)).clamp(1.0, SWING.2)
        } else {
            1.0
        };
        // The syllable's beat against the swing's own size: a full pulse
        // whether the speech is slow and wide or fast and even, so the heart
        // keeps time with the words instead of settling at their loudness.
        let beat = if level > 0.1 {
            (0.5 + 0.5 * (level - self.syllable) / self.swing.max(0.02)).clamp(0.0, 1.0)
        } else {
            0.5
        };
        self.bands = [
            self.phrase * (0.5 + 0.5 * beat),
            self.syllable * (0.35 + 0.65 * beat),
            level,
            ((level - self.syllable) * 2.5 * gain).clamp(0.0, 1.0),
            ((level - self.quick).abs() * 4.0 * gain).clamp(0.0, 1.0),
        ];
    }

    pub fn bands(&self) -> Bands {
        self.bands
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn inks() -> Inks {
        Inks {
            agent: [[1.0, 0.0, 0.0]; 3],
            user: [[0.0, 0.0, 1.0]; 3],
        }
    }

    fn at(cells: &[Cell], row: usize, col: usize) -> Cell {
        *cells
            .iter()
            .find(|cell| cell.row == row as f32 && cell.col == col as f32)
            .expect("a kept dot")
    }

    const GRID: Grid = Grid { dot: 6.0, gap: 2.0 };

    #[test]
    fn a_sound_starts_at_the_centre_and_is_pushed_out_ring_by_ring() {
        let start = Instant::now();
        let mut field = Field::new(Rings::Round);
        let (quiet, loud) = (
            Voice::default(),
            Voice {
                user: [0.0; BANDS],
                agent: [1.0; BANDS],
            },
        );
        field.hear(quiet, start);
        field.hear(quiet, start + Duration::from_millis(200));
        field.hear(loud, start + Duration::from_millis(210));
        let cells =
            |field: &Field| field.cells(9, 9, GRID, Mode::Processing, &inks(), Outline::Disc, &[]);
        let now = cells(&field);
        assert!(at(&now, 4, 4).level > 0.6, "centre lit at once");
        assert!(at(&now, 4, 8).level < 0.05, "edge not yet");
        field.hear(loud, start + Duration::from_millis(210 + 6 * 70));
        let later = cells(&field);
        assert!(at(&later, 4, 7).level > 0.3, "edge reached");
        assert!(
            at(&later, 4, 8).level < at(&later, 4, 7).level,
            "the outer dot fades where it touches the outline"
        );
        assert_eq!(
            at(&later, 4, 4).rgb,
            [1.0, 0.0, 0.0],
            "Codex in its own ink"
        );
    }

    #[test]
    fn the_user_is_told_apart_by_colour_and_hidden_while_muted() {
        let mut field = Field::new(Rings::Round);
        field.hear(
            Voice {
                user: [0.9; BANDS],
                agent: [0.0; BANDS],
            },
            Instant::now(),
        );
        let cells = field.cells(5, 5, GRID, Mode::Listening, &inks(), Outline::Square, &[]);
        assert!(at(&cells, 2, 2).level > 0.6);
        assert_eq!(at(&cells, 2, 2).rgb, [0.0, 0.0, 1.0]);
        let muted = field.cells(5, 5, GRID, Mode::Muted, &inks(), Outline::Square, &[]);
        assert!(muted.iter().all(|cell| cell.level < 0.01));
    }

    #[test]
    fn a_matrix_takes_the_outline_of_what_holds_it() {
        let kept = |outline: Outline, cols, rows| {
            (0..rows)
                .flat_map(|row| (0..cols).map(move |col| (col, row)))
                .filter(|&(col, row)| outline.keeps(col as f32, row as f32, cols, rows))
                .count()
        };
        // A 5×5 disc is the 21-dot LED circle.
        assert_eq!(kept(Outline::Disc, 5, 5), 21);
        // A pill keeps its straight middle and rounds both ends alike.
        let pill = |col: usize, row: usize| Outline::Stadium.keeps(col as f32, row as f32, 20, 5);
        assert!(pill(10, 0) && pill(10, 4) && pill(0, 2) && pill(19, 2));
        assert!(!pill(0, 0) && !pill(19, 0) && !pill(0, 4) && !pill(19, 4));
        // A card's corners are cut as round as the card.
        assert!(!Outline::Rounded(3.0).keeps(0.0, 0.0, 30, 10));
        assert!(Outline::Rounded(3.0).keeps(3.0, 0.0, 30, 10));
        // A strip under a card keeps its top corners and rounds its bottom.
        assert!(Outline::Under(3.0).keeps(0.0, 0.0, 60, 4));
        assert!(!Outline::Under(3.0).keeps(0.0, 3.0, 60, 4));
    }

    #[test]
    fn a_dissolving_field_has_no_edge_and_a_voice_carries_it_further() {
        let mut field = Field::new(Rings::Round).dissolving();
        let count = |field: &Field| {
            field
                .cells(41, 41, GRID, Mode::Listening, &inks(), Outline::Square, &[])
                .iter()
                .filter(|cell| cell.presence > 0.05)
                .count()
        };
        let quiet = count(&field);
        let cells = field.cells(41, 41, GRID, Mode::Listening, &inks(), Outline::Square, &[]);
        assert!(
            !cells.iter().any(|cell| cell.row == 0.0 && cell.col == 0.0),
            "the corners are gone"
        );
        let edge = cells
            .iter()
            .filter(|cell| cell.row == 20.0)
            .map(|cell| cell.presence);
        assert!(edge.fold(1.0f32, f32::min) < 0.2, "it thins out, no edge");
        field.hear(
            Voice {
                user: [0.0; BANDS],
                agent: [1.0; BANDS],
            },
            Instant::now(),
        );
        assert!(count(&field) > quiet, "a voice carries it further");
    }

    #[test]
    fn controls_are_holes_and_text_is_a_quiet_patch() {
        let mut field = Field::new(Rings::Screen(4.0));
        field.hear(
            Voice {
                user: [0.0; BANDS],
                agent: [1.0; BANDS],
            },
            Instant::now(),
        );
        let button = Skip::circle(80.0, 200.0, 64.0);
        let words = Skip::rect(16.0, 40.0, 120.0, 40.0, 8.0).soft();
        let cells = field.cells(
            20,
            40,
            GRID,
            Mode::Listening,
            &inks(),
            Outline::Square,
            &[button, words],
        );
        let centre = |cell: &Cell| (cell.col * 8.0 + 3.0, cell.row * 8.0 + 3.0);
        // No dot under the button.
        assert!(cells.iter().all(|cell| {
            let (x, y) = centre(cell);
            ((x - 80.0).powi(2) + (y - 200.0).powi(2)).sqrt() >= 32.0
        }));
        // Under the words the grid stays, quiet and unlit.
        let under: Vec<_> = cells
            .iter()
            .filter(|cell| {
                let (x, y) = centre(cell);
                (30.0..120.0).contains(&x) && (50.0..70.0).contains(&y)
            })
            .collect();
        assert!(!under.is_empty());
        assert!(
            under
                .iter()
                .all(|cell| cell.level < 0.05 && cell.presence < 0.5)
        );
    }

    #[test]
    fn a_grid_fills_its_box_and_centres_what_is_left() {
        let ((cols, rows), (mx, my)) = Grid::dense(3.0).fit(100.0, 20.0);
        assert_eq!((cols, rows), (25, 5));
        assert!((0.0..4.0).contains(&mx) && (0.0..4.0).contains(&my));
    }

    #[test]
    fn a_voice_known_only_by_its_loudness_is_split_by_its_rhythm() {
        let start = Instant::now();
        let mut rhythm = Rhythm::default();
        let mut attack = 0.0f32;
        for tick in 0..100u32 {
            let t = tick as f32 * 0.04;
            let peak = 0.004 + 0.3 * (t * 5.0 * TAU).sin().max(0.0);
            rhythm.hear(peak, start + Duration::from_millis(u64::from(tick) * 40));
            attack = attack.max(rhythm.bands()[3]);
            assert!(tick < 30 || rhythm.bands()[0] > 0.2, "{:?}", rhythm.bands());
        }
        assert!(attack > 0.5, "{attack}");
        for tick in 100..150u32 {
            rhythm.hear(0.0, start + Duration::from_millis(u64::from(tick) * 40));
        }
        assert!(rhythm.bands().iter().all(|band| *band < 0.02));
    }

    #[test]
    fn a_mode_arriving_mid_passage_joins_the_blend_without_a_jump() {
        let inks = Inks::new(SUNRISE);
        let start = Instant::now();
        let mut field = Field::new(Rings::Round);
        let step = |field: &mut Field, ms: u64| {
            field.hear(Voice::default(), start + Duration::from_millis(ms))
        };
        step(&mut field, 0);
        field.shift(Mode::Thinking);
        step(&mut field, 300);
        // Halfway into Thinking, Asking arrives: the next frame continues
        // from the blend as it stood, rather than cutting to a new passage.
        let before = field.cells(21, 21, GRID, Mode::Thinking, &inks, Outline::Disc, &[]);
        field.shift(Mode::Asking);
        step(&mut field, 316);
        let after = field.cells(21, 21, GRID, Mode::Asking, &inks, Outline::Disc, &[]);
        let jump = before
            .iter()
            .zip(&after)
            .map(|(a, b)| (a.level - b.level).abs())
            .fold(0.0f32, f32::max);
        assert!(jump < 0.3, "a frame's change stays small: {jump}");
        // Once the passage is over, Asking alone shows (a frame at a time:
        // a stalled frame never leaps).
        for ms in (350..=2000).step_by(50) {
            step(&mut field, ms);
        }
        let settled = field.cells(21, 21, GRID, Mode::Asking, &inks, Outline::Disc, &[]);
        let mut plain = Field::new(Rings::Round);
        step(&mut plain, 0);
        plain.shift(Mode::Asking);
        step(&mut plain, 300);
        step(&mut plain, 316);
        for ms in (350..=2000).step_by(50) {
            step(&mut plain, ms);
        }
        let same = plain.cells(21, 21, GRID, Mode::Asking, &inks, Outline::Disc, &[]);
        for (a, b) in settled.iter().zip(&same) {
            assert!((a.level - b.level).abs() < 1e-3);
        }
    }

    #[test]
    fn a_touch_lights_the_dots_under_it_and_lets_go_after() {
        let field = Field::new(Rings::Round).resting(0.4);
        let inks = Inks::new(SUNRISE);
        let at = |touch: Option<Touch>| {
            field.cells_touched(21, 21, GRID, Mode::Muted, &inks, Outline::Disc, &[], touch)
        };
        let quiet = at(None);
        let (x, y) = (
            3.0 * GRID.pitch() + GRID.dot / 2.0,
            10.0 * GRID.pitch() + GRID.dot / 2.0,
        );
        let touched = at(Some(Touch {
            x,
            y,
            strength: 1.0,
        }));
        let dot = |cells: &[Cell], col: f32, row: f32| {
            look(cells.iter().find(|c| c.col == col && c.row == row).unwrap())
        };
        // Under the pointer the dot swells and lights; across the field it
        // is as it was.
        assert!(dot(&touched, 3.0, 10.0).size > dot(&quiet, 3.0, 10.0).size + 0.2);
        assert!(dot(&touched, 3.0, 10.0).alpha > dot(&quiet, 3.0, 10.0).alpha);
        assert_eq!(dot(&touched, 16.0, 10.0), dot(&quiet, 16.0, 10.0));
        // It holds for a moment, then fades out.
        assert_eq!(Touch::strength_after(0.2), 1.0);
        assert!(Touch::strength_after(0.8) > 0.0 && Touch::strength_after(0.8) < 1.0);
        assert_eq!(Touch::strength_after(1.5), 0.0);
    }
}
