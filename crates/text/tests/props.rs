//! Property tests: random chat-like text, random spans, random widths; every layout invariant
//! checked at every width. Deterministic (tiny xorshift PRNG), no external dependency.

mod common;

use common::*;
use unicode_segmentation::UnicodeSegmentation;
use zeron_text::*;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() >> 40) as f32 / (1u64 << 24) as f32 * (hi - lo)
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

const WORDS: &[&str] = &[
    "the",
    "quick",
    "brown",
    "fox",
    "a",
    "I",
    "layout",
    "measurement",
    "virtualization",
    "don't",
    "naïve",
    "café",
    "e\u{301}",
    "hello,",
    "world.",
    "(parens)",
    "\"quoted\"",
    "—",
    "-",
    "foo-bar",
    "trans\u{AD}atlantic",
    "hy\u{AD}phen\u{AD}ation",
    "$500",
    "50%",
    "7:00-9:00",
    "x\u{A0}y",
    "alpha\u{200B}beta",
    "word\u{2060}joined",
    "WWWWWWWWWWWWWWWW",
    "supercalifragilisticexpialidocious",
    "https://example.com/a/b?c=d&e=f",
    "crates/text/src/layout.rs",
    "snake_case_identifier",
    "中文",
    "测试。",
    "日本語のテキスト",
    "한국어",
    "😀",
    "👨\u{200D}👩\u{200D}👧",
    "🇯🇵",
    "1\u{FE0F}\u{20E3}",
    "👍🏾",
    "مرحبا",
    "שלום",
    "𠀀𠀁",
    "!",
    "?",
    "…",
    "z\u{334}\u{335}\u{336}a\u{337}\u{338}lgo",
    "\u{FFFC}",
    "#",
    "a/b/c/d/e/f/g/h",
    "***",
    "🏳\u{FE0F}\u{200D}🌈",
];
const SPACES: &[&str] = &[
    " ", " ", " ", "  ", "\n", " \n ", "\t", "\u{3000}", "", "\r\n", "\r", "\u{2028}", "\u{0B}",
    "\n\n", " \t ",
];

fn random_text(rng: &mut Rng) -> String {
    let n = 1 + rng.below(30);
    let mut s = String::new();
    for i in 0..n {
        if i > 0 || rng.below(5) == 0 {
            s.push_str(rng.pick(SPACES));
        }
        s.push_str(rng.pick(WORDS));
    }
    if rng.below(5) == 0 {
        s.push_str(rng.pick(SPACES));
    }
    s
}

fn random_spans(rng: &mut Rng, text: &str, styles: &[StyleId]) -> Vec<Span> {
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
        let mut s = Span::new(prev..c, styles[rng.below(styles.len())]);
        if rng.below(3) == 0 {
            s = s.with_padding(rng.f32(0.0, 6.0), rng.f32(0.0, 6.0));
        }
        if rng.below(6) == 0 {
            s = s.with_atomic(true);
        }
        spans.push(s);
        prev = c;
    }
    spans
}

