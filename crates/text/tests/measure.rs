//! Measurement ground truth, the width cache, and the fallback measurer.

mod common;

use std::sync::Arc;

use common::*;
use zeron_text::*;

fn assert_send_sync<T: Send + Sync>() {}
fn assert_send<T: Send>() {}

#[test]
fn thread_safety() {
    assert_send_sync::<FontBook>();
    assert_send_sync::<Prepared>();
    assert_send::<WidthCache>();
}

/// Independent rustybuzz shaping, the ground truth `prepare` must reproduce.
fn shaped(face_file: &str, size: f32, text: &str) -> f32 {
    let data = font_bytes(face_file);
    let face = rustybuzz::Face::from_slice(&data, 0).unwrap();
    let mut buf = rustybuzz::UnicodeBuffer::new();
    buf.push_str(text);
    let glyphs = rustybuzz::shape(&face, &[], buf);
    let adv: i32 = glyphs.glyph_positions().iter().map(|p| p.x_advance).sum();
    adv as f32 * size / face.units_per_em() as f32
}

#[test]
fn widths_match_rustybuzz_ground_truth() {
    let mut fx = Fixture::new();
    for text in ["Hello, World!", "AVATAR Toyota", "naïve café", "0123456789", "“quotes” — dash"] {
        let got = fx.width(text, fx.sans);
        assert_close(got, shaped("Geist.ttf", 15.0, text), text);
    }
    let got = fx.width("fn main() -> i32", fx.mono);
    // mono has ligatures off; plain shaping may differ only if the face had liga for this text
    assert!(got > 0.0);
    assert_eq!(fx.fallback_calls(), 0, "covered text never reaches the fallback");
}

#[test]
fn kerning_is_applied() {
    let mut fx = Fixture::new();
    let kerned = ["AV", "To", "Ty", "LT", "Yo", "P."].iter().any(|pair| {
        let whole = fx.width(pair, fx.sans);
        let (a, b) = pair.split_at(1);
        let parts = fx.width(a, fx.sans) + fx.width(b, fx.sans);
        whole < parts - 0.01
    });
    assert!(kerned, "Geist kerns at least one classic pair");
}

#[test]
fn ligature_toggle_uses_separate_plans() {
    let mut fx = Fixture::new();
    let geist = FaceId(0);
    let liga = fx.style(geist, 15.0, StyleOptions::default());
    let noliga = fx.style(
        geist,
        15.0,
        StyleOptions {
            ligatures: false,
            ..Default::default()
        },
    );
    assert_eq!(liga, fx.sans, "interned");
    assert_ne!(liga, noliga);
    for text in ["office affine", "plain"] {
        let a = fx.width(text, liga);
        let b = fx.width(text, noliga);
        assert!(a > 0.0 && b > 0.0);
        assert!((a - b).abs() < 2.0, "{text}: {a} vs {b}");
    }
}

#[test]
fn letter_spacing_is_per_grapheme() {
    let mut fx = Fixture::new();
    let spaced = fx.style(
        FaceId(0),
        15.0,
        StyleOptions {
            letter_spacing: 2.0,
            ..Default::default()
        },
    );
    for (text, graphemes) in [("hello", 5), ("cafe\u{301}", 4), ("a b", 3)] {
        let base = fx.width(text, fx.sans);
        let ls = fx.width(text, spaced);
        assert_close(ls, base + 2.0 * graphemes as f32, text);
    }
    // per-grapheme split advances carry the spacing too: at width 0 each grapheme is a line
    let spans = [Span::new(0..5, spaced)];
    let p = fx.prep_spans("hello", &spans, PrepareOptions::default());
    let lines = p.lines(0.0);
    assert_eq!(lines.len(), 5);
    let sum: f32 = lines.iter().map(|l| l.width).sum();
    assert_close(sum, fx.width("hello", spaced), "units sum to the run");
}

#[test]
fn grapheme_advances_sum_to_the_shaped_run() {
    let mut fx = Fixture::new();
    for text in ["Wavefunction", "office", "AVATAR", "naïve", "e\u{301}e\u{301}x"] {
        let p = fx.prep(text);
        let lines = p.lines(0.0);
        let sum: f32 = lines.iter().map(|l| l.width).sum();
        assert_close(sum, fx.width(text, fx.sans), text);
        // and splitting at any width tiles the run
        for w in [10.0, 20.0, 35.0] {
            let sum: f32 = p.lines(w).iter().map(|l| l.width).sum();
            assert_close(sum, fx.width(text, fx.sans), text);
        }
    }
}

