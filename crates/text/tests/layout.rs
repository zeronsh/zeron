//! Behavioral layout tests against the real Geist faces (ports the spirit of pretext's
//! layout.test.ts invariants).

mod common;

use common::*;
use unicode_segmentation::UnicodeSegmentation;
use zeron_text::*;

fn segs(p: &Prepared) -> Vec<(&str, &str)> {
    p.segment_ranges()
        .into_iter()
        .map(|(c, h, _)| (&p.text()[c], &p.text()[h]))
        .collect()
}

fn contents(p: &Prepared) -> Vec<&str> {
    segs(p).into_iter().map(|(c, _)| c).collect()
}

// ---------------------------------------------------------------- prepare / segmentation

#[test]
fn empty_and_whitespace_only() {
    let mut fx = Fixture::new();
    let p = fx.prep("");
    assert_eq!(p.line_count(100.0), 0);
    assert!(p.lines(100.0).is_empty());
    let p = fx.prep("  \t\n  ");
    assert_eq!(p.text(), "");
    assert_eq!(p.line_count(100.0), 0);
    // pre-wrap keeps whitespace-only input visible as one (zero-width) line
    let p = fx.prep_with("   ", opts(WhiteSpace::PreWrap));
    assert_eq!(p.line_count(100.0), 1);
    let l = &p.lines(100.0)[0];
    assert_eq!(l.width, 0.0);
    assert!(l.range.is_empty());
}

#[test]
fn normal_collapses_and_trims() {
    let mut fx = Fixture::new();
    let p = fx.prep("  Hello\t \n  World  ");
    assert_eq!(p.text(), "Hello World");
    assert_eq!(segs(&p), vec![("Hello", " "), ("World", "")]);
}

#[test]
fn pre_line_keeps_newlines_collapses_spaces() {
    let mut fx = Fixture::new();
    let p = fx.prep_with("  a  b \n  c\t\td  \r\ne", opts(WhiteSpace::PreLine));
    assert_eq!(p.text(), "a b\nc d\ne");
    assert_eq!(line_texts(&p, 1000.0), vec!["a b", "c d", "e"]);
}

#[test]
fn pre_wrap_preserves_spaces_and_hard_breaks() {
    let mut fx = Fixture::new();
    let p = fx.prep_with("  Hello   World  ", opts(WhiteSpace::PreWrap));
    assert_eq!(p.text(), "  Hello   World  ");
    assert_eq!(segs(&p), vec![("", "  "), ("Hello", "   "), ("World", "  ")]);
    let p = fx.prep_with("Hello\r\nWorld\rX", opts(WhiteSpace::PreWrap));
    assert_eq!(p.text(), "Hello\nWorld\nX");
    assert_eq!(line_texts(&p, 1000.0), vec!["Hello", "World", "X"]);
}

#[test]
fn glue_characters_do_not_break() {
    let mut fx = Fixture::new();
    for text in ["Hello\u{A0}world", "10\u{202F}000", "foo\u{2060}bar"] {
        let p = fx.prep(text);
        assert_eq!(contents(&p), vec![text], "{text:?}");
        // even at a tiny width the glue holds the words together (Normal overflow wrap)
        let p = fx.prep_with(
            text,
            PrepareOptions {
                overflow_wrap: OverflowWrap::Normal,
                ..Default::default()
            },
        );
        assert_eq!(p.line_count(1.0), 1);
    }
    let p = fx.prep("\u{A0}");
    assert_eq!(p.line_count(200.0), 1);
}

#[test]
fn zero_width_space_is_a_break_opportunity() {
    let mut fx = Fixture::new();
    let p = fx.prep("alpha\u{200B}beta");
    assert_eq!(contents(&p), vec!["alpha\u{200B}", "beta"]);
    let alpha = fx.width("alpha", fx.sans);
    assert_eq!(p.line_count(alpha + 0.1), 2);
    assert_eq!(line_texts(&p, alpha + 0.1), vec!["alpha\u{200B}", "beta"]);
    // ZWSP itself is invisible
    assert_close(fx.width("alpha\u{200B}", fx.sans), alpha, "zwsp width");
}

