//! Width-dependent presentation; selection offsets always refer to original text.
use super::render::FlatText;
use gpui::{SharedString, TextRun};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// Replaced ranges, `(original, shown)` in document order. A shown range may
/// be shorter (a truncated link label) or longer (a math placeholder) than
/// the text it stands for.
#[derive(Clone, Debug, Default)]
pub struct OffsetMap {
    pub omissions: Vec<(Range<usize>, Range<usize>)>,
}
impl OffsetMap {
    pub fn original(&self, displayed: usize) -> usize {
        let mut shift = 0isize;
        for (original, shown) in &self.omissions {
            if displayed < shown.start {
                break;
            }
            if displayed < shown.end {
                return original.start;
            }
            shift = original.end as isize - shown.end as isize;
        }
        displayed.saturating_add_signed(shift)
    }
    pub fn displayed(&self, original: usize) -> usize {
        let mut shift = 0isize;
        for (source, shown) in &self.omissions {
            if original < source.start {
                break;
            }
            if original < source.end {
                return shown.start;
            }
            shift = source.end as isize - shown.end as isize;
        }
        original.saturating_add_signed(-shift)
    }
    /// This map followed by `next`, which presents this map's shown text
    /// again. The two must replace disjoint ranges.
    pub fn then(&self, next: &OffsetMap) -> OffsetMap {
        let mut omissions: Vec<(Range<usize>, Range<usize>)> = self
            .omissions
            .iter()
            .map(|(original, shown)| {
                (
                    original.clone(),
                    next.displayed(shown.start)..next.displayed(shown.end),
                )
            })
            .chain(next.omissions.iter().map(|(shown, displayed)| {
                (
                    self.original(shown.start)..self.original(shown.end),
                    displayed.clone(),
                )
            }))
            .collect();
        omissions.sort_by_key(|(original, _)| original.start);
        OffsetMap { omissions }
    }
    pub fn displayed_range(&self, range: Range<usize>) -> Range<usize> {
        let mut result = self.displayed(range.start)..self.displayed(range.end);
        for (source, shown) in &self.omissions {
            if range.start < source.end && range.end > source.start {
                result.start = result.start.min(shown.start);
                result.end = result.end.max(shown.end);
            }
        }
        result
    }
}
#[derive(Clone)]
pub struct OriginalText {
    pub text: SharedString,
    pub offsets: OffsetMap,
}

fn slice_runs(runs: &[TextRun], range: Range<usize>) -> Vec<TextRun> {
    let mut at = 0;
    runs.iter()
        .filter_map(|run| {
            let start = at;
            at += run.len;
            let len = at.min(range.end).saturating_sub(start.max(range.start));
            (len > 0).then(|| {
                let mut run = run.clone();
                run.len = len;
                run
            })
        })
        .collect()
}

