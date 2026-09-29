//! CoreText ground truth (macOS only): lay a corpus out with zeron-text and with CTFramesetter
//! using the *same font bytes*, and compare line starts — the methodology of
//! apps/ios/ZeronTests/LineBreakAccuracyTests.swift, runnable with plain `cargo test`.
//!
//! `cargo test -p zeron-text --release --test coretext -- --nocapture` prints accuracy and the
//! first mismatches. `ZT_CT_VERBOSE=n` prints up to `n` mismatches with line texts;
//! `ZT_CT_FILTER=substr` restricts the corpus.

#![cfg(target_os = "macos")]

mod common;

use std::sync::Arc;

use common::font_bytes;
use core_foundation::attributed_string::CFMutableAttributedString;
use core_foundation::base::{CFRange, TCFType};
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::data_provider::CGDataProvider;
use core_graphics::font::CGFont;
use core_graphics::geometry::{CGPoint, CGRect, CGSize};
use core_graphics::path::CGPath;
use core_text::font::{CTFont, new_from_CGFont};
use core_text::framesetter::CTFramesetter;
use core_text::line::CTLine;
use core_text::string_attributes::{kCTFontAttributeName, kCTLigatureAttributeName};
use zeron_text::*;

/// CTFonts indexed by StyleId, plus whether the style uses ligatures.
struct CtFonts {
    fonts: Vec<(CTFont, bool)>,
}

// CTFont is immutable and documented thread-safe.
unsafe impl Send for CtFonts {}
unsafe impl Sync for CtFonts {}

fn attributed(text: &str, font: &CTFont, ligatures: Option<bool>) -> CFMutableAttributedString {
    let mut s = CFMutableAttributedString::new();
    s.replace_str(&CFString::new(text), CFRange::init(0, 0));
    let len = s.char_len();
    let range = CFRange::init(0, len);
    unsafe {
        s.set_attribute(range, kCTFontAttributeName, font);
        if let Some(l) = ligatures {
            s.set_attribute(range, kCTLigatureAttributeName, &CFNumber::from(l as i32));
        }
    }
    s
}

fn ct_width(text: &str, font: &CTFont, ligatures: bool) -> f64 {
    let s = attributed(text, font, Some(ligatures));
    let line = CTLine::new_with_attributed_string(s.as_concrete_TypeRef());
    line.get_typographic_bounds().width
}

/// Line start UTF-16 offsets from CTFramesetter — exactly what the Swift harness does
/// (no ligature attribute: CoreText's default).
fn ct_line_starts(text: &str, font: &CTFont, width: f64) -> Vec<u32> {
    let s = attributed(text, font, None);
    let setter = CTFramesetter::new_with_attributed_string(s.as_concrete_TypeRef());
    let path = CGPath::from_rect(
        CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(width, 100_000.0)),
        None,
    );
    let frame = setter.create_frame(CFRange::init(0, 0), &path);
    frame
        .get_lines()
        .iter()
        .map(|l| l.get_string_range().location as u32)
        .collect()
}

#[link(name = "CoreText", kind = "framework")]
unsafe extern "C" {
    fn CTRunGetAdvances(run: core_text::run::CTRunRef, range: CFRange, buffer: *mut CGSize);
}

/// Per-char advances of `text` laid out as one CTLine: each glyph's advance attributed to the
/// char its string index points into.
fn ct_run_advances(text: &str, font: &CTFont, ligatures: bool, out: &mut Vec<f32>) {
    let s = attributed(text, font, Some(ligatures));
    let line = CTLine::new_with_attributed_string(s.as_concrete_TypeRef());
    // UTF-16 index -> char index
    let mut char_of = Vec::new();
    for (ci, c) in text.chars().enumerate() {
        for _ in 0..c.len_utf16() {
            char_of.push(ci);
        }
    }
    let base = out.len();
    out.resize(base + text.chars().count(), 0.0);
    for run in line.glyph_runs().iter() {
        let n = run.glyph_count();
        let mut adv = vec![CGSize::new(0.0, 0.0); n as usize];
        unsafe {
            CTRunGetAdvances(
                run.as_concrete_TypeRef(),
                CFRange::init(0, 0),
                adv.as_mut_ptr(),
            )
        };
        for (g, &si) in run.string_indices().iter().enumerate() {
            out[base + char_of[si as usize]] += adv[g].width as f32;
        }
    }
}

struct CtFallback(Arc<CtFonts>, bool);

impl FallbackMeasurer for CtFallback {
    fn measure(&self, style: StyleId, text: &str) -> f32 {
        let (font, lig) = &self.0.fonts[style.0 as usize];
        ct_width(text, font, *lig) as f32
    }

