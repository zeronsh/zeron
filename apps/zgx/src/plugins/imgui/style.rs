//! A plugin's own Dear ImGui look: Zeron's palette, worn, and overridable.
//!
//! A plugin window wears the theme Zeron wears — the palette the main window
//! renders, read from the same place (`zeron_theme::catalog`) — and may
//! change it for itself alone:
//!
//! ```text
//! imgui.style()          the resolved look: theme, 15 colour tokens, the
//!                        metrics, the font, the window's scale
//! imgui.set_style{ … }   this plugin's overrides
//! imgui.reset_style()    back to Zeron's
//! ```
//!
//! Nothing here reaches another plugin's context: a [`PluginStyle`] belongs
//! to one plugin, and [`PluginStyle::resolve`] is asked for that plugin's
//! windows only. Zeron's own window never reads this module.
//!
//! ## The fifteen tokens
//!
//! Mapped from the theme variant the app wears (its `ThemeColors` and
//! `AccentRoles`): window background, three inks, the hairline, the accent
//! and its hover and on-colours, three tones, the selection wash, hover,
//! pressed and the focus ring. Which variant is worn is `catalog`'s
//! question — the theme chosen for the scheme, else the running template's
//! own, else Zeron's — answered here because the app answers it in its own
//! scripts.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use dear_imgui_rs::{Context, StyleColor};
use zeron_theme::catalog::{self, TEMPLATES_DIR};
use zeron_theme::{Color, CustomThemeLibrary, ThemeVariant};
use zgx_script::Value;

use super::texture::read_plugin_file;

/// The colour tokens a plugin sees, in the order `docs/plugins.md` lists
/// them.
pub const TOKENS: [&str; 15] = [
    "$background",
    "$text.primary",
    "$text.secondary",
    "$text.tertiary",
    "$border",
    "$accent",
    "$accent.hover",
    "$accent.text",
    "$danger",
    "$warning",
    "$success",
    "$selection",
    "$hover",
    "$pressed",
    "$focus.ring",
];

/// The largest rounding, padding or border a plugin may ask for, in points:
/// past this a widget is bigger than any window and the plugin has slipped a
/// digit.
const MAX_METRIC: f32 = 512.0;

/// One scheme's fifteen colours, resolved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    colours: [[f32; 4]; TOKENS.len()],
}

impl Tokens {
    /// Zeron's dark palette, worn before the app says otherwise and by the
    /// headless tests, which read no library.
    pub fn zeron_dark() -> Self {
        catalog::find_builtin("zeron")
            .and_then(|theme| theme.variant(true))
            .map(Tokens::of)
            .unwrap_or(Tokens {
                colours: [[0.10, 0.11, 0.13, 1.0]; TOKENS.len()],
            })
    }

    /// The fifteen tokens of `variant`, derived the way Zeron's own mapping
    /// (`ui/ds/palettes.tokens`) derives them: the window's glass tint from
    /// its background, the hairline and washes from the primary ink, and the
    /// accent's hover, on-colour, selection and ring from the accent, so an
    /// accent chosen in Settings carries all of them.
    pub fn of(variant: &ThemeVariant) -> Self {
        let colors = &variant.colors;
        let accent = variant.accent.primary;
        let ink = colors.text;
        let dark = variant.appearance.is_dark();
        let on = if dark { Color::WHITE } else { Color::BLACK };
        let border = ink.with_alpha(if dark { 0.07 } else { 0.14 });
        let hover = ink.with_alpha(0.05);
        let pressed = ink.with_alpha(if dark { 0.09 } else { 0.08 });
        let selection = accent.with_alpha(if dark { 0.35 } else { 0.24 });
        let ring = accent.with_alpha(0.6);
        let accent_hover = accent.mix(on, if dark { 0.12 } else { 0.10 });
        let accent_text = accent.best_on_color();
        // The window's background as the theme library states it: a plugin
        // window is opaque, so the glass tint's alpha is no part of it.
        let background = colors.background;
        Self {
            colours: [
                background,
                ink,
                colors.text_muted,
                colors.text_faint,
                border,
                accent,
                accent_hover,
                accent_text,
                colors.danger,
                colors.warning,
                colors.success,
                selection,
                hover,
                pressed,
                ring,
            ]
            .map(rgba),
        }
    }

