//! Ghostty's sprite font: box drawing, blocks, braille, branch/git-graph,
//! geometric shapes, Powerline and Legacy Computing glyphs drawn to the cell
//! on the pixel grid instead of taken from the font.
//!
//! Ported from Ghostty's src/font/sprite/ @ main, 2026-10 (MIT, see
//! THIRD_PARTY_NOTICES.md): `canvas.zig` → [`canvas`], `draw/*.zig` → the submodules.
//! z2d is replaced by tiny-skia. Cursor and underline sprites
//! (`special.zig`) are not ported: the terminal paints those itself.
//! `testdata/` holds Ghostty's reference renders; the test below compares
//! against them.

pub mod canvas;
pub mod common;

mod block;
mod box_drawing;
mod braille;
mod branch;
mod geometric_shapes;
mod legacy_computing;
mod legacy_computing_supplement;
mod powerline;

use canvas::Canvas;

/// Grid metrics, in device pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Metrics {
    pub cell_width: u32,
    pub cell_height: u32,
    /// Light line thickness; heavy is 2×. Ghostty uses the font's underline
    /// thickness.
    pub box_thickness: u32,
}

/// Draws codepoint `cp` on a canvas of `width`×`height` (the sprite size).
pub type DrawFn = fn(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics);

/// Codepoint ranges and their drawing functions, ascending.
const RANGES: &[(u32, u32, DrawFn)] = &[
    (0x2500, 0x257F, box_drawing::draw_2500_257f),
    (0x2580, 0x259F, block::draw_2580_259f),
    (0x25E2, 0x25E5, geometric_shapes::draw_25e2_25e5),
    (0x25F8, 0x25FA, geometric_shapes::draw_25f8_25fa),
    (0x25FF, 0x25FF, geometric_shapes::draw_25ff),
    (0x2800, 0x28FF, braille::draw_2800_28ff),
    (0xE0B0, 0xE0B0, powerline::draw_e0b0),
    (0xE0B1, 0xE0B1, powerline::draw_e0b1),
    (0xE0B2, 0xE0B2, powerline::draw_e0b2),
    (0xE0B3, 0xE0B3, powerline::draw_e0b3),
    (0xE0B4, 0xE0B4, powerline::draw_e0b4),
    (0xE0B5, 0xE0B5, powerline::draw_e0b5),
    (0xE0B6, 0xE0B6, powerline::draw_e0b6),
    (0xE0B7, 0xE0B7, powerline::draw_e0b7),
    (0xE0B8, 0xE0B8, powerline::draw_e0b8),
    (0xE0B9, 0xE0B9, powerline::draw_e0b9),
    (0xE0BA, 0xE0BA, powerline::draw_e0ba),
    (0xE0BB, 0xE0BB, powerline::draw_e0bb),
    (0xE0BC, 0xE0BC, powerline::draw_e0bc),
    (0xE0BD, 0xE0BD, powerline::draw_e0bd),
    (0xE0BE, 0xE0BE, powerline::draw_e0be),
    (0xE0BF, 0xE0BF, powerline::draw_e0bf),
    (0xE0D2, 0xE0D2, powerline::draw_e0d2),
    (0xE0D4, 0xE0D4, powerline::draw_e0d4),
    (0xF5D0, 0xF60D, branch::draw_f5d0_f60d),
    (
        0x1CC1B,
        0x1CC1E,
        legacy_computing_supplement::draw_1cc1b_1cc1e,
    ),
    (
        0x1CC21,
        0x1CC2F,
        legacy_computing_supplement::draw_1cc21_1cc2f,
    ),
    (
        0x1CC30,
        0x1CC3F,
        legacy_computing_supplement::draw_1cc30_1cc3f,
    ),
    (
        0x1CD00,
        0x1CDE5,
        legacy_computing_supplement::draw_1cd00_1cde5,
    ),
    (0x1CE00, 0x1CE00, legacy_computing_supplement::draw_1ce00),
    (0x1CE01, 0x1CE01, legacy_computing_supplement::draw_1ce01),
    (0x1CE0B, 0x1CE0B, legacy_computing_supplement::draw_1ce0b),
    (0x1CE0C, 0x1CE0C, legacy_computing_supplement::draw_1ce0c),
    (
        0x1CE16,
        0x1CE19,
        legacy_computing_supplement::draw_1ce16_1ce19,
    ),
    (
        0x1CE51,
        0x1CE8F,
        legacy_computing_supplement::draw_1ce51_1ce8f,
    ),
    (
        0x1CE90,
        0x1CEAF,
        legacy_computing_supplement::draw_1ce90_1ceaf,
    ),
    (0x1FB00, 0x1FB3B, legacy_computing::draw_1fb00_1fb3b),
    (0x1FB3C, 0x1FB67, legacy_computing::draw_1fb3c_1fb67),
    (0x1FB68, 0x1FB6F, legacy_computing::draw_1fb68_1fb6f),
    (0x1FB70, 0x1FB75, legacy_computing::draw_1fb70_1fb75),
    (0x1FB76, 0x1FB7B, legacy_computing::draw_1fb76_1fb7b),
    (0x1FB7C, 0x1FB97, legacy_computing::draw_1fb7c_1fb97),
    (0x1FB98, 0x1FB98, legacy_computing::draw_1fb98),
    (0x1FB99, 0x1FB99, legacy_computing::draw_1fb99),
    (0x1FB9A, 0x1FB9F, legacy_computing::draw_1fb9a_1fb9f),
    (0x1FBA0, 0x1FBAE, legacy_computing::draw_1fba0_1fbae),
    (0x1FBAF, 0x1FBAF, legacy_computing::draw_1fbaf),
    (0x1FBBD, 0x1FBBD, legacy_computing::draw_1fbbd),
    (0x1FBBE, 0x1FBBE, legacy_computing::draw_1fbbe),
    (0x1FBBF, 0x1FBBF, legacy_computing::draw_1fbbf),
    (0x1FBCE, 0x1FBCE, legacy_computing::draw_1fbce),
    (0x1FBCF, 0x1FBCF, legacy_computing::draw_1fbcf),
    (0x1FBD0, 0x1FBDF, legacy_computing::draw_1fbd0_1fbdf),
    (0x1FBE0, 0x1FBEF, legacy_computing::draw_1fbe0_1fbef),
];

