//! Streaming fade veil for GPUI text: the chunk tracking lives in
//! `zeron-veil` (shared with the voice captions on desktop and mobile); this
//! module recolors GPUI text runs with its spans.
//!
//! [`apply_veil`] multiplies the fading alpha into the `TextRun` colors
//! covering each chunk. This is paint-only by construction: a color-only run
//! split cannot change layout — gpui shapes text through cosmic-text, whose
//! `Attrs::compatible` ignores color/metadata, so adjacent same-font runs are
//! shaped as one contiguous word even across the split (kerning and ligatures
//! survive; wrapping is byte-identical to the unsplit render).

use gpui::TextRun;
pub use zeron_veil::{RowVeil, VeilSpan, slice_spans};

/// Multiply veil opacities into the runs' paint colors, splitting runs at span
/// boundaries. Fonts, lengths, and text are untouched — the total run length is
/// preserved exactly, so shaping and wrapping cannot change (see module docs).
pub fn apply_veil(runs: Vec<TextRun>, spans: &[VeilSpan]) -> Vec<TextRun> {
    if spans.is_empty() || spans.iter().all(|(_, a)| *a >= 1.0) {
        return runs;
    }
    let mut out = Vec::with_capacity(runs.len() + spans.len() * 2);
    let mut pos = 0usize;
    for run in runs {
        let (start, end) = (pos, pos + run.len);
        pos = end;
        let mut cuts = vec![start, end];
        for (r, _) in spans {
            if r.start > start && r.start < end {
                cuts.push(r.start);
            }
            if r.end > start && r.end < end {
                cuts.push(r.end);
            }
        }
        cuts.sort_unstable();
        cuts.dedup();
        for w in cuts.windows(2) {
            let (s, e) = (w[0], w[1]);
            let mut piece = run.clone();
            piece.len = e - s;
            if let Some(alpha) = spans
                .iter()
                .find(|(r, _)| r.start <= s && e <= r.end)
                .map(|(_, a)| *a)
                && alpha < 1.0
            {
                piece.color = piece.color.opacity(alpha);
                piece.background_color = piece.background_color.map(|c| c.opacity(alpha));
                if let Some(u) = &mut piece.underline {
                    u.color = u.color.map(|c| c.opacity(alpha));
                }
                if let Some(st) = &mut piece.strikethrough {
                    st.color = st.color.map(|c| c.opacity(alpha));
                }
            }
            out.push(piece);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{font, px};

    fn run(len: usize, color: gpui::Hsla) -> TextRun {
        TextRun {
            len,
            font: font("Test"),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    #[test]
    fn apply_veil_preserves_length_and_fonts() {
        let color = gpui::white();
        let runs = vec![run(4, color), run(6, color)];
        let spans = vec![(2..8, 0.5)];
        let out = apply_veil(runs.clone(), &spans);
        let total: usize = out.iter().map(|r| r.len).sum();
        assert_eq!(total, 10, "split must cover the text exactly");
        assert!(out.iter().all(|r| r.font == runs[0].font));
        // Pieces: [0..2 full] [2..4 faded] [4..8 faded] [8..10 full].
        assert_eq!(
            out.iter().map(|r| r.len).collect::<Vec<_>>(),
            vec![2, 2, 4, 2]
        );
        assert_eq!(out[0].color.a, 1.0);
        assert_eq!(out[1].color.a, 0.5);
        assert_eq!(out[2].color.a, 0.5);
        assert_eq!(out[3].color.a, 1.0);
    }

    #[test]
    fn apply_veil_without_spans_is_identity() {
        let runs = vec![run(5, gpui::white())];
        let out = apply_veil(runs.clone(), &[]);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].len, 5);
        // Settled spans (alpha 1) also pass straight through unsplit — the
        // settled frame is byte-identical to an unsplit render.
        let out = apply_veil(runs.clone(), &[(0..5, 1.0)]);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn apply_veil_fades_decorations_too() {
        let mut r = run(5, gpui::white());
        r.background_color = Some(gpui::white());
        r.underline = Some(gpui::UnderlineStyle {
            color: Some(gpui::white()),
            thickness: px(1.0),
            wavy: false,
        });
        let out = apply_veil(vec![r], &[(0..5, 0.25)]);
        assert_eq!(out[0].background_color.unwrap().a, 0.25);
        assert_eq!(out[0].underline.as_ref().unwrap().color.unwrap().a, 0.25);
    }
}
