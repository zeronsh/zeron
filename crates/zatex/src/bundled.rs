//! Math fonts compiled into the binary (feature `bundled`): six faces, each
//! falling back to STIX Two Math for what it lacks. A face loads on first use
//! and is shared, so every host and thread uses the same parsed font and
//! caches; faces nobody picks are never parsed (their bytes stay untouched
//! pages of the binary).
//!
//! [`layout`] numbers glyph fonts against one fixed table ([`FONTS`],
//! [`font_at`]) rather than the chosen face's fallback chain, so a host can
//! resolve a glyph's font without knowing which face laid it out.

use std::sync::{Arc, OnceLock};

use crate::{Error, Formula, Item, MathFont, Options};

/// The bundled faces apps can pick between.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Face {
    /// Fira Math: a sans face that sits beside UI sans-serifs.
    #[default]
    Fira,
    /// STIX Two Math: Times-like; covers everything, so it backs every face.
    StixTwo,
    /// New Computer Modern Math, Book weight: the LaTeX look, a little
    /// heavier than Latin Modern so it holds up at UI sizes.
    ComputerModern,
    /// Libertinus Math: Linux Libertine's warm book serif.
    Libertinus,
    /// TeX Gyre Pagella Math: Palatino.
    Pagella,
    /// Euler Math: Zapf's upright, handwritten math (AMS Euler).
    Euler,
}

impl Face {
    /// Every face, in table order.
    pub const ALL: [Face; 6] = [
        Face::Fira,
        Face::StixTwo,
        Face::ComputerModern,
        Face::Libertinus,
        Face::Pagella,
        Face::Euler,
    ];

    /// The face's own font in the [`font_at`] table. Append-only, so an
    /// index a host stored or sent across FFI never changes meaning.
    pub fn index(self) -> u8 {
        self as u8
    }

    fn data(self) -> &'static [u8] {
        match self {
            Face::Fira => include_bytes!("../fonts/FiraMath-Regular.otf"),
            Face::StixTwo => include_bytes!("../fonts/STIXTwoMath-Regular.otf"),
            Face::ComputerModern => include_bytes!("../fonts/NewCMMath-Book.otf"),
            Face::Libertinus => include_bytes!("../fonts/LibertinusMath-Regular.otf"),
            Face::Pagella => include_bytes!("../fonts/texgyrepagella-math.otf"),
            Face::Euler => include_bytes!("../fonts/Euler-Math.otf"),
        }
    }
}

/// How many fonts [`layout`] can name: one per [`Face`], by [`Face::index`].
pub const FONTS: u8 = Face::ALL.len() as u8;

fn stix_arc() -> &'static Arc<MathFont> {
    static FONT: OnceLock<Arc<MathFont>> = OnceLock::new();
    FONT.get_or_init(|| {
        Arc::new(MathFont::new(Face::StixTwo.data()).expect("bundled STIX Two Math"))
    })
}

/// The font for `face` (with its fallback chain), loaded on first use.
pub fn font(face: Face) -> &'static MathFont {
    static FONTS: [OnceLock<MathFont>; Face::ALL.len()] = [const { OnceLock::new() }; 6];
    if face == Face::StixTwo {
        return stix_arc();
    }
    FONTS[face as usize].get_or_init(|| {
        MathFont::new(face.data())
            .expect("bundled math font")
            .with_fallback(stix_arc().clone())
    })
}

/// The font a [`layout`]ed glyph's `font` index names (see [`FONTS`]): that
/// face's own font, whose [`MathFont::data`] and [`MathFont::outline`] are
/// its own. Out-of-range indices name STIX Two.
pub fn font_at(index: u8) -> &'static MathFont {
    match Face::ALL.get(index as usize) {
        Some(&face) => font(face),
        None => stix_arc(),
    }
}

/// [`crate::layout`] in a bundled face, with glyph font indices into the
/// fixed table [`font_at`] resolves — the same for every face.
pub fn layout(source: &str, face: Face, options: &Options) -> Result<Formula, Error> {
    let mut formula = crate::layout(source, font(face), options)?;
    // A chain is the face then STIX Two (STIX Two's is just itself).
    let table = |chain: u8| match (chain, face) {
        (0, face) => face.index(),
        _ => Face::StixTwo.index(),
    };
    for item in &mut formula.items {
        if let Item::Glyph { font, .. } = item {
            *font = table(*font);
        }
    }
    Ok(formula)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_face_resolves_through_one_table() {
        let names: Vec<String> = Face::ALL
            .iter()
            .map(|&f| font_at(f.index()).name().unwrap())
            .collect();
        for (name, part) in names.iter().zip([
            "Fira",
            "STIX",
            "Computer Modern",
            "Libertinus",
            "Pagella",
            "Euler",
        ]) {
            assert!(
                name.replace("ComputerModern", "Computer Modern")
                    .contains(part),
                "{names:?}"
            );
        }
        let src = r"\mathcal{L} = \frac{x}{y} + \mathfrak{g}";
        let opts = Options::inline(16.0);
        for face in Face::ALL {
            let f = layout(src, face, &opts).unwrap();
            let raw = crate::layout(src, font(face), &opts).unwrap();
            assert_eq!(f.items.len(), raw.items.len());
            for (item, raw) in f.items.iter().zip(&raw.items) {
                if let (
                    Item::Glyph { font: i, glyph, .. },
                    Item::Glyph {
                        font: r, glyph: g, ..
                    },
                ) = (item, raw)
                {
                    assert!(*i < FONTS);
                    assert_eq!(glyph, g);
                    assert_eq!(
                        font_at(*i).data().as_ptr(),
                        font(face).at(*r).data().as_ptr(),
                        "{face:?}"
                    );
                }
            }
        }
        // Fira has no script letters: they come from STIX Two.
        let f = layout(r"\mathcal{L}", Face::Fira, &opts).unwrap();
        assert!(matches!(f.items[0], Item::Glyph { font: 1, .. }));
        // Its own letters stay its own.
        let f = layout("x", Face::Euler, &opts).unwrap();
        assert!(matches!(f.items[0], Item::Glyph { font, .. } if font == Face::Euler.index()));
    }

    #[test]
    fn corpus_lays_out_in_every_bundled_face() {
        for face in Face::ALL {
            for src in crate::tests::CORPUS {
                for opts in [Options::inline(16.0), Options::display(16.0)] {
                    let f =
                        layout(src, face, &opts).unwrap_or_else(|e| panic!("{face:?}: {src}: {e}"));
                    assert!(f.width > 0.0 && f.height() > 0.0, "{face:?}: {src}");
                }
            }
        }
    }
}
