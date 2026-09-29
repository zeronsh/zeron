//! Font registry: faces, interned styles, vertical metrics, and the host fallback hook.

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rustc_hash::FxBuildHasher;
use rustybuzz::Feature;
use rustybuzz::ttf_parser::{self, Tag};

/// Every `FontBook` gets a process-unique id so a [`crate::WidthCache`] can detect being handed a
/// different book (StyleIds are only meaningful within one book) and drop its entries.
static NEXT_BOOK_ID: AtomicU64 = AtomicU64::new(1);

fn next_book_id() -> u64 {
    NEXT_BOOK_ID.fetch_add(1, Ordering::Relaxed)
}

/// Index of a face registered with [`FontBook::add_face`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FaceId(pub u16);

/// Index of an interned style (face + size + options) from [`FontBook::add_style`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StyleId(pub u16);

/// Per-style shaping options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StyleOptions {
    /// Extra advance (points) added after every grapheme cluster, including spaces and the last
    /// grapheme of a run — the NSAttributedString `.kern` model.
    pub letter_spacing: f32,
    /// Standard ligatures (`liga`, `clig`). On by default like CoreText; turn off for code.
    /// Kerning is always on.
    pub ligatures: bool,
}

impl Default for StyleOptions {
    fn default() -> Self {
        Self {
            letter_spacing: 0.0,
            ligatures: true,
        }
    }
}

/// A resolved style: which face, at what point size, with which options.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Face the style shapes with.
    pub face: FaceId,
    /// Point size.
    pub size: f32,
    /// Letter spacing / ligature options.
    pub options: StyleOptions,
}

/// Vertical metrics of a style, in points. `descent` is positive (distance below the baseline).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FontMetrics {
    /// Distance from the baseline to the top of the line box.
    pub ascent: f32,
    /// Distance from the baseline to the bottom of the line box (positive).
    pub descent: f32,
    /// Extra leading the font recommends between lines.
    pub line_gap: f32,
    /// Height of capital letters (0 when the font does not say).
    pub cap_height: f32,
    /// Height of lowercase `x` (0 when the font does not say).
    pub x_height: f32,
}

impl FontMetrics {
    /// `ascent + descent + line_gap` — the font's natural line height.
    pub fn line_height(&self) -> f32 {
        self.ascent + self.descent + self.line_gap
    }
}

/// Host-supplied measurement for text the style's face can't render (emoji, CJK, Arabic, ...).
///
/// Implementations measure with the platform text engine (e.g. CoreText with its font cascade,
/// using the host's font for `style`) and return the natural advance width in points **without**
/// letter spacing — the crate adds `letter_spacing` per grapheme itself, uniformly with shaped
/// text. Results are cached per `(style, text)` in the [`crate::WidthCache`], so the measurer is
/// called at most once per distinct piece per cache.
pub trait FallbackMeasurer: Send + Sync {
    /// Advance width of `text` set in `style`, in points.
    fn measure(&self, style: StyleId, text: &str) -> f32;

    /// Optional (recommended): lays `text` — a maximal run of characters the style's face can't
    /// draw, e.g. a whole CJK sentence — out as one line in `style` and appends one advance per
    /// `char` of `text` (in `chars()` order) to `advances`: each glyph's advance, in points and
    /// without letter spacing, attributed to the character it came from (a multi-char cluster
    /// on its first char, zero on the rest). Returns `false` (the default) when unsupported.
    ///
    /// Platform engines pick fallback fonts and kern per run (CoreText sets `テキスト` in
    /// Hiragino with kana kerning, and a `。` between kana and Han in either Hiragino or
    /// PingFang depending on its neighbors), so widths measured piece by piece drift from what
    /// the engine draws. With run advances, every piece inside the run gets its in-context
    /// width. With CoreText: build a `CTLine` from the run and read each `CTRun`'s
    /// `CTRunGetAdvances` + `CTRunGetStringIndices` (UTF-16 indices; map them to chars).
    fn measure_run(&self, style: StyleId, text: &str, advances: &mut Vec<f32>) -> bool {
        let _ = (style, text, advances);
        false
    }
}

