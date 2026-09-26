//! prepare (cold / warm) and layout (line_count / stats / lines) over a chat-like corpus:
//! prose, inline code chips, paths, URLs, bold runs, some CJK and emoji.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use criterion::{BatchSize, Criterion, black_box, criterion_group, criterion_main};
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

fn benches(c: &mut Criterion) {
    let (book, corpus, st) = setup();
    let bytes: usize = corpus.iter().map(|(t, _)| t.len()).sum();
    let mut warm = WidthCache::new();
    let prepared = prepare_all(&book, &mut warm, &corpus);
    let segs: usize = prepared.iter().map(|p| p.segment_count()).sum();
    let heap: usize = prepared.iter().map(|p| p.heap_bytes()).sum();
    println!(
        "corpus: {} paragraphs, {bytes} bytes, {segs} segments, prepared heap {heap} bytes, cache {:?}",
        corpus.len(),
        warm.stats()
    );

    let mut g = c.benchmark_group("prepare");
    g.sample_size(20).measurement_time(Duration::from_secs(4));
    g.bench_function("cold_1k_paragraphs", |b| {
        b.iter_batched(
            WidthCache::new,
            |mut cache| black_box(prepare_all(&book, &mut cache, &corpus)),
            BatchSize::LargeInput,
        )
    });
    g.bench_function("warm_1k_paragraphs", |b| {
        b.iter(|| black_box(prepare_all(&book, &mut warm, &corpus)))
    });
    g.finish();

    let mut g = c.benchmark_group("layout");
    g.sample_size(50).measurement_time(Duration::from_secs(3));
    for w in [320.0f32, 390.0, 430.0] {
        g.bench_function(format!("line_count_1k_paragraphs_w{w}"), |b| {
            b.iter(|| {
                prepared
                    .iter()
                    .map(|p| p.line_count(black_box(w)))
                    .sum::<usize>()
            })
        });
    }
    g.bench_function("stats_1k_paragraphs_w390", |b| {
        b.iter(|| {
            prepared
                .iter()
                .map(|p| p.stats(black_box(390.0)).line_count)
                .sum::<usize>()
        })
    });
    g.bench_function("lines_1k_paragraphs_w390", |b| {
        b.iter(|| {
            prepared
                .iter()
                .map(|p| p.lines(black_box(390.0)).len())
                .sum::<usize>()
        })
    });
    g.finish();

    // Hot path for one short chat message (~20 words).
    let short = "Sounds good — I'll push the fix for the layout cache tonight and ping you when CI is green 👍";
    let spans = [Span::new(0..short.len(), st.sans)];
    let p = prepare(&book, &mut warm, short, &spans, &PrepareOptions::default());
    let mut g = c.benchmark_group("short_paragraph");
    g.bench_function("line_count_w390", |b| b.iter(|| p.line_count(black_box(390.0))));
    g.bench_function("line_count_w120", |b| b.iter(|| p.line_count(black_box(120.0))));
    g.bench_function("prepare_warm", |b| {
        b.iter(|| {
            prepare(
                &book,
                &mut warm,
                black_box(short),
                &spans,
                &PrepareOptions::default(),
            )
        })
    });
    g.finish();
}

criterion_group!(layout_benches, benches);
criterion_main!(layout_benches);
