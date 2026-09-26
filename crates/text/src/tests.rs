//! Unit tests for the analysis internals (normalization, span repair, break mapping, tabs).

use crate::analysis::{
    BRK_ALLOWED, BRK_MANDATORY, BRK_SOFT_HYPHEN, RawSeg, break_opportunities, normalize,
    repair_spans, segments,
};
use crate::layout::tab_advance;
use crate::prepare::utf16_len;
use crate::{Span, StyleId, WhiteSpace};

fn span(r: std::ops::Range<usize>, s: u16) -> Span {
    Span::new(r, StyleId(s))
}

fn ranges(spans: &[Span]) -> Vec<std::ops::Range<usize>> {
    spans.iter().map(|s| s.range.clone()).collect()
}

#[test]
fn normal_collapse_assigns_space_to_the_span_where_the_run_starts() {
    // "ab" | "  " | "cd"
    let text = "ab  cd";
    let (out, spans) = normalize(
        text,
        &[span(0..3, 0), span(3..4, 1), span(4..6, 2)],
        WhiteSpace::Normal,
    );
    assert_eq!(out, "ab cd");
    assert_eq!(ranges(&spans), vec![0..3, 3..3, 3..5]);
}

#[test]
fn normal_trims_trailing_space_owned_by_an_earlier_span() {
    let text = "ab   ";
    let (out, spans) = normalize(text, &[span(0..3, 0), span(3..5, 1)], WhiteSpace::Normal);
    assert_eq!(out, "ab");
    assert_eq!(ranges(&spans), vec![0..2, 2..2]);
}

#[test]
fn pre_line_drops_spaces_around_newlines_across_spans() {
    let text = "a  \n  b";
    let (out, spans) = normalize(
        text,
        &[span(0..2, 0), span(2..4, 1), span(4..7, 2)],
        WhiteSpace::PreLine,
    );
    assert_eq!(out, "a\nb");
    assert_eq!(ranges(&spans), vec![0..1, 1..2, 2..3]);
}

#[test]
fn crlf_across_a_span_boundary_is_one_newline() {
    let text = "a\r\nb";
    let (out, spans) = normalize(text, &[span(0..2, 0), span(2..4, 1)], WhiteSpace::PreWrap);
    assert_eq!(out, "a\nb");
    assert_eq!(ranges(&spans), vec![0..1, 1..3]);
}

#[test]
fn untouched_text_is_copied_verbatim() {
    for (t, m) in [
        ("a b\tc", WhiteSpace::PreWrap),
        ("a b", WhiteSpace::Normal),
        ("a b\nc", WhiteSpace::PreLine),
    ] {
        let (out, spans) = normalize(t, &[span(0..t.len(), 0)], m);
        assert_eq!(out, t);
        assert_eq!(ranges(&spans), vec![0..t.len()]);
    }
}

#[test]
fn repair_spans_makes_a_contiguous_cover() {
    let text = "héllo";
    assert!(repair_spans(text, &[span(0..6, 0)]).is_none());
    // gap, overlap, mid-char end, short cover
    let fixed = repair_spans(text, &[span(1..2, 0), span(1..4, 1)]).unwrap();
    assert_eq!(ranges(&fixed), vec![0..3, 3..6]);
    let fixed = repair_spans(text, &[span(0..9, 0), span(9..12, 1)]).unwrap();
    assert_eq!(ranges(&fixed), vec![0..6, 6..6]);
}

#[test]
fn atomic_spans_map_breaks_back_through_u_fffc() {
    // "go @al.ice now": the chip's internal '.' would otherwise be irrelevant, but its trailing
    // text must not be separated from following closing punctuation.
    let text = "go @al.ice, now";
    let a = 3;
    let b = 10;
    let spans = [
        span(0..a, 0),
        span(a..b, 1).with_atomic(true),
        span(b..text.len(), 0),
    ];
    let mut out = Vec::new();
    break_opportunities(text, &spans, WhiteSpace::Normal, &mut out);
    let positions: Vec<u32> = out.iter().map(|b| b.0).collect();
    assert_eq!(positions, vec![3, 12, text.len() as u32]);
    assert!(out.last().unwrap().1);
}

#[test]
fn pre_breaks_only_at_hard_breaks() {
    let mut out = Vec::new();
    break_opportunities("a b-c\nd e", &[span(0..9, 0)], WhiteSpace::Pre, &mut out);
    assert_eq!(out, vec![(6, true), (9, true)]);
}

#[test]
fn no_breaks_inside_grapheme_clusters() {
    let text = "a👨\u{200D}👩b🇺🇸🇯🇵";
    let mut out = Vec::new();
    break_opportunities(text, &[span(0..text.len(), 0)], WhiteSpace::Normal, &mut out);
    let bounds: Vec<usize> = unicode_segmentation::UnicodeSegmentation::grapheme_indices(text, true)
        .map(|(i, _)| i)
        .collect();
    for (p, _) in out {
        let p = p as usize;
        assert!(p == text.len() || bounds.contains(&p), "break inside cluster at {p}");
    }
}

#[test]
fn segments_split_hang_and_classify_breaks() {
    let text = "foo  trans\u{AD}bar \t\nx";
    let mut br = Vec::new();
    break_opportunities(text, &[span(0..text.len(), 0)], WhiteSpace::PreWrap, &mut br);
    let mut segs = Vec::new();
    segments(text, &br, WhiteSpace::PreWrap, &mut segs);
    let shy_end = text.find('b').unwrap() as u32;
    let nl = text.find('\n').unwrap() as u32;
    assert_eq!(
        segs,
        vec![
            RawSeg {
                start: 0,
                content_end: 3,
                ws_end: 5,
                end: 5,
                brk: BRK_ALLOWED
            },
            RawSeg {
                start: 5,
                content_end: shy_end,
                ws_end: shy_end,
                end: shy_end,
                brk: BRK_SOFT_HYPHEN
            },
            // "bar " then "\t\n" folds into bar's hang
            RawSeg {
                start: shy_end,
                content_end: shy_end + 3,
                ws_end: nl,
                end: nl + 1,
                brk: BRK_MANDATORY
            },
            RawSeg {
                start: nl + 1,
                content_end: nl + 2,
                ws_end: nl + 2,
                end: nl + 2,
                brk: BRK_MANDATORY
            },
        ]
    );
}

#[test]
fn tab_stops() {
    // stop 20, tab_size 4 => half a space is 2.5
    assert_eq!(tab_advance(20.0, 0.0, 4), 20.0);
    assert_eq!(tab_advance(20.0, 5.0, 4), 15.0);
    assert_eq!(tab_advance(20.0, 18.0, 4), 22.0); // 2 < 2.5: skip to the following stop
    assert_eq!(tab_advance(20.0, 20.0, 4), 20.0);
    assert_eq!(tab_advance(0.0, 7.0, 0), 0.0);
}

#[test]
fn utf16_lengths() {
    for s in ["", "abc", "é", "😀", "a😀b𠀀c", "中文"] {
        assert_eq!(utf16_len(s.as_bytes()), s.encode_utf16().count(), "{s}");
    }
}