/// Errors from [`FontBook::add_face`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FontError {
    /// The data is not a parseable OpenType/TrueType font.
    Parse,
    /// More than `u16::MAX` faces.
    TooManyFaces,
}

impl fmt::Display for FontError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FontError::Parse => f.write_str("font data could not be parsed"),
            FontError::TooManyFaces => f.write_str("too many faces registered"),
        }
    }
}

impl std::error::Error for FontError {}

pub(crate) struct FaceData {
    pub(crate) hb: rustybuzz::Face<'static>,
    pub(crate) upem: f32,
    /// Bit `c` set when printable ASCII char `c` (0x20..0x7F) maps to a glyph. Lets the cold
    /// coverage check skip the cmap lookup for the overwhelmingly common case.
    pub(crate) ascii: u128,
    /// Coverage of the whole BMP as a bitmap (8 KiB), built from the cmap's own code point
    /// lists, so coverage checks never walk cmap subtables outside the astral planes.
    pub(crate) bmp: Box<[u64; 1024]>,
}

impl FaceData {
    /// Whether the face's cmap has a glyph for `c`.
    #[inline]
    pub(crate) fn covers(&self, c: char) -> bool {
        let u = c as u32;
        if (0x20..0x80).contains(&u) {
            return self.ascii & (1u128 << u) != 0;
        }
        if u < 0x10000 {
            return self.bmp[u as usize / 64] & (1u64 << (u % 64)) != 0;
        }
        self.hb.glyph_index(c).is_some()
    }
}

pub(crate) struct StyleData {
    pub(crate) style: Style,
    /// Points per font unit.
    pub(crate) scale: f32,
    pub(crate) features: Box<[Feature]>,
}

type StyleKey = (u16, u32, u32, bool);

/// Registry of faces and interned styles. Build it once (add faces, styles, the fallback), then
/// share it immutably (e.g. in an `Arc`) — it is `Send + Sync`.
pub struct FontBook {
    id: u64,
    faces: Vec<FaceData>,
    styles: Vec<StyleData>,
    interned: HashMap<StyleKey, StyleId, FxBuildHasher>,
    fallback: Option<Arc<dyn FallbackMeasurer>>,
}

impl Default for FontBook {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for FontBook {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FontBook")
            .field("faces", &self.faces.len())
            .field("styles", &self.styles.len())
            .field("fallback", &self.fallback.is_some())
            .finish()
    }
}

impl FontBook {
    /// An empty book.
    pub fn new() -> Self {
        Self {
            id: next_book_id(),
            faces: Vec::new(),
            styles: Vec::new(),
            interned: HashMap::default(),
            fallback: None,
        }
    }

    /// Registers a font file (face index 0 of a collection). The bytes are leaked to `'static`;
    /// faces are expected to live for the process.
    pub fn add_face(&mut self, data: Vec<u8>) -> Result<FaceId, FontError> {
        if self.faces.len() >= u16::MAX as usize {
            return Err(FontError::TooManyFaces);
        }
        // Validate before leaking so a bad file doesn't leak.
        ttf_parser::Face::parse(&data, 0).map_err(|_| FontError::Parse)?;
        let data: &'static [u8] = Box::leak(data.into_boxed_slice());
        let hb = rustybuzz::Face::from_slice(data, 0).ok_or(FontError::Parse)?;
        let upem = hb.units_per_em() as f32;
        let mut ascii = 0u128;
        for u in 0x20u32..0x7F {
            if let Some(c) = char::from_u32(u)
                && hb.glyph_index(c).is_some()
            {
                ascii |= 1u128 << u;
            }
        }
        let mut bmp = Box::new([0u64; 1024]);
        if let Some(cmap) = hb.tables().cmap {
            for sub in cmap.subtables {
                if !sub.is_unicode() {
                    continue;
                }
                sub.codepoints(|u| {
                    if u < 0x10000
                        && let Some(c) = char::from_u32(u)
                        && hb.glyph_index(c).is_some()
                    {
                        bmp[u as usize / 64] |= 1u64 << (u % 64);
                    }
                });
            }
        }
        // Format 4's end sentinel isn't enumerated but may still resolve.
        if hb.glyph_index('\u{FFFF}').is_some() {
            bmp[1023] |= 1 << 63;
        }
        let id = FaceId(self.faces.len() as u16);
        self.faces.push(FaceData {
            hb,
            upem,
            ascii,
            bmp,
        });
        Ok(id)
    }

