//! Shared fixture: the app's Geist faces (read from crates/ui) plus a deterministic fallback.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use unicode_segmentation::UnicodeSegmentation;
use zeron_text::*;

pub fn font_bytes(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ui/assets/fonts")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// Deterministic stand-in for the platform engine: 1em per wide grapheme (emoji, CJK), 0.55em
/// otherwise. Records every call.
pub struct TestFallback {
    pub sizes: Vec<f32>,
    pub calls: AtomicUsize,
    pub log: Mutex<Vec<String>>,
}

pub fn is_wide(g: &str) -> bool {
    g.chars().any(|c| {
        let u = c as u32;
        (0x1F000..=0x1FAFF).contains(&u)
            || (0x2600..=0x27BF).contains(&u)
            || (0x2E80..=0x9FFF).contains(&u)
            || (0xAC00..=0xD7AF).contains(&u)
            || (0xF900..=0xFAFF).contains(&u)
            || (0xFF00..=0xFFEF).contains(&u)
            || (0x20000..=0x3FFFF).contains(&u)
            || c == '\u{FE0F}'
            || c == '\u{20E3}'
    })
}

impl FallbackMeasurer for TestFallback {
    fn measure(&self, style: StyleId, text: &str) -> f32 {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.log.lock().unwrap().push(text.to_owned());
        let size = self.sizes[style.0 as usize];
        text.graphemes(true)
            .map(|g| if is_wide(g) { size } else { size * 0.55 })
            .sum()
    }
}

pub struct Fixture {
    pub book: FontBook,
    pub cache: WidthCache,
    pub sans: StyleId,
    pub bold: StyleId,
    pub mono: StyleId,
    pub fallback: Option<Arc<TestFallback>>,
}

impl Fixture {
    pub fn new() -> Self {
        Self::build(true)
    }

    pub fn without_fallback() -> Self {
        Self::build(false)
    }

    fn build(with_fallback: bool) -> Self {
        let mut book = FontBook::new();
        let geist = book.add_face(font_bytes("Geist.ttf")).unwrap();
        let geist_bold = book.add_face(font_bytes("Geist-Bold.ttf")).unwrap();
        let mono = book.add_face(font_bytes("GeistMono.ttf")).unwrap();
        let sans = book.add_style(geist, 15.0, StyleOptions::default());
        let bold = book.add_style(geist_bold, 15.0, StyleOptions::default());
        let mono = book.add_style(
            mono,
            13.0,
            StyleOptions {
                ligatures: false,
                ..Default::default()
            },
        );
        let fallback = with_fallback.then(|| {
            let fb = Arc::new(TestFallback {
                sizes: (0..book.style_count())
                    .map(|i| book.style(StyleId(i as u16)).size)
                    .collect(),
                calls: AtomicUsize::new(0),
                log: Mutex::new(Vec::new()),
            });
            book.set_fallback(fb.clone());
            fb
        });
        Self {
            book,
            cache: WidthCache::new(),
            sans,
            bold,
            mono,
            fallback,
        }
    }

    pub fn fallback_calls(&self) -> usize {
        self.fallback
            .as_ref()
            .map_or(0, |f| f.calls.load(Ordering::Relaxed))
    }

    /// Adds a style to the book, keeping the fallback's size table in sync.
    pub fn style(&mut self, face: FaceId, size: f32, opts: StyleOptions) -> StyleId {
        let id = self.book.add_style(face, size, opts);
        if let Some(fb) = &self.fallback {
            // Rebuild the fallback with the new size table.
            let sizes: Vec<f32> = (0..self.book.style_count())
                .map(|i| self.book.style(StyleId(i as u16)).size)
                .collect();
            let new = Arc::new(TestFallback {
                sizes,
                calls: AtomicUsize::new(fb.calls.load(Ordering::Relaxed)),
                log: Mutex::new(Vec::new()),
            });
            self.book.set_fallback(new.clone());
            self.fallback = Some(new);
        }
        id
    }

    pub fn prep(&mut self, text: &str) -> Prepared {
        self.prep_with(text, PrepareOptions::default())
    }

    pub fn prep_with(&mut self, text: &str, opts: PrepareOptions) -> Prepared {
        let spans = [Span::new(0..text.len(), self.sans)];
        prepare(&self.book, &mut self.cache, text, &spans, &opts)
    }

    pub fn prep_spans(&mut self, text: &str, spans: &[Span], opts: PrepareOptions) -> Prepared {
        prepare(&self.book, &mut self.cache, text, spans, &opts)
    }

    /// Width of `text` as one prepared unbreakable line in `style`.
    pub fn width(&mut self, text: &str, style: StyleId) -> f32 {
        let spans = [Span::new(0..text.len(), style)];
        let opts = PrepareOptions {
            white_space: WhiteSpace::Pre,
            ..Default::default()
        };
        let p = prepare(&self.book, &mut self.cache, text, &spans, &opts);
        p.max_content_width()
    }
}

pub fn opts(ws: WhiteSpace) -> PrepareOptions {
    PrepareOptions {
        white_space: ws,
        ..Default::default()
    }
}

pub fn line_texts(p: &Prepared, w: f32) -> Vec<String> {
    p.lines(w)
        .iter()
        .map(|l| p.text()[l.range.clone()].to_owned())
        .collect()
}

pub fn assert_close(a: f32, b: f32, what: &str) {
    assert!((a - b).abs() < 1e-3, "{what}: {a} vs {b}");
}
