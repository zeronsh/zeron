//! Width-dependent presentation; selection offsets always refer to original text.
use super::render::FlatText;
use gpui::{SharedString, TextRun};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

#[derive(Clone, Debug, Default)]
pub struct OffsetMap {
    pub omissions: Vec<(Range<usize>, Range<usize>)>,
}
impl OffsetMap {
    pub fn original(&self, displayed: usize) -> usize {
        let mut shift = 0;
        for (original, shown) in &self.omissions {
            if displayed < shown.start {
                break;
            }
            if displayed < shown.end {
                return original.start;
            }
            shift = original.end - shown.end;
        }
        displayed + shift
    }
    pub fn displayed(&self, original: usize) -> usize {
        let mut shift = 0;
        for (source, shown) in &self.omissions {
            if original < source.start {
                break;
            }
            if original < source.end {
                return shown.start;
            }
            shift = source.end - shown.end;
        }
        original - shift
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
        original: Some(OriginalText {
            text: flat.text.clone(),
            offsets,
        }),
    }
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
}
pub(super) fn present(
    flat: &FlatText,
    width: Pixels,
    font_size: Pixels,
    window: &Window,
) -> FlatText {
    truncate(flat, f32::from(width), |text, runs| {
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
    })
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
        let id = window.request_measured_layout(
            Default::default(),
            move |known, available, window, _| {
                let width = known.width.or(match available.width {
                    AvailableSpace::Definite(width) => Some(width),
                    _ => None,
                });
                let shown = width.map(|width| present(&flat, width, font_size, window));
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
        let flat = present(&self.flat, bounds.size.width, font_size, window);
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