#[test]
fn punctuation_attaches_like_uax14() {
    let mut fx = Fixture::new();
    assert_eq!(contents(&fx.prep("hello.")), vec!["hello."]);
    assert_eq!(
        contents(&fx.prep("said \"hello\" there")),
        vec!["said", "\"hello\"", "there"]
    );
    assert_eq!(contents(&fx.prep("“Whenever")), vec!["“Whenever"]);
    assert_eq!(contents(&fx.prep("$500 500€ 50°C")), vec!["$500", "500€", "50°C"]);
    assert_eq!(contents(&fx.prep("universe—so")), vec!["universe", "—", "so"]);
    assert_eq!(contents(&fx.prep("foo-bar")), vec!["foo-", "bar"]);
    assert_eq!(contents(&fx.prep("(hello)")), vec!["(hello)"]);
}

#[test]
fn cjk_breaks_between_ideographs_with_kinsoku() {
    let mut fx = Fixture::new();
    let p = fx.prep("中文，测试。");
    // no break before the fullwidth comma / ideographic full stop
    assert_eq!(contents(&p), vec!["中", "文，", "测", "试。"]);
    let p = fx.prep("foo 世界 bar");
    assert_eq!(contents(&p), vec!["foo", "世", "界", "bar"]);
    let w = fx.width("foo 世", fx.sans) + 0.1;
    assert_eq!(line_texts(&p, w), vec!["foo 世", "界 bar"]);
    // Hangul syllables break per syllable only across spaces in UAX #14? No: Korean uses spaces;
    // within a word H2/H3 × JL.. keep syllables breakable like browsers (per-syllable breaks).
    let p = fx.prep("ㅋㅋㅋ 진짜");
    assert!(p.segment_count() >= 3);
    // astral CJK ideographs
    let p = fx.prep("𠀀𠀀");
    assert_eq!(contents(&p), vec!["𠀀", "𠀀"]);
}

#[test]
fn urls_and_paths_break_with_anywhere() {
    let mut fx = Fixture::new();
    let url = "https://example.com/reports/q3?lang=ar&mode=full";
    let p = fx.prep(url);
    assert_eq!(
        contents(&p),
        vec!["https://", "example.com/", "reports/", "q3?", "lang=ar&mode=full"]
    );
    let path = "crates/text/src/layout.rs";
    let p = fx.prep(path);
    assert_eq!(contents(&p), vec!["crates/", "text/", "src/", "layout.rs"]);
    // a long unbroken token splits at grapheme boundaries under overflow-wrap: anywhere
    let token = "a".repeat(200);
    let p = fx.prep(&token);
    let lines = p.lines(100.0);
    assert!(lines.len() > 1);
    for l in &lines {
        assert!(l.width <= 100.0 + LINE_FIT_EPSILON);
    }
    assert_eq!(
        lines.iter().map(|l| &p.text()[l.range.clone()]).collect::<String>(),
        token
    );
    // ...and overflows with overflow-wrap: normal
    let p = fx.prep_with(
        &token,
        PrepareOptions {
            overflow_wrap: OverflowWrap::Normal,
            ..Default::default()
        },
    );
    assert_eq!(p.line_count(100.0), 1);
    assert!(p.lines(100.0)[0].width > 100.0);
}

#[test]
fn overlong_word_moves_to_a_fresh_line_before_splitting() {
    let mut fx = Fixture::new();
    let p = fx.prep("foo abcdefghijklmnopqrstuvwxyz");
    let w = fx.width("foo abc", fx.sans);
    let lines = line_texts(&p, w);
    assert_eq!(lines[0], "foo");
    assert!(lines[1].starts_with("abc"));
}

