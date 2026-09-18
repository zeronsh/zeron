//! Device-local interface typography: bundled font registration, the persisted
//! catalog choice, and the effective family installed into [`crate::theme::Theme`].

use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
};

use gpui::{App, Global, Rems, SharedString, Window, px, rems};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::settings::{self, SavePolicy};

/// A bundled, virtual, or device-local interface font choice.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub enum UiFontFamily {
    #[default]
    Geist,
    GeistMono,
    System,
    Installed(String),
}

impl UiFontFamily {
    pub fn label(&self) -> &str {
        match self {
            Self::Geist => "Geist",
            Self::GeistMono => "Geist Mono",
            Self::System => "System UI",
            Self::Installed(name) => name,
        }
    }

    pub fn family_name(&self) -> &str {
        match self {
            Self::Geist => "Geist",
            Self::GeistMono => "Geist Mono",
            Self::System => ".SystemUIFont",
            Self::Installed(name) => name,
        }
    }
}

impl Serialize for UiFontFamily {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Geist => serializer.serialize_str("geist"),
            Self::GeistMono => serializer.serialize_str("geistMono"),
            Self::System => serializer.serialize_str("system"),
            Self::Installed(name) => serializer.serialize_str(&format!("installed:{name}")),
        }
    }
}

impl<'de> Deserialize<'de> for UiFontFamily {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(match value.as_str() {
            "geist" => Self::Geist,
            "geistMono" => Self::GeistMono,
            "system" => Self::System,
            // Preserve selections written by the previous fixed catalog. They
            // now resolve only when the family is installed on this device.
            "inter" => Self::Installed("Inter".into()),
            "atkinsonHyperlegibleNext" => Self::Installed("Atkinson Hyperlegible Next".into()),
            value if value.starts_with("installed:") && value.len() > "installed:".len() => {
                Self::Installed(value["installed:".len()..].to_owned())
            }
            _ => Self::Geist,
        })
    }
}

/// Base size used by rem-based interface text. Persisted as a number so future
/// builds can add choices without changing the settings format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UiFontSize(u8);

impl UiFontSize {
    pub const ALL: [Self; 7] = [
        Self(12),
        Self(13),
        Self(14),
        Self(15),
        Self(16),
        Self(18),
        Self(20),
    ];

    pub const fn pixels(self) -> f32 {
        self.0 as f32
    }

    pub fn label(self) -> SharedString {
        format!("{} px", self.0).into()
    }

    pub fn normalized(self) -> Self {
        Self::ALL
            .into_iter()
            .min_by_key(|candidate| candidate.0.abs_diff(self.0))
            .unwrap_or_default()
    }
}

impl Default for UiFontSize {
    fn default() -> Self {
        Self(16)
    }
}

/// Convert a size designed at the 16px baseline into a scalable interface rem.
/// Code, diffs, and terminal text deliberately keep absolute pixel sizes.
pub const fn ui_rems(pixels_at_default: f32) -> Rems {
    rems(pixels_at_default / 16.0)
}

pub const CODE_FONT_SIZE_DEFAULT: f32 = 12.5;
pub const TERMINAL_FONT_SIZE_DEFAULT: f32 = 13.0;
pub const FONT_SIZE_MIN: f32 = 8.0;
pub const FONT_SIZE_MAX: f32 = 32.0;

/// Clamp an absolute code/terminal pixel size into the supported range.
pub fn clamp_font_size(size: f32) -> f32 {
    size.clamp(FONT_SIZE_MIN, FONT_SIZE_MAX)
}

/// Which catalog families successfully registered during this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontAvailability {
    geist: bool,
    geist_mono: bool,
    choices: Vec<UiFontFamily>,
    fixed_width: Vec<UiFontFamily>,
}

impl FontAvailability {
    #[cfg(test)]
    pub fn all() -> Self {
        Self {
            geist: true,
            geist_mono: true,
            choices: vec![
                UiFontFamily::Geist,
                UiFontFamily::GeistMono,
                UiFontFamily::System,
                UiFontFamily::Installed("Arial".into()),
                UiFontFamily::Installed("Menlo".into()),
            ],
            fixed_width: vec![
                UiFontFamily::GeistMono,
                UiFontFamily::Installed("Menlo".into()),
            ],
        }
    }

