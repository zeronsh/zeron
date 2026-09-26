//! Typography: bundled faces, the transcript's type scale, and the resolved
//! `StyleId`s layout uses. Numbers drive layout; colors are paint.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use zeron_text::{FaceId, FontBook, FontMetrics, StyleId, StyleOptions};

/// Faces the platform registers (same bytes it hands CoreText/Skia).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum FaceRole {
    Sans,
    SansMedium,
    SansSemibold,
    SansBold,
    SansItalic,
    SansMediumItalic,
    SansSemiboldItalic,
    SansBoldItalic,
    Mono,
    MonoMedium,
    MonoSemibold,
    MonoItalic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Family {
    Sans,
    Mono,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[allow(dead_code)]
pub(crate) enum Weight {
    Regular,
    Medium,
    Semibold,
    Bold,
}

impl FaceRole {
    fn of(family: Family, weight: Weight, italic: bool) -> Self {
        use FaceRole::*;
        match (family, weight, italic) {
            (Family::Sans, Weight::Regular, false) => Sans,
            (Family::Sans, Weight::Medium, false) => SansMedium,
            (Family::Sans, Weight::Semibold, false) => SansSemibold,
            (Family::Sans, Weight::Bold, false) => SansBold,
            (Family::Sans, Weight::Regular, true) => SansItalic,
            (Family::Sans, Weight::Medium, true) => SansMediumItalic,
            (Family::Sans, Weight::Semibold, true) => SansSemiboldItalic,
            (Family::Sans, Weight::Bold, true) => SansBoldItalic,
            (Family::Mono, Weight::Regular, false) => Mono,
            (Family::Mono, Weight::Medium, false) => MonoMedium,
            (Family::Mono, Weight::Semibold | Weight::Bold, false) => MonoSemibold,
            (Family::Mono, _, true) => MonoItalic,
        }
    }

    /// Nearest registered substitute when a face is missing.
    fn fallbacks(self) -> &'static [FaceRole] {
        use FaceRole::*;
        match self {
            Sans => &[],
            SansMedium | SansSemibold | SansBold | SansItalic => &[Sans],
            SansMediumItalic => &[SansItalic, SansMedium, Sans],
            SansSemiboldItalic => &[SansBoldItalic, SansSemibold, SansItalic, Sans],
            SansBoldItalic => &[SansSemiboldItalic, SansBold, SansItalic, Sans],
            Mono => &[Sans],
            MonoMedium | MonoSemibold | MonoItalic => &[Mono, Sans],
        }
    }
}

/// Inverse of `FaceRole::of` (for harnesses that name a face directly).
pub(crate) fn decompose(face: FaceRole) -> (Family, Weight, bool) {
    use FaceRole::*;
    match face {
        Sans => (Family::Sans, Weight::Regular, false),
        SansMedium => (Family::Sans, Weight::Medium, false),
        SansSemibold => (Family::Sans, Weight::Semibold, false),
        SansBold => (Family::Sans, Weight::Bold, false),
        SansItalic => (Family::Sans, Weight::Regular, true),
        SansMediumItalic => (Family::Sans, Weight::Medium, true),
        SansSemiboldItalic => (Family::Sans, Weight::Semibold, true),
        SansBoldItalic => (Family::Sans, Weight::Bold, true),
        Mono => (Family::Mono, Weight::Regular, false),
        MonoMedium => (Family::Mono, Weight::Medium, false),
        MonoSemibold => (Family::Mono, Weight::Semibold, false),
        MonoItalic => (Family::Mono, Weight::Regular, true),
    }
}

/// Transcript rhythm in points at text scale 1.0.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TypeScale {
    pub body: (f32, f32),
    pub h: [(f32, f32); 4],
    pub code: (f32, f32),
    pub inline_code: f32,
    pub small: (f32, f32),
}

pub(crate) const TYPE: TypeScale = TypeScale {
    body: (16.5, 25.0),
    h: [(22.0, 29.0), (19.5, 27.0), (17.5, 25.0), (16.5, 25.0)],
    code: (13.5, 20.0),
    inline_code: 14.5,
    small: (13.5, 19.0),
};