    /// The colour a token name stands for, if it is one of the fifteen.
    pub fn get(&self, token: &str) -> Option<[f32; 4]> {
        TOKENS
            .iter()
            .position(|name| *name == token)
            .map(|at| self.colours[at])
    }

    /// Every token and its colour, in the docs' order.
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, [f32; 4])> + '_ {
        TOKENS.iter().copied().zip(self.colours.iter().copied())
    }

    /// This palette with the plugin's overrides on top.
    fn with(&self, overrides: &HashMap<String, [f32; 4]>) -> Self {
        let mut tokens = *self;
        for (name, colour) in overrides {
            if let Some(at) = TOKENS.iter().position(|token| token == name) {
                tokens.colours[at] = *colour;
            }
        }
        tokens
    }
}

/// A `zeron_theme` colour as Dear ImGui takes it: straight alpha, 0 to 1.
fn rgba(colour: Color) -> [f32; 4] {
    [
        f32::from(colour.r) / 255.0,
        f32::from(colour.g) / 255.0,
        f32::from(colour.b) / 255.0,
        f32::from(colour.a) / 255.0,
    ]
}

/// A `#rrggbb` or `#rrggbbaa` colour.
pub fn parse_hex(text: &str) -> Option<[f32; 4]> {
    let digits = text.strip_prefix('#')?;
    if (digits.len() != 6 && digits.len() != 8) || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let at = |index: usize| {
        u8::from_str_radix(&digits[index..index + 2], 16).expect("two hex digits") as f32 / 255.0
    };
    let alpha = if digits.len() == 8 { at(6) } else { 1.0 };
    Some([at(0), at(2), at(4), alpha])
}

/// Which scheme a plugin window wears.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Theme {
    /// Zeron's: dark or light, as the app wears it.
    #[default]
    Auto,
    Dark,
    Light,
}

impl Theme {
    /// The scheme this wears, given the app's own.
    fn dark(self, app_dark: bool) -> bool {
        match self {
            Self::Auto => app_dark,
            Self::Dark => true,
            Self::Light => false,
        }
    }
}

/// The metric fields a plugin set, each `None` when it left Zeron's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    pub frame_rounding: Option<f32>,
    pub window_rounding: Option<f32>,
    pub window_padding: Option<[f32; 2]>,
    pub item_spacing: Option<[f32; 2]>,
    pub border_size: Option<f32>,
    pub text_size: Option<f32>,
}

/// A font file a plugin loaded, ready for any of its windows to use.
#[derive(Debug, PartialEq)]
pub struct PluginFont {
    /// Tells one loaded font from another across windows' atlases.
    pub id: u64,
    /// The path the plugin named, as `imgui.style()` reports it.
    pub path: String,
    pub size: f32,
    pub bytes: Rc<[u8]>,
}

/// One resolved look: what a frame is drawn with.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub theme: Theme,
    /// The scheme worn: `true` for dark.
    pub dark: bool,
    pub tokens: Tokens,
    pub metrics: Metrics,
    pub font: Option<Rc<PluginFont>>,
}

impl Resolved {
    /// The name of the scheme worn, as `imgui.style()` says it.
    pub fn theme_name(&self) -> &'static str {
        if self.dark { "dark" } else { "light" }
    }

    /// The colour a plugin's `"$token"` or `"#rrggbb[aa]"` stands for.
    pub fn colour(&self, text: &str) -> Option<[f32; 4]> {
        if text.starts_with('$') {
            return self.tokens.get(text);
        }
        parse_hex(text)
    }
}

/// What a plugin asked `imgui.set_style` for, kept until its next frame.
#[derive(Debug, Default, PartialEq)]
struct Settings {
    theme: Theme,
    colours: HashMap<String, [f32; 4]>,
    metrics: Metrics,
    font: Option<Rc<PluginFont>>,
}

/// One plugin's style: Zeron's, with the plugin's own changes on top.
///
/// Shared by every window of the plugin, so what a plugin sets reaches its
/// next frame on all of them and no other plugin's.
pub struct PluginStyle {
    settings: RefCell<Settings>,
    /// The plugin's folder: what `imgui.font{ path = … }` is read inside.
    root: PathBuf,
    next_font: std::cell::Cell<u64>,
}