    pub fn choices(&self) -> &[UiFontFamily] {
        &self.choices
    }

    /// Families whose glyph advances match the terminal renderer's cell grid.
    pub fn fixed_width_choices(&self) -> &[UiFontFamily] {
        &self.fixed_width
    }

    pub fn is_available(&self, family: &UiFontFamily) -> bool {
        match family {
            UiFontFamily::Geist => self.geist,
            UiFontFamily::GeistMono => self.geist_mono,
            UiFontFamily::System => true,
            UiFontFamily::Installed(_) => self.choices.contains(family),
        }
    }

    pub fn is_fixed_width_available(&self, family: &UiFontFamily) -> bool {
        self.is_available(family) && self.fixed_width.contains(family)
    }

    fn fallback(&self) -> UiFontFamily {
        if self.geist {
            UiFontFamily::Geist
        } else {
            UiFontFamily::System
        }
    }

    /// Code and terminal degrade to the bundled mono face instead of the
    /// interface sans: a device-local pick that later disappears should not
    /// silently turn fixed-width surfaces proportional.
    fn fallback_mono(&self) -> UiFontFamily {
        if self.geist_mono {
            UiFontFamily::GeistMono
        } else {
            self.fallback()
        }
    }

    #[cfg(test)]
    pub(crate) fn without(mut self, family: &UiFontFamily) -> Self {
        match family {
            UiFontFamily::Geist => self.geist = false,
            UiFontFamily::GeistMono => self.geist_mono = false,
            UiFontFamily::System => {}
            UiFontFamily::Installed(_) => self.choices.retain(|choice| choice != family),
        }
        self.fixed_width.retain(|choice| choice != family);
        self
    }
}

impl Default for FontAvailability {
    fn default() -> Self {
        Self {
            geist: false,
            geist_mono: false,
            choices: vec![UiFontFamily::System],
            fixed_width: Vec::new(),
        }
    }
}

/// Requested and effective typography for the process.
///
/// Interface and code/diff pick from the whole catalog — they lay text out
/// naturally, so proportional faces are legal. The terminal paints into a cell
/// grid sized by the `m` advance, so it picks from the fixed-width subset only.
pub struct TypographyState {
    pub requested: UiFontFamily,
    pub effective: UiFontFamily,
    pub size: UiFontSize,
    pub terminal_requested: UiFontFamily,
    pub terminal_effective: UiFontFamily,
    pub terminal_font_size: f32,
    pub code_requested: UiFontFamily,
    pub code_effective: UiFontFamily,
    pub code_font_size: f32,
    pub availability: FontAvailability,
    /// Monotonic signal for layout caches whose measurements depend on
    /// typography. Kept separate from the theme style generation so palette
    /// changes do not force expensive list remeasurement.
    generation: u32,
}

impl Global for TypographyState {}

const GEIST: [&[u8]; 8] = [
    include_bytes!("../assets/fonts/Geist.ttf"),
    include_bytes!("../assets/fonts/Geist-Italic.ttf"),
    include_bytes!("../assets/fonts/Geist-Medium.ttf"),
    include_bytes!("../assets/fonts/Geist-MediumItalic.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBold.ttf"),
    include_bytes!("../assets/fonts/Geist-SemiBoldItalic.ttf"),
    include_bytes!("../assets/fonts/Geist-Bold.ttf"),
    include_bytes!("../assets/fonts/Geist-BoldItalic.ttf"),
];

const GEIST_MONO: [&[u8]; 8] = [
    include_bytes!("../assets/fonts/GeistMono.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Italic.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Medium.ttf"),
    include_bytes!("../assets/fonts/GeistMono-MediumItalic.ttf"),
    include_bytes!("../assets/fonts/GeistMono-SemiBold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-SemiBoldItalic.ttf"),
    include_bytes!("../assets/fonts/GeistMono-Bold.ttf"),
    include_bytes!("../assets/fonts/GeistMono-BoldItalic.ttf"),
];