/// One resolved style: its id plus the metrics layout needs for baselines.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Resolved {
    pub id: StyleId,
    pub ascent: f32,
    pub descent: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct StyleKey {
    pub family: Family,
    pub weight: Weight,
    pub italic: bool,
    /// Size in 1/100 pt (post-scale) so keys hash exactly.
    pub centi: u32,
}

/// A style the platform must be able to draw: `id` → (face, size).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StyleDesc {
    pub id: u16,
    pub face: FaceRole,
    pub size: f32,
    pub ligatures: bool,
}

/// Faces + interned styles. Owned by the layout thread; the platform gets a
/// copy of the style table whenever it grows.
pub(crate) struct Typography {
    pub book: FontBook,
    faces: HashMap<FaceRole, FaceId>,
    styles: HashMap<StyleKey, Resolved>,
    table: Vec<StyleDesc>,
    /// Shared with the fallback measurer bridge (id → font description).
    registry: Arc<Mutex<HashMap<u16, StyleDesc>>>,
    pub scale: f32,
}

impl Typography {
    pub fn new(
        faces: &[(FaceRole, Arc<Vec<u8>>)],
        fallback: Option<Arc<dyn zeron_text::FallbackMeasurer>>,
        registry: Arc<Mutex<HashMap<u16, StyleDesc>>>,
    ) -> Self {
        let mut book = FontBook::new();
        let mut ids = HashMap::new();
        for (role, bytes) in faces {
            if let Ok(id) = book.add_face(bytes.as_ref().clone()) {
                ids.insert(*role, id);
            }
        }
        if let Some(fallback) = fallback {
            book.set_fallback(fallback);
        }
        Self {
            book,
            faces: ids,
            styles: HashMap::new(),
            table: Vec::new(),
            registry,
            scale: 1.0,
        }
    }

    pub fn has_faces(&self) -> bool {
        self.faces.contains_key(&FaceRole::Sans)
    }

    fn face(&self, role: FaceRole) -> Option<(FaceRole, FaceId)> {
        std::iter::once(role)
            .chain(role.fallbacks().iter().copied())
            .find_map(|r| self.faces.get(&r).map(|id| (r, *id)))
    }

    /// Resolve (and intern) a style at `size` points before text scale.
    pub fn style(&mut self, family: Family, weight: Weight, italic: bool, size: f32) -> Resolved {
        let size = (size * self.scale * 100.0).round() / 100.0;
        let key = StyleKey {
            family,
            weight,
            italic,
            centi: (size * 100.0) as u32,
        };
        if let Some(r) = self.styles.get(&key) {
            return *r;
        }
        let (role, face) = self
            .face(FaceRole::of(family, weight, italic))
            .expect("the Sans face is registered before layout");
        // Code keeps ligatures off: `->`/`!=` must read as typed.
        let ligatures = family == Family::Sans;
        let id = self.book.add_style(
            face,
            size,
            StyleOptions {
                ligatures,
                ..StyleOptions::default()
            },
        );
        let FontMetrics {
            ascent, descent, ..
        } = self.book.metrics(id);
        let resolved = Resolved {
            id,
            ascent,
            descent,
        };
        self.styles.insert(key, resolved);
        if !self.table.iter().any(|d| d.id == id.0) {
            let desc = StyleDesc {
                id: id.0,
                face: role,
                size,
                ligatures,
            };
            self.registry.lock().unwrap().insert(id.0, desc.clone());
            self.table.push(desc);
        }
        resolved
    }

    pub fn table(&self) -> &[StyleDesc] {
        &self.table
    }

    /// Line-box scale for a size given at scale 1.0.
    pub fn px(&self, v: f32) -> f32 {
        v * self.scale
    }
}

/// Baseline offset inside a line box of height `lh` for a style (CSS
/// half-leading: the ascent+descent box is centered in the line box).
pub(crate) fn baseline(lh: f32, style: Resolved) -> f32 {
    (lh - (style.ascent + style.descent)) / 2.0 + style.ascent
}