#[test]
fn emoji_sequences_are_never_split() {
    let mut fx = Fixture::new();
    let seqs = [
        "👨\u{200D}👩\u{200D}👧\u{200D}👦",
        "🇺🇸",
        "1\u{FE0F}\u{20E3}",
        "👍🏽",
        "🏳\u{FE0F}\u{200D}🌈",
        "e\u{301}",
    ];
    let text: String = seqs.concat();
    let p = fx.prep(&text);
    for w in [0.0, 1.0, 5.0, 10.0, 20.0, 40.0] {
        let lines = p.lines(w);
        let got: Vec<&str> = lines.iter().map(|l| &p.text()[l.range.clone()]).collect();
        for l in &got {
            // every line is a whole number of our clusters
            assert!(
                seqs.iter().any(|s| l.contains(s)) || l.is_empty(),
                "split cluster at w={w}: {got:?}"
            );
            for g in l.graphemes(true) {
                assert!(seqs.contains(&g), "unexpected grapheme {g:?} at w={w}");
            }
        }
    }
    // at a tiny width every cluster gets its own line
    assert_eq!(p.line_count(1.0), seqs.len());
}

#[test]
fn soft_hyphen_breaks_with_visible_hyphen() {
    let mut fx = Fixture::new();
    let p = fx.prep("trans\u{AD}atlantic");
    assert_eq!(contents(&p), vec!["trans\u{AD}", "atlantic"]);
    let wide = p.lines(500.0);
    assert_eq!(wide.len(), 1);
    assert!(!wide[0].hyphenated);
    // The soft hyphen contributes nothing; segments are measured separately (like pretext), so
    // compare against the parts rather than the kerned whole.
    let parts = fx.width("trans", fx.sans) + fx.width("atlantic", fx.sans);
    assert_close(wide[0].width, parts, "soft hyphen invisible when unused");

    let p = fx.prep("foo trans\u{AD}atlantic");
    let hyphen = fx.width("-", fx.sans);
    let first = fx.width("foo trans", fx.sans) + hyphen;
    let w = first.max(fx.width("atlantic", fx.sans)) + 0.1;
    let lines = p.lines(w);
    assert_eq!(line_texts(&p, w), vec!["foo trans\u{AD}", "atlantic"]);
    assert!(lines[0].hyphenated);
    assert!(!lines[1].hyphenated);
    assert_close(lines[0].width, first, "hyphenated width includes hyphen");
    // the hyphen must fit: one unit less and we break at the space instead
    let p2 = fx.prep_with(
        "foo trans\u{AD}atlantic",
        PrepareOptions {
            overflow_wrap: OverflowWrap::Normal,
            ..Default::default()
        },
    );
    let tight = fx.width("foo trans", fx.sans) + hyphen / 2.0;
    assert_eq!(line_texts(&p2, tight)[0], "foo");
}

#[test]
fn soft_hyphen_not_chosen_when_later_space_breaks() {
    let mut fx = Fixture::new();
    let p = fx.prep("foo trans\u{AD}atlantic labels");
    let w = fx.width("foo transatlantic", fx.sans) + 0.1;
    let lines = p.lines(w);
    assert_eq!(line_texts(&p, w), vec!["foo trans\u{AD}atlantic", "labels"]);
    assert!(!lines[0].hyphenated);
}

// ---------------------------------------------------------------- line breaking

#[test]
fn trailing_whitespace_hangs() {
    let mut fx = Fixture::new();
    let p = fx.prep_with("Hello ", opts(WhiteSpace::PreWrap));
    let hello = fx.width("Hello", fx.sans);
    assert_eq!(p.line_count(hello), 1);
    let l = &p.lines(hello)[0];
    assert_eq!(&p.text()[l.range.clone()], "Hello");
    assert_close(l.width, hello, "hanging space excluded");

    let p = fx.prep("aaa bbb ccc");
    let w = fx.width("aaa bbb", fx.sans);
    let lines = p.lines(w);
    assert_eq!(line_texts(&p, w), vec!["aaa bbb", "ccc"]);
    assert_close(lines[0].width, w, "line width without trailing space");
}

