//! The voice glyph, painted through GPUI: the shared engine
//! ([`zeron_orb::glyph`]) decides every dot, this module lays them out and
//! lights them. Fields that stand alone are sized in dots; a beam fills the
//! box it is given, counting its dots at paint time, so it always reaches
//! that box's edges.
use gpui::{Bounds, Hsla, IntoElement, Pixels, Rgba, Styled, Window, point, px, size};
pub use zeron_orb::glyph::{
    BANDS, Beam, Cell, Field, Grid, Inks, Mode, Outline, Rings, Skip, Touch, Voice,
};
use zeron_orb::glyph::{Rgb, look};

use crate::theme::Theme;

/// The dots of every beam: the dictation track's, the call strip's and the
/// footer's alike, so the one voice reads the same wherever it runs.
pub const BEAM_DOT: f32 = 2.4;

fn rgb(colour: Hsla) -> Rgb {
    let Rgba { r, g, b, .. } = colour.into();
    [r, g, b]
}

fn hsla([r, g, b]: Rgb, a: f32) -> Hsla {
    Rgba { r, g, b, a }.into()
}

/// Codex in sunrise, the user in the theme's accent.
pub fn inks(theme: &Theme) -> Inks {
    Inks::new(theme.glyph.rows().map(rgb))
}

/// Lights `cells` laid out on `grid` from `origin`: one quad a dot, its
/// light told by colour alone, so even a large field stays cheap.
fn paint(window: &mut Window, origin: gpui::Point<Pixels>, grid: Grid, cells: &[Cell]) {
    let pitch = grid.pitch();
    // Every dot lands on whole device pixels, its centre and its size alike:
    // a dot straddling pixels differently from its neighbour reads as a
    // second, slower pattern over the grid (moiré), not as light.
    let scale = window.scale_factor().max(1.0);
    let snap = |v: f32| (v * scale).round() / scale;
    for cell in cells {
        let look = look(cell);
        if look.alpha < 0.01 {
            continue;
        }
        let across = snap(grid.dot * look.size).max(1.0 / scale);
        let centre = (
            f32::from(origin.x) + cell.col * pitch + grid.dot / 2.0,
            f32::from(origin.y) + cell.row * pitch + grid.dot / 2.0,
        );
        let at = Bounds::new(
            point(
                px(snap(centre.0 - across / 2.0)),
                px(snap(centre.1 - across / 2.0)),
            ),
            size(px(across), px(across)),
        );
        window
            .paint_quad(gpui::fill(at, hsla(look.fill, look.alpha)).corner_radii(px(across / 2.0)));
    }
}

/// A field `cols` × `rows` dots on `grid`, inside `outline`, with a
/// pointer's `touch` on it (in points from its corner) if there is one.
#[allow(clippy::too_many_arguments)]
pub fn field(
    field: &Field,
    cols: usize,
    rows: usize,
    grid: Grid,
    mode: Mode,
    inks: &Inks,
    outline: Outline,
    touch: Option<Touch>,
) -> impl IntoElement {
    let (w, h) = grid.extent(cols, rows);
    let cells = field.cells_touched(cols, rows, grid, mode, inks, outline, &[], touch);
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| paint(window, bounds.origin, grid, &cells),
    )
    .w(px(w))
    .h(px(h))
    .flex_none()
}

/// A matrix filling its box: `build` is asked for the dots once the box is
/// known, given how many fit and the rooms `skip` leaves, which are measured
/// from the box's corner and moved here onto the matrix's first dot.
fn filling(
    grid: Grid,
    skip: Vec<Skip>,
    build: impl FnOnce(usize, usize, &[Skip]) -> Vec<Cell> + 'static,
) -> gpui::Canvas<()> {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let ((cols, rows), (mx, my)) =
                grid.fit(f32::from(bounds.size.width), f32::from(bounds.size.height));
            let skip: Vec<_> = skip.iter().map(|room| room.shifted(-mx, -my)).collect();
            let cells = build(cols, rows, &skip);
            paint(window, bounds.origin + point(px(mx), px(my)), grid, &cells);
        },
    )
}

/// A beam filling its box inside `outline`, whose radii are in points: it
/// meets the edges of what holds it and fades where it touches them.
pub fn beam(
    beam: &Beam,
    grid: Grid,
    mode: Mode,
    inks: Inks,
    outline: Outline,
) -> gpui::Canvas<()> {
    let beam = beam.clone();
    let outline = match outline {
        Outline::Rounded(radius) => Outline::Rounded(radius / grid.pitch()),
        Outline::Under(radius) => Outline::Under(radius / grid.pitch()),
        other => other,
    };
    // The matrix covers the box and runs past it: the box's own outline is
    // where the dots fade out, so the beam fills it to its curves.
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let (width, height) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
            let ((cols, rows), (mx, my)) = grid.cover(width, height);
            let area = (width / grid.pitch(), height / grid.pitch());
            let cells = beam.cells_over(cols, rows, area, mode, &inks, outline);
            window.paint_quad(gpui::fill(bounds, gpui::transparent_black()));
            paint(window, bounds.origin + point(px(mx), px(my)), grid, &cells);
        },
    )
}