/// `ZERON_TEXT_PROP_CASES` / `ZERON_TEXT_PROP_SEED` scale a run up for bug hunting.
fn knobs(default_cases: usize, default_seed: u64) -> (usize, u64) {
    let cases = std::env::var("ZERON_TEXT_PROP_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_cases);
    let seed = std::env::var("ZERON_TEXT_PROP_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default_seed);
    (cases, seed)
}

fn is_hang_or_break(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '\n' | '\u{0B}' | '\u{85}' | '\u{2028}' | '\u{2029}'
    )
}

fn check(p: &Prepared, w: f32, ctx: &str) {
    let text = p.text();
    let lines = p.lines(w);
    let mut walked = Vec::new();
    p.walk_lines(w, |l| walked.push(l.clone()));
    let stats = p.stats(w);

    // All entry points agree.
    assert_eq!(p.line_count(w), lines.len(), "{ctx}");
    assert_eq!(stats.line_count, lines.len(), "{ctx}");
    assert_eq!(walked.len(), lines.len(), "{ctx}");
    let widest = lines.iter().map(|l| l.width).fold(0.0, f32::max);
    assert_eq!(stats.max_line_width, widest, "{ctx}");

    let graphemes: Vec<usize> = text
        .grapheme_indices(true)
        .map(|(i, _)| i)
        .chain(std::iter::once(text.len()))
        .collect();
    let is_boundary = |b: usize| graphemes.binary_search(&b).is_ok();

    let mut prev_end = 0usize;
    let mut cursor = Cursor::START;
    for (li, (line, walk)) in lines.iter().zip(&walked).enumerate() {
        let lctx = format!("{ctx} line {li} {:?}", &text[line.range.clone()]);
        assert_eq!(line.range, walk.range, "{lctx}");
        assert_eq!(line.width, walk.width, "{lctx}");
        assert_eq!(line.hyphenated, walk.hyphenated, "{lctx}");
        assert_eq!(walk.start, cursor, "{lctx}");
        assert!(walk.end > walk.start, "{lctx}: no progress");
        // layout_next_line agrees from every line start
        assert_eq!(
            p.layout_next_line(walk.start, w).as_ref(),
            Some(walk),
            "{lctx}"
        );
        cursor = walk.end;

        // Lines are in order; the only bytes between lines are hanging whitespace / forced breaks.
        assert!(line.range.start >= prev_end, "{lctx}");
        assert!(
            text[prev_end..line.range.start]
                .chars()
                .all(is_hang_or_break),
            "{lctx}: dropped visible text {:?}",
            &text[prev_end..line.range.start]
        );
        prev_end = line.range.end;

        // Never split a grapheme cluster.
        assert!(
            is_boundary(line.range.start) && is_boundary(line.range.end),
            "{lctx}"
        );

        // Fragments tile the line's visible bytes in order.
        let mut at = line.range.start;
        let mut x = 0.0f32;
        for f in &line.fragments {
            assert_eq!(f.range.start, at, "{lctx}: fragment gap");
            assert!(f.range.end > f.range.start, "{lctx}: empty fragment");
            let span = &p.spans()[f.span];
            assert!(
                span.range.start <= f.range.start && f.range.end <= span.range.end,
                "{lctx}: fragment outside its span"
            );
            assert!((f.x - x).abs() < 1e-3, "{lctx}: x {} vs {x}", f.x);
            assert!(f.width >= -1e-3 || span.pad_start < 0.0, "{lctx}");
            let u16a = text[..f.range.start].encode_utf16().count();
            let u16b = text[..f.range.end].encode_utf16().count();
            assert_eq!(f.utf16, u16a..u16b, "{lctx}");
            at = f.range.end;
            x = f.x + f.width;
        }
        assert_eq!(at, line.range.end, "{lctx}: fragments short of line end");
        let hyphen = if line.hyphenated { line.width - x } else { 0.0 };
        assert!(
            (x + hyphen - line.width).abs() < 1e-2,
            "{lctx}: fragments sum {x} vs width {}",
            line.width
        );
        if line.hyphenated {
            assert!(text[line.range.clone()].ends_with('\u{AD}'), "{lctx}");
            assert!(hyphen > 0.0, "{lctx}");
        }

        // Overflow only for a single unbreakable unit.
        if line.width > w + LINE_FIT_EPSILON && p.options().white_space != WhiteSpace::Pre {
            let body = text[line.range.clone()].trim_end_matches('\u{AD}');
            // Zero-advance graphemes (WJ, ligature tails) stay with the unit before them.
            let visible = body
                .graphemes(true)
                .filter(|g| !g.chars().all(|c| matches!(c, '\u{200B}'..='\u{200F}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}')))
                .count();
            let one_grapheme = visible <= 1
                || (visible <= 3 && line.width <= p.min_content_width() + LINE_FIT_EPSILON);
            let in_atomic = p.spans().iter().any(|s| {
                s.atomic && s.range.start <= line.range.start && line.range.end <= s.range.end
            });
            let atomic_plus = p.spans().iter().any(|s| {
                s.atomic
                    && !s.range.is_empty()
                    && line.range.start <= s.range.start
                    && s.range.end <= line.range.end
            });
            let one_segment = p.options().overflow_wrap == OverflowWrap::Normal
                && p.segment_ranges()
                    .iter()
                    .any(|(c, _, _)| c.start == line.range.start && c.end == line.range.end);
            assert!(
                one_grapheme || in_atomic || atomic_plus || one_segment,
                "{lctx}: overflow {} > {w}",
                line.width
            );
        }
    }
    assert!(
        cursor.segment == p.segment_count(),
        "{ctx}: not all segments consumed"
    );
    assert!(
        text[prev_end..].chars().all(is_hang_or_break),
        "{ctx}: trailing visible text dropped {:?}",
        &text[prev_end..]
    );
    assert!(p.layout_next_line(cursor, w).is_none(), "{ctx}");
}

#[test]
fn random_paragraphs_hold_invariants() {
    let mut fx = Fixture::new();
    let geist = FaceId(0);
    let spaced = fx.style(
        geist,
        14.0,
        StyleOptions {
            letter_spacing: 1.5,
            ..Default::default()
        },
    );
    let styles = [fx.sans, fx.bold, fx.mono, spaced];
    let modes = [
        WhiteSpace::Normal,
        WhiteSpace::PreLine,
        WhiteSpace::PreWrap,
        WhiteSpace::Pre,
    ];
    let (cases, seed) = knobs(1500, 0x9E37_79B9_7F4A_7C15);
    let mut rng = Rng(seed);
    for case in 0..cases {
        let text = random_text(&mut rng);
        let spans = random_spans(&mut rng, &text, &styles);
        let opts = PrepareOptions {
            white_space: modes[rng.below(modes.len())],
            overflow_wrap: if rng.below(4) == 0 {
                OverflowWrap::Normal
            } else {
                OverflowWrap::Anywhere
            },
            tab_size: [0u8, 2, 4, 8][rng.below(4)],
        };
        let p = fx.prep_spans(&text, &spans, opts);
        let ctx = format!("case {case} {opts:?} text={text:?} spans={spans:?}");
        let mut widths: Vec<f32> = (0..6).map(|_| rng.f32(0.0, 420.0)).collect();
        widths.extend([0.0, 1.0, p.min_content_width(), p.max_content_width(), 1e6]);
        for &w in &widths {
            check(&p, w, &ctx);
        }

        // At min-content nothing overflows except units wider than it can't happen by definition.
        if opts.white_space != WhiteSpace::Pre {
            let m = p.min_content_width();
            for l in p.lines(m) {
                assert!(
                    l.width <= m + 0.02,
                    "{ctx}: min-content overflow {} > {m}",
                    l.width
                );
            }
        }
        // At max-content (and wider) only forced breaks remain.
        let hard = p.line_count(f32::INFINITY);
        assert_eq!(p.line_count(p.max_content_width()), hard, "{ctx}");

        // Warm prepare is identical to cold.
        let mut cold = WidthCache::new();
        let q = prepare(&fx.book, &mut cold, &text, &spans, &opts);
        for &w in &widths[..3] {
            assert_eq!(p.lines(w), q.lines(w), "{ctx}");
        }
    }
}

#[test]
fn line_count_is_monotone_without_tabs_or_negative_spacing() {
    let mut fx = Fixture::new();
    let styles = [fx.sans, fx.bold, fx.mono];
    let (cases, seed) = knobs(500, 42);
    let mut rng = Rng(seed);
    for case in 0..cases {
        let text = random_text(&mut rng).replace('\t', " ");
        let spans = random_spans(&mut rng, &text, &styles);
        let opts = PrepareOptions {
            white_space: [WhiteSpace::Normal, WhiteSpace::PreWrap][rng.below(2)],
            overflow_wrap: [OverflowWrap::Normal, OverflowWrap::Anywhere][rng.below(2)],
            tab_size: 4,
        };
        let p = fx.prep_spans(&text, &spans, opts);
        if p.segment_breaks()
            .any(|b| b == SegmentBreak::WhitespaceOverflow)
        {
            // CoreText's overflow-inside-whitespace rule is not monotone (neither is CoreText).
            continue;
        }
        let mut prev = usize::MAX;
        let mut w = 0.0;
        while w < 500.0 {
            let n = p.line_count(w);
            assert!(
                n <= prev,
                "case {case} {text:?} {opts:?}: {prev} -> {n} at {w}\n{:?}\n{:?}",
                line_texts(&p, w - 3.7),
                line_texts(&p, w)
            );
            prev = n;
            w += 3.7;
        }
    }
}
