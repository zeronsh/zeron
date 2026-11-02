//! Port of Ghostty's src/font/sprite/draw/common.zig: helpers shared by the
//! glyph families.

use super::Metrics;
use super::canvas::{Canvas, Color};

pub const ONE_EIGHTH: f64 = 0.125;
pub const ONE_QUARTER: f64 = 0.25;
pub const ONE_THIRD: f64 = 1.0 / 3.0;
pub const THREE_EIGHTHS: f64 = 0.375;
pub const HALF: f64 = 0.5;
pub const FIVE_EIGHTHS: f64 = 0.625;
pub const TWO_THIRDS: f64 = 2.0 / 3.0;
pub const THREE_QUARTERS: f64 = 0.75;
pub const SEVEN_EIGHTHS: f64 = 0.875;

/// The thickness of a line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Thickness {
    SuperLight,
    Light,
    Heavy,
}

impl Thickness {
    /// The line's height in pixels for a base (box) thickness.
    pub fn height(self, base: u32) -> u32 {
        match self {
            Thickness::SuperLight => (base / 2).max(1),
            Thickness::Light => base,
            Thickness::Heavy => base * 2,
        }
    }
}

/// Shades, as coverage values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Shade {
    Off = 0x00,
    Light = 0x40,
    Medium = 0x80,
    Dark = 0xc0,
    On = 0xff,
}

impl From<Shade> for Color {
    fn from(s: Shade) -> Color {
        s as u8
    }
}

/// Features that may be present in each quadrant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Quads {
    pub tl: bool,
    pub tr: bool,
    pub bl: bool,
    pub br: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corner {
    Tl,
    Tr,
    Bl,
    Br,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Top,
    Left,
    Bottom,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Horizontal {
    Left,
    Right,
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vertical {
    Top,
    Bottom,
    Middle,
}

/// Alignment of a figure within a cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Alignment {
    pub horizontal: Horizontal,
    pub vertical: Vertical,
}

impl Alignment {
    const fn new(horizontal: Horizontal, vertical: Vertical) -> Alignment {
        Alignment {
            horizontal,
            vertical,
        }
    }
    pub const CENTER: Alignment = Alignment::new(Horizontal::Center, Vertical::Middle);
    pub const UPPER: Alignment = Alignment::new(Horizontal::Center, Vertical::Top);
    pub const LOWER: Alignment = Alignment::new(Horizontal::Center, Vertical::Bottom);
    pub const LEFT: Alignment = Alignment::new(Horizontal::Left, Vertical::Middle);
    pub const RIGHT: Alignment = Alignment::new(Horizontal::Right, Vertical::Middle);
    pub const UPPER_LEFT: Alignment = Alignment::new(Horizontal::Left, Vertical::Top);
    pub const UPPER_RIGHT: Alignment = Alignment::new(Horizontal::Right, Vertical::Top);
    pub const LOWER_LEFT: Alignment = Alignment::new(Horizontal::Left, Vertical::Bottom);
    pub const LOWER_RIGHT: Alignment = Alignment::new(Horizontal::Right, Vertical::Bottom);
}

/// A fraction across the cell, horizontally or vertically. Ghostty's
/// redundant names (`left`/`top`/`zero`, …) collapse to one each here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fraction {
    Zero,
    OneEighth,
    OneQuarter,
    OneThird,
    ThreeEighths,
    Half,
    FiveEighths,
    TwoThirds,
    ThreeQuarters,
    SevenEighths,
    One,
}

impl Fraction {
    /// `EIGHTHS[i]` is `i/8`.
    pub const EIGHTHS: [Fraction; 9] = [
        Fraction::Zero,
        Fraction::OneEighth,
        Fraction::OneQuarter,
        Fraction::ThreeEighths,
        Fraction::Half,
        Fraction::FiveEighths,
        Fraction::ThreeQuarters,
        Fraction::SevenEighths,
        Fraction::One,
    ];
    pub const QUARTERS: [Fraction; 5] = [
        Fraction::Zero,
        Fraction::OneQuarter,
        Fraction::Half,
        Fraction::ThreeQuarters,
        Fraction::One,
    ];
    pub const THIRDS: [Fraction; 4] = [
        Fraction::Zero,
        Fraction::OneThird,
        Fraction::TwoThirds,
        Fraction::One,
    ];
    pub const HALVES: [Fraction; 3] = [Fraction::Zero, Fraction::Half, Fraction::One];

    pub fn fraction(self) -> f64 {
        match self {
            Fraction::Zero => 0.0,
            Fraction::OneEighth => 0.125,
            Fraction::OneQuarter => 0.25,
            Fraction::OneThird => 1.0 / 3.0,
            Fraction::ThreeEighths => 0.375,
            Fraction::Half => 0.5,
            Fraction::FiveEighths => 0.625,
            Fraction::TwoThirds => 2.0 / 3.0,
            Fraction::ThreeQuarters => 0.75,
            Fraction::SevenEighths => 0.875,
            Fraction::One => 1.0,
        }
    }

    /// Position as a min (left/top) coordinate. Rounds from the far end so
    /// `start→half` and `half→end` split an odd size evenly (see Ghostty).
    pub fn min(self, size: u32) -> i32 {
        let s = size as f64;
        (s - ((1.0 - self.fraction()) * s).round()) as i32
    }

    /// Position as a max (right/bottom) coordinate.
    pub fn max(self, size: u32) -> i32 {
        (self.fraction() * size as f64).round() as i32
    }

    /// Unrounded position, for path drawing.
    pub fn float(self, size: u32) -> f64 {
        self.fraction() * size as f64
    }
}

/// Fill the section of the cell between two pairs of fraction lines.
pub fn fill(
    metrics: &Metrics,
    canvas: &mut Canvas,
    x0: Fraction,
    x1: Fraction,
    y0: Fraction,
    y1: Fraction,
) {
    canvas.box_(
        x0.min(metrics.cell_width),
        y0.min(metrics.cell_height),
        x1.max(metrics.cell_width),
        y1.max(metrics.cell_height),
        super::canvas::ON,
    );
}

/// Centered vertical line.
pub fn vline_middle(metrics: &Metrics, canvas: &mut Canvas, thickness: Thickness) {
    let thick_px = thickness.height(metrics.box_thickness);
    vline(
        canvas,
        0,
        metrics.cell_height as i32,
        (metrics.cell_width.saturating_sub(thick_px) / 2) as i32,
        thick_px,
    );
}

/// Centered horizontal line.
pub fn hline_middle(metrics: &Metrics, canvas: &mut Canvas, thickness: Thickness) {
    let thick_px = thickness.height(metrics.box_thickness);
    hline(
        canvas,
        0,
        metrics.cell_width as i32,
        (metrics.cell_height.saturating_sub(thick_px) / 2) as i32,
        thick_px,
    );
}

/// Vertical line with its left edge at `x`, from `y1` to `y2`.
pub fn vline(canvas: &mut Canvas, y1: i32, y2: i32, x: i32, thickness_px: u32) {
    canvas.box_(x, y1, x + thickness_px as i32, y2, super::canvas::ON);
}

/// Horizontal line with its top edge at `y`, from `x1` to `x2`.
pub fn hline(canvas: &mut Canvas, x1: i32, x2: i32, y: i32, thickness_px: u32) {
    canvas.box_(x1, y, x2, y + thickness_px as i32, super::canvas::ON);
}