impl PluginStyle {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            settings: RefCell::default(),
            root: root.into(),
            next_font: std::cell::Cell::new(0),
        }
    }

    /// The folder this plugin's own files are read from.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// This plugin's look now.
    pub fn resolve(&self) -> Resolved {
        let settings = self.settings.borrow();
        let dark = settings.theme.dark(app_dark());
        let palettes = palettes();
        let base = if dark { palettes.0 } else { palettes.1 };
        Resolved {
            theme: settings.theme,
            dark,
            tokens: base.with(&settings.colours),
            metrics: settings.metrics,
            font: settings.font.clone(),
        }
    }

    /// The colour a plugin's `"$token"` stands for now.
    pub fn colour(&self, name: &str) -> Option<[f32; 4]> {
        self.resolve().tokens.get(name)
    }

    /// `imgui.set_style{ … }`. Every field is checked before any of it is
    /// applied: a refused call changes nothing.
    pub fn set(&self, arg: &Value) -> Result<(), String> {
        let theme = match arg.field("theme") {
            None => None,
            Some(value) => Some(match text_of(value) {
                Some(name) if name == "auto" => Theme::Auto,
                Some(name) if name == "dark" => Theme::Dark,
                Some(name) if name == "light" => Theme::Light,
                Some(other) => {
                    return Err(format!(
                        "imgui.set_style can't use theme \"{other}\": theme is \"auto\", \"dark\" or \"light\"."
                    ));
                }
                None => {
                    return Err(
                        "imgui.set_style can't use theme: theme is \"auto\", \"dark\" or \"light\"."
                            .into(),
                    );
                }
            }),
        };
        let colours = self.colours(arg)?;
        let metrics = Metrics {
            frame_rounding: metric(arg, "frame_rounding")?,
            window_rounding: metric(arg, "window_rounding")?,
            window_padding: pair(arg, "window_padding")?,
            item_spacing: pair(arg, "item_spacing")?,
            border_size: metric(arg, "border_size")?,
            text_size: metric(arg, "text_size")?,
        };
        if let Some(size) = metrics.text_size
            && size > 0.0
            && size < 6.0
        {
            return Err("imgui.set_style needs text_size to be at least 6 points.".into());
        }
        let font = match arg.field("font") {
            None => None,
            Some(value) => Some(self.font_handle(value)?),
        };
        let mut settings = self.settings.borrow_mut();
        if let Some(theme) = theme {
            settings.theme = theme;
        }
        if let Some(colours) = colours {
            settings.colours.extend(colours);
        }
        let given = metrics;
        let kept = settings.metrics;
        settings.metrics = Metrics {
            frame_rounding: given.frame_rounding.or(kept.frame_rounding),
            window_rounding: given.window_rounding.or(kept.window_rounding),
            window_padding: given.window_padding.or(kept.window_padding),
            item_spacing: given.item_spacing.or(kept.item_spacing),
            border_size: given.border_size.or(kept.border_size),
            text_size: given.text_size.or(kept.text_size),
        };
        if font.is_some() {
            settings.font = font;
        }
        Ok(())
    }

    /// `imgui.reset_style()`: Zeron's look again, on the plugin's next frame.
    pub fn reset(&self) {
        *self.settings.borrow_mut() = Settings::default();
    }

    /// `imgui.font{ path, size }`: the file read once and kept.
    pub fn load_font(&self, arg: &Value) -> Result<Rc<PluginFont>, String> {
        let Some(path) = arg.field("path").and_then(text_of) else {
            return Err(
                "imgui.font needs a path, like imgui.font{ path = \"assets/serif.ttf\" }.".into(),
            );
        };
        let size = match arg.field("size") {
            None => 16.0,
            Some(Value::Num(size)) if size.is_finite() && *size >= 6.0 && *size <= 96.0 => {
                *size as f32
            }
            Some(Value::Num(_)) => {
                return Err("imgui.font needs size to be between 6 and 96 points.".into());
            }
            Some(other) => {
                return Err(format!(
                    "imgui.font needs size to be a number, not {}.",
                    super::frame::type_name(other)
                ));
            }
        };
        let bytes = read_plugin_file(&self.root, &path, "a font")?;
        let id = self.next_font.get() + 1;
        self.next_font.set(id);
        Ok(Rc::new(PluginFont {
            id,
            path,
            size,
            bytes: Rc::from(bytes),
        }))
    }

    /// The font size a frame draws its widgets at.
    pub fn text_size(&self, resolved: &Resolved) -> Option<f32> {
        resolved
            .metrics
            .text_size
            .filter(|size| *size > 0.0)
            .or_else(|| resolved.font.as_ref().map(|font| font.size))
    }

    fn colours(&self, arg: &Value) -> Result<Option<HashMap<String, [f32; 4]>>, String> {
        let Some(value) = arg.field("colours") else {
            return Ok(None);
        };
        let Value::Map(entries) = value else {
            return Err(
                "imgui.set_style can't use colours: colours is a table of \"$token\" = \"#rrggbb\"."
                    .into(),
            );
        };
        let mut overrides = HashMap::with_capacity(entries.len());
        for (name, colour) in entries.iter() {
            let name = name.to_string();
            if !TOKENS.contains(&name.as_str()) {
                return Err(format!(
                    "imgui.set_style can't use colour \"{name}\": the tokens are {}.",
                    TOKENS.join(", ")
                ));
            }
            let Some(text) = text_of(colour) else {
                return Err(format!(
                    "imgui.set_style needs colour \"{name}\" to be \"#rrggbb\" or \"#rrggbbaa\"."
                ));
            };
            let Some(rgba) = parse_hex(&text) else {
                return Err(format!(
                    "imgui.set_style can't use colour \"{text}\": colour must be \"#rrggbb\" or \"#rrggbbaa\"."
                ));
            };
            overrides.insert(name, rgba);
        }
        Ok(Some(overrides))
    }

    /// The font a `font = …` value names: the handle `imgui.font` handed out
    /// to this plugin.
    fn font_handle(&self, value: &Value) -> Result<Rc<PluginFont>, String> {
        if !matches!(value, Value::Map(_)) {
            return Err(
                "imgui.set_style needs font to be a handle from imgui.font{ path = … }.".into(),
            );
        }
        let Some(Value::Num(id)) = value.field("__font") else {
            return Err(
                "imgui.set_style needs font to be a handle from imgui.font{ path = … }, not another value."
                    .into(),
            );
        };
        self.loaded_font(*id as u64).ok_or_else(|| {
            "That font belongs to another plugin. Load one with imgui.font{ path = … }.".to_string()
        })
    }

    fn loaded_font(&self, id: u64) -> Option<Rc<PluginFont>> {
        self.settings
            .borrow()
            .font
            .as_ref()
            .filter(|font| font.id == id)
            .cloned()
    }

    /// The font a frame must add to its context's atlas, if the plugin set
    /// one.
    pub fn font(&self) -> Option<Rc<PluginFont>> {
        self.settings.borrow().font.clone()
    }
}