#[test]
fn pre_wrap_hanging_spaces_and_hard_breaks() {
    let mut fx = Fixture::new();
    let o = opts(WhiteSpace::PreWrap);
    let p = fx.prep_with("foo   bar", o);
    let w = fx.width("foo", fx.sans).max(fx.width("bar", fx.sans)) + 0.1;
    assert_eq!(line_texts(&p, w), vec!["foo", "bar"]);
    let p = fx.prep_with("a\nb", o);
    assert_eq!(line_texts(&p, 200.0), vec!["a", "b"]);
    let p = fx.prep_with("foo\n  \nbar", o);
    assert_eq!(line_texts(&p, 200.0), vec!["foo", "", "bar"]);
    let p = fx.prep_with("foo  \nbar", o);
    assert_eq!(line_texts(&p, 200.0), vec!["foo", "bar"]);
    let p = fx.prep_with("\n\n", o);
    assert_eq!(line_texts(&p, 200.0), vec!["", ""]);
    let p = fx.prep_with("a\n", o);
    assert_eq!(line_texts(&p, 200.0), vec!["a"]);
    // leading spaces at a hard-line start are content when followed by text on the line
    let p = fx.prep_with("x\n    indented", o);
    let lines = p.lines(500.0);
    assert_eq!(&p.text()[lines[1].range.clone()], "    indented");
    assert_close(
        lines[1].width,
        fx.width("    indented", fx.sans),
        "leading spaces count",
    );
}

#[test]
fn pre_wrap_tabs_advance_to_tab_stops() {
    let mut fx = Fixture::new();
    let o = PrepareOptions {
        white_space: WhiteSpace::PreWrap,
        tab_size: 4,
        ..Default::default()
    };
    let space = fx.width(" ", fx.sans);
    let stop = 4.0 * space;
    let a = fx.width("a", fx.sans);
    let b = fx.width("b", fx.sans);
    let p = fx.prep_with("a\tb", o);
    let lines = p.lines(500.0);
    assert_eq!(lines.len(), 1);
    assert_close(lines[0].width, stop + b, "a\\tb width (a < stop)");
    assert!(a < stop);
    // the tab is its own fragment, positioned at a's end, spanning to the stop
    let frags = &lines[0].fragments;
    assert_eq!(frags.len(), 3);
    assert_eq!(&p.text()[frags[1].range.clone()], "\t");
    assert_close(frags[1].x, a, "tab x");
    assert_close(frags[1].x + frags[1].width, stop, "tab ends on the stop");
    assert_close(frags[2].x, stop, "b at stop");
    // consecutive tabs are distinct stops
    let p = fx.prep_with("a\t\tb", o);
    assert_close(p.lines(500.0)[0].width, 2.0 * stop + b, "two tabs");
    // tab stops restart after a hard break
    let p = fx.prep_with("foo\n\tbar", o);
    let lines = p.lines(500.0);
    assert_close(
        lines[1].width,
        stop + fx.width("bar", fx.sans),
        "tab after newline",
    );
    // a trailing tab hangs at a wrap
    let p = fx.prep_with("a\tb", o);
    let lines = line_texts(&p, stop + b - 0.1);
    assert_eq!(lines, vec!["a", "b"]);
    // tab_size 0 renders tabs zero-width
    let p = fx.prep_with(
        "a\tb",
        PrepareOptions {
            tab_size: 0,
            ..o
        },
    );
    assert_close(p.lines(500.0)[0].width, a + b, "tab_size 0");
}

