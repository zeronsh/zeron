//! The CoreText-fidelity parts of the model, checked against independent rustybuzz shaping and
//! scripted fallback measurers (platform independent; tests/coretext.rs checks the same rules
//! against CoreText itself on macOS).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::*;
use unicode_segmentation::UnicodeSegmentation;
use zeron_text::*;

/// Independent rustybuzz shaping of `text` in Geist at `size` (default features: kerning and
/// standard ligatures, like the fixture's sans style).
fn shaped(text: &str, size: f32) -> f32 {
    let data = font_bytes("Geist.ttf");
    let face = rustybuzz::Face::from_slice(&data, 0).unwrap();
    let mut buf = rustybuzz::UnicodeBuffer::new();
    buf.push_str(text);
    let glyphs = rustybuzz::shape(&face, &[], buf);
    let adv: i32 = glyphs.glyph_positions().iter().map(|p| p.x_advance).sum();
    adv as f32 * size / face.units_per_em() as f32
}

fn normal() -> PrepareOptions {
    PrepareOptions::default()
}

fn pre_wrap() -> PrepareOptions {
    opts(WhiteSpace::PreWrap)
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Segments are measured separately and cached, but the seams carry pair context (kerning,
/// ligatures, contextual forms), so an unwrapped line is exactly the paragraph shaped whole —
/// whatever the segmentation and however the text is cut into same-style spans.
#[test]
fn segment_seams_reproduce_whole_paragraph_shaping() {
    const WORDS: &[&str] = &[
        "long/path/to",
        "AVATAR",
        "Toyota",
        "Wa",
        "T.",
        "V.A.T.",
        "f(x)",
        "a->b",
        "x => y",
        "don't",
        "“quoted”",
        "(parens)",
        "[1, 2]",
        "{k: v}",
        "e.g.",
        "3.14",
        "1,000",
        "50%",
        "$5",
        "office",
        "fluffy",
        "fine",
        "Type",
        "yT",
        "L'",
        "-dash-",
        "—",
        "…",
        "a/b/c",
        "https://x.io/a?b=c",
        "snake_case",
        "camelCase",
        "i++;",
        "!=",
        "<=",
        "#tag",
        "@you",
    ];
    let mut fx = Fixture::without_fallback();
    let mut rng = Rng(0xFEED);
    for case in 0..400 {
        let n = 1 + rng.below(12);
        let mut text = String::new();
        for i in 0..n {
            if i > 0 {
                text.push_str(if rng.below(6) == 0 { "" } else { " " });
            }
            text.push_str(WORDS[rng.below(WORDS.len())]);
        }
        // Random same-style span cuts on grapheme boundaries.
        let bounds: Vec<usize> = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .skip(1)
            .collect();
        let mut cuts: Vec<usize> = (0..rng.below(4))
            .filter(|_| !bounds.is_empty())
            .map(|_| bounds[rng.below(bounds.len())])
            .collect();
        cuts.sort();
        cuts.dedup();
        let mut spans = Vec::new();
        let mut prev = 0;
        for c in cuts.into_iter().chain(std::iter::once(text.len())) {
            spans.push(Span::new(prev..c, fx.sans));
            prev = c;
        }
        let p = fx.prep_spans(&text, &spans, normal());
        let want = shaped(&text, 15.0);
        let got = p.max_content_width();
        assert!(
            (got - want).abs() < 2e-3,
            "case {case} {text:?} {spans:?}: {got} vs whole-run shaping {want}"
        );
    }
}

/// A line's advance includes the kerning between its last glyph and the next line's first
/// glyph, as CoreText's paragraph-level fit does: `long/` followed by `path` is narrower than
/// `long/` alone.
#[test]
fn kerning_counts_across_the_break() {
    let mut fx = Fixture::without_fallback();
    let p = fx.prep("long/path");
    let lines = p.lines(p.max_content_width() - 1.0);
    assert_eq!(
        line_texts(&p, p.max_content_width() - 1.0),
        ["long/", "path"]
    );
    let in_context = shaped("long/p", 15.0) - shaped("p", 15.0);
    assert_close(lines[0].width, in_context, "long/ before p");
    assert!(in_context < shaped("long/", 15.0) - 0.05, "Geist kerns /p");
}

/// Where UAX #14 has no opportunity after whitespace (`( x`, `8 !=`), the line still ends after
/// the whitespace when it overflows inside it — and only then.
#[test]
fn whitespace_overflow_breaks() {
    let mut fx = Fixture::without_fallback();
    let p = fx.prep_with("aa ( bb", pre_wrap());
    assert!(
        p.segment_breaks()
            .any(|b| b == SegmentBreak::WhitespaceOverflow)
    );
    let open = shaped("aa (", 15.0);
    let space = shaped(" ", 15.0);
    // Overflow inside the space: break after it.
    assert_eq!(line_texts(&p, open + space / 2.0), ["aa (", "bb"]);
    // The space fits and `b` overflows: back up to the real opportunity after `aa `.
    assert_eq!(line_texts(&p, open + space + 1.0), ["aa", "( bb"]);
    // Everything fits.
    assert_eq!(p.line_count(1000.0), 1);
}

/// `line_count` agrees with CoreText, which is not monotone in the width once
/// whitespace-overflow breaks are in play (a slightly wider line may skip one and back up
/// further).
#[test]
fn whitespace_overflow_can_make_line_counts_non_monotone() {
    let mut fx = Fixture::without_fallback();
    let text = "aaaa ( bbbbbbbbbbbbbbbbbbbbbbb";
    let p = fx.prep_with(text, pre_wrap());
    let open = shaped("aaaa (", 15.0);
    let space = shaped(" ", 15.0);
    let tight = open + space / 2.0; // breaks after "( "
    let loose = open + space + 0.5; // backs up to "aaaa "
    assert_eq!(line_texts(&p, tight)[0], "aaaa (");
    assert_eq!(line_texts(&p, loose)[0], "aaaa");
}

/// A line fits when its advance is within `max_width + LINE_FIT_EPSILON` (CoreText's slack).
#[test]
fn fit_uses_coretext_slack() {
    let mut fx = Fixture::without_fallback();
    let p = fx.prep("aa bb");
    let w = p.max_content_width();
    assert_eq!(p.line_count(w - LINE_FIT_EPSILON / 2.0), 1);
    assert_eq!(p.line_count(w - LINE_FIT_EPSILON * 5.0), 2);
    assert_eq!(LINE_FIT_EPSILON, 0.0002);
}

/// Overflow splits happen between glyphs, never inside a ligature: `fi` stays together.
#[test]
fn ligatures_are_never_split() {
    let ligates = shaped("fi", 15.0) != shaped("f", 15.0) + shaped("i", 15.0);
    assert!(ligates, "Geist has an fi ligature");
    let mut fx = Fixture::without_fallback();
    let p = fx.prep("fine");
    assert_eq!(line_texts(&p, 0.0), ["fi", "n", "e"]);
    // Also across a same-style span boundary.
    let spans = [Span::new(0..1, fx.sans), Span::new(1..4, fx.sans)];
    let p = fx.prep_spans("fine", &spans, normal());
    assert_eq!(line_texts(&p, 0.0), ["fi", "n", "e"]);
    assert_close(p.max_content_width(), shaped("fine", 15.0), "fine");
}

fn contents(p: &Prepared) -> Vec<&str> {
    p.segment_ranges()
        .into_iter()
        .map(|(c, _, _)| &p.text()[c])
        .collect()
}

/// CoreText (Apple's ICU) breaks curly quotes like brackets: `‘ “` open (OP), `”` closes (CL);
/// `’` stays an ambiguous quote because it is also the apostrophe.
#[test]
fn curly_quotes_break_like_brackets() {
    let mut fx = Fixture::new();
    // ” is CL: break after it before a letter; “ is OP: no break after it (even after spaces).
    assert_eq!(contents(&fx.prep("x“y”z")), ["x“y”", "z"]);
    let p = fx.prep("say “ hi” ok");
    assert_eq!(contents(&p), ["say", "“", "hi”", "ok"]);
    use SegmentBreak::*;
    assert_eq!(
        p.segment_breaks().collect::<Vec<_>>(),
        [Allowed, WhitespaceOverflow, Allowed, Mandatory]
    );
    // ’ is QU: no break around the apostrophe.
    assert_eq!(contents(&fx.prep("don’t stop")), ["don’t", "stop"]);
    // Between ideographs, an opening quote may start a line.
    assert_eq!(contents(&fx.prep("例如“引号”")), ["例", "如", "“引", "号”"]);
}

/// Unicode 17 rules: a word-initial hyphen sticks to its word (LB20a), and `/` may break before
/// digits when it isn't inside a number (LB25).
#[test]
fn current_uax14_rules() {
    let mut fx = Fixture::new();
    assert_eq!(contents(&fx.prep("run -p pkg")), ["run", "-p", "pkg"]);
    assert_eq!(contents(&fx.prep("x -> y")), ["x", "->", "y"]);
    assert_eq!(contents(&fx.prep("doc/123 1/2")), ["doc/", "123", "1/2"]);
}

/// Thai, Lao, Khmer and Myanmar break at dictionary words (ICU's dictionaries), and a run's
/// edges follow the ordinary rules (no break before `?`).
#[test]
fn complex_scripts_break_at_words() {
    let mut fx = Fixture::new();
    let p = fx.prep("ภาษาไทยไม่มีการเว้นวรรค");
    let segs = contents(&p);
    assert!(segs.len() >= 4, "{segs:?}");
    assert_eq!(segs.concat(), p.text());
    assert_eq!(contents(&fx.prep("ภาษา?")), ["ภาษา?"]);
}

/// CoreText itemizes by script before shaping (ICU UScriptRun: neutrals join the run they
/// follow; a font change starts afresh), so no kerning crosses a script change.
#[test]
fn shaping_runs_follow_scripts() {
    let mut fx = Fixture::without_fallback();
    let w = fx.width("virtualizationрусский", fx.sans);
    assert_close(
        w,
        shaped("virtualization", 15.0) + shaped("русский", 15.0),
        "Latin|Cyrillic",
    );
    // The quote after Cyrillic belongs to the Cyrillic run: no `"q` kerning.
    let w = fx.width("р \"q", fx.sans);
    assert_close(
        w,
        shaped("р \"", 15.0) + shaped("q", 15.0),
        "Cyrillic neutral",
    );
    // Same script throughout: one run.
    let w = fx.width("\"quoted\"", fx.sans);
    assert_close(w, shaped("\"quoted\"", 15.0), "Latin");
}

/// In a right-to-left paragraph the bidi runs are shaped separately: `V.A.T.` before Hebrew
/// loses its final `T.` kerning because the period resolves to the right-to-left run.
#[test]
fn shaping_runs_follow_bidi_levels() {
    let mut fx = Fixture::without_fallback();
    let rtl = fx.width("שלום V.A.T.שלום", fx.sans);
    let want = shaped("שלום ", 15.0) + shaped("V.A.T", 15.0) + shaped(".שלום", 15.0);
    assert_close(rtl, want, "RTL paragraph");
    // Left-to-right paragraph: the period stays with the Latin run.
    let ltr = fx.width("x V.A.T.שלום", fx.sans);
    let want = shaped("x V.A.T.", 15.0) + shaped("שלום", 15.0);
    assert_close(ltr, want, "LTR paragraph");
}

/// A host that measures whole runs in context: every char is 10pt, but `テ` kerns −2pt before
/// `キ` — something piece-by-piece measurement can't see.
struct RunHost {
    runs: AtomicUsize,
    pieces: AtomicUsize,
    supports_runs: bool,
}

impl FallbackMeasurer for RunHost {
    fn measure(&self, _style: StyleId, text: &str) -> f32 {
        self.pieces.fetch_add(1, Ordering::Relaxed);
        10.0 * text.chars().count() as f32
    }

    fn measure_run(&self, _style: StyleId, text: &str, advances: &mut Vec<f32>) -> bool {
        self.runs.fetch_add(1, Ordering::Relaxed);
        if !self.supports_runs {
            return false;
        }
        let chars: Vec<char> = text.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            let kern = if c == 'テ' && chars.get(i + 1) == Some(&'キ') {
                -2.0
            } else {
                0.0
            };
            advances.push(10.0 + kern);
        }
        true
    }
}

fn run_book(supports_runs: bool) -> (FontBook, StyleId, Arc<RunHost>) {
    let mut book = FontBook::new();
    let face = book.add_face(font_bytes("Geist.ttf")).unwrap();
    let style = book.add_style(face, 15.0, StyleOptions::default());
    let host = Arc::new(RunHost {
        runs: AtomicUsize::new(0),
        pieces: AtomicUsize::new(0),
        supports_runs,
    });
    book.set_fallback(host.clone());
    (book, style, host)
}

#[test]
fn fallback_runs_are_measured_in_context_once() {
    let (book, style, host) = run_book(true);
    let mut cache = WidthCache::new();
    let text = "see テキスト now";
    let spans = [Span::new(0..text.len(), style)];
    let p = prepare(&book, &mut cache, text, &spans, &normal());
    // One run call for the kana run; each kana is its own segment, yet widths are in context.
    assert_eq!(host.runs.load(Ordering::Relaxed), 1);
    assert_eq!(host.pieces.load(Ordering::Relaxed), 0);
    let whole = p.max_content_width();
    let latin = shaped("see ", 15.0) + shaped(" now", 15.0);
    assert_close(whole, latin + 38.0, "テキスト with テキ kerning");
    // Line breaks inside the run use the in-context advances: テ is 8pt wide before キ.
    let w = shaped("see ", 15.0) + 18.0;
    assert_eq!(line_texts(&p, w)[0], "see テキ");
    assert_eq!(line_texts(&p, w - 0.01)[0], "see テ");
    // Warm prepare: cached, no host calls.
    let _ = prepare(&book, &mut cache, text, &spans, &normal());
    assert_eq!(host.runs.load(Ordering::Relaxed), 1);
}

#[test]
fn hosts_without_run_support_are_asked_once() {
    let (book, style, host) = run_book(false);
    let mut cache = WidthCache::new();
    for text in ["テキスト", "日本語", "かな"] {
        let spans = [Span::new(0..text.len(), style)];
        let p = prepare(&book, &mut cache, text, &spans, &normal());
        assert_close(
            p.max_content_width(),
            10.0 * text.chars().count() as f32,
            text,
        );
    }
    // measure_run declined once and was never asked again; pieces went through measure().
    assert_eq!(host.runs.load(Ordering::Relaxed), 1);
    assert!(host.pieces.load(Ordering::Relaxed) > 0);
}

/// Letter spacing still applies per grapheme to host-measured runs.
#[test]
fn fallback_runs_get_letter_spacing() {
    let mut book = FontBook::new();
    let face = book.add_face(font_bytes("Geist.ttf")).unwrap();
    let style = book.add_style(
        face,
        15.0,
        StyleOptions {
            letter_spacing: 1.0,
            ..Default::default()
        },
    );
    book.set_fallback(Arc::new(RunHost {
        runs: AtomicUsize::new(0),
        pieces: AtomicUsize::new(0),
        supports_runs: true,
    }));
    let mut cache = WidthCache::new();
    let text = "テキスト";
    let p = prepare(
        &book,
        &mut cache,
        text,
        &[Span::new(0..text.len(), style)],
        &normal(),
    );
    assert_close(p.max_content_width(), 38.0 + 4.0, "4 graphemes × 1pt");
}
