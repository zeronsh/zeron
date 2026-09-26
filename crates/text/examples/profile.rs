//! prepare (cold / warm) and layout (line_count / stats / lines) over a chat-like corpus:
//! prose, inline code chips, paths, URLs, bold runs, some CJK and emoji.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use std::hint::black_box;
use zeron_text::*;

struct Fallback;

impl FallbackMeasurer for Fallback {
    fn measure(&self, _style: StyleId, text: &str) -> f32 {
        // Stand-in for CoreText: ~1em per char at 15pt.
        text.chars().count() as f32 * 15.0
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
}

const PROSE: &[&str] = &[
    "the", "layout", "engine", "measures", "text", "once", "and", "then", "wraps", "it", "at",
    "any", "width", "with", "pure", "arithmetic", "so", "scrolling", "a", "long", "transcript",
    "never", "touches", "CoreText", "for", "heights", "we", "should", "probably", "check", "whether",
    "virtualization", "works", "when", "messages", "stream", "in", "quickly", "I", "think", "that's",
    "fine,", "but", "let's", "verify", "it.", "Also,", "the", "tests", "pass", "now.", "Okay!",
    "don't", "worry", "about", "edge-cases", "(mostly)", "—", "yes.",
];
const CODE: &[&str] = &[
    "prepare()", "line_count", "Vec<Line>", "crates/text/src/layout.rs", "cargo test -p zeron-text",
    "Arc<FontBook>", "&mut WidthCache", "u32::MAX", "fn main()", "let x = 42;",
];
const LINKS: &[&str] = &[
    "https://github.com/chenglou/pretext/blob/main/src/layout.ts",
    "https://docs.rs/rustybuzz/latest/rustybuzz/fn.shape_with_plan.html",
    "~/Documents/GitHub/comet-native/apps/ios/Sources/Transcript/RowView.swift",
];
const CJK: &[&str] = &["这个布局引擎很快。", "日本語のテキストも折り返します。", "한국어 문장도 됩니다."];
const EMOJI: &[&str] = &["😀", "👍🏽", "🎉", "👨‍👩‍👧", "🇯🇵", "🚀"];

struct Styles {
    sans: StyleId,
    bold: StyleId,
    mono: StyleId,
}

fn paragraph(rng: &mut Rng, st: &Styles) -> (String, Vec<Span>) {
    let mut text = String::new();
    let mut spans: Vec<Span> = Vec::new();
    let mut push = |text: &mut String, s: &str, style: StyleId, pad: f32| {
        let start = text.len();
        text.push_str(s);
        match spans.last_mut() {
            Some(last) if last.style == style && pad == 0.0 && last.pad_end == 0.0 => {
                last.range.end = text.len()
            }
            _ => spans.push(Span::new(start..text.len(), style).with_padding(pad, pad)),
        }
    };
    let words = 4 + rng.below(60);
    for i in 0..words {
        if i > 0 {
            push(&mut text, " ", st.sans, 0.0);
        }
        match rng.below(40) {
            0..=2 => push(&mut text, CODE[rng.below(CODE.len())], st.mono, 3.0),
            3 => push(&mut text, LINKS[rng.below(LINKS.len())], st.sans, 0.0),
            4 => push(&mut text, CJK[rng.below(CJK.len())], st.sans, 0.0),
            5 => push(&mut text, EMOJI[rng.below(EMOJI.len())], st.sans, 0.0),
            6..=7 => push(&mut text, PROSE[rng.below(PROSE.len())], st.bold, 0.0),
            _ => push(&mut text, PROSE[rng.below(PROSE.len())], st.sans, 0.0),
        }
    }
    (text, spans)
}

fn setup() -> (FontBook, Vec<(String, Vec<Span>)>, Styles) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../ui/assets/fonts");
    let read = |n: &str| std::fs::read(dir.join(n)).unwrap();
    let mut book = FontBook::new();
    let geist = book.add_face(read("Geist.ttf")).unwrap();
    let geist_bold = book.add_face(read("Geist-SemiBold.ttf")).unwrap();
    let mono = book.add_face(read("GeistMono.ttf")).unwrap();
    let st = Styles {
        sans: book.add_style(geist, 15.0, StyleOptions::default()),
        bold: book.add_style(geist_bold, 15.0, StyleOptions::default()),
        mono: book.add_style(
            mono,
            13.0,
            StyleOptions {
                ligatures: false,
                ..Default::default()
            },
        ),
    };
    book.set_fallback(Arc::new(Fallback));
    let mut rng = Rng(0xC0FFEE);
    let corpus = (0..1000).map(|_| paragraph(&mut rng, &st)).collect();
    (book, corpus, st)
}

fn prepare_all(book: &FontBook, cache: &mut WidthCache, corpus: &[(String, Vec<Span>)]) -> Vec<Prepared> {
    let opts = PrepareOptions::default();
    corpus
        .iter()
        .map(|(t, s)| prepare(book, cache, t, s, &opts))
        .collect()
}


fn main() {
    let (book, corpus, _st) = setup();
    let mut warm = WidthCache::new();
    let _ = prepare_all(&book, &mut warm, &corpus);
    let mode = std::env::args().nth(1).unwrap_or_default();
    let start = std::time::Instant::now();
    let mut n = 0;
    while start.elapsed().as_secs() < 8 {
        if mode == "cold" {
            let mut c = WidthCache::new();
            black_box(prepare_all(&book, &mut c, &corpus));
        } else {
            black_box(prepare_all(&book, &mut warm, &corpus));
        }
        n += 1;
    }
    println!("{n} iterations");
}