fn metric(arg: &Value, field: &str) -> Result<Option<f32>, String> {
    match arg.field(field) {
        None => Ok(None),
        Some(Value::Num(value))
            if value.is_finite() && *value >= 0.0 && *value <= MAX_METRIC as f64 =>
        {
            Ok(Some(*value as f32))
        }
        Some(Value::Num(_)) => Err(format!(
            "imgui.set_style needs {field} to be between 0 and {MAX_METRIC} points."
        )),
        Some(other) => Err(format!(
            "imgui.set_style needs {field} to be a number, not {}.",
            super::frame::type_name(other)
        )),
    }
}

fn pair(arg: &Value, field: &str) -> Result<Option<[f32; 2]>, String> {
    let Some(value) = arg.field(field) else {
        return Ok(None);
    };
    let pair = match (number_field(value, "x"), number_field(value, "y")) {
        (Some(x), Some(y)) => [x, y],
        _ => match (number_field(value, "width"), number_field(value, "height")) {
            (Some(x), Some(y)) => [x, y],
            _ => {
                return Err(format!(
                    "imgui.set_style needs {field} to be {{ x = …, y = … }}."
                ));
            }
        },
    };
    for value in pair {
        if !value.is_finite() || value < 0.0 || value as f64 > MAX_METRIC as f64 {
            return Err(format!(
                "imgui.set_style needs {field} to be between 0 and {MAX_METRIC} points."
            ));
        }
    }
    Ok(Some(pair))
}