/// Font faces shared by the interface and SVG text-to-path conversion.
pub(crate) fn bundled_font_faces() -> impl Iterator<Item = &'static [u8]> {
    GEIST.iter().chain(GEIST_MONO.iter()).copied()
}

fn register_family(cx: &App, family: &UiFontFamily, faces: &'static [&'static [u8]]) -> bool {
    let fonts = faces.iter().map(|face| Cow::Borrowed(*face)).collect();
    match cx.text_system().add_fonts(fonts) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(font_family = family.label(), error = %err, "failed to register bundled font family");
            false
        }
    }
}

/// Relative spread the sampled advances may show and still count as fixed
/// width. Hinting and rounding leave sub-unit differences in real mono faces;
/// proportional faces differ by whole multiples, never a percent.
const FIXED_WIDTH_TOLERANCE: f32 = 0.01;

/// Whether a face satisfies the terminal renderer's assumption that every
/// Latin cell is one `m` wide.
///
/// The PANOSE bit and `fontdb`'s `monospaced` flag are metadata a font can set
/// wrongly; what the grid actually needs is equal advances, so measure them.
fn face_is_fixed_width(font: &ttf_parser::Face) -> bool {
    let Some(advances) = ['i', 'm', 'W', '0']
        .iter()
        .map(|ch| {
            font.glyph_index(*ch)
                .and_then(|id| font.glyph_hor_advance(id))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return false;
    };
    let (min, max) = (
        advances.iter().copied().min().unwrap_or(0),
        advances.iter().copied().max().unwrap_or(0),
    );
    min > 0 && f32::from(max - min) <= FIXED_WIDTH_TOLERANCE * f32::from(min)
}

/// Families that contain the Latin glyph GPUI relies on for text metrics,
/// mapped to whether every one of their faces is fixed width.
///
/// `all_font_names` intentionally reports every system family, including
/// script-specific and symbol fonts. GPUI must reject a face without `m`, but
/// doing that only after it has been selected both falls back silently and
/// emits a warning. Inspect the installed files first so those families never
/// appear in an interface-font picker.
fn installed_families_with_latin_metrics() -> BTreeMap<String, bool> {
    let mut database = fontdb::Database::new();
    database.load_system_fonts();

    let mut families = BTreeMap::new();
    for face in database.faces() {
        let Some(Some(fixed_width)) = database.with_face_data(face.id, |data, index| {
            ttf_parser::Face::parse(data, index)
                .ok()
                .filter(|font| font.glyph_index('m').is_some())
                .map(|font| face_is_fixed_width(&font))
        }) else {
            continue;
        };
        for (name, _) in &face.families {
            // Selecting a family selects all its faces, so one proportional
            // italic is enough to break the grid.
            families
                .entry(name.clone())
                .and_modify(|all_fixed| *all_fixed &= fixed_width)
                .or_insert(fixed_width);
        }
    }
    families
}

/// Register each family independently so one bad optional asset cannot hide
/// the rest of the catalog.
pub fn register_fonts(cx: &App) -> FontAvailability {
    // Capture device fonts before adding ours so the Installed section only
    // contains families supplied by the OS/user, not our embedded assets.
    let families_with_latin_metrics = installed_families_with_latin_metrics();
    let system_names: BTreeSet<_> = cx
        .text_system()
        .all_font_names()
        .into_iter()
        .filter(|name| !name.starts_with('.'))
        .filter(|name| name != "Geist" && name != "Geist Mono")
        .filter(|name| families_with_latin_metrics.contains_key(name))
        .collect();
    let geist = register_family(cx, &UiFontFamily::Geist, &GEIST);
    let geist_mono = register_family(cx, &UiFontFamily::GeistMono, &GEIST_MONO);
    let mut choices = vec![
        UiFontFamily::Geist,
        UiFontFamily::GeistMono,
        UiFontFamily::System,
    ];
    let mut fixed_width = vec![UiFontFamily::GeistMono];
    for name in system_names {
        if families_with_latin_metrics.get(&name) == Some(&true) {
            fixed_width.push(UiFontFamily::Installed(name.clone()));
        }
        choices.push(UiFontFamily::Installed(name));
    }
    FontAvailability {
        geist,
        geist_mono,
        choices,
        fixed_width,
    }
}

fn resolve_effective(requested: &UiFontFamily, availability: &FontAvailability) -> UiFontFamily {
    if availability.is_available(requested) {
        requested.clone()
    } else {
        availability.fallback()
    }
}

/// `fixed_width_only` guards the terminal grid: a settings file may already
/// name a proportional family, which must fall back rather than render cells
/// the painted glyphs do not fill.
fn resolve_effective_mono(
    requested: &UiFontFamily,
    availability: &FontAvailability,
    fixed_width_only: bool,
) -> UiFontFamily {
    let usable = if fixed_width_only {
        availability.is_fixed_width_available(requested)
    } else {
        availability.is_available(requested)
    };
    if usable {
        requested.clone()
    } else {
        availability.fallback_mono()
    }
}

/// Install typography state before appearance builds the first [`crate::theme::Theme`].
#[allow(clippy::too_many_arguments)]
pub fn init(
    requested: UiFontFamily,
    size: UiFontSize,
    terminal_requested: UiFontFamily,
    terminal_font_size: f32,
    code_requested: UiFontFamily,
    code_font_size: f32,
    availability: FontAvailability,
    cx: &mut App,
) {
    let effective = resolve_effective(&requested, &availability);
    let terminal_effective = resolve_effective_mono(&terminal_requested, &availability, true);
    let code_effective = resolve_effective_mono(&code_requested, &availability, false);
    cx.set_global(TypographyState {
        requested,
        effective,
        size: size.normalized(),
        terminal_requested,
        terminal_effective,
        terminal_font_size: clamp_font_size(terminal_font_size),
        code_requested,
        code_effective,
        code_font_size: clamp_font_size(code_font_size),
        availability,
        generation: 0,
    });
}

pub fn requested(cx: &App) -> UiFontFamily {
    cx.try_global::<TypographyState>()
        .map(|state| state.requested.clone())
        .unwrap_or_default()
}

pub fn effective(cx: &App) -> UiFontFamily {
    cx.try_global::<TypographyState>()
        .map(|state| state.effective.clone())
        .unwrap_or_default()
}

pub fn effective_family_name(cx: &App) -> SharedString {
    effective(cx).family_name().into()
}

pub fn terminal_requested(cx: &App) -> UiFontFamily {
    cx.try_global::<TypographyState>()
        .map(|state| state.terminal_requested.clone())
        .unwrap_or(UiFontFamily::GeistMono)
}

pub fn terminal_effective(cx: &App) -> UiFontFamily {
    cx.try_global::<TypographyState>()
        .map(|state| state.terminal_effective.clone())
        .unwrap_or(UiFontFamily::GeistMono)
}

pub fn terminal_effective_family_name(cx: &App) -> SharedString {
    terminal_effective(cx).family_name().into()
}

pub fn terminal_font_size(cx: &App) -> f32 {
    cx.try_global::<TypographyState>()
        .map(|state| state.terminal_font_size)
        .unwrap_or(TERMINAL_FONT_SIZE_DEFAULT)
}

pub fn code_requested(cx: &App) -> UiFontFamily {
    cx.try_global::<TypographyState>()
        .map(|state| state.code_requested.clone())
        .unwrap_or(UiFontFamily::GeistMono)
}

pub fn code_effective(cx: &App) -> UiFontFamily {
    cx.try_global::<TypographyState>()
        .map(|state| state.code_effective.clone())
        .unwrap_or(UiFontFamily::GeistMono)
}

pub fn code_effective_family_name(cx: &App) -> SharedString {
    code_effective(cx).family_name().into()
}

pub fn code_font_size(cx: &App) -> f32 {
    cx.try_global::<TypographyState>()
        .map(|state| state.code_font_size)
        .unwrap_or(CODE_FONT_SIZE_DEFAULT)
}

pub fn font_size(cx: &App) -> UiFontSize {
    cx.try_global::<TypographyState>()
        .map(|state| state.size)
        .unwrap_or_default()
}

pub fn availability(cx: &App) -> FontAvailability {
    cx.try_global::<TypographyState>()
        .map(|state| state.availability.clone())
        .unwrap_or_default()
}

/// Monotonic id of the current effective UI typography (family + size).
/// Long-lived layout caches compare this value to invalidate measurements
/// that can change when prose wraps differently.
pub fn generation(cx: &App) -> u32 {
    cx.try_global::<TypographyState>()
        .map(|state| state.generation)
        .unwrap_or_default()
}

/// Validate, apply, repaint, and persist one confirmed choice. Returns whether
/// the effective family changed (re-selecting the current family is a no-op).
pub fn set_family(family: UiFontFamily, cx: &mut App) -> bool {
    let Some(state) = cx.try_global::<TypographyState>() else {
        return false;
    };
    if !state.availability.is_available(&family) {
        return false;
    }
    let effective = resolve_effective(&family, &state.availability);
    if state.requested == family && state.effective == effective {
        return false;
    }

    let effective_changed = state.effective != effective;
    let state = cx.global_mut::<TypographyState>();
    state.requested = family.clone();
    state.effective = effective;

    if effective_changed {
        state.generation = state.generation.wrapping_add(1);
        reinstall_theme(cx);
    }
    settings::update(SavePolicy::Immediate, cx, |settings| {
        settings.ui_font_family = family;
    });
    effective_changed
}

/// Rebuild the [`crate::theme::Theme`] so it picks up the new families and
/// sizes it carries, then repaint.
fn reinstall_theme(cx: &mut App) {
    crate::theme::bump_style_generation();
    let appearance = crate::theme::current_appearance();
    let themes = crate::appearance::themes(cx);
    crate::theme::Theme::install_selection(
        appearance,
        themes.variant_id(match appearance {
            crate::theme::Appearance::Dark => zeron_theme::Appearance::Dark,
            crate::theme::Appearance::Light => zeron_theme::Appearance::Light,
        }),
        crate::appearance::accent(cx),
        crate::appearance::surface(cx),
        cx,
    );
    cx.refresh_windows();
}

/// Apply and persist the terminal family. Returns whether anything changed.
pub fn set_terminal_family(family: UiFontFamily, cx: &mut App) -> bool {
    set_category_family(family, Category::Terminal, cx)
}

/// Apply and persist the code/diff family. Returns whether anything changed.
pub fn set_code_family(family: UiFontFamily, cx: &mut App) -> bool {
    set_category_family(family, Category::Code, cx)
}

#[derive(Clone, Copy)]
enum Category {
    Terminal,
    Code,
}

impl Category {
    fn fixed_width_only(self) -> bool {
        matches!(self, Self::Terminal)
    }
}

fn set_category_family(family: UiFontFamily, category: Category, cx: &mut App) -> bool {
    let Some(state) = cx.try_global::<TypographyState>() else {
        return false;
    };
    let fixed_width_only = category.fixed_width_only();
    let selectable = if fixed_width_only {
        state.availability.is_fixed_width_available(&family)
    } else {
        state.availability.is_available(&family)
    };
    if !selectable {
        return false;
    }
    let effective = resolve_effective_mono(&family, &state.availability, fixed_width_only);
    let (current_requested, current_effective) = match category {
        Category::Terminal => (&state.terminal_requested, &state.terminal_effective),
        Category::Code => (&state.code_requested, &state.code_effective),
    };
    if *current_requested == family && *current_effective == effective {
        return false;
    }

    let state = cx.global_mut::<TypographyState>();
    state.generation = state.generation.wrapping_add(1);
    match category {
        Category::Terminal => {
            state.terminal_requested = family.clone();
            state.terminal_effective = effective;
        }
        Category::Code => {
            state.code_requested = family.clone();
            state.code_effective = effective;
        }
    }
    reinstall_theme(cx);
    settings::update(SavePolicy::Immediate, cx, |settings| match category {
        Category::Terminal => settings.terminal_font_family = family,
        Category::Code => settings.code_font_family = family,
    });
    true
}

/// Apply and persist the terminal pixel size. Returns whether it changed.
pub fn set_terminal_font_size(size: f32, cx: &mut App) -> bool {
    set_category_font_size(size, Category::Terminal, cx)
}

/// Apply and persist the code/diff pixel size. Returns whether it changed.
pub fn set_code_font_size(size: f32, cx: &mut App) -> bool {
    set_category_font_size(size, Category::Code, cx)
}

fn set_category_font_size(size: f32, category: Category, cx: &mut App) -> bool {
    let size = clamp_font_size(size);
    let Some(state) = cx.try_global::<TypographyState>() else {
        return false;
    };
    let current = match category {
        Category::Terminal => state.terminal_font_size,
        Category::Code => state.code_font_size,
    };
    if current == size {
        return false;
    }

    let state = cx.global_mut::<TypographyState>();
    state.generation = state.generation.wrapping_add(1);
    match category {
        Category::Terminal => state.terminal_font_size = size,
        Category::Code => state.code_font_size = size,
    }
    reinstall_theme(cx);
    settings::update(SavePolicy::Immediate, cx, |settings| match category {
        Category::Terminal => settings.terminal_font_size = size,
        Category::Code => settings.code_font_size = size,
    });
    true
}

/// Apply a supported UI size to the current window and persist it.
pub fn set_font_size(size: UiFontSize, window: &mut Window, cx: &mut App) -> bool {
    let size = size.normalized();
    let Some(state) = cx.try_global::<TypographyState>() else {
        return false;
    };
    if state.size == size {
        return false;
    }

    let state = cx.global_mut::<TypographyState>();
    state.size = size;
    state.generation = state.generation.wrapping_add(1);
    window.set_rem_size(px(size.pixels()));
    crate::theme::bump_style_generation();
    cx.refresh_windows();
    settings::update(SavePolicy::Immediate, cx, |settings| {
        settings.ui_font_size = size;
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values_round_trip_and_unknown_falls_back() {
        for family in [
            UiFontFamily::Geist,
            UiFontFamily::GeistMono,
            UiFontFamily::System,
            UiFontFamily::Installed("Helvetica Neue".into()),
        ] {
            let json = serde_json::to_string(&family).unwrap();
            assert_eq!(serde_json::from_str::<UiFontFamily>(&json).unwrap(), family);
        }
        assert_eq!(
            serde_json::from_str::<UiFontFamily>(r#""futureFont""#).unwrap(),
            UiFontFamily::Geist
        );
    }

    #[test]
    fn unavailable_family_resolves_to_geist() {
        let unavailable = UiFontFamily::Installed("Inter".into());
        let availability = FontAvailability::all().without(&unavailable);
        assert_eq!(
            resolve_effective(&unavailable, &availability),
            UiFontFamily::Geist
        );
        assert!(availability.is_available(&UiFontFamily::System));
    }

    #[test]
    fn unavailable_code_and_terminal_families_stay_monospaced() {
        let unavailable = UiFontFamily::Installed("MesloLGS NF".into());
        let availability = FontAvailability::all();
        assert_eq!(
            resolve_effective_mono(&unavailable, &availability, false),
            UiFontFamily::GeistMono
        );
        assert_eq!(
            resolve_effective_mono(
                &unavailable,
                &availability.clone().without(&unavailable),
                true
            ),
            UiFontFamily::GeistMono
        );
        // Only when the bundled mono face itself failed to register does a
        // fixed-width surface fall back to the interface family.
        let without_mono = availability.without(&UiFontFamily::GeistMono);
        assert_eq!(
            resolve_effective_mono(&unavailable, &without_mono, false),
            UiFontFamily::Geist
        );
    }

    #[test]
    fn advance_check_separates_bundled_mono_from_bundled_sans() {
        for bytes in GEIST_MONO {
            assert!(face_is_fixed_width(
                &ttf_parser::Face::parse(bytes, 0).unwrap()
            ));
        }
        for bytes in GEIST {
            assert!(!face_is_fixed_width(
                &ttf_parser::Face::parse(bytes, 0).unwrap()
            ));
        }
    }

    #[test]
    fn terminal_catalog_drops_proportional_families() {
        let availability = FontAvailability::all();
        for proportional in [
            UiFontFamily::Geist,
            UiFontFamily::System,
            UiFontFamily::Installed("Arial".into()),
        ] {
            assert!(availability.is_available(&proportional));
            assert!(!availability.is_fixed_width_available(&proportional));
            assert!(!availability.fixed_width_choices().contains(&proportional));
        }
        for fixed in [
            UiFontFamily::GeistMono,
            UiFontFamily::Installed("Menlo".into()),
        ] {
            assert!(availability.is_fixed_width_available(&fixed));
            assert!(availability.fixed_width_choices().contains(&fixed));
        }
    }

    #[test]
    fn persisted_proportional_terminal_family_falls_back() {
        let availability = FontAvailability::all();
        // Written by a build before the terminal picker was constrained.
        let persisted = UiFontFamily::Installed("Arial".into());
        assert_eq!(
            resolve_effective_mono(&persisted, &availability, true),
            UiFontFamily::GeistMono
        );
        assert_eq!(
            resolve_effective_mono(&UiFontFamily::System, &availability, true),
            UiFontFamily::GeistMono
        );
        // The same value is still legal for code and diffs.
        assert_eq!(
            resolve_effective_mono(&persisted, &availability, false),
            persisted
        );
    }

    #[test]
    fn catalog_keeps_bundled_and_virtual_choices_first() {
        let availability = FontAvailability::all();
        assert_eq!(
            availability.choices()[..3],
            [
                UiFontFamily::Geist,
                UiFontFamily::GeistMono,
                UiFontFamily::System
            ]
        );
    }

    #[test]
    fn legacy_optional_families_become_installed_choices() {
        assert_eq!(
            serde_json::from_str::<UiFontFamily>(r#""inter""#).unwrap(),
            UiFontFamily::Installed("Inter".into())
        );
        assert_eq!(
            serde_json::from_str::<UiFontFamily>(r#""atkinsonHyperlegibleNext""#).unwrap(),
            UiFontFamily::Installed("Atkinson Hyperlegible Next".into())
        );
    }

    #[test]
    fn latin_metric_filter_requires_the_glyph_gpui_measures() {
        let latin = ttf_parser::Face::parse(GEIST[0], 0).unwrap();
        assert!(latin.glyph_index('m').is_some());
    }

    #[test]
    fn ui_font_sizes_have_stable_labels_and_normalize() {
        assert_eq!(UiFontSize::default().label().as_ref(), "16 px");
        assert_eq!(UiFontSize(19).normalized(), UiFontSize(18));
        assert_eq!(UiFontSize(250).normalized(), UiFontSize(20));
        assert_eq!(ui_rems(14.0).0, 0.875);
    }

    #[test]
    fn bundled_families_have_required_static_faces() {
        for (expected_family, faces) in [
            ("Geist", GEIST.as_slice()),
            ("Geist Mono", GEIST_MONO.as_slice()),
        ] {
            let mut found = Vec::new();
            for bytes in faces {
                let face = ttf_parser::Face::parse(bytes, 0).unwrap();
                found.push((face.weight().to_number(), face.is_italic()));
                let has_family = face.names().into_iter().any(|name| {
                    name.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY
                        && name.to_string().as_deref() == Some(expected_family)
                }) || face.names().into_iter().any(|name| {
                    name.name_id == ttf_parser::name_id::FAMILY
                        && name.to_string().as_deref() == Some(expected_family)
                });
                assert!(has_family, "wrong family metadata for {expected_family}");
            }
            found.sort_unstable();
            assert_eq!(
                found,
                vec![
                    (400, false),
                    (400, true),
                    (500, false),
                    (500, true),
                    (600, false),
                    (600, true),
                    (700, false),
                    (700, true),
                ],
                "missing static faces for {expected_family}"
            );
        }
    }
}