    /// The face's PostScript name (name ID 6), for the host to resolve the same font natively.
    pub fn face_postscript_name(&self, face: FaceId) -> Option<String> {
        let face = &self.faces.get(face.0 as usize)?.hb;
        face.names().into_iter().find_map(|name| {
            (name.name_id == ttf_parser::name_id::POST_SCRIPT_NAME)
                .then(|| name.to_string())
                .flatten()
        })
    }

    /// Interns a style. The same `(face, size, opts)` always returns the same id.
    ///
    /// # Panics
    /// If `face` was not returned by this book, or more than `u16::MAX` styles are created.
    pub fn add_style(&mut self, face: FaceId, size: f32, opts: StyleOptions) -> StyleId {
        let face_data = self
            .faces
            .get(face.0 as usize)
            .expect("add_style: unknown FaceId");
        let key = (
            face.0,
            size.to_bits(),
            opts.letter_spacing.to_bits(),
            opts.ligatures,
        );
        if let Some(&id) = self.interned.get(&key) {
            return id;
        }
        assert!(self.styles.len() < u16::MAX as usize, "too many styles");
        let features: Box<[Feature]> = if opts.ligatures {
            Box::new([])
        } else {
            Box::new([
                Feature::new(Tag::from_bytes(b"liga"), 0, ..),
                Feature::new(Tag::from_bytes(b"clig"), 0, ..),
            ])
        };
        let id = StyleId(self.styles.len() as u16);
        self.styles.push(StyleData {
            style: Style {
                face,
                size,
                options: opts,
            },
            scale: size / face_data.upem,
            features,
        });
        self.interned.insert(key, id);
        id
    }

    /// The style behind `id`.
    ///
    /// # Panics
    /// If `id` was not returned by this book.
    pub fn style(&self, id: StyleId) -> &Style {
        &self.style_data(id).style
    }

    /// Number of interned styles.
    pub fn style_count(&self) -> usize {
        self.styles.len()
    }

    /// Vertical metrics for `id`, in points.
    pub fn metrics(&self, id: StyleId) -> FontMetrics {
        let sd = self.style_data(id);
        let face = &self.face_data(sd.style.face).hb;
        let s = sd.scale;
        FontMetrics {
            ascent: face.ascender() as f32 * s,
            descent: -(face.descender() as f32) * s,
            line_gap: face.line_gap() as f32 * s,
            cap_height: face.capital_height().unwrap_or(0) as f32 * s,
            x_height: face.x_height().unwrap_or(0) as f32 * s,
        }
    }

    /// Installs the measurer for text the style's face can't cover. Without one, uncovered text
    /// is shaped anyway and measures with the face's `.notdef` advance.
    pub fn set_fallback(&mut self, m: Arc<dyn FallbackMeasurer>) {
        self.fallback = Some(m);
        // Cached widths may have come from `.notdef` shaping; force caches to start over.
        self.id = next_book_id();
    }

    pub(crate) fn id(&self) -> u64 {
        self.id
    }

    #[inline]
    pub(crate) fn style_data(&self, id: StyleId) -> &StyleData {
        self.styles
            .get(id.0 as usize)
            .expect("StyleId not from this FontBook")
    }

    #[inline]
    pub(crate) fn face_data(&self, id: FaceId) -> &FaceData {
        &self.faces[id.0 as usize]
    }

    #[inline]
    pub(crate) fn fallback(&self) -> Option<&dyn FallbackMeasurer> {
        self.fallback.as_deref()
    }
}