fn number_field(value: &Value, field: &str) -> Option<f32> {
    match value.field(field) {
        Some(Value::Num(number)) if number.is_finite() => Some(*number as f32),
        _ => None,
    }
}

fn text_of(value: &Value) -> Option<String> {
    match value {
        Value::Str(text) => Some(text.to_string()),
        Value::Text(text) => Some(text.to_string()),
        _ => None,
    }
}

thread_local! {
    /// The two palettes, read from the theme library once per change.
    static PALETTES: RefCell<Option<(Tokens, Tokens)>> = const { RefCell::new(None) };
    /// The app's scheme, as its own `appearance` action last said.
    static APP_DARK: Cell<bool> = const { Cell::new(true) };
}

/// Told by the host's `appearance` action what the app wears now. `None` is
/// the system's own appearance, which the app's scripts resolve to a scheme;
/// without one to read, Zeron's dark default stands.
pub fn set_appearance(dark: Option<bool>) {
    APP_DARK.with(|current| current.set(dark.unwrap_or(true)));
}

/// The app's scheme now.
pub fn app_dark() -> bool {
    APP_DARK.with(Cell::get)
}

/// Drops the palette cache: the theme library changed
/// (`crate::themes::publish`).
pub fn invalidate() {
    PALETTES.with(|palettes| *palettes.borrow_mut() = None);
}

/// The two palettes the app can wear, from the worn theme.
pub fn palettes() -> (Tokens, Tokens) {
    if let Some(palettes) = PALETTES.with(|palettes| *palettes.borrow()) {
        return palettes;
    }
    let resolved = resolve_palettes();
    PALETTES.with(|palettes| *palettes.borrow_mut() = Some(resolved));
    resolved
}

fn resolve_palettes() -> (Tokens, Tokens) {
    let dark = Tokens::zeron_dark();
    let Some(dir) = crate::engine::data_dir() else {
        return (dark, dark);
    };
    let worn_dark = worn_variant(&dir, true).map(|variant| Tokens::of(&variant));
    let worn_light = worn_variant(&dir, false).map(|variant| Tokens::of(&variant));
    (worn_dark.unwrap_or(dark), worn_light.unwrap_or(dark))
}

/// The variant the app wears for one scheme: the theme chosen for it, else
/// the running template's own, else Zeron's.
fn worn_variant(dir: &Path, dark: bool) -> Option<ThemeVariant> {
    let scheme = if dark { "dark" } else { "light" };
    if let Some(id) = catalog::chosen(dir).get(scheme)
        && let Some(variant) = variant_by_id(dir, id)
        && variant.appearance.is_dark() == dark
    {
        return Some(variant);
    }
    let templates = dir.join(TEMPLATES_DIR);
    if let Some(template) = catalog::running_template(&templates)
        && let Some(variant) = template_variant(&templates, &template, dark)
    {
        return Some(variant);
    }
    catalog::find_builtin("zeron")
        .and_then(|theme| theme.variant(dark))
        .cloned()
}

/// The variant `id` names, wherever it comes from: Zeron's and the other
/// built-ins, a template's own, or one of the person's.
fn variant_by_id(dir: &Path, id: &str) -> Option<ThemeVariant> {
    for theme in catalog::builtin() {
        if let Some(variant) = theme.variants.iter().find(|variant| variant.id == id) {
            return Some((*variant).clone());
        }
    }
    let templates = dir.join(TEMPLATES_DIR);
    for scheme_dark in [true, false] {
        for theme in catalog::template_themes(&templates) {
            if let Some(variant) = template_variant(&templates, &theme.template, scheme_dark)
                && variant.id == id
            {
                return Some(variant);
            }
        }
    }
    let library = CustomThemeLibrary::load(dir).ok()?;
    library
        .entries
        .iter()
        .flat_map(|entry| entry.family.variants.iter())
        .find(|variant| variant.id == id)
        .cloned()
}