#[test]
fn pre_never_wraps() {
    let mut fx = Fixture::new();
    let o = opts(WhiteSpace::Pre);
    let text = "fn main() {\n    let x = some_long_function_name(argument_one, argument_two);\n}";
    let p = fx.prep_with(text, o);
    for w in [10.0, 100.0, 1000.0] {
        assert_eq!(p.line_count(w), 3);
    }
    let lines = p.lines(10.0);
    assert_eq!(
        &p.text()[lines[1].range.clone()],
        "    let x = some_long_function_name(argument_one, argument_two);"
    );
    // trailing spaces don't hang in pre
    let p = fx.prep_with("ab  ", o);
    assert_close(
        p.lines(1.0)[0].width,
        fx.width("ab  ", fx.sans),
        "pre keeps trailing spaces",
    );
    assert_eq!(p.min_content_width(), p.max_content_width());
}

#[test]
fn line_count_monotone_in_width() {
    let mut fx = Fixture::new();
    let p = fx.prep("The quick brown fox jumps over the lazy dog and keeps running far away.");
    let mut prev = usize::MAX;
    for w in (10..600).step_by(7) {
        let n = p.line_count(w as f32);
        assert!(n <= prev, "line count grew at {w}");
        prev = n;
    }
}

#[test]
fn walk_lines_stats_and_lines_agree() {
    let mut fx = Fixture::new();
    let p = fx.prep("foo trans\u{AD}atlantic said \"hello\" to 世界 and waved. alpha\u{200B}beta 🚀");
    for w in [20.0, 48.0, 72.0, 90.0, 120.0, 400.0] {
        let lines = p.lines(w);
        let mut walked = Vec::new();
        p.walk_lines(w, |l| walked.push(l.clone()));
        assert_eq!(walked.len(), lines.len());
        assert_eq!(p.line_count(w), lines.len());
        let stats = p.stats(w);
        assert_eq!(stats.line_count, lines.len());
        let widest = lines.iter().map(|l| l.width).fold(0.0, f32::max);
        assert_eq!(stats.max_line_width, widest);
        for (a, b) in walked.iter().zip(&lines) {
            assert_eq!(a.range, b.range);
            assert_eq!(a.width, b.width);
            assert_eq!(a.hyphenated, b.hyphenated);
        }
        // layout_next_line resumes from any line start without hidden state
        let mut cur = Cursor::START;
        for want in &walked {
            let got = p.layout_next_line(cur, w).unwrap();
            assert_eq!(&got, want);
            cur = got.end;
        }
        assert!(p.layout_next_line(cur, w).is_none());
        for (i, want) in walked.iter().enumerate() {
            let got = p.layout_next_line(want.start, w).unwrap();
            assert_eq!(&got, &walked[i]);
        }
    }
}

#[test]
fn variable_width_streaming_is_contiguous() {
    let mut fx = Fixture::new();
    let p = fx.prep("foo trans\u{AD}atlantic said \"hello\" to 世界 and waved. According to محمد الأحمد, alpha\u{200B}beta 🚀 supercalifragilisticexpialidocious");
    let widths = [140.0, 72.0, 30.0, 64.0, 160.0, 20.0, 116.0, 70.0];
    let mut cur = Cursor::START;
    let mut prev_end = 0;
    let mut i = 0;
    while let Some(l) = p.layout_next_line(cur, widths[i % widths.len()]) {
        assert!(l.end > l.start);
        assert!(l.range.start >= prev_end);
        prev_end = l.range.end;
        cur = l.end;
        i += 1;
    }
    assert!(i > 5);
    assert_eq!(prev_end, p.text().len());
}

#[test]
fn widths_are_monotone_in_text() {
    let mut fx = Fixture::new();
    let mut prev = 0.0;
    let s = "Monotone widths hold for prefixes of plain text";
    for (i, _) in s.char_indices().skip(1) {
        let w = fx.width(&s[..i], fx.sans);
        assert!(w >= prev - 1e-4, "prefix {:?}", &s[..i]);
        prev = w;
    }
}

// ---------------------------------------------------------------- spans, fragments, offsets