fn draw_fn(cp: u32) -> Option<DrawFn> {
    let ix = RANGES.partition_point(|&(_, max, _)| max < cp);
    RANGES
        .get(ix)
        .filter(|&&(min, _, _)| min <= cp)
        .map(|&(_, _, f)| f)
}

/// Whether `ch` is drawn here rather than by the font.
pub fn has_char(ch: char) -> bool {
    draw_fn(ch as u32).is_some()
}

/// A drawn glyph: coverage for a surface larger than the cell by `padding`
/// on every side, so its top-left sits at (-padding_x, -padding_y).
pub struct Sprite {
    pub width: u32,
    pub height: u32,
    pub padding_x: u32,
    pub padding_y: u32,
    pub alpha: Vec<u8>,
}

/// Draw `ch` for a `width`×`height` sprite (`width` spans every cell the
/// glyph covers).
pub fn render(ch: char, width: u32, height: u32, metrics: &Metrics) -> Option<Sprite> {
    let draw = draw_fn(ch as u32)?;
    let (padding_x, padding_y) = (width / 4, height / 4);
    let mut canvas = Canvas::new(width, height, padding_x, padding_y);
    draw(ch as u32, &mut canvas, width, height, metrics);
    let (w, h) = (canvas.surface_width(), canvas.surface_height());
    Some(Sprite {
        width: w,
        height: h,
        padding_x,
        padding_y,
        alpha: canvas.into_alpha(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Ghostty's "sprite face render all sprites": every range drawn into
    /// 16×16-glyph sheets, one per 256 codepoints, at four metric sets, and
    /// compared with Ghostty's reference PNGs. Anti-aliasing differs between
    /// z2d and tiny-skia, so a glyph passes when few pixels differ by much.
    ///
    /// `SPRITE_RANGE=2500-257F` limits the report; `SPRITE_DUMP=<dir>` writes
    /// our sheets and red/green diffs (red = Ghostty only, green = ours only).
    #[test]
    fn sprites_match_ghostty_reference() {
        let filter = std::env::var("SPRITE_RANGE").ok().map(|r| {
            let (a, b) = r.split_once('-').unwrap_or((&r, &r));
            (
                u32::from_str_radix(a, 16).unwrap(),
                u32::from_str_radix(b, 16).unwrap(),
            )
        });
        let dump = std::env::var("SPRITE_DUMP").ok();
        // (width, ascent, descent, thickness), as in Ghostty.
        let sets = [
            (18u32, 30u32, 6u32, 4u32),
            (12, 20, 4, 3),
            (11, 19, 2, 2),
            (9, 15, 2, 1),
        ];
        let mut failures: BTreeMap<u32, Vec<String>> = BTreeMap::new();

        for (width, ascent, descent, thickness) in sets {
            let height = ascent + descent;
            let metrics = Metrics {
                cell_width: width,
                cell_height: height,
                box_thickness: thickness,
            };
            let (pad_x, pad_y) = (width / 4, height / 4);
            let (stride_x, stride_y) = (width + 2 * pad_x, height + 2 * pad_y);

            let mut sheets: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
            for &(min, max, _) in RANGES {
                for cp in min..=max {
                    let base = cp & !0xFF;
                    let sheet = sheets
                        .entry(base)
                        .or_insert_with(|| vec![0; (stride_x * 16 * stride_y * 16) as usize]);
                    let sprite =
                        render(char::from_u32(cp).unwrap(), width, height, &metrics).unwrap();
                    let (ox, oy) = (stride_x * ((cp - base) % 16), stride_y * ((cp - base) / 16));
                    for y in 0..stride_y {
                        for x in 0..stride_x {
                            sheet[((oy + y) * stride_x * 16 + ox + x) as usize] =
                                sprite.alpha[(y * stride_x + x) as usize];
                        }
                    }
                }
            }

            for (base, ours) in sheets {
                let name = format!(
                    "U+{:X}...U+{:X}-{}x{}+{}.png",
                    base,
                    base + 0xFF,
                    width,
                    height,
                    thickness
                );
                let path = format!(
                    "{}/src/terminal/sprite/testdata/{name}",
                    env!("CARGO_MANIFEST_DIR")
                );
                let reference =
                    tiny_skia::Pixmap::load_png(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
                let sheet_w = stride_x * 16;
                assert_eq!(
                    (reference.width(), reference.height()),
                    (sheet_w, stride_y * 16),
                    "{name}"
                );
                let theirs: Vec<u8> = reference
                    .pixels()
                    .iter()
                    .map(|p| p.demultiply().red())
                    .collect();

                for &(min, max, _) in RANGES {
                    for cp in (min..=max).filter(|cp| cp & !0xFF == base) {
                        if filter.is_some_and(|(a, b)| cp < a || cp > b) {
                            continue;
                        }
                        let (ox, oy) =
                            (stride_x * ((cp - base) % 16), stride_y * ((cp - base) / 16));
                        let mut bad = 0;
                        for y in oy..oy + stride_y {
                            for x in ox..ox + stride_x {
                                let i = (y * sheet_w + x) as usize;
                                if ours[i].abs_diff(theirs[i]) > 96 {
                                    bad += 1;
                                }
                            }
                        }
                        // ponytail: a few edge pixels may round the other way under different AA.
                        if bad > 2 {
                            failures
                                .entry(cp)
                                .or_default()
                                .push(format!("{width}x{height}+{thickness}: {bad}px"));
                        }
                    }
                }

                if let Some(dir) = &dump {
                    let mut out = tiny_skia::Pixmap::new(sheet_w, stride_y * 16).unwrap();
                    let mut diff = tiny_skia::Pixmap::new(sheet_w, stride_y * 16).unwrap();
                    for (i, (o, t)) in ours.iter().zip(&theirs).enumerate() {
                        out.pixels_mut()[i] =
                            tiny_skia::PremultipliedColorU8::from_rgba(*o, *o, *o, 255).unwrap();
                        let g = (*o.min(t)) / 3;
                        let (r, gr) = if o == t { (g, g) } else { (*t, *o) };
                        diff.pixels_mut()[i] =
                            tiny_skia::PremultipliedColorU8::from_rgba(r, gr, g, 255).unwrap();
                    }
                    out.save_png(format!("{dir}/ours-{name}")).unwrap();
                    diff.save_png(format!("{dir}/diff-{name}")).unwrap();
                }
            }
        }

        let report: Vec<String> = failures
            .iter()
            .map(|(cp, sizes)| {
                format!(
                    "U+{cp:X} {}: {}",
                    char::from_u32(*cp).unwrap(),
                    sizes.join(", ")
                )
            })
            .collect();
        assert!(
            report.is_empty(),
            "{} glyphs differ from Ghostty:\n{}",
            report.len(),
            report.join("\n")
        );
    }
}