    fn measure_run(&self, style: StyleId, text: &str, advances: &mut Vec<f32>) -> bool {
        if !self.1 {
            return false;
        }
        let (font, lig) = &self.0.fonts[style.0 as usize];
        ct_run_advances(text, font, *lig, advances);
        true
    }
}

struct Harness {
    book: FontBook,
    cache: WidthCache,
    fonts: Arc<CtFonts>,
    /// (label, style) per face × size.
    styles: Vec<(String, StyleId)>,
}

impl Harness {
    fn new() -> Self {
        let mut book = FontBook::new();
        let mut fonts = Vec::new();
        let mut styles = Vec::new();
        for (file, label, ligatures) in [
            ("Geist.ttf", "sans", true),
            ("Geist-SemiBold.ttf", "semibold", true),
            ("GeistMono.ttf", "mono", false),
        ] {
            let bytes = font_bytes(file);
            let cg =
                CGFont::from_data_provider(CGDataProvider::from_buffer(Arc::new(bytes.clone())))
                    .expect("CGFont");
            let face = book.add_face(bytes).unwrap();
            for size in [14.0f32, 16.5, 19.0] {
                let id = book.add_style(
                    face,
                    size,
                    StyleOptions {
                        ligatures,
                        ..Default::default()
                    },
                );
                assert_eq!(id.0 as usize, fonts.len());
                fonts.push((new_from_CGFont(&cg, size as f64), ligatures));
                styles.push((format!("{label} {size}pt"), id));
            }
        }
        let fonts = Arc::new(CtFonts { fonts });
        let runs = std::env::var("ZT_CT_NO_RUNS").is_err();
        book.set_fallback(Arc::new(CtFallback(fonts.clone(), runs)));
        Self {
            book,
            cache: WidthCache::new(),
            fonts,
            styles,
        }
    }

    fn rust_lines(&mut self, text: &str, style: StyleId, width: f32) -> (Prepared, Vec<u32>) {
        let spans = [Span::new(0..text.len(), style)];
        let p = prepare(
            &self.book,
            &mut self.cache,
            text,
            &spans,
            &PrepareOptions {
                white_space: WhiteSpace::PreWrap,
                overflow_wrap: OverflowWrap::Anywhere,
                ..Default::default()
            },
        );
        let starts = p
            .lines(width)
            .iter()
            .map(|l| p.utf16_offset(l.range.start) as u32)
            .collect();
        (p, starts)
    }
}

/// The Swift harness corpus: ten fixed strings plus the mobile layout fixture's paragraphs.
fn swift_corpus() -> Vec<String> {
    let mut c: Vec<String> = [
        "The transcript is laid out analytically: every row's height is known before it is shown, so scrolling never guesses.",
        "Inline code spans get chips, links are tappable, and old ideas are struck through when they no longer apply to the plan.",
        "Nested bullet with a very/long/path/that/must/wrap/somewhere/in/the/middle/because/it/is/too/wide.rs and then more words.",
        "See https://example.com/a/very/long/url/that/keeps/going/and/going?query=parameters&more=stuff for the details.",
        "Run cargo test -p zeron-text --release -- --nocapture, then compare the numbers against the previous baseline run.",
        "CJK: 日本語のテキストも正しく折り返されます。中文也可以正确换行，不需要空格。한국어 문장도 줄바꿈이 됩니다.",
        "Emoji sequences 👩‍💻 🧑🏽‍🚀 🇯🇵 1️⃣ never split, even when a line is tight 🚀✨🔥 around them.",
        "Numbers like 3,100 and 1.5×, dates like 2026-09-26, and times like 12:48 stay intact; so do e.g. and i.e. abbreviations.",
        "A sentence with an em—dash, an en–dash, “smart quotes”, and (parentheses) [brackets] {braces} that should break sensibly.",
        "supercalifragilisticexpialidocious_is_a_single_identifier_that_is_longer_than_any_phone_screen_is_wide_by_itself",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let fixture = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../mobile/src/layout/fixture.md"),
    )
    .unwrap_or_default();
    use unicode_segmentation::UnicodeSegmentation;
    for p in fixture.split("\n\n") {
        if p.graphemes(true).count() > 40 && !p.starts_with("```") && !p.starts_with('|') {
            c.push(p.replace('\n', " "));
        }
    }
    c
}