#[test]
fn fragments_split_by_span_and_carry_exact_x() {
    let mut fx = Fixture::new();
    let text = "see the file crates/text/src/lib.rs for details";
    let code = text.find("crates").unwrap();
    let code_end = code + "crates/text/src/lib.rs".len();
    let spans = [
        Span::new(0..code, fx.sans),
        Span::new(code..code_end, fx.mono).with_padding(4.0, 4.0),
        Span::new(code_end..text.len(), fx.sans),
    ];
    let p = fx.prep_spans(text, &spans, PrepareOptions::default());
    let lines = p.lines(10_000.0);
    assert_eq!(lines.len(), 1);
    let f = &lines[0].fragments;
    assert_eq!(f.len(), 3);
    assert_eq!(&p.text()[f[1].range.clone()], "crates/text/src/lib.rs");
    assert_eq!(f[1].span, 1);
    assert_eq!(f[0].x, 0.0);
    assert_close(f[0].width, fx.width("see the file ", fx.sans), "prefix width");
    assert_close(
        f[1].width,
        fx.width("crates/text/src/lib.rs", fx.mono) + 8.0,
        "code chip width incl padding",
    );
    assert_close(f[1].x, f[0].x + f[0].width, "x cumulative");
    assert_close(f[2].x, f[1].x + f[1].width, "x cumulative");
    assert_close(
        lines[0].width,
        f[2].x + f[2].width,
        "line width is the fragment sum",
    );

    // wrapped inside the code span: padding only at the span's true start/end
    let mono_w = fx.width("crates/", fx.mono);
    let lines = p.lines(fx.width("see the file", fx.sans).max(mono_w + 8.0) + 1.0);
    let code_frags: Vec<&Fragment> = lines
        .iter()
        .flat_map(|l| &l.fragments)
        .filter(|f| f.span == 1)
        .collect();
    assert!(code_frags.len() >= 2);
    let total: f32 = code_frags.iter().map(|f| f.width).sum();
    assert_close(
        total,
        fx.width("crates/text/src/lib.rs", fx.mono) + 8.0,
        "padding counted exactly once each side across lines",
    );
}

#[test]
fn atomic_spans_are_unbreakable_units() {
    let mut fx = Fixture::new();
    let text = "ping @maya.long.handle.name please";
    let a = text.find('@').unwrap();
    let b = a + "@maya.long.handle.name".len();
    let spans = [
        Span::new(0..a, fx.sans),
        Span::new(a..b, fx.bold).with_padding(6.0, 6.0).with_atomic(true),
        Span::new(b..text.len(), fx.sans),
    ];
    let p = fx.prep_spans(text, &spans, PrepareOptions::default());
    let chip = fx.width("@maya.long.handle.name", fx.bold) + 12.0;
    for w in [5.0, 30.0, 80.0, chip - 1.0, chip + 1.0, 400.0] {
        let lines = p.lines(w);
        let chip_frags: Vec<&Fragment> = lines
            .iter()
            .flat_map(|l| &l.fragments)
            .filter(|f| f.span == 1)
            .collect();
        assert_eq!(chip_frags.len(), 1, "chip split at w={w}");
        assert_close(chip_frags[0].width, chip, "chip width");
    }
    // a chip is breakable around, like an atomic inline, but glued to closing punctuation
    let text = "hi @maya, bye";
    let a = 3;
    let b = 8;
    let spans = [
        Span::new(0..a, fx.sans),
        Span::new(a..b, fx.bold).with_atomic(true),
        Span::new(b..text.len(), fx.sans),
    ];
    let p = fx.prep_spans(text, &spans, PrepareOptions::default());
    assert_eq!(contents(&p), vec!["hi", "@maya,", "bye"]);
}

