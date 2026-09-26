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

struct CtFallback(Arc<CtFonts>);

impl FallbackMeasurer for CtFallback {
    fn measure(&self, style: StyleId, text: &str) -> f32 {
        let (font, lig) = &self.0.fonts[style.0 as usize];
        ct_width(text, font, *lig) as f32
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
            let cg = CGFont::from_data_provider(CGDataProvider::from_buffer(Arc::new(bytes.clone())))
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
        book.set_fallback(Arc::new(CtFallback(fonts.clone())));
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
                    let rw: Vec<String> =
                        p.lines(w).iter().map(|l| format!("{:.3}", l.width)).collect();
                    t.report.push(format!(
                        "{label} w={w}: rust {rust:?} vs ct {ct:?}\n    rust widths {rw:?}:\n{}\n    ct:\n{}",
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
}

// ---------------------------------------------------------------- PROBE (temporary)
fn probe_font(file: &str, size: f64) -> CTFont {
    let cg = CGFont::from_data_provider(CGDataProvider::from_buffer(Arc::new(font_bytes(file))))
        .unwrap();
    new_from_CGFont(&cg, size)
}

fn probe_lines(text: &str, font: &CTFont, w: f64) -> Vec<String> {
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

fn para_x(text: &str, font: &CTFont, idx: &[usize]) -> Vec<f64> {
    let s = attributed(text, font, None);
    let line = CTLine::new_with_attributed_string(s.as_concrete_TypeRef());
    idx.iter().map(|&i| line.get_string_offset_for_string_index(i as isize)).collect()
}

#[test]
#[ignore]
fn probe2() {
    let f = probe_font("Geist.ttf", 14.0);
    for t in ["xx example.com/abc", "xx example.com/very", "xx long/path", "xx long/that", "xx must/wrap"] {
        let i = t.find('/').unwrap() + 1;
        let w1 = ct_width(&t[..i], &f, true);
        let w2 = para_x(t, &f, &[i])[0];
        println!("{t:?}: standalone {w1:.3} para {w2:.3}");
        let (lo, hi) = if w1 < w2 { (w1, w2) } else { (w2, w1) };
        for w in [lo - 0.01, lo + 0.001, (lo + hi) / 2.0, hi - 0.001, hi + 0.001] {
            println!("   w={w:.3} {:?}", probe_lines(t, &f, w));
        }
    }
}

fn threshold(t: &str, f: &CTFont, first: &str) -> f64 {
    let (mut lo, mut hi) = (1.0f64, 2000.0f64);
    for _ in 0..60 {
        let m = (lo + hi) / 2.0;
        if probe_lines(t, f, m)[0].len() >= first.len() { hi = m } else { lo = m }
    }
    hi
}

#[test]
#[ignore]
fn probe3() {
    let f = probe_font("Geist.ttf", 14.0);
    for (t, first) in [("xx long/path/that/must", "xx long/path/that/"), ("xx long/path/that/must", "xx long/path/"), ("xx long/path/that must", "xx long/path/that "), ("Ta/Ta/Ta", "Ta/Ta/"), ("xx Ta/Ta/Ta", "xx Ta/"), ("AVAVAV ToTo xx", "AVAVAV ToTo "), ("AVAVAV ToTo xx", "AVAVAV "), ("xx long/path", "xx long/"), ("xx long/that", "xx long/"), ("xx example.com/abc", "xx example.com/"), ("xx long path", "xx long "), ("xx long-path", "xx long-"), ("xx long.path", "xx long."), ("xx long,path", "xx long,"), ("xx long, path", "xx long, "), ("xx long; path", "xx long; "), ("xx long! path", "xx long! "), ("xx long? path", "xx long? "), ("xx long) path", "xx long) "), ("xx long. path", "xx long. ")] {
        let first_trim = first.trim_end();
        println!("{t:?} -> {first:?}: threshold {:.4}  standalone {:.4} para {:.4} k {:.4}", threshold(t, &f, first), ct_width(first_trim, &f, true), para_x(t, &f, &[first_trim.encode_utf16().count()])[0], { let n = first_trim.len(); let pair = &t[n-1..n+1]; ct_width(pair, &f, true) - ct_width(&pair[..1], &f, true) - ct_width(&pair[1..], &f, true) });
    }
}

#[test]
#[ignore]
fn probe() {
    let f = probe_font("Geist.ttf", 14.0);
    let t = "CJK: 日本語のテキストも正しく折り返されます。中文也可以。 Emoji: 🚀✨👩‍💻 and https://example.com/a/very/long/url/that/keeps/going/and/going?query=parameters&more=stuff";
    let u: Vec<u16> = t.encode_utf16().collect();
    let idx: Vec<usize> = (0..=u.len()).collect();
    let xs = para_x(t, &f, &idx);
    for i in 0..u.len() {
        println!("{i:>3} {:?} x={:.3} adv={:.3}", String::from_utf16_lossy(&u[i..i+1]), xs[i], xs[i+1]-xs[i]);
    }
    println!("line 21..51 {:.3}", xs[51]-xs[21]);
    let lt = String::from_utf16_lossy(&u[21..51]);
    let lu: Vec<u16> = lt.encode_utf16().collect();
    let lx = para_x(&lt, &f, &(0..=lu.len()).collect::<Vec<_>>());
    for i in 0..lu.len() { println!("  L{i:>3} x={:.3} adv={:.3}  para adv={:.3}", lx[i], lx[i+1]-lx[i], xs[21+i+1]-xs[21+i]); }
    for txt in [t.to_string(), lt.clone(), "日本語のテキストも正しく折り返されます。中文也可以。".to_string(), "れます。中文也可以。".to_string()] {
        let a = attributed(&txt, &f, None);
        let line = CTLine::new_with_attributed_string(a.as_concrete_TypeRef());
        let runs = line.glyph_runs();
        let mut desc = Vec::new();
        for r in runs.iter() {
            let attrs = r.attributes().unwrap();
            let font = attrs.find(CFString::from_static_string("NSFont")).map(|v| unsafe { CTFont::wrap_under_get_rule(v.as_CFTypeRef() as _) });
            let idx = r.string_indices();
            desc.push(format!("[{}..] {} w={:.3}", idx.first().copied().unwrap_or(-1), font.map(|f| f.postscript_name()).unwrap_or_default(), r.get_typographic_bounds().width));
        }
        println!("{:?}\n   {}", &txt.chars().take(12).collect::<String>(), desc.join(" | "));
    }
    for s in ["🚀", "✨", "👩‍💻", "🚀✨", " 🚀", "🚀 ", "👩‍💻 "] { println!("{s:?} {:.3}", ct_width(s, &f, true)); }
    println!("standalone {:.3}", ct_width(&String::from_utf16_lossy(&u[21..51]), &f, true));
    println!("{:?}", probe_lines(t, &f, 270.0));
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

/// Line-break token ends from CFStringTokenizer (UTF-16 offsets).
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

fn rust_breaks(h: &mut Harness, text: &str) -> Vec<u32> {
    let (p, _) = h.rust_lines(text, h.styles[0].1, 1e9);
    let mut v: Vec<u32> = p
        .segment_ranges()
        .iter()
        .map(|(_, hg, b)| p.utf16_offset(if b.is_empty() { hg.end } else { b.end }) as u32)
        .collect();
    v.dedup();
    v
}

#[test]
#[ignore]
fn probe_breaks() {
    let mut h = Harness::new();
    let mut corpus = swift_corpus();
    corpus.extend(broad_corpus());
    if let Ok(extra) = std::env::var("ZT_TEXT") {
        corpus = vec![extra];
    }
    for t in &corpus {
        let cf = cf_breaks(t);
        let r = rust_breaks(&mut h, t);
        if cf != r {
            let u: Vec<u16> = t.encode_utf16().collect();
            let ctx = |b: u32| {
                format!(
                    "{}\u{2038}{}",
                    String::from_utf16_lossy(&u[(b as usize).saturating_sub(6)..b as usize]),
                    String::from_utf16_lossy(&u[b as usize..(b as usize + 4).min(u.len())])
                )
            };
            let only_cf: Vec<String> = cf.iter().filter(|b| !r.contains(b)).map(|&b| ctx(b)).collect();
            let only_r: Vec<String> = r.iter().filter(|b| !cf.contains(b)).map(|&b| ctx(b)).collect();
            println!(
                "{}\n   only CF: {:?}\n   only rust: {:?}",
                t.chars().take(50).collect::<String>(),
                only_cf,
                only_r
            );
        }
    }
}

#[test]
#[ignore]
fn fuzz_breaks() {
    let palette: &[&str] = &[
        "a", "Z", ">", "#", "א", "1", "9", "/", ".", ",", ":", "-", "\u{2010}", "\u{B4}", "—",
        "}", "、", "。", ")", "(", "[", "„", "\"", "'", "“", "”", "‘", "’", "«", "»", "!", "?",
        "$", "+", "\\", "%", "¢", "\u{A0}", "\u{2060}", "\u{200B}", " ", "  ", "\u{301}",
        "\u{200D}", "日", "本", "🎉", "👋", "🏽", "🇯", "🇵", "ー", "ぁ", "…", "한", "가", "ᄀ",
        "ᅡ", "ᆨ", "ภาษา", "|", "\t", "&", "*", "=", "~", "_", "@", "é", "ا", "ع", "·", "‧",
        "\u{2014}\u{2014}", "（", "）", "《", "》", "「", "」", "\u{3000}", "ア", "ｱ", "Ａ",
        "\u{FE0F}", "❤", "☀", "#\u{FE0F}\u{20E3}", "\u{AD}", "°", "€", "£", "№", "§", "¹",
        "‥", "々", "〜", "ゃ", "ッ", "！", "？", "：", "；", "，", "．", "′", "″", "℃", "–", "−",
        "•", "※", "‼", "⁉", "·", "क्", "ष", "ि", "्", "ᬓ", "᭄", "ក", "្", "မ", "ြ", "ا،", "؟",
        "ש", "־", "׳", "ـ", "\u{2028}", "\n", "\u{3000}", "〔", "〕", "［", "］", "｛", "｝",
        "👨\u{200D}👩", "🏳\u{FE0F}\u{200D}🌈", "✌", "☝", "⛹", "🇦🇧", "5", "0.5", "1,000", "a.b",
        "e.g.", "http://x.y/z", "#tag", "@user", "x²", "½", "ﬁ", "Ⅻ", "ⓐ", "㍿", "㈱",
    ];
    let mut h = Harness::new();
    let mut seed: u64 = std::env::var("ZT_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let iters: usize = std::env::var("ZT_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(50_000);
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    let mut seen = std::collections::HashMap::<String, (usize, String)>::new();
    let mut bad = 0;
    for _ in 0..iters {
        let n = 2 + (next() % 6) as usize;
        let t: String = (0..n).map(|_| palette[(next() % palette.len() as u64) as usize]).collect();
        let cf = cf_breaks(&t);
        let r = rust_breaks(&mut h, &t);
        if cf == r {
            continue;
        }
        bad += 1;
        let u: Vec<u16> = t.encode_utf16().collect();
        // Known, deliberate differences: we never break inside a grapheme cluster (a lone skin
        // tone modifier after a non-emoji), and whitespace-only runs fold into the previous
        // segment's hang.
        let ignorable = |b: u32| {
            let after = String::from_utf16_lossy(&u[b as usize..]);
            let before = String::from_utf16_lossy(&u[..b as usize]);
            let mut gc = unicode_segmentation::GraphemeCursor::new(before.len(), t.len(), true);
            !gc.is_boundary(&t, 0).unwrap_or(true)
                || (before.ends_with([' ', '\t', '\u{200B}']) && after.starts_with([' ', '\t']))
        };
        let cf: Vec<u32> = cf.into_iter().filter(|&b| !ignorable(b)).collect();
        let r: Vec<u32> = r.into_iter().filter(|&b| !ignorable(b)).collect();
        if cf == r {
            bad -= 1;
            continue;
        }
        for (&b, who) in cf.iter().filter(|b| !r.contains(b)).map(|b| (b, "CF-only"))
            .chain(r.iter().filter(|b| !cf.contains(b)).map(|b| (b, "rust-only")))
        {
            let l = String::from_utf16_lossy(&u[(b as usize).saturating_sub(2)..b as usize]);
            let rr = String::from_utf16_lossy(&u[b as usize..(b as usize + 2).min(u.len())]);
            let key = format!("{who} {l:?}|{rr:?}");
            let e = seen.entry(key).or_insert((0, t.clone()));
            e.0 += 1;
            if t.len() < e.1.len() {
                e.1 = t.clone();
            }
        }
    }
    let mut v: Vec<_> = seen.into_iter().collect();
    v.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    println!("{bad}/{iters} strings differ; {} distinct contexts", v.len());
    for (k, (n, ex)) in v.iter().take(80) {
        println!("{n:>5} {k}   e.g. {ex:?}");
    }
}

#[test]
#[ignore]
fn probe_cf() {
    let mut h = Harness::new();
    let list = std::env::var("ZT_LIST").unwrap_or_default();
    for t in list.split(";;") {
        let u: Vec<u16> = t.encode_utf16().collect();
        let show = |v: &[u32]| {
            let mut s = String::new();
            let mut prev = 0;
            for &b in v {
                s.push_str(&String::from_utf16_lossy(&u[prev as usize..b as usize]));
                s.push('|');
                prev = b;
            }
            s
        };
        println!("CF   {:?}\nRUST {:?}", show(&cf_breaks(t)), show(&rust_breaks(&mut h, t)));
    }
}