/// A broader corpus: prose, identifiers, paths, URLs, numbers, quotes/brackets, CJK/Korean/Thai,
/// Arabic/Hebrew, emoji sequences.
fn broad_corpus() -> Vec<String> {
    [
        // prose
        "It was the best of times, it was the worst of times, it was the age of wisdom, it was the age of foolishness, it was the epoch of belief.",
        "Well—I don't know. Maybe we should (just maybe) try again tomorrow? \"Sure,\" she said; 'why not.' Then: nothing at all…",
        "Mr. Smith's co-worker, a well-known self-taught engineer, re-examined the state-of-the-art long-term plan in mid-2024.",
        "Pack my box with five dozen liquor jugs! How vexingly quick daft zebras jump; the five boxing wizards jump quickly.",
        "Don't stop — keep going… until the end. “Quoted text,” ‘single quotes’, «guillemets», and „low quotes“ all appear here.",
        // code-ish
        "Call self.layout_engine.prepare_paragraph(&mut cache, &text, spans.as_slice()) and then Prepared::line_count(max_width) in the hot loop.",
        "The error was `thread 'main' panicked at 'index out of bounds: the len is 3 but the index is 7', src/main.rs:42:17` again.",
        "Use std::collections::HashMap<String, Vec<Option<Box<dyn Fn(&str) -> Result<(), Error>>>>> sparingly, please.",
        "x=1;y=2;z=x+y*3-4/5%6;if(a&&b||!c){return a?b:c;}else{while(i<10)i++;} // terse C-ish code with operators",
        "Run `npm install --save-dev @types/node@^20.11.0 typescript@~5.4.2` then `npx tsc --noEmit --strict` and fix errors.",
        "camelCaseIdentifier snake_case_identifier SCREAMING_CASE_CONSTANT kebab-case-identifier PascalCaseTypeName __dunder__ methods",
        // paths / urls
        "Edit /Users/wing/Documents/GitHub/comet-native/crates/text/src/layout.rs and ~/Library/Application Support/Zeron/config.toml today.",
        "C:\\Program Files\\Zeron\\bin\\zeron.exe --config C:\\Users\\wing\\AppData\\Roaming\\Zeron\\settings.json --verbose",
        "Docs at https://developer.apple.com/documentation/coretext/1509588-ctframesettercreateframe?language=objc#discussion and more.",
        "Mail wing@anara.com or visit http://www.example.org:8080/path/to/resource.html?a=1&b=two#section-3 for further info.",
        "git@github.com:chenglou/pretext.git, ssh://git@host.example.com:2222/repo.git, and file:///tmp/some%20file.txt are URLs.",
        // numbers / punctuation
        "Prices: $1,234.56, €99.99, £0.50, ¥10,000; percentages like 12.5% and -3.2%; ranges 10–20 and 1990-2000; ratios 16:9.",
        "Version v0.2.93-beta.1+build.456 shipped on 2026/09/26 at 14:03:22 UTC (±0.5s), up 3.5× from 1.2.3.",
        "Math: a+b=c, x^2 + y^2 = r^2, f(x) = (x - 1)/(x + 1), 1/2 + 1/3 = 5/6, and 10 > 9 >= 8 != 7 <= 6 < 5.",
        "Lists: (a) first; (b) second; [c] third; {d} fourth; <e> fifth — and finally, «f» sixth! Done?! Yes!!! Really??",
        "Phone +1 (555) 123-4567, ISBN 978-3-16-148410-0, SKU #A-1234/B, order №42, section §3.2, and footnote¹ ² ³.",
        // CJK / Korean / Thai
        "日本語の文章では、句読点「、」や「。」が行頭に来ないように禁則処理を行います。括弧（かっこ）も同様です。",
        "中文排版需要处理标点符号，例如“引号”、《书名号》、（括号）以及省略号……还有破折号——等等。",
        "한국어는 띄어쓰기 단위로 줄을 바꿉니다. 그러나 긴 단어는 중간에서 나뉠 수도 있습니다. 확인해 봅시다!",
        "ภาษาไทยไม่มีการเว้นวรรคระหว่างคำ ดังนั้นการตัดบรรทัดจึงต้องใช้พจนานุกรม เพื่อหาจุดตัดคำที่ถูกต้อง",
        "Mixed 日本語 and English text, with カタカナ and ひらがな, plus 中文 words inside an English sentence here.",
        // RTL
        "مرحبا بالعالم! هذا نص عربي طويل يجب أن يلتف بشكل صحيح عبر عدة أسطر في واجهة المستخدم الخاصة بنا.",
        "שלום עולם! זהו טקסט בעברית שצריך לעטוף כראוי על פני מספר שורות בממשק המשתמש שלנו.",
        "English with embedded עברית words and عربي words, then back to English for the remainder of this sentence.",
        // emoji
        "Family 👨‍👩‍👧‍👦, flags 🇺🇸🇬🇧🇫🇷🇩🇪🇯🇵, keycaps #️⃣ *️⃣ 0️⃣ 9️⃣, skin tones 👋🏻👋🏼👋🏽👋🏾👋🏿, and hearts ❤️‍🔥 ❤️ 💔 all render.",
        "🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉🎉 a run of emoji without spaces 🎉🎉🎉",
        "Status: ✅ done, ❌ failed, ⚠️ warning, ℹ️ info, ☑️ checked, ⭐ star, ☀️ sun — mixed text/emoji presentation.",
        // long unbreakables
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa bbbbbbbbbb",
        "sha256:9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08 is the digest of the test string.",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

#[derive(Default)]
struct Tally {
    cases: usize,
    exact: usize,
    count_diff: usize,
    report: Vec<String>,
}

fn run(h: &mut Harness, corpus: &[String], verbose: usize) -> Tally {
    let filter = std::env::var("ZT_CT_FILTER").ok();
    let mut t = Tally::default();
    for si in 0..h.styles.len() {
        let (label, style) = h.styles[si].clone();
        let font = h.fonts.fonts[style.0 as usize].0.clone();
        let mut w = 180.0f32;
        while w <= 430.0 {
            for text in corpus {
                if filter.as_deref().is_some_and(|f| !text.contains(f)) {
                    continue;
                }
                let (p, rust) = h.rust_lines(text, style, w);
                let ct = ct_line_starts(text, &font, w as f64);
                t.cases += 1;
                if rust == ct {
                    t.exact += 1;
                    continue;
                }
                if rust.len() != ct.len() {
                    t.count_diff += 1;
                }
                if t.report.len() < verbose {
                    let u16: Vec<u16> = text.encode_utf16().collect();
                    let lig = h.fonts.fonts[style.0 as usize].1;
                    let show = |starts: &[u32]| -> String {
                        starts
                            .iter()
                            .enumerate()
                            .map(|(i, &s)| {
                                let e = starts.get(i + 1).map_or(u16.len(), |&e| e as usize);
                                let line = String::from_utf16_lossy(&u16[s as usize..e]);
                                let trimmed = line.trim_end_matches(' ');
                                format!(
                                    "      {s:>4} ct={:>7.3} |{line}|",
                                    ct_width(trimmed, &font, lig)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    let rw: Vec<String> = p
                        .lines(w)
                        .iter()
                        .map(|l| format!("{:.3}", l.width))
                        .collect();
                    t.report.push(format!(
                        "{label} w={w}: rust {rust:?} vs ct {ct:?}\n    rust widths {rw:?}:\n{}\n    ct:\n{}\n    text {text:?}",
                        show(&rust),
                        show(&ct)
                    ));
                }
            }
            w += 9.0;
        }
    }
    t
}

fn print(name: &str, t: &Tally) {
    println!(
        "{name}: exact {}/{} ({:.2}%), line-count agreement {:.3}% ({} diffs)",
        t.exact,
        t.cases,
        100.0 * t.exact as f64 / t.cases.max(1) as f64,
        100.0 * (1.0 - t.count_diff as f64 / t.cases.max(1) as f64),
        t.count_diff
    );
    for r in &t.report {
        println!("  MISMATCH {r}");
    }
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
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

const WORDS: &[&str] = &[
    "the",
    "quick",
    "brown",
    "fox",
    "jumps",
    "over",
    "lazy",
    "dog",
    "a",
    "I",
    "layout",
    "measurement",
    "virtualization",
    "don't",
    "naïve",
    "café",
    "hello,",
    "world.",
    "(parens)",
    "\"quoted\"",
    "“smart”",
    "‘single’",
    "—",
    "-",
    "–",
    "foo-bar",
    "$500",
    "50%",
    "7:00-9:00",
    "x\u{A0}y",
    "WWWWWWWWWW",
    "supercalifragilisticexpialidocious",
    "https://example.com/a/b?c=d&e=f",
    "crates/text/src/layout.rs",
    "snake_case_identifier",
    "camelCaseName",
    "fn(x)",
    "a->b",
    "x != y",
    "i++;",
    "{ key: value }",
    "[1, 2, 3]",
    "中文",
    "测试。",
    "日本語のテキスト",
    "「かっこ」",
    "한국어",
    "문장도",
    "😀",
    "👨\u{200D}👩\u{200D}👧",
    "🇯🇵",
    "1\u{FE0F}\u{20E3}",
    "👍🏾",
    "مرحبا",
    "שלום",
    "!",
    "?",
    "…",
    "#tag",
    "@user",
    "e.g.",
    "i.e.,",
    "3.14",
    "1,000",
    "10×",
    "2026-09-26",
    "AVATAR",
    "Toffee",
    "office",
    "fluffy",
    "V.A.T.",
    "ﬁne",
    "ภาษาไทย",
    "Ünïcödé",
    "ÅÄÖ",
    "Straße",
    "œuvre",
    "Ελληνικά",
    "русский",
];

/// Random chat-like paragraphs (held out: not used to tune any rule).
fn random_corpus(seed: u64, n: usize) -> Vec<String> {
    let mut rng = Rng(seed);
    (0..n)
        .map(|_| {
            let words = 3 + rng.below(40);
            let mut s = String::new();
            for i in 0..words {
                if i > 0 {
                    s.push_str(match rng.below(20) {
                        0 => "",
                        1 => "  ",
                        _ => " ",
                    });
                }
                s.push_str(rng.pick(WORDS));
            }
            s
        })
        .collect()
}

/// Line starts from CTFramesetter for text styled per span (ligature attribute per style, as the
/// app renders).
fn ct_line_starts_spans(
    text: &str,
    spans: &[(std::ops::Range<usize>, &CTFont, bool)],
    width: f64,
) -> Vec<u32> {
    let mut s = CFMutableAttributedString::new();
    s.replace_str(&CFString::new(text), CFRange::init(0, 0));
    for (r, font, lig) in spans {
        let a = text[..r.start].encode_utf16().count() as isize;
        let n = text[r.clone()].encode_utf16().count() as isize;
        unsafe {
            s.set_attribute(CFRange::init(a, n), kCTFontAttributeName, *font);
            s.set_attribute(
                CFRange::init(a, n),
                kCTLigatureAttributeName,
                &CFNumber::from(*lig as i32),
            );
        }
    }
    let setter = CTFramesetter::new_with_attributed_string(s.as_concrete_TypeRef());
    let path = CGPath::from_rect(
        CGRect::new(&CGPoint::new(0.0, 0.0), &CGSize::new(width, 100_000.0)),
        None,
    );
    let frame = setter.create_frame(CFRange::init(0, 0), &path);
    frame
        .get_lines()
        .iter()
        .map(|l| l.get_string_range().location as u32)
        .collect()
}

/// Mixed styles in one paragraph: body sans with semibold words and mono code words, spans cut
/// at word boundaries and occasionally inside words.
#[test]
fn styled_runs_match_coretext() {
    let mut h = Harness::new();
    let seed = std::env::var("ZT_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3);
    let corpus = random_corpus(seed, 150);
    let styles = [h.styles[1].1, h.styles[4].1, h.styles[7].1];
    let mut rng = Rng(seed ^ 0x9E37);
    let (mut cases, mut exact, mut count_diff) = (0usize, 0usize, 0usize);
    let mut shown = 0;
    for text in &corpus {
        // Random cut points on grapheme boundaries (markup never styles half a cluster).
        let bounds: Vec<usize> =
            unicode_segmentation::UnicodeSegmentation::grapheme_indices(text.as_str(), true)
                .map(|(i, _)| i)
                .collect();
        let mut cuts: Vec<usize> = (0..1 + rng.below(6))
            .map(|_| bounds[rng.below(bounds.len())])
            .collect();
        cuts.sort();
        cuts.dedup();
        let mut spans = Vec::new();
        let mut prev = 0;
        for c in cuts.into_iter().chain(std::iter::once(text.len())) {
            if c > prev {
                spans.push(Span::new(prev..c, styles[rng.below(3)]));
                prev = c;
            }
        }
        let ct_spans: Vec<_> = spans
            .iter()
            .map(|s| {
                let (f, lig) = &h.fonts.fonts[s.style.0 as usize];
                (s.range.clone(), f, *lig)
            })
            .collect();
        let mut w = 150.0f32;
        while w <= 430.0 {
            let p = prepare(
                &h.book,
                &mut h.cache,
                text,
                &spans,
                &PrepareOptions {
                    white_space: WhiteSpace::PreWrap,
                    overflow_wrap: OverflowWrap::Anywhere,
                    ..Default::default()
                },
            );
            let rust: Vec<u32> = p
                .lines(w)
                .iter()
                .map(|l| p.utf16_offset(l.range.start) as u32)
                .collect();
            let ct = ct_line_starts_spans(text, &ct_spans, w as f64);
            cases += 1;
            if rust == ct {
                exact += 1;
            } else {
                if rust.len() != ct.len() {
                    count_diff += 1;
                }
                if shown < 8 {
                    shown += 1;
                    println!(
                        "  MISMATCH w={w}: rust {rust:?} ct {ct:?}\n    {text:?}\n    {:?}",
                        spans
                            .iter()
                            .map(|s| (s.range.clone(), s.style.0))
                            .collect::<Vec<_>>()
                    );
                }
            }
            w += 7.3;
        }
    }
    let e = exact as f64 / cases as f64;
    let l = 1.0 - count_diff as f64 / cases as f64;
    println!(
        "styled corpus: exact {exact}/{cases} ({:.2}%), line-count agreement {:.3}% ({count_diff} diffs)",
        e * 100.0,
        l * 100.0
    );
    assert!(e >= 0.99 && l >= 0.999);
}

#[test]
fn line_breaks_match_coretext() {
    let verbose = std::env::var("ZT_CT_VERBOSE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6);
    let mut h = Harness::new();
    let swift = run(&mut h, &swift_corpus(), verbose);
    print("swift corpus", &swift);
    let broad = run(&mut h, &broad_corpus(), verbose);
    print("broad corpus", &broad);
    let seed = std::env::var("ZT_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);
    let n = std::env::var("ZT_N")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);
    let random = run(&mut h, &random_corpus(seed, n), verbose);
    print("random corpus", &random);
    for (name, t) in [("swift", &swift), ("broad", &broad), ("random", &random)] {
        let exact = t.exact as f64 / t.cases as f64;
        let lines = 1.0 - t.count_diff as f64 / t.cases as f64;
        assert!(exact >= 0.99, "{name}: exact {exact}");
        assert!(lines >= 0.999, "{name}: line count agreement {lines}");
    }
}

// ------------------------------------------------ CoreText behaviors the layout model relies on
//
// Each test pins one finding about CoreText's line breaker, then checks zeron-text reproduces it.
// If an OS update changes CoreText, these say which assumption broke.

fn font_at(file: &str, size: f64) -> CTFont {
    let cg = CGFont::from_data_provider(CGDataProvider::from_buffer(Arc::new(font_bytes(file))))
        .unwrap();
    new_from_CGFont(&cg, size)
}

fn ct_lines(text: &str, font: &CTFont, w: f64) -> Vec<String> {
    let u16: Vec<u16> = text.encode_utf16().collect();
    let starts = ct_line_starts(text, font, w);
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let e = starts.get(i + 1).map_or(u16.len(), |&e| e as usize);
            String::from_utf16_lossy(&u16[s as usize..e])
        })
        .collect()
}

/// Smallest width at which CTFramesetter's first line of `text` reaches at least `first`.
fn ct_threshold(text: &str, font: &CTFont, first: &str) -> f64 {
    let (mut lo, mut hi) = (0.5f64, 2000.0f64);
    for _ in 0..60 {
        let m = (lo + hi) / 2.0;
        if ct_lines(text, font, m)[0].len() >= first.len() {
            hi = m
        } else {
            lo = m
        }
    }
    hi
}

/// zeron-text's lines for `text` in style `si` of the harness (PreWrap, anywhere).
fn rust_lines(h: &mut Harness, si: usize, text: &str, w: f32) -> Vec<String> {
    let (p, starts) = h.rust_lines(text, h.styles[si].1, w);
    let u16: Vec<u16> = p.text().encode_utf16().collect();
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let e = starts.get(i + 1).map_or(u16.len(), |&e| e as usize);
            String::from_utf16_lossy(&u16[s as usize..e])
        })
        .collect()
}

/// CoreText fits a line when its advance is within `width + 0.0002` — the crate's
/// `LINE_FIT_EPSILON`.
#[test]
fn coretext_fit_slack() {
    let f = font_at("Geist.ttf", 14.0);
    for first in [
        "xx long ",
        "the quick brown fox ",
        "a much longer line of text that goes on ",
    ] {
        let t = format!("{first}zz");
        let slack = ct_width(first.trim_end(), &f, true) - ct_threshold(&t, &f, first);
        assert!(
            (slack - LINE_FIT_EPSILON as f64).abs() < 1e-6,
            "{first:?}: slack {slack}"
        );
    }
}

/// The last glyph's advance includes its kerning with the next line's first glyph (the
/// paragraph is shaped once), so `long/` needs less room when followed by `p` than alone.
#[test]
fn coretext_counts_kerning_across_the_break() {
    let mut h = Harness::new();
    let f = h.fonts.fonts[0].0.clone();
    let t = "xx long/path";
    let alone = ct_width("xx long/", &f, true);
    let th = ct_threshold(t, &f, "xx long/");
    assert!(
        th < alone - 0.1,
        "kerned threshold {th} vs standalone {alone}"
    );
    for w in [th - 0.01, th + 0.01] {
        assert_eq!(
            rust_lines(&mut h, 0, t, w as f32),
            ct_lines(t, &f, w),
            "w={w}"
        );
    }
}

/// Where UAX #14 forbids a break after whitespace (`OP SP* ×`, `× EX`, `× CL`…), CoreText still
/// ends the line after the whitespace when the overflow lands inside it — but not when it lands
/// in the next word.
#[test]
fn coretext_breaks_after_overflowing_whitespace() {
    let mut h = Harness::new();
    let f = h.fonts.fonts[0].0.clone();
    for (t, before_ws, line) in [
        ("aa ( bb", "aa (", "aa ( "),
        ("x = 8 != 7", "x = 8", "x = 8 "),
        ("f(i++;} // c", "f(i++;}", "f(i++;} "),
    ] {
        let a = ct_width(before_ws, &f, true);
        let with_space = ct_width(&format!("{before_ws} "), &f, true);
        let w = (a + with_space) / 2.0;
        assert_eq!(ct_lines(t, &f, w)[0], line, "{t:?} at {w}");
        assert_eq!(
            rust_lines(&mut h, 0, t, w as f32),
            ct_lines(t, &f, w),
            "{t:?} at {w}"
        );
        let w = with_space + 0.5;
        assert_ne!(ct_lines(t, &f, w)[0], line, "{t:?} at {w}");
        assert_eq!(
            rust_lines(&mut h, 0, t, w as f32),
            ct_lines(t, &f, w),
            "{t:?} at {w}"
        );
    }
}

/// Emergency (overflow) breaks never land inside a ligature: `identi|fier`, not `identif|ier`.
#[test]
fn coretext_keeps_ligatures_whole() {
    let mut h = Harness::new();
    let f = h.fonts.fonts[4].0.clone(); // semibold 16.5
    let t = "supercalifragilisticexpialidocious_is_a_single_identifier_that_is_longer";
    let mut w = 150.0;
    while w < 420.0 {
        assert_eq!(
            rust_lines(&mut h, 4, t, w as f32),
            ct_lines(t, &f, w),
            "w={w}"
        );
        w += 1.3;
    }
}

#[allow(non_upper_case_globals)]
mod tok {
    use core_foundation::base::{CFAllocatorRef, CFOptionFlags, CFRange, CFTypeRef};
    use core_foundation::string::CFStringRef;
    pub type CFStringTokenizerRef = *mut std::ffi::c_void;
    pub const kCFStringTokenizerUnitLineBreak: CFOptionFlags = 3;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFStringTokenizerCreate(
            alloc: CFAllocatorRef,
            string: CFStringRef,
            range: CFRange,
            options: CFOptionFlags,
            locale: CFTypeRef,
        ) -> CFStringTokenizerRef;
        pub fn CFStringTokenizerAdvanceToNextToken(t: CFStringTokenizerRef) -> CFOptionFlags;
        pub fn CFStringTokenizerGetCurrentTokenRange(t: CFStringTokenizerRef) -> CFRange;
        pub fn CFRelease(t: CFTypeRef);
    }
}

/// Line-break token ends from CFStringTokenizer — the platform ICU's break iterator (UTF-16).
fn cf_breaks(text: &str) -> Vec<u32> {
    let s = CFString::new(text);
    let len = text.encode_utf16().count() as isize;
    let mut out = Vec::new();
    unsafe {
        let t = tok::CFStringTokenizerCreate(
            std::ptr::null(),
            s.as_concrete_TypeRef(),
            CFRange::init(0, len),
            tok::kCFStringTokenizerUnitLineBreak,
            std::ptr::null(),
        );
        while tok::CFStringTokenizerAdvanceToNextToken(t) != 0 {
            let r = tok::CFStringTokenizerGetCurrentTokenRange(t);
            out.push((r.location + r.length) as u32);
        }
        tok::CFRelease(t as _);
    }
    out
}

/// zeron-text's rule-based break opportunities (segment ends, excluding the CoreText-only
/// whitespace-overflow boundaries), as UTF-16 offsets.
fn rust_breaks(h: &mut Harness, text: &str) -> Vec<u32> {
    let (p, _) = h.rust_lines(text, h.styles[0].1, 1e9);
    let mut v: Vec<u32> = p
        .segment_ranges()
        .iter()
        .zip(p.segment_breaks())
        .filter(|(_, b)| *b != SegmentBreak::WhitespaceOverflow)
        .map(|((_, hg, b), _)| p.utf16_offset(if b.is_empty() { hg.end } else { b.end }) as u32)
        .collect();
    v.dedup();
    v
}

/// Differential fuzz of break opportunities against CFStringTokenizer over random strings drawn
/// from every line-break class (including Apple's curly-quote tailoring, SA dictionaries,
/// emoji sequences). Deliberate differences are excluded: zeron-text never breaks inside a
/// grapheme cluster (CoreText's tokenizer does before a lone combining mark / skin tone), and
/// folds whitespace-only runs into the previous segment's hang.
#[test]
fn break_opportunities_match_cf_tokenizer() {
    let palette: &[&str] = &[
        "a",
        "Z",
        ">",
        "#",
        "א",
        "1",
        "9",
        "/",
        ".",
        ",",
        ":",
        "-",
        "\u{2010}",
        "\u{B4}",
        "—",
        "}",
        "、",
        "。",
        ")",
        "(",
        "[",
        "„",
        "\"",
        "'",
        "“",
        "”",
        "‘",
        "’",
        "«",
        "»",
        "!",
        "?",
        "$",
        "+",
        "\\",
        "%",
        "¢",
        "\u{A0}",
        "\u{2060}",
        "\u{200B}",
        " ",
        "  ",
        "\u{301}",
        "\u{200D}",
        "日",
        "本",
        "🎉",
        "👋",
        "🏽",
        "🇯",
        "🇵",
        "ー",
        "ぁ",
        "…",
        "한",
        "가",
        "ᄀ",
        "ᅡ",
        "ᆨ",
        "ภาษา",
        "|",
        "\t",
        "&",
        "*",
        "=",
        "~",
        "_",
        "@",
        "é",
        "ا",
        "ع",
        "·",
        "‧",
        "\u{2014}\u{2014}",
        "（",
        "）",
        "《",
        "》",
        "「",
        "」",
        "\u{3000}",
        "ア",
        "ｱ",
        "Ａ",
        "\u{FE0F}",
        "❤",
        "☀",
        "#\u{FE0F}\u{20E3}",
        "\u{AD}",
        "°",
        "€",
        "£",
        "№",
        "§",
        "¹",
        "‥",
        "々",
        "〜",
        "ゃ",
        "ッ",
        "！",
        "？",
        "：",
        "；",
        "，",
        "．",
        "′",
        "″",
        "℃",
        "–",
        "−",
        "•",
        "※",
        "‼",
        "⁉",
        "क्",
        "ष",
        "ि",
        "ᬓ",
        "ក",
        "មិន",
        "ا،",
        "؟",
        "ש",
        "־",
        "׳",
        "ـ",
        "\n",
        "〔",
        "〕",
        "［",
        "］",
        "👨\u{200D}👩",
        "🏳\u{FE0F}\u{200D}🌈",
        "✌",
        "🇦🇧",
        "0.5",
        "1,000",
        "a.b",
        "e.g.",
        "http://x.y/z",
        "#tag",
        "@user",
        "x²",
        "½",
        "ﬁ",
        "Ⅻ",
        "㈱",
    ];
    let mut h = Harness::new();
    let mut seed: u64 = std::env::var("ZT_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let iters: usize = std::env::var("ZT_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20_000);
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut differ = 0;
    let mut examples = Vec::new();
    for _ in 0..iters {
        let n = 2 + (next() % 6) as usize;
        let t: String = (0..n)
            .map(|_| palette[(next() % palette.len() as u64) as usize])
            .collect();
        let u: Vec<u16> = t.encode_utf16().collect();
        let ignorable = |b: u32| {
            let before = String::from_utf16_lossy(&u[..b as usize]);
            let after = String::from_utf16_lossy(&u[b as usize..]);
            let mut gc = unicode_segmentation::GraphemeCursor::new(before.len(), t.len(), true);
            !gc.is_boundary(&t, 0).unwrap_or(true)
                || (before.ends_with([' ', '\t', '\u{200B}', '\n'])
                    && after.starts_with([' ', '\t', '\n']))
        };
        let cf: Vec<u32> = cf_breaks(&t)
            .into_iter()
            .filter(|&b| !ignorable(b))
            .collect();
        let r: Vec<u32> = rust_breaks(&mut h, &t)
            .into_iter()
            .filter(|&b| !ignorable(b))
            .collect();
        if cf != r {
            differ += 1;
            if examples.len() < 10 {
                examples.push(format!("{t:?}: cf {cf:?} rust {r:?}"));
            }
        }
    }
    println!("break fuzz: {differ}/{iters} strings differ");
    for e in &examples {
        println!("  {e}");
    }
    // What remains is SA text starting with an orphaned combining mark (a Khmer coeng with no
    // base), where ICU's dictionary and CoreText disagree about the first syllable.
    assert!(differ * 1000 <= iters, "{differ}/{iters} differ");
}

/// Debugging aid: `ZT_TEXT=… ZT_W=… [ZT_STYLE=i] cargo test -p zeron-text --release --test
/// coretext debug_case -- --ignored --nocapture` prints both engines' lines and the segments.
#[test]
#[ignore]
fn debug_case() {
    let mut h = Harness::new();
    let t = std::env::var("ZT_TEXT").expect("ZT_TEXT");
    let si: usize = std::env::var("ZT_STYLE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let w: f32 = std::env::var("ZT_W").expect("ZT_W").parse().unwrap();
    let font = h.fonts.fonts[h.styles[si].1.0 as usize].0.clone();
    println!("style {}", h.styles[si].0);
    let (p, _) = h.rust_lines(&t, h.styles[si].1, w);
    for l in p.lines(w) {
        println!("  rust {:>8.3} {:?}", l.width, &p.text()[l.range.clone()]);
    }
    for l in ct_lines(&t, &font, w as f64) {
        println!("  ct   {:>8.3} {l:?}", ct_width(l.trim_end(), &font, true));
    }
    for ((c, hg, _), b) in p.segment_ranges().into_iter().zip(p.segment_breaks()) {
        println!("  seg {:?} hang {:?} {b:?}", &p.text()[c], &p.text()[hg]);
    }
}