/// A template's own theme for one scheme, built from its `theme.luau`.
fn template_variant(templates: &Path, template: &str, dark: bool) -> Option<ThemeVariant> {
    let theme = catalog::template_themes(templates)
        .into_iter()
        .find(|theme| theme.template == template)?;
    let palette = catalog::Palette::from_json(&theme.file, template).ok()?;
    let scheme = if dark { "dark" } else { "light" };
    let entry = catalog::template_entry(template);
    let name = theme
        .file
        .get("name")
        .and_then(|name| name.as_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(&theme.name);
    Some(palette.variant(
        &entry,
        &format!("template-{template}-{scheme}"),
        name,
        if dark {
            zeron_theme::Appearance::Dark
        } else {
            zeron_theme::Appearance::Light
        },
    ))
}

/// Dear ImGui's own defaults for the metrics a plugin did not set: what its
/// style carries before any plugin touches it.
const DEFAULT_FRAME_ROUNDING: f32 = 0.0;
const DEFAULT_WINDOW_ROUNDING: f32 = 0.0;
const DEFAULT_BORDER_SIZE: f32 = 1.0;
const DEFAULT_WINDOW_PADDING: [f32; 2] = [8.0, 8.0];
const DEFAULT_ITEM_SPACING: [f32; 2] = [8.0, 4.0];
/// The size Dear ImGui's built-in font draws at.
const DEFAULT_TEXT_SIZE: f32 = 13.0;

/// A colour as Zeron writes it: `#rrggbb`, or `#rrggbbaa` when it is not
/// opaque.
pub fn hex(colour: [f32; 4]) -> String {
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (r, g, b, a) = (
        byte(colour[0]),
        byte(colour[1]),
        byte(colour[2]),
        byte(colour[3]),
    );
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

/// `imgui.style()`: the look a plugin reads back, resolved.
pub fn view(resolved: &Resolved, dpi_scale: f64) -> Value {
    let mut colours: Vec<(Rc<str>, Value)> = resolved
        .tokens
        .iter()
        .map(|(name, colour)| (Rc::from(name), Value::from(hex(colour).as_str())))
        .collect();
    colours.sort_by(|left, right| left.0.cmp(&right.0));
    let metrics = resolved.metrics;
    let mut fields = vec![
        ("ok", Value::Bool(true)),
        ("theme", Value::from(resolved.theme_name())),
        ("colours", Value::Map(Rc::from(colours))),
        (
            "dpi_scale",
            Value::from(if dpi_scale > 0.0 { dpi_scale } else { 1.0 }),
        ),
        (
            "frame_rounding",
            Value::from(f64::from(
                metrics.frame_rounding.unwrap_or(DEFAULT_FRAME_ROUNDING),
            )),
        ),
        (
            "window_rounding",
            Value::from(f64::from(
                metrics.window_rounding.unwrap_or(DEFAULT_WINDOW_ROUNDING),
            )),
        ),
        (
            "border_size",
            Value::from(f64::from(
                metrics.border_size.unwrap_or(DEFAULT_BORDER_SIZE),
            )),
        ),
        (
            "text_size",
            Value::from(f64::from(metrics.text_size.unwrap_or(DEFAULT_TEXT_SIZE))),
        ),
    ];
    for (name, pair) in [
        (
            "window_padding",
            metrics.window_padding.unwrap_or(DEFAULT_WINDOW_PADDING),
        ),
        (
            "item_spacing",
            metrics.item_spacing.unwrap_or(DEFAULT_ITEM_SPACING),
        ),
    ] {
        fields.push((
            name,
            Value::record([
                ("x", Value::from(f64::from(pair[0]))),
                ("y", Value::from(f64::from(pair[1]))),
            ]),
        ));
    }
    fields.push((
        "font",
        match resolved.font.as_ref() {
            Some(font) => Value::from(font.path.as_str()),
            None => Value::Nil,
        },
    ));
    Value::record(fields)
}

/// Applies `resolved` to a window's context: colours and metrics. Called
/// before the frame opens, so every widget it draws — popups and tooltips
/// included — wears it.
pub fn apply(context: &mut Context, resolved: &Resolved) {
    let tokens = resolved.tokens;
    let token = |name: &str| tokens.get(name).unwrap_or([0.0, 0.0, 0.0, 1.0]);
    let mut background = token("$background");
    // A plugin window is opaque: the background's own alpha, if the theme
    // carries one, is the glass's, not this window's.
    background[3] = 1.0;
    let text = token("$text.primary");
    let border = token("$border");
    let accent = token("$accent");
    let selection = token("$selection");
    let hover = token("$hover");
    let pressed = token("$pressed");
    let muted = token("$text.secondary");
    let faint = token("$text.tertiary");
    let mut frame = hover;
    frame[3] = frame[3].max(0.10);
    let colours: [(StyleColor, [f32; 4]); 56] = [
        (StyleColor::Text, text),
        (StyleColor::TextDisabled, faint),
        (StyleColor::WindowBg, background),
        (StyleColor::ChildBg, [0.0, 0.0, 0.0, 0.0]),
        (StyleColor::PopupBg, background),
        (StyleColor::Border, border),
        (StyleColor::BorderShadow, [0.0, 0.0, 0.0, 0.0]),
        (StyleColor::FrameBg, frame),
        (StyleColor::FrameBgHovered, hover),
        (StyleColor::FrameBgActive, pressed),
        (StyleColor::TitleBg, background),
        (StyleColor::TitleBgActive, background),
        (StyleColor::TitleBgCollapsed, background),
        (StyleColor::MenuBarBg, background),
        (StyleColor::ScrollbarBg, [0.0, 0.0, 0.0, 0.0]),
        (StyleColor::ScrollbarGrab, muted),
        (StyleColor::ScrollbarGrabHovered, text),
        (StyleColor::ScrollbarGrabActive, text),
        (StyleColor::CheckMark, accent),
        (StyleColor::CheckboxSelectedBg, accent),
        (StyleColor::SliderGrab, accent),
        (StyleColor::SliderGrabActive, accent),
        (StyleColor::Button, frame),
        (StyleColor::ButtonHovered, hover),
        (StyleColor::ButtonActive, pressed),
        (StyleColor::Header, selection),
        (StyleColor::HeaderHovered, hover),
        (StyleColor::HeaderActive, pressed),
        (StyleColor::Separator, border),
        (StyleColor::SeparatorHovered, border),
        (StyleColor::SeparatorActive, border),
        (StyleColor::ResizeGrip, border),
        (StyleColor::ResizeGripHovered, border),
        (StyleColor::ResizeGripActive, border),
        (StyleColor::Tab, background),
        (StyleColor::TabHovered, hover),
        (StyleColor::TabSelected, selection),
        (StyleColor::TabSelectedOverline, accent),
        (StyleColor::TabDimmed, background),
        (StyleColor::TabDimmedSelected, selection),
        (StyleColor::TabDimmedSelectedOverline, accent),
        (StyleColor::TableHeaderBg, [0.0, 0.0, 0.0, 0.0]),
        (StyleColor::TableBorderStrong, border),
        (StyleColor::TableBorderLight, border),
        (StyleColor::TableRowBg, [0.0, 0.0, 0.0, 0.0]),
        (StyleColor::TableRowBgAlt, [1.0, 1.0, 1.0, 0.03]),
        (StyleColor::TextSelectedBg, selection),
        (StyleColor::DragDropTarget, accent),
        (StyleColor::NavCursor, token("$focus.ring")),
        (StyleColor::NavWindowingHighlight, accent),
        (StyleColor::ModalWindowDimBg, [0.0, 0.0, 0.0, 0.32]),
        (StyleColor::PlotLines, accent),
        (StyleColor::PlotLinesHovered, accent),
        (StyleColor::PlotHistogram, accent),
        (StyleColor::PlotHistogramHovered, accent),
        (StyleColor::InputTextCursor, text),
    ];
    let style = context.style_mut();
    for (which, colour) in colours {
        style.set_color(which, colour);
    }
    let metrics = resolved.metrics;
    if let Some(value) = metrics.frame_rounding {
        style.set_frame_rounding(value);
    }
    if let Some(value) = metrics.window_rounding {
        style.set_window_rounding(value);
    }
    if let Some(value) = metrics.window_padding {
        style.set_window_padding(value);
    }
    if let Some(value) = metrics.item_spacing {
        style.set_item_spacing(value);
    }
    if let Some(value) = metrics.border_size {
        style.set_window_border_size(value);
        style.set_child_border_size(value);
        style.set_popup_border_size(value);
        style.set_frame_border_size(value);
    }
}