#[test]
fn fallback_only_for_uncovered_text_and_cached() {
    let mut fx = Fixture::new();
    let text = "hello 😀 world 中文 again 😀";
    let p = fx.prep(text);
    let calls = fx.fallback_calls();
    assert!(calls >= 3, "emoji and CJK go to the host: {calls}");
    let log = fx.fallback.as_ref().unwrap().log.lock().unwrap().clone();
    for t in &log {
        assert!(
            t.chars().any(|c| !c.is_ascii()),
            "covered text reached the fallback: {t:?}"
        );
    }
    // deterministic fallback widths flow into layout: 😀 is 1em
    let emoji_line = fx.prep("😀");
    assert_close(emoji_line.max_content_width(), 15.0, "fallback width used");
    // everything is cached now: preparing again makes no calls
    let before = fx.fallback_calls();
    let p2 = fx.prep(text);
    assert_eq!(fx.fallback_calls(), before);
    assert_eq!(p.lines(100.0), p2.lines(100.0));
}

#[test]
fn emoji_presentation_forces_fallback() {
    let mut fx = Fixture::new();
    fx.prep("↩\u{FE0F}");
    let log = fx.fallback.as_ref().unwrap().log.lock().unwrap().clone();
    assert!(log.iter().any(|t| t.contains('\u{FE0F}')), "{log:?}");
}

#[test]
fn fallback_graphemes_split_proportionally() {
    let mut fx = Fixture::new();
    // One long uncovered word (no break opportunities inside).
    let word = "مرحبامرحبامرحبامرحبا";
    let p = fx.prep(word);
    let total = p.max_content_width();
    for w in [10.0, 30.0, 60.0] {
        let lines = p.lines(w);
        assert!(lines.len() > 1);
        let sum: f32 = lines.iter().map(|l| l.width).sum();
        assert_close(sum, total, "split fallback word tiles its width");
        for l in &lines {
            assert!(l.width <= w + LINE_FIT_EPSILON);
        }
    }
}

#[test]
fn no_fallback_measures_notdef() {
    let mut fx = Fixture::without_fallback();
    let w = fx.width("中", fx.sans);
    assert_close(w, shaped("Geist.ttf", 15.0, "中"), "notdef advance");
    assert!(w > 0.0);
}

#[test]
fn cache_hits_on_warm_prepare() {
    let mut fx = Fixture::new();
    let text = "The quick brown fox jumps over the lazy dog. The dog sleeps.";
    fx.prep(text);
    let cold = fx.cache.stats();
    assert!(cold.misses > 0);
    fx.cache.reset_stats();
    let _ = fx.prep(text);
    let warm = fx.cache.stats();
    assert_eq!(warm.misses, 0, "warm prepare measures nothing");
    assert!(warm.hits > 0);
    assert_eq!(warm.entries, cold.entries);
    fx.cache.clear();
    assert_eq!(fx.cache.stats().entries, 0);
}

#[test]
fn cache_rebinds_to_a_different_book() {
    let mut fx = Fixture::new();
    fx.prep("some words here");
    assert!(fx.cache.stats().entries > 0);
    let mut other = FontBook::new();
    let face = other.add_face(font_bytes("GeistMono.ttf")).unwrap();
    let style = other.add_style(face, 30.0, StyleOptions::default());
    // Same StyleId(0) in a different book must not reuse the old widths.
    assert_eq!(style, StyleId(0));
    let spans = [Span::new(0..4, style)];
    let p = prepare(&other, &mut fx.cache, "abcd", &spans, &PrepareOptions::default());
    assert_close(p.max_content_width(), shaped("GeistMono.ttf", 30.0, "abcd"), "rebind");
}

#[test]
fn set_fallback_invalidates_cached_widths() {
    let mut book = FontBook::new();
    let face = book.add_face(font_bytes("Geist.ttf")).unwrap();
    let style = book.add_style(face, 10.0, StyleOptions::default());
    let mut cache = WidthCache::new();
    let spans = [Span::new(0..3, style)];
    let before = prepare(&book, &mut cache, "中", &spans, &PrepareOptions::default());
    struct Fixed;
    impl FallbackMeasurer for Fixed {
        fn measure(&self, _: StyleId, _: &str) -> f32 {
            42.0
        }
    }
    book.set_fallback(Arc::new(Fixed));
    let after = prepare(&book, &mut cache, "中", &spans, &PrepareOptions::default());
    assert_ne!(before.max_content_width(), 42.0);
    assert_eq!(after.max_content_width(), 42.0);
}

#[test]
fn metrics_and_names() {
    let fx = Fixture::new();
    let m = fx.book.metrics(fx.sans);
    assert!(m.ascent > 0.0 && m.descent > 0.0);
    assert!(m.cap_height > 0.0 && m.x_height > 0.0);
    assert!(m.cap_height < m.ascent);
    assert!(m.line_height() >= m.ascent + m.descent);
    let name = fx.book.face_postscript_name(FaceId(0)).unwrap();
    assert!(name.starts_with("Geist"), "{name}");
    assert!(fx.book.face_postscript_name(FaceId(99)).is_none());
    assert_eq!(fx.book.style(fx.mono).size, 13.0);
    assert!(!fx.book.style(fx.mono).options.ligatures);
}

#[test]
fn bad_font_data_is_an_error() {
    let mut book = FontBook::new();
    assert_eq!(book.add_face(vec![1, 2, 3]), Err(FontError::Parse));
}