#[test]
fn normalization_remaps_spans() {
    let mut fx = Fixture::new();
    let text = "  hello   big \n world  ";
    let big = text.find("big").unwrap();
    let spans = [
        Span::new(0..big, fx.sans),
        Span::new(big..big + 3, fx.bold),
        Span::new(big + 3..text.len(), fx.sans),
    ];
    let p = fx.prep_spans(text, &spans, PrepareOptions::default());
    assert_eq!(p.text(), "hello big world");
    let s = p.spans();
    assert_eq!(s.len(), 3);
    assert_eq!(&p.text()[s[0].range.clone()], "hello ");
    assert_eq!(&p.text()[s[1].range.clone()], "big");
    assert_eq!(&p.text()[s[2].range.clone()], " world");
    assert_eq!(s[0].range.start, 0);
    assert_eq!(s[2].range.end, p.text().len());
    // a whitespace-only span collapses into its neighbor's space and ends up empty
    let text = "a   b";
    let spans = [
        Span::new(0..2, fx.sans),
        Span::new(2..3, fx.bold),
        Span::new(3..5, fx.sans),
    ];
    let p = fx.prep_spans(text, &spans, PrepareOptions::default());
    assert_eq!(p.text(), "a b");
    assert!(p.spans()[1].range.is_empty());
    let frags: Vec<usize> = p.lines(500.0)[0].fragments.iter().map(|f| f.span).collect();
    assert_eq!(frags, vec![0, 2]);
}

#[test]
fn utf16_offsets_handle_astral_chars() {
    let mut fx = Fixture::new();
    let text = "a😀b 𠀀c é";
    let p = fx.prep(text);
    for l in p.lines(10_000.0).iter().chain(p.lines(20.0).iter()) {
        for f in &l.fragments {
            let a = p.text()[..f.range.start].encode_utf16().count();
            let b = p.text()[..f.range.end].encode_utf16().count();
            assert_eq!(f.utf16, a..b);
            assert_eq!(p.utf16_offset(f.range.start), a);
        }
    }
}

#[test]
fn rtl_detection() {
    let mut fx = Fixture::new();
    assert!(!fx.prep("hello world").has_rtl());
    assert!(!fx.prep("héllo 世界 😀").has_rtl());
    assert!(fx.prep("According to محمد الأحمد, the results").has_rtl());
    assert!(fx.prep("שלום").has_rtl());
    let p = fx.prep("According to محمد الأحمد, the results improved.");
    let joined: String = p
        .lines(120.0)
        .iter()
        .flat_map(|l| l.fragments.iter().map(|f| &p.text()[f.range.clone()]))
        .collect::<Vec<_>>()
        .join("|");
    assert!(joined.contains("محمد"));
}

#[test]
fn min_and_max_content_width() {
    let mut fx = Fixture::new();
    let p = fx.prep("aa bbbb c");
    assert_close(p.max_content_width(), fx.width("aa bbbb c", fx.sans), "max");
    // anywhere: min-content is the widest grapheme
    let widest = ["a", "b", "c"]
        .iter()
        .map(|g| fx.width(g, fx.sans))
        .fold(0.0, f32::max);
    assert!(p.min_content_width() <= widest + 1e-3);
    let p = fx.prep_with(
        "aa bbbb c",
        PrepareOptions {
            overflow_wrap: OverflowWrap::Normal,
            ..Default::default()
        },
    );
    assert_close(p.min_content_width(), fx.width("bbbb", fx.sans), "min normal");
    // laying out at min-content never overflows
    let w = p.min_content_width();
    assert!(p.lines(w).iter().all(|l| l.width <= w + LINE_FIT_EPSILON));
    let p = fx.prep_with("wide line\nfit\nmid", opts(WhiteSpace::PreWrap));
    assert_close(p.max_content_width(), fx.width("wide line", fx.sans), "hard lines");
}

#[test]
fn hostile_widths_are_handled() {
    let mut fx = Fixture::new();
    let p = fx.prep("hello world");
    assert_eq!(p.line_count(f32::NAN), 1);
    assert_eq!(p.line_count(f32::INFINITY), 1);
    assert_eq!(p.line_count(-5.0), p.line_count(0.0));
    assert_eq!(p.line_count(0.0), "helloworld".len());
}