pub fn truncate(
    flat: &FlatText,
    width: f32,
    measure: impl Fn(&str, &[TextRun]) -> f32,
) -> FlatText {
    let mut omissions = Vec::new();
    for (range, url) in &flat.links {
        if super::links::LinkTarget::new("", url).navigation.is_err() {
            continue;
        }
        // A label holding a formula keeps its full width: a cut placeholder
        // would leave the formula nowhere to sit.
        if flat
            .math
            .iter()
            .any(|math| math.range.start < range.end && math.range.end > range.start)
        {
            continue;
        }
        let label = &flat.text[range.clone()];
        let runs = slice_runs(&flat.runs, range.clone());
        if measure(label, &runs) <= width {
            continue;
        }
        let boundaries: Vec<_> = label.grapheme_indices(true).map(|(i, _)| i).collect();
        let candidate = |end: usize| {
            let text = format!("{}…", &label[..end]);
            let mut runs = slice_runs(&runs, 0..end);
            let mut ellipsis = slice_runs(&flat.runs, range.start + end..range.end).remove(0);
            ellipsis.len = '…'.len_utf8();
            runs.push(ellipsis);
            (text, runs)
        };
        let mut low = 0;
        let mut high = boundaries.len();
        while low < high {
            let middle = (low + high) / 2;
            let (text, runs) = candidate(boundaries[middle]);
            if measure(&text, &runs) <= width {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let prefix = boundaries[low.saturating_sub(1)];
        // Never make a short label longer just to show an ellipsis.
        if range.end - (range.start + prefix) > '…'.len_utf8() {
            omissions.push(range.start + prefix..range.end);
        }
    }
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut offsets = OffsetMap::default();
    let mut at = 0;
    for omitted in omissions {
        text.push_str(&flat.text[at..omitted.start]);
        runs.extend(slice_runs(&flat.runs, at..omitted.start));
        let start = text.len();
        text.push('…');
        let mut style = slice_runs(&flat.runs, omitted.clone()).remove(0);
        style.len = '…'.len_utf8();
        runs.push(style);
        at = omitted.end;
        offsets.omissions.push((omitted, start..text.len()));
    }
    text.push_str(&flat.text[at..]);
    runs.extend(slice_runs(&flat.runs, at..flat.text.len()));
    FlatText {
        text: text.into(),
        runs,
        links: flat
            .links
            .iter()
            .map(|(r, url)| (offsets.displayed_range(r.clone()), url.clone()))
            .collect(),
        code_ranges: flat
            .code_ranges
            .iter()
            .map(|r| offsets.displayed_range(r.clone()))
            .collect(),
        math: flat
            .math
            .iter()
            .map(|math| super::render::MathPlacement {
                range: offsets.displayed_range(math.range.clone()),
                ..math.clone()
            })
            .collect(),
        // Selection and copy always resolve to the source text: math
        // placeholders already map there, so truncation composes onto them.
        original: Some(match &flat.original {
            Some(original) => OriginalText {
                text: original.text.clone(),
                offsets: original.offsets.then(&offsets),
            },
            None => OriginalText {
                text: flat.text.clone(),
                offsets,
            },
        }),
    }
}

/// Shrink inline formulas wider than `width` until their placeholders fit a
/// row: the line breaker would otherwise split an over-wide placeholder and
/// leave its tail indenting the next row. A fitted formula leaves room for
/// the space after it, which would otherwise start the next row.
/// `placeholder` builds a placeholder that many pixels wide. `None` when
/// every formula already fits.
pub fn fit_math(
    flat: &FlatText,
    width: f32,
    measure: impl Fn(&str, &[TextRun]) -> f32,
    placeholder: impl Fn(f32) -> String,
) -> Option<FlatText> {
    let mut fitted: Vec<(usize, String, f32)> = Vec::new();
    for (ix, math) in flat.math.iter().enumerate() {
        if math.display {
            continue;
        }
        let runs = slice_runs(&flat.runs, math.range.clone());
        let shown = measure(&flat.text[math.range.clone()], &runs);
        let mut space = runs[0].clone();
        space.len = 1;
        let room = (width - measure(" ", &[space]) - 1.0).max(1.0);
        if shown <= room || shown <= 0.0 {
            continue;
        }
        fitted.push((ix, placeholder(room), room / shown));
    }
    if fitted.is_empty() {
        return None;
    }
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut offsets = OffsetMap::default();
    let mut at = 0;
    for (ix, placeholder, _) in &fitted {
        let range = flat.math[*ix].range.clone();
        text.push_str(&flat.text[at..range.start]);
        runs.extend(slice_runs(&flat.runs, at..range.start));
        let start = text.len();
        text.push_str(placeholder);
        let mut run = slice_runs(&flat.runs, range.clone()).remove(0);
        run.len = placeholder.len();
        runs.push(run);
        at = range.end;
        offsets.omissions.push((range, start..text.len()));
    }
    text.push_str(&flat.text[at..]);
    runs.extend(slice_runs(&flat.runs, at..flat.text.len()));
    Some(FlatText {
        text: text.into(),
        runs,
        links: flat
            .links
            .iter()
            .map(|(r, url)| (offsets.displayed_range(r.clone()), url.clone()))
            .collect(),
        code_ranges: flat
            .code_ranges
            .iter()
            .map(|r| offsets.displayed_range(r.clone()))
            .collect(),
        math: flat
            .math
            .iter()
            .enumerate()
            .map(|(ix, math)| super::render::MathPlacement {
                range: offsets.displayed_range(math.range.clone()),
                scale: fitted
                    .iter()
                    .find(|(fit, _, _)| *fit == ix)
                    .map_or(math.scale, |(_, _, scale)| math.scale * scale),
                ..math.clone()
            })
            .collect(),
        original: Some(match &flat.original {
            Some(original) => OriginalText {
                text: original.text.clone(),
                offsets: original.offsets.then(&offsets),
            },
            None => OriginalText {
                text: flat.text.clone(),
                offsets,
            },
        }),
    })
}

use super::render::RenderOptions;
use crate::theme::Theme;
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, ElementId, GlobalElementId,
    InspectorElementId, LayoutId, Pixels, Size, Window, prelude::*,
};
use std::rc::Rc;

pub struct ResponsiveText {
    pub flat: Rc<FlatText>,
    pub opts: RenderOptions,
    pub theme: Theme,
    pub ix: usize,
    /// Cut over-wide link labels (links that open inside the app).
    pub truncate_links: bool,
}
/// Width-dependent presentation: over-wide inline formulas shrink to fit,
/// then (with `truncate_links`) over-wide link labels are cut.
pub(super) fn present(
    flat: &FlatText,
    width: Pixels,
    font_size: Pixels,
    window: &Window,
    truncate_links: bool,
    theme: &Theme,
) -> FlatText {
    let measure = |text: &str, runs: &[TextRun]| {
        window
            .text_system()
            .shape_text(text.to_owned().into(), font_size, runs, None, None)
            .map(|lines| {
                lines
                    .iter()
                    .map(|line| f32::from(line.size(font_size).width))
                    .fold(0., f32::max)
            })
            .unwrap_or(f32::INFINITY)
    };
    let fitted = fit_math(flat, f32::from(width), &measure, |target| {
        let font = super::render::math_placeholder_font(theme);
        super::math::spacer(&font, window).fill(target / f32::from(font_size))
    });
    let flat = fitted.as_ref().unwrap_or(flat);
    if truncate_links {
        truncate(flat, f32::from(width), &measure)
    } else {
        flat.clone()
    }
}
impl IntoElement for ResponsiveText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for ResponsiveText {
    type RequestLayoutState = ();
    type PrepaintState = AnyElement;
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let line_height = window.pixel_snap(
            style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let flat = self.flat.clone();
        let theme = self.theme.clone();
        let truncate_links = self.truncate_links;
        let id = window.request_measured_layout(
            Default::default(),
            move |known, available, window, _| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let shown = width
                    .map(|width| present(&flat, width, font_size, window, truncate_links, &theme));
                let flat = shown.as_ref().unwrap_or(&flat);
                let lines = window
                    .text_system()
                    .shape_text(flat.text.clone(), font_size, &flat.runs, width, None)
                    .unwrap_or_default();
                let mut result: Size<Pixels> = Size::default();
                for line in lines.iter() {
                    let size = line.size(line_height);
                    result.width = result.width.max(size.width).ceil();
                    result.height += size.height;
                }
                // Keep the final width equal to the one that decided truncation.
                if let Some(width) = width {
                    result.width = width;
                }
                result
            },
        );
        (id, ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let flat = present(
            &self.flat,
            bounds.size.width,
            font_size,
            window,
            self.truncate_links,
            &self.theme,
        );
        let mut child =
            super::render::flat_text_presented_element(&flat, self.ix, &self.opts, &self.theme);
        child.prepaint_as_root(
            bounds.origin,
            bounds.size.map(AvailableSpace::Definite),
            window,
            cx,
        );
        child
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        parser::{InlineRun, InlineStyle},
        render::flatten_runs,
    };
    use super::*;
    fn link(text: &str) -> InlineRun {
        InlineRun {
            text: text.into(),
            style: InlineStyle {
                link: Some("https://example.com/destination".into()),
                ..Default::default()
            },
        }
    }
    fn measured(text: &str, _: &[TextRun]) -> f32 {
        text.graphemes(true).count() as f32
    }
    #[test]
    fn truncation_preserves_graphemes_destinations_and_original_selection() {
        let label = "https://example.com/á🙂👨‍👩‍👧‍👦界/very/long/path";
        let flat = flatten_runs(&[link(label)], &Theme::dark(), false);
        for width in [1., 12., 25., 32.] {
            let shown = truncate(&flat, width, measured);
            assert!(measured(&shown.text, &[]) <= width);
            let original = shown.original.as_ref().unwrap();
            assert_eq!(original.text.as_ref(), label);
            assert_eq!(original.offsets.original(shown.text.len()), label.len());
            assert_eq!(shown.links[0].1, flat.links[0].1);
            assert_eq!(
                shown.runs.iter().map(|r| r.len).sum::<usize>(),
                shown.text.len()
            );
            for (i, _) in shown.text.char_indices() {
                assert!(label.is_char_boundary(original.offsets.original(i)));
            }
        }
        let shown = truncate(&flat, 25., measured);
        assert!(shown.text.starts_with("https://example.com/"));
    }
    #[test]
    fn partial_and_cross_block_copies_map_back_to_source() {
        let source = "https://example.com/abcdefghijklmnopqrstuvwxyz";
        let flat = flatten_runs(
            &[
                link(source),
                InlineRun {
                    text: " between ".into(),
                    style: Default::default(),
                },
                link("https://second.example/long/path/to/resource"),
            ],
            &Theme::dark(),
            false,
        );
        let shown = truncate(&flat, 24., measured);
        let map = &shown.original.as_ref().unwrap().offsets;
        assert_eq!(map.omissions.len(), 2);
        let first = &shown.links[0].0;
        assert_eq!(
            &flat.text[map.original(first.start)..map.original(first.end)],
            source
        );
        assert_eq!(&flat.text[map.original(0)..map.original(5)], "https");
        let end = map.original(shown.text.len());
        let spans = super::super::selection::resolve_spans(
            &[("a", &flat.text), ("b", "next block")],
            (0, 0),
            (1, 4),
        );
        let copied = spans
            .iter()
            .map(|span| &span.text[span.range.clone()])
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(copied, format!("{}\nnext", &flat.text[..end]));
        let omission = &map.omissions[0];
        assert_eq!(
            map.displayed_range(omission.0.start + 1..omission.0.end - 1),
            omission.1
        );
    }
    #[test]
    fn widening_and_streaming_recompute_presentation_without_stale_offsets() {
        let flat = flatten_runs(
            &[link("https://example.com/a/long/link")],
            &Theme::dark(),
            false,
        );
        let narrow = truncate(&flat, 15., measured);
        let wide = truncate(&flat, 1000., measured);
        assert!(narrow.text.ends_with('…'));
        assert_eq!(wide.text, flat.text);
        assert!(wide.original.as_ref().unwrap().offsets.omissions.is_empty());
        let growing = flatten_runs(
            &[link("https://example.com/a/long/link/streaming-tail")],
            &Theme::dark(),
            false,
        );
        let next = truncate(&growing, 15., measured);
        assert_eq!(
            next.original.unwrap().offsets.original(next.text.len()),
            growing.text.len()
        );
    }
    #[test]
    fn longer_replacements_map_both_ways() {
        // A 3-byte formula shown as a 5-byte placeholder.
        let map = OffsetMap {
            omissions: vec![(2..5, 2..7)],
        };
        assert_eq!(map.original(0), 0);
        assert_eq!(map.original(4), 2);
        assert_eq!(map.original(7), 5);
        assert_eq!(map.original(9), 7);
        assert_eq!(map.displayed(3), 2);
        assert_eq!(map.displayed(5), 7);
        assert_eq!(map.displayed(7), 9);
        assert_eq!(map.displayed_range(1..6), 1..8);
    }

    #[test]
    fn over_wide_formulas_shrink_to_fit_and_still_copy_as_tex() {
        let source = "see $x_1 + x_2 + x_3 + x_4$ now";
        let tree = super::super::parser::parse_full(source);
        let super::super::parser::Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!("paragraph");
        };
        let spacer = super::super::math::Spacer::new([('M', 1.0), ('i', 0.25)]);
        let flat = super::super::render::flatten_runs_weighted(
            runs,
            &Theme::dark(),
            gpui::FontWeight::NORMAL,
            Some(&spacer),
        );
        let wide = flat.math[0].range.len();
        let placeholder = |width: f32| "M".repeat(width.floor().max(1.0) as usize);
        assert!(
            fit_math(&flat, 1000., measured, placeholder).is_none(),
            "a formula that fits is left alone"
        );
        // Room for the placeholder and the space after it: 8 - 1 - 1.
        let fitted = fit_math(&flat, 8., measured, placeholder).expect("shrinks");
        let math = &fitted.math[0];
        assert_eq!(math.range.len(), 6);
        assert!((math.scale - 6.0 / wide as f32).abs() < 1e-6);
        assert_eq!(
            fitted.runs.iter().map(|r| r.len).sum::<usize>(),
            fitted.text.len()
        );
        let original = fitted.original.as_ref().unwrap();
        let map = &original.offsets;
        assert_eq!(original.text.as_ref(), source);
        assert_eq!(
            &original.text[map.original(math.range.start)..map.original(math.range.end)],
            "$x_1 + x_2 + x_3 + x_4$"
        );
        assert_eq!(
            &original.text[map.original(0)..map.original(fitted.text.len())],
            source
        );
    }

    #[test]
    fn truncation_composes_with_math_placeholders() {
        let tree = super::super::parser::parse_full(
            "$\\alpha$ see [https://example.com/a/very/long/path](https://example.com/a/very/long/path) $\\beta$",
        );
        let super::super::parser::Block::Paragraph { runs } = &tree.blocks[0].block else {
            panic!("paragraph");
        };
        let spacer = super::super::math::Spacer::new([('M', 0.83), ('n', 0.55), ('i', 0.22)]);
        let flat = super::super::render::flatten_runs_weighted(
            runs,
            &Theme::dark(),
            gpui::FontWeight::NORMAL,
            Some(&spacer),
        );
        let source = flat.original.as_ref().unwrap().text.clone();
        let shown = truncate(&flat, 24., measured);
        assert!(shown.text.len() < flat.text.len(), "the link label was cut");
        let original = shown.original.as_ref().unwrap();
        assert_eq!(
            original.text, source,
            "copy still resolves to the TeX source"
        );
        let map = &original.offsets;
        assert_eq!(
            &source[map.original(0)..map.original(shown.text.len())],
            source.as_ref()
        );
        let formulas: Vec<&str> = shown
            .math
            .iter()
            .map(|m| &source[map.original(m.range.start)..map.original(m.range.end)])
            .collect();
        assert_eq!(formulas, ["$\\alpha$", "$\\beta$"]);
        let link = &shown.links[0].0;
        assert!(shown.text[link.clone()].ends_with('…'));
        assert!(source[map.original(link.start)..map.original(link.end)].starts_with("https://"));
    }

    #[test]
    fn styled_link_runs_are_trimmed_without_corrupting_offsets() {
        let mut bold = link("very-long-bold-suffix");
        bold.style.bold = true;
        let flat = flatten_runs(&[link("https://example.com/"), bold], &Theme::dark(), false);
        let shown = truncate(&flat, 24., measured);
        assert_eq!(shown.links.len(), 1);
        assert!(shown.runs.last().unwrap().font.weight.0 >= gpui::FontWeight::SEMIBOLD.0);
        assert_eq!(
            shown.runs.iter().map(|r| r.len).sum::<usize>(),
            shown.text.len()
        );
    }
}
