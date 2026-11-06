//! The themes Zeron's window can wear, and the one chosen for each scheme,
//! as both the window (apps/zgx/src/themes.rs) and the engine's theme tools
//! (crates/engine/src/plugins/themes.rs) read and write them. A theme is
//! data: a palette for light, for dark or both. It never carries code.
//!
//! Three kinds are listed:
//! - Zeron's built-in themes (`builtin_registry`): each family's first light
//!   and first dark together, any other variant on its own;
//! - a template's own `theme.luau`, read as data ([`crate::luau_data`]);
//! - yours, in `<data>/theme-library.json`: VS Code themes imported, and
//!   themes made from palettes. Only these can be changed or deleted.
//!
//! What's chosen is one theme for light and one for dark, in
//! `<data>/theme-choice.json`. A slot with nothing chosen wears the running
//! template's own theme, else Zeron's.
//!
//! A [`Palette`] is the shape the window's mapping (`ds/palettes`) wears, and
//! the shape the tools take: surfaces, inks, accent and tones, code colours
//! and, if given, the terminal's. The window keeps every palette legible,
//! moving an ink that reads too faint toward black or white; [`legibility`]
//! says which it moves, as the window does (checked against it by the app's
//! tests).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};
use serde_json::{Map, Value as Json, json};
use sha2::{Digest, Sha256};

use crate::{
    AccentRoles, Appearance, Color, CustomThemeLibrary, CustomThemeStatus, InstallMode,
    SurfaceTreatment, TerminalPalette, ThemeColors, ThemeFamily, ThemeSource, ThemeVariant,
    builtin_registry,
};

/// The schemes a theme can have, in the order its variants are listed.
pub const SCHEMES: [&str; 2] = ["light", "dark"];
/// Where the choice is kept, in the engine's data dir.
pub const CHOICE_FILE: &str = "theme-choice.json";
/// The templates folder, in the engine's data dir.
pub const TEMPLATES_DIR: &str = "templates";
/// A template's own theme, at its folder's root.
pub const TEMPLATE_THEME_FILE: &str = "theme.luau";
/// The code colours a palette carries, in order.
pub const SYNTAX: [&str; 12] = [
    "comment", "keyword", "string", "number", "type", "function", "property", "variable",
    "punctuation", "tag", "attribute", "invalid",
];
/// The terminal's sixteen colours, in order.
pub const ANSI: [&str; 16] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white", "bright black",
    "bright red", "bright green", "bright yellow", "bright blue", "bright magenta", "bright cyan",
    "bright white",
];
/// The longest name a theme takes.
pub const MAX_NAME: usize = 80;
/// The largest VS Code theme taken inline.
pub const MAX_VSCODE_BYTES: usize = 1024 * 1024;

const REQUIRED: [&str; 9] = [
    "background", "raised", "text", "muted", "faint", "accent", "danger", "warning", "success",
];
const OPTIONAL: [&str; 3] = ["shell", "card", "terminal"];
/// Fields a palette read back carries that say what it is, not its colours:
/// taken as they come back and ignored.
const LABELS: [&str; 4] = ["id", "name", "kind", "entry"];

// ── Palettes ────────────────────────────────────────────────────────────────

/// A scheme's colours, as the window wears them.
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    /// The window.
    pub background: Color,
    /// Raised things: a chip, a selected row, a menu's lift.
    pub raised: Color,
    /// Body text, quieter text, the faintest text.
    pub text: Color,
    pub muted: Color,
    pub faint: Color,
    pub accent: Color,
    pub danger: Color,
    pub warning: Color,
    pub success: Color,
    /// The [`SYNTAX`] colours, in order.
    pub syntax: [Color; 12],
    /// The chrome (sidebar, title bar) and a card; the window's where absent.
    pub shell: Option<Color>,
    pub card: Option<Color>,
    /// The terminal's background and its [`ANSI`] colours.
    pub terminal: Option<Color>,
    pub ansi: Option<[Color; 16]>,
}

impl Palette {
    /// A variant's palette.
    pub fn of(variant: &ThemeVariant) -> Self {
        let c = &variant.colors;
        Self {
            background: c.background,
            raised: c.raised,
            text: c.text,
            muted: c.text_muted,
            faint: c.text_faint,
            accent: variant.accent.primary,
            danger: c.danger,
            warning: c.warning,
            success: c.success,
            syntax: SYNTAX.map(|name| variant.syntax.get(name).copied().unwrap_or(c.text)),
            shell: Some(c.shell),
            card: Some(c.card),
            terminal: Some(variant.terminal.background),
            ansi: Some(variant.terminal.ansi),
        }
    }

    /// Read from JSON, every problem said (`at` names it: `dark`). Colours are
    /// `#rrggbb` or `#rrggbbaa` (`#rgb` too); `id`, `name`, `kind` and
    /// `entry` (what a palette read back carries) are ignored.
    pub fn from_json(value: &Json, at: &str) -> std::result::Result<Self, Vec<String>> {
        let Some(map) = value.as_object() else {
            return Err(vec![format!("`{at}` must be a palette: an object of colours.")]);
        };
        let mut errors = Vec::new();
        for key in map.keys() {
            let known = REQUIRED.contains(&key.as_str())
                || OPTIONAL.contains(&key.as_str())
                || LABELS.contains(&key.as_str())
                || key == "syntax"
                || key == "ansi";
            if !known {
                errors.push(format!(
                    "`{at}.{key}` isn't part of a palette. A palette has {}, syntax (12 colours) and, if you like, {}, ansi (16 colours).",
                    REQUIRED.join(", "),
                    OPTIONAL.join(", ")
                ));
            }
        }
        let mut colour = |key: &str, value: Option<&Json>, required: bool| -> Option<Color> {
            match value {
                None | Some(Json::Null) if !required => None,
                None | Some(Json::Null) => {
                    errors.push(format!("`{at}.{key}` is missing."));
                    None
                }
                Some(Json::String(s)) => match s.parse::<Color>() {
                    Ok(c) => Some(c),
                    Err(_) => {
                        errors.push(format!("`{at}.{key}`: {s:?} isn't a colour. Write it as #rrggbb or #rrggbbaa."));
                        None
                    }
                },
                Some(other) => {
                    errors.push(format!("`{at}.{key}`: {other} isn't a colour. Write it as #rrggbb or #rrggbbaa."));
                    None
                }
            }
        };
        let required: Vec<Option<Color>> = REQUIRED.iter().map(|k| colour(k, map.get(*k), true)).collect();
        let shell = colour("shell", map.get("shell"), false);
        let card = colour("card", map.get("card"), false);
        let terminal = colour("terminal", map.get("terminal"), false);
        let list = |key: &str, names: &[&str], required: bool, errors: &mut Vec<String>| -> Option<Vec<Color>> {
            match map.get(key) {
                None | Some(Json::Null) if !required => None,
                Some(Json::Array(items)) if items.len() == names.len() => {
                    let mut out = Vec::new();
                    for (i, item) in items.iter().enumerate() {
                        match item.as_str().map(str::parse::<Color>) {
                            Some(Ok(c)) => out.push(c),
                            _ => errors.push(format!(
                                "`{at}.{key}[{}]` ({}): {item} isn't a colour. Write it as #rrggbb or #rrggbbaa.",
                                i + 1,
                                names[i]
                            )),
                        }
                    }
                    (out.len() == names.len()).then_some(out)
                }
                _ => {
                    errors.push(format!("`{at}.{key}` needs {} colours, in this order: {}.", names.len(), names.join(", ")));
                    None
                }
            }
        };
        let syntax = list("syntax", &SYNTAX, true, &mut errors);
        let ansi = list("ansi", &ANSI, false, &mut errors);
        if !errors.is_empty() {
            return Err(errors);
        }
        let c = |i: usize| required[i].expect("checked");
        Ok(Self {
            background: c(0),
            raised: c(1),
            text: c(2),
            muted: c(3),
            faint: c(4),
            accent: c(5),
            danger: c(6),
            warning: c(7),
            success: c(8),
            syntax: syntax.expect("checked").try_into().expect("12 colours"),
            shell,
            card,
            terminal,
            ansi: ansi.map(|a| a.try_into().expect("16 colours")),
        })
    }

    /// As JSON, the window's shape and the tools': the palette's colours,
    /// labelled with its variant's `id`, `name` and `kind` and the theme it's
    /// under (`entry`) where given.
    pub fn to_json(&self, labels: Option<(&str, &str, &str, &str)>) -> Json {
        let hex = |c: Color| json!(c.to_string());
        let mut out = Map::new();
        if let Some((id, name, kind, entry)) = labels {
            out.insert("id".into(), json!(id));
            out.insert("name".into(), json!(name));
            out.insert("kind".into(), json!(kind));
            out.insert("entry".into(), json!(entry));
        }
        for (key, c) in REQUIRED.iter().zip([
            self.background, self.raised, self.text, self.muted, self.faint, self.accent, self.danger, self.warning, self.success,
        ]) {
            out.insert((*key).into(), hex(c));
        }
        out.insert("syntax".into(), json!(self.syntax.map(|c| c.to_string())));
        for (key, c) in OPTIONAL.iter().zip([self.shell, self.card, self.terminal]) {
            if let Some(c) = c {
                out.insert((*key).into(), hex(c));
            }
        }
        if let Some(ansi) = self.ansi {
            out.insert("ansi".into(), json!(ansi.map(|c| c.to_string())));
        }
        Json::Object(out)
    }

    /// The variant this palette makes: what it doesn't say grown from it
    /// (hairlines and washes from the text's ink, the terminal's colours
    /// from the tones), so reading it back gives these colours again.
    pub fn variant(&self, family_id: &str, id: &str, name: &str, appearance: Appearance) -> ThemeVariant {
        let bg = self.background;
        let over = |c: Color, alpha: f32| c.with_alpha(alpha).blend_over(bg);
        let shell = self.shell.unwrap_or(bg);
        let card = self.card.unwrap_or(bg);
        let colors = ThemeColors {
            background: bg,
            shell,
            raised: self.raised,
            card,
            dialog: card,
            overlay: self.raised,
            hover: over(self.text, 0.06),
            active: over(self.text, 0.1),
            border: over(self.text, 0.12),
            border_strong: over(self.text, 0.22),
            text: self.text,
            text_muted: self.muted,
            text_faint: self.faint,
            solid: self.text,
            on_solid: bg,
            danger: self.danger,
            danger_muted: over(self.danger, 0.16),
            warning: self.warning,
            warning_muted: over(self.warning, 0.16),
            success: self.success,
            success_muted: over(self.success, 0.16),
            input: card,
            cursor: self.accent,
            diff_add: self.success,
            diff_delete: self.danger,
            diff_hunk: self.accent,
        };
        let mut accent = AccentRoles::derive(self.accent, appearance, bg);
        // (As given: the window keeps it legible as it wears it.)
        accent.primary = self.accent;
        let s = &self.syntax;
        let ansi = self.ansi.unwrap_or([
            self.faint, self.danger, self.success, self.warning, self.accent, s[1], s[4], self.text,
            self.muted, self.danger, self.success, self.warning, self.accent, s[1], s[4], self.text,
        ]);
        let encoded = self.to_json(None).to_string();
        ThemeVariant {
            id: id.into(),
            family_id: family_id.into(),
            name: name.into(),
            appearance,
            recommended_surface_treatment: SurfaceTreatment::Opaque,
            colors,
            accent,
            syntax: SYNTAX.iter().zip(self.syntax).map(|(k, c)| ((*k).to_string(), c)).collect::<BTreeMap<_, _>>(),
            terminal: TerminalPalette {
                background: self.terminal.unwrap_or(bg),
                foreground: self.text,
                selection: self.accent.with_alpha(0.3),
                ansi,
            },
            source: ThemeSource {
                format: "zeron-palette".into(),
                url: "zeron:palette".into(),
                revision: "1".into(),
                license: "Yours".into(),
                asset_hash: format!("sha256:{:x}", Sha256::digest(encoded.as_bytes())),
            },
        }
    }
}

// ── Legibility: what the window moves ───────────────────────────────────────

/// An ink the window moves to keep it legible: its role, the palette's
/// colour, what's worn instead, and the contrast it's held to.
#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub role: String,
    pub from: String,
    pub to: String,
    pub min: f64,
}

impl Fix {
    pub fn to_json(&self) -> Json {
        json!({ "role": self.role, "from": self.from, "to": self.to, "min": self.min })
    }
}

/// Colours as `ds/palettes` does its sums: channels as numbers, rounded to a
/// byte only when written, so the inks moved here are the window's.
#[derive(Clone, Copy)]
struct Ink(f64, f64, f64, f64);

impl Ink {
    fn of(c: Color) -> Self {
        Ink(c.r as f64, c.g as f64, c.b as f64, c.a as f64)
    }
    fn hex(self) -> String {
        let byte = |x: f64| (x + 0.5).floor().clamp(0.0, 255.0) as u8;
        let a = if self.3 < 255.0 { format!("{:02x}", byte(self.3)) } else { String::new() };
        format!("#{:02x}{:02x}{:02x}{a}", byte(self.0), byte(self.1), byte(self.2))
    }
    fn parse(hex: &str) -> Self {
        Ink::of(hex.parse().unwrap_or(Color::BLACK))
    }
    fn mix(self, to: Ink, t: f64) -> String {
        let t = t.clamp(0.0, 1.0);
        Ink(
            self.0 + (to.0 - self.0) * t,
            self.1 + (to.1 - self.1) * t,
            self.2 + (to.2 - self.2) * t,
            self.3 + (to.3 - self.3) * t,
        )
        .hex()
    }
    fn luminance(self) -> f64 {
        let linear = |ch: f64| {
            let v = ch / 255.0;
            if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * linear(self.0) + 0.7152 * linear(self.1) + 0.0722 * linear(self.2)
    }
}

fn contrast(ink: &str, under: &str) -> f64 {
    let mut i = Ink::parse(ink);
    let u = Ink::parse(under);
    if i.3 < 255.0 {
        let t = i.3 / 255.0;
        i = Ink::parse(&Ink(i.0 * t + u.0 * (1.0 - t), i.1 * t + u.1 * (1.0 - t), i.2 * t + u.2 * (1.0 - t), 255.0).hex());
    }
    let (x, y) = (i.luminance(), u.luminance());
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

fn least(ink: &str, unders: &[String]) -> f64 {
    unders.iter().map(|u| contrast(ink, u)).fold(f64::INFINITY, f64::min)
}

/// `c` moved toward black or white (whichever the first surface takes
/// better) until it reads at `min` on every one of `unders`.
fn ensure(c: &str, unders: &[String], min: f64) -> String {
    if least(c, unders) >= min {
        return c.to_string();
    }
    let (black, white) = ("#000000", "#ffffff");
    let target = if contrast(black, &unders[0]) >= contrast(white, &unders[0]) { black } else { white };
    for step in 1..=20 {
        let candidate = Ink::parse(c).mix(Ink::parse(target), step as f64 / 20.0);
        if least(&candidate, unders) >= min {
            return candidate;
        }
    }
    target.to_string()
}

/// The inks the window moves to keep `palette` legible in `scheme`, as
/// `ds/palettes.tokens` moves them: text on every surface (4.5:1, the
/// faintest 3:1), fills on the window (3:1) and each tone's words on every
/// surface (4.5:1), code's colours on the window and a card (4.5:1, comments
/// and punctuation 3:1), a chart's colours on the window (3:1), and the
/// terminal's sixteen ANSI colours on every surface (4.5:1; 3:1 the two that
/// are the background's own shade).
pub fn legibility(palette: &Palette, scheme: &str) -> Vec<Fix> {
    let dark = scheme != "light";
    let hex = |c: Color| c.to_string();
    let mut fixes = Vec::new();
    let mut fix = |role: &str, c: &str, unders: &[String], min: f64| -> String {
        let out = ensure(c, unders, min);
        if out != c {
            fixes.push(Fix { role: role.into(), from: c.into(), to: out.clone(), min });
        }
        out
    };
    let bg = hex(palette.background);
    let shell = palette.shell.map(hex).unwrap_or_else(|| bg.clone());
    let card = palette.card.map(hex).unwrap_or_else(|| bg.clone());
    let menu = Ink::parse(&card).mix(Ink::of(palette.raised), if dark { 0.34 } else { 0.02 });
    let surfaces = [bg.clone(), shell, card.clone(), menu];
    let code = [bg.clone(), card];
    let window = [bg.clone()];
    fix("text.primary", &hex(palette.text), &surfaces, 4.5);
    fix("text.secondary", &hex(palette.muted), &surfaces, 4.5);
    fix("text.tertiary", &hex(palette.faint), &surfaces, 3.0);
    let ansi = palette.ansi;
    let info = ansi.map(|a| a[if dark { 12 } else { 4 }]).unwrap_or(palette.accent);
    for (tone, c) in [("accent", palette.accent), ("danger", palette.danger), ("warning", palette.warning), ("success", palette.success), ("info", info)] {
        let filled = fix(tone, &hex(c), &window, 3.0);
        fix(&format!("{tone} ink"), &filled, &surfaces, 4.5);
    }
    const FIRST: [&str; 11] = ["comment", "keyword", "string", "number", "type", "function", "property", "variable", "punctuation", "tag", "attribute"];
    for (i, name) in FIRST.iter().enumerate() {
        let min = if i == 0 || i == 8 { 3.0 } else { 4.5 };
        fix(&format!("zeron.syntax.{name}"), &hex(palette.syntax[i]), &code, min);
    }
    if let Some(ansi) = ansi {
        for (name, i) in [("teal", 7), ("amber", 4), ("rose", 2), ("violet", 6), ("lime", 3), ("sky", 5)] {
            let c = ansi[if dark { i + 8 } else { i } - 1];
            fix(&format!("zeron.visual.series.{name}"), &hex(c), &window, 3.0);
        }
        // The terminal's sixteen, on the window's surfaces as its inks: 3:1
        // the two that are the background's own shade (black and bright
        // black on dark, white and bright white on light), else 4.5:1.
        for (n, c) in ansi.iter().enumerate() {
            let dim = if dark { n == 0 || n == 8 } else { n == 7 || n == 15 };
            fix(&format!("terminal.ansi.{n}"), &hex(*c), &surfaces, if dim { 3.0 } else { 4.5 });
        }
    }
    fixes
}

// ── Built in, templates' ────────────────────────────────────────────────────

/// One built-in theme: its id (a family's, or a variant's own), its name and
/// its variants (a light, a dark, or both).
#[derive(Clone, Debug)]
pub struct Builtin {
    pub id: String,
    pub name: String,
    pub variants: Vec<&'static ThemeVariant>,
}

impl Builtin {
    pub fn variant(&self, dark: bool) -> Option<&'static ThemeVariant> {
        self.variants.iter().copied().find(|v| v.appearance.is_dark() == dark)
    }
}

/// Every built-in theme, in the shipping app's order (Zeron's first): each
/// family's first light and first dark as one theme under the family's name,
/// and each variant after those as a theme of its own, under its own name.
pub fn builtin() -> Vec<Builtin> {
    let mut out = Vec::new();
    for family in &builtin_registry().families {
        let light = family.variants.iter().find(|v| !v.appearance.is_dark());
        let dark = family.variants.iter().find(|v| v.appearance.is_dark());
        let paired: Vec<&'static ThemeVariant> = [light, dark].into_iter().flatten().collect();
        out.push(Builtin { id: family.id.clone(), name: family.name.clone(), variants: paired.clone() });
        for v in &family.variants {
            if !paired.iter().any(|p| p.id == v.id) {
                out.push(Builtin { id: v.id.clone(), name: v.name.clone(), variants: vec![v] });
            }
        }
    }
    out
}

pub fn find_builtin(id: &str) -> Option<Builtin> {
    builtin().into_iter().find(|b| b.id == id)
}

/// Built-in theme `theme` chosen for every scheme it has.
pub fn choice_of(theme: &Builtin) -> Choice {
    Choice { light: theme.variant(false).map(|v| v.id.clone()), dark: theme.variant(true).map(|v| v.id.clone()) }
}

/// A template's own theme: its `theme.luau` read as data (`{ name?, light?,
/// dark? }`, each a palette), with the template's id and name.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateTheme {
    pub template: String,
    pub name: String,
    pub file: Json,
}

/// The id a template's theme is listed under.
pub fn template_entry(template: &str) -> String {
    format!("template-{template}")
}

fn template_variant(template: &str, scheme: &str) -> String {
    format!("template-{template}-{scheme}")
}

/// Each template's own theme in `templates` (`<data>/templates`): every
/// folder of one (not Zeron's own, `.zeron`, nor any hidden one) with a
/// manifest, its `theme.luau` read as data. One without a theme is listed
/// with a null `file`.
pub fn template_themes(templates: &Path) -> Vec<TemplateTheme> {
    let Ok(entries) = std::fs::read_dir(templates) else { return Vec::new() };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if id.starts_with('.') || !entry.file_type().is_ok_and(|k| k.is_dir()) {
            continue;
        }
        let dir = entry.path();
        let manifest: Json = read_small(&dir.join("zeron-template.json")).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        if !manifest.is_object() {
            continue;
        }
        let name = manifest["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or(&id).to_string();
        let file = read_small(&dir.join(TEMPLATE_THEME_FILE)).and_then(|t| crate::luau_data::parse(&t)).unwrap_or(Json::Null);
        out.push(TemplateTheme { template: id, name, file });
    }
    out.sort_by(|a, b| a.template.cmp(&b.template));
    out
}

/// The template the window runs now (`<data>/templates/.active`), if not
/// the default.
pub fn running_template(templates: &Path) -> Option<String> {
    let id = read_small(&templates.join(".active"))?;
    let id = id.trim();
    let ok = !id.is_empty()
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && templates.join(id).join("zeron-template.json").is_file();
    ok.then(|| id.to_string())
}

fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > crate::luau_data::MAX_BYTES as u64 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// A template's theme as listed: its palettes (each scheme its file has,
/// named as the theme and labelled as its scheme) and its entry. None for a
/// file with no palette.
fn template_theme(t: &TemplateTheme) -> Option<(Vec<Json>, Json)> {
    let name = t.file["name"].as_str().filter(|n| !n.trim().is_empty()).unwrap_or(&t.name).to_string();
    let entry = template_entry(&t.template);
    let mut palettes = Vec::new();
    for scheme in SCHEMES {
        let Some(fields) = t.file[scheme].as_object() else { continue };
        let mut p = Json::Object(fields.clone());
        p["id"] = json!(template_variant(&t.template, scheme));
        p["name"] = json!(name);
        p["kind"] = json!(scheme);
        p["entry"] = json!(entry);
        palettes.push(p);
    }
    if palettes.is_empty() {
        return None;
    }
    let variants: Vec<Json> = palettes.iter().map(|p| p["id"].clone()).collect();
    Some((palettes, json!({ "id": entry, "name": name, "source": "template", "template": t.template, "variants": variants })))
}

// ── The choice ──────────────────────────────────────────────────────────────

/// The theme chosen for each scheme: a variant's id, or none (the running
/// template's own theme for it, else Zeron's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Choice {
    pub light: Option<String>,
    pub dark: Option<String>,
}

impl Choice {
    pub fn is_empty(&self) -> bool {
        self.light.is_none() && self.dark.is_none()
    }

    pub fn slot(&mut self, scheme: &str) -> Option<&mut Option<String>> {
        match scheme {
            "light" => Some(&mut self.light),
            "dark" => Some(&mut self.dark),
            _ => None,
        }
    }

    pub fn get(&self, scheme: &str) -> Option<&str> {
        match scheme {
            "light" => self.light.as_deref(),
            "dark" => self.dark.as_deref(),
            _ => None,
        }
    }

    pub fn to_json(&self) -> Json {
        let mut out = Map::new();
        for scheme in SCHEMES {
            if let Some(id) = self.get(scheme) {
                out.insert(scheme.into(), json!(id));
            }
        }
        Json::Object(out)
    }
}

/// A variant's id as a choice keeps it: short, of the characters ids are
/// made of.
fn an_id(value: &str) -> Option<String> {
    let value = value.trim();
    let ok = !value.is_empty() && value.len() <= 128 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| value.to_string())
}

/// What's chosen in `data_dir` (none where nothing is, or the file is
/// unreadable).
pub fn chosen(data_dir: &Path) -> Choice {
    let file: Json = read_small(&data_dir.join(CHOICE_FILE)).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let pick = |scheme: &str| file[scheme].as_str().and_then(an_id);
    Choice { light: pick("light"), dark: pick("dark") }
}

pub fn save_choice(data_dir: &Path, choice: &Choice) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(&choice.to_json())?;
    write_atomically(&data_dir.join(CHOICE_FILE), &bytes).map_err(|e| anyhow!("Couldn't keep the theme: {e}"))
}

/// The slots `asked` names (`{ light?, dark? }`), changed in what's chosen:
/// each to a variant's id, or let go for "".
pub fn choose(data_dir: &Path, asked: &Json) -> Result<Choice> {
    let mut choice = chosen(data_dir);
    for scheme in SCHEMES {
        let Some(want) = asked.get(scheme) else { continue };
        let want = want.as_str().ok_or_else(|| anyhow!("A theme is chosen by its id"))?;
        let slot = choice.slot(scheme).expect("a scheme");
        *slot = if want.is_empty() { None } else { Some(an_id(want).ok_or_else(|| anyhow!("That isn't a theme"))?) };
    }
    save_choice(data_dir, &choice)?;
    Ok(choice)
}

/// `bytes` at `path` through a file beside it renamed over it, so nothing
/// reads half of it; nothing written when it holds them already.
fn write_atomically(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|now| now == bytes) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

// ── The list ────────────────────────────────────────────────────────────────

/// What the window is told: every theme (built in, the templates', then
/// yours) and its palettes, and what's chosen.
pub fn describe(library: &CustomThemeLibrary, templates: &[TemplateTheme], choice: &Choice, warning: Option<&str>) -> Json {
    let mut list = Vec::new();
    let mut entries = Vec::new();
    for b in builtin() {
        for v in &b.variants {
            list.push(variant_json(v, &b.id));
        }
        let variants: Vec<&str> = b.variants.iter().map(|v| v.id.as_str()).collect();
        entries.push(json!({ "id": b.id, "name": b.name, "source": "builtin", "variants": variants }));
    }
    for t in templates {
        if let Some((palettes, entry)) = template_theme(t) {
            list.extend(palettes);
            entries.push(entry);
        }
    }
    for entry in &library.entries {
        for v in &entry.family.variants {
            list.push(variant_json(v, &entry.id));
        }
        let status = match &entry.status {
            CustomThemeStatus::Ready => json!({ "ok": true }),
            CustomThemeStatus::Warning { message } => json!({ "ok": false, "message": message }),
        };
        entries.push(json!({
            "id": entry.id,
            "name": entry.name,
            "source": "imported",
            "status": status,
            "variants": entry.family.variants.iter().map(|v| v.id.clone()).collect::<Vec<_>>(),
        }));
    }
    let mut out = json!({ "state": "ready", "list": list, "entries": entries, "chosen": choice.to_json() });
    if let Some(warning) = warning {
        out["warning"] = json!(warning);
    }
    out
}

fn variant_json(v: &ThemeVariant, entry: &str) -> Json {
    let kind = if v.appearance.is_dark() { "dark" } else { "light" };
    Palette::of(v).to_json(Some((&v.id, &v.name, kind, entry)))
}

/// The themes in `data_dir` as the window lists them: [`describe`] over the
/// library there, the templates' own themes and what's chosen.
pub fn describe_dir(data_dir: &Path) -> Json {
    let (library, warning) = match CustomThemeLibrary::load(data_dir) {
        Ok(library) => (library, None),
        Err(error) => (CustomThemeLibrary::default(), Some(error.to_string())),
    };
    describe(&library, &template_themes(&data_dir.join(TEMPLATES_DIR)), &chosen(data_dir), warning.as_deref())
}

/// The words the tools say where a theme comes from.
fn source_said(source: &str) -> &'static str {
    match source {
        "builtin" => "built in",
        "template" => "a template's",
        _ => "yours",
    }
}

/// What each scheme wears now, by id: the theme chosen for it, else the
/// running template's own, else Zeron's.
fn worn(described: &Json, data_dir: &Path) -> BTreeMap<&'static str, (String, String)> {
    let entries = described["entries"].as_array().cloned().unwrap_or_default();
    let list = described["list"].as_array().cloned().unwrap_or_default();
    let kind_of = |id: &str| list.iter().find(|p| p["id"] == id).and_then(|p| p["kind"].as_str().map(str::to_string));
    let entry_of = |variant: &str| entries.iter().find(|e| e["variants"].as_array().is_some_and(|v| v.iter().any(|x| x == variant)));
    let running = running_template(&data_dir.join(TEMPLATES_DIR)).map(|t| template_entry(&t));
    let mut out = BTreeMap::new();
    for scheme in SCHEMES {
        let chosen = described["chosen"][scheme].as_str().filter(|id| kind_of(id).as_deref() == Some(scheme)).map(str::to_string);
        let suggested = running.as_ref().and_then(|e| entries.iter().find(|x| x["id"] == e.as_str())).and_then(|e| {
            e["variants"].as_array()?.iter().filter_map(Json::as_str).find(|v| kind_of(v).as_deref() == Some(scheme)).map(str::to_string)
        });
        let variant = chosen.or(suggested).unwrap_or_else(|| format!("zeron-{scheme}"));
        let theme = entry_of(&variant).and_then(|e| e["id"].as_str()).unwrap_or("zeron").to_string();
        out.insert(scheme, (theme, variant));
    }
    out
}

/// Every theme, for an agent: each one's id, name, where it comes from,
/// whether it can be changed, its variant for each scheme, and what each
/// scheme wears now.
pub fn list(data_dir: &Path) -> Json {
    let described = describe_dir(data_dir);
    let list = described["list"].as_array().cloned().unwrap_or_default();
    let kind_of = |id: &str| list.iter().find(|p| p["id"] == id).and_then(|p| p["kind"].as_str().map(str::to_string));
    let worn = worn(&described, data_dir);
    let mut themes = Vec::new();
    for e in described["entries"].as_array().into_iter().flatten() {
        let source = e["source"].as_str().unwrap_or("");
        let mut variants = Map::new();
        for v in e["variants"].as_array().into_iter().flatten().filter_map(Json::as_str) {
            if let Some(kind) = kind_of(v)
                && !variants.contains_key(&kind)
            {
                variants.insert(kind, json!(v));
            }
        }
        let in_use: Vec<&str> = SCHEMES.iter().copied().filter(|s| worn.get(s).is_some_and(|(t, _)| e["id"] == t.as_str())).collect();
        let mut theme = json!({
            "id": e["id"], "name": e["name"], "source": source_said(source), "editable": source == "imported",
            "variants": variants, "in_use": in_use,
        });
        if let Some(t) = e["template"].as_str() {
            theme["template"] = json!(t);
        }
        if e["status"]["ok"] == json!(false) {
            theme["problem"] = e["status"]["message"].clone();
        }
        themes.push(theme);
    }
    let wearing: Map<String, Json> = worn.iter().map(|(s, (t, v))| ((*s).to_string(), json!({ "theme": t, "variant": v }))).collect();
    let mut out = json!({ "themes": themes, "wearing": wearing, "chosen": described["chosen"] });
    if let Some(w) = described.get("warning") {
        out["warning"] = w.clone();
    }
    out
}

/// One theme's colours, by its id (a theme's or one of its variants'): its
/// palette for each scheme it has, in the shape [`create`] and [`update`]
/// take, whether it can be changed, and what the window moves to keep it
/// legible (`adjusted`).
pub fn get(data_dir: &Path, id: &str) -> Result<Json> {
    let described = describe_dir(data_dir);
    let entries = described["entries"].as_array().cloned().unwrap_or_default();
    let entry = entries
        .iter()
        .find(|e| e["id"] == id || e["variants"].as_array().is_some_and(|v| v.iter().any(|x| x == id)))
        .ok_or_else(|| anyhow!("There's no theme {id}. list_themes names them."))?;
    let source = entry["source"].as_str().unwrap_or("");
    let list = described["list"].as_array().cloned().unwrap_or_default();
    let mut out = json!({
        "id": entry["id"], "name": entry["name"], "source": source_said(source), "editable": source == "imported",
    });
    let mut adjusted = Vec::new();
    for v in entry["variants"].as_array().into_iter().flatten() {
        let Some(p) = list.iter().find(|p| &p["id"] == v) else { continue };
        let Some(kind) = p["kind"].as_str() else { continue };
        if out.get(kind).is_some() {
            continue;
        }
        let mut colours = p.clone();
        if let Some(map) = colours.as_object_mut() {
            for label in LABELS {
                map.remove(label);
            }
        }
        if let Ok(palette) = Palette::from_json(&colours, kind) {
            adjusted.extend(legibility(&palette, kind).into_iter().map(|f| scheme_fix(kind, f)));
        }
        out[kind] = colours;
    }
    out["adjusted"] = json!(adjusted);
    if source != "imported" {
        out["note"] = json!("Read-only. To change it, call create_theme with these colours and a new name.");
    }
    Ok(out)
}

fn scheme_fix(scheme: &str, f: Fix) -> Json {
    let mut j = f.to_json();
    j["scheme"] = json!(scheme);
    j
}

// ── Yours: made, changed, imported, removed ─────────────────────────────────

/// A name as a theme keeps it: trimmed, one line, not too long.
fn a_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        bail!("A theme needs a name.");
    }
    if name.chars().count() > MAX_NAME || name.chars().any(char::is_control) {
        bail!("A theme's name is one line of at most {MAX_NAME} characters.");
    }
    Ok(name.to_string())
}

/// `value` as an id: lowercase letters and digits, dashes between.
pub fn slug(value: &str) -> String {
    let mut out = String::new();
    let mut separator = false;
    for c in value.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            if separator && !out.is_empty() {
                out.push('-');
            }
            out.push(c);
            separator = false;
        } else {
            separator = true;
        }
    }
    if out.is_empty() { "theme".into() } else { out }
}

/// What saving a theme did: its id, its variants' ids by scheme, and what
/// the window moves to keep it legible.
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub id: String,
    pub name: String,
    pub variants: BTreeMap<String, String>,
    pub adjusted: Vec<Json>,
}

impl Saved {
    pub fn to_json(&self) -> Json {
        json!({ "id": self.id, "name": self.name, "variants": self.variants, "adjusted": self.adjusted })
    }
}

/// The palettes asked for, each scheme's read and checked: every problem
/// said at once. A scheme given null is `Some(None)` (taken away, in an
/// update).
type Asked = BTreeMap<&'static str, Option<Palette>>;

fn palettes_asked(light: Option<&Json>, dark: Option<&Json>) -> Result<Asked> {
    let mut out = BTreeMap::new();
    let mut errors = Vec::new();
    for (scheme, value) in [("light", light), ("dark", dark)] {
        match value {
            None => {}
            Some(Json::Null) => {
                out.insert(scheme, None);
            }
            Some(value) => match Palette::from_json(value, scheme) {
                Ok(p) => {
                    out.insert(scheme, Some(p));
                }
                Err(e) => errors.extend(e),
            },
        }
    }
    if !errors.is_empty() {
        bail!("{}", errors.join("\n"));
    }
    Ok(out)
}

fn appearance_of(scheme: &str) -> Appearance {
    if scheme == "dark" { Appearance::Dark } else { Appearance::Light }
}

fn saved(entry: &crate::CustomThemeEntry) -> Saved {
    let mut variants = BTreeMap::new();
    let mut adjusted = Vec::new();
    for v in &entry.family.variants {
        let scheme = if v.appearance.is_dark() { "dark" } else { "light" };
        if variants.contains_key(scheme) {
            continue;
        }
        variants.insert(scheme.to_string(), v.id.clone());
        adjusted.extend(legibility(&Palette::of(v), scheme).into_iter().map(|f| scheme_fix(scheme, f)));
    }
    Saved { id: entry.id.clone(), name: entry.name.clone(), variants, adjusted }
}

/// A variant's name in a theme: the theme's, alone; "Light" or "Dark" beside
/// another (the window says "Ink Light").
fn variant_name(theme: &str, scheme: &str, alone: bool) -> String {
    if alone { theme.to_string() } else if scheme == "dark" { "Dark".into() } else { "Light".into() }
}

/// A new theme of yours from palettes (`light`, `dark`: one at least).
pub fn create(data_dir: &Path, name: &str, light: Option<&Json>, dark: Option<&Json>) -> Result<Saved> {
    let name = a_name(name)?;
    let asked = palettes_asked(light, dark)?;
    let given: Vec<(&str, &Palette)> = asked.iter().filter_map(|(s, p)| p.as_ref().map(|p| (*s, p))).collect();
    if given.is_empty() {
        bail!("Give a palette for `light`, `dark` or both.");
    }
    let mut library = CustomThemeLibrary::load(data_dir)?;
    let family_id = format!("custom-{}", slug(&name));
    let variants = given
        .iter()
        .map(|(scheme, p)| p.variant(&family_id, &format!("{family_id}-{scheme}"), &variant_name(&name, scheme, given.len() == 1), appearance_of(scheme)))
        .collect();
    let id = library.install_made(ThemeFamily { id: family_id, name: name.clone(), variants })?;
    library.save(data_dir)?;
    Ok(saved(library.entry(&id).expect("just made")))
}

/// A theme of yours from a VS Code colour theme (its JSON), through the
/// shipping app's importer, named `name` or as the theme names itself.
pub fn create_from_vscode(data_dir: &Path, name: Option<&str>, theme: &Json) -> Result<Saved> {
    let text = match theme {
        Json::String(s) => s.clone(),
        Json::Object(_) => theme.to_string(),
        _ => bail!("`vscode` is a VS Code colour theme: its JSON, as an object or as text."),
    };
    if text.len() > MAX_VSCODE_BYTES {
        bail!("That VS Code theme is over {} KB.", MAX_VSCODE_BYTES / 1024);
    }
    let parsed: Json = json5::from_str(&text).map_err(|e| anyhow!("That isn't a VS Code theme's JSON: {e}"))?;
    if parsed.get("include").is_some() {
        bail!("A theme given inline can't `include` another file. Put its colours in this one.");
    }
    let name = a_name(name.or(parsed["name"].as_str()).unwrap_or("VS Code theme"))?;
    std::fs::create_dir_all(data_dir)?;
    let dir = data_dir.join(format!(".theme-import-{}-{:x}", std::process::id(), Sha256::digest(text.as_bytes())[0]));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join("theme.json");
    let made = (|| -> Result<Saved> {
        std::fs::write(&file, text.as_bytes())?;
        let compilation = CustomThemeLibrary::compile(&file, &format!("custom-{}", slug(&name)), &name)?;
        if !compilation.failures.is_empty() && compilation.family.variants.is_empty() {
            bail!("{}", compilation.failures.iter().map(|f| f.message.clone()).collect::<Vec<_>>().join("; "));
        }
        let mut library = CustomThemeLibrary::load(data_dir)?;
        let id = library.install(compilation, &[], InstallMode::Snapshot)?;
        if let Some(entry) = library.entries.iter_mut().find(|e| e.id == id) {
            // (Made from text, not a file of the person's.)
            entry.source = crate::CustomThemeSource::ImportedSnapshot { imported_from: None };
        }
        library.save(data_dir)?;
        Ok(saved(library.entry(&id).expect("just imported")))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    made
}

/// What an import adds: the entry's id, its variants' ids, and those that
/// didn't compile (a package's other variants still come in).
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub entry: String,
    pub variants: Vec<String>,
    pub skipped: Vec<String>,
}

/// A source's name, as the shipping app names it: the file's stem, or the
/// extension's folder for its `package.json`.
pub fn source_name(path: &Path) -> String {
    let path = if path.file_name().and_then(|n| n.to_str()) == Some("package.json") { path.parent().unwrap_or(path) } else { path };
    path.file_stem().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or("Custom theme").to_owned()
}

/// `path` (a VS Code theme's `.json`, or an extension's folder) compiled and
/// added to the library in `data_dir`, saved.
pub fn import(data_dir: &Path, path: &Path) -> Result<Imported> {
    let name = source_name(path);
    let compilation = CustomThemeLibrary::compile(path, &format!("custom-{}", slug(&name)), &name)?;
    let skipped: Vec<String> = compilation.failures.iter().map(|f| format!("{}: {}", f.name, f.message)).collect();
    let mut library = CustomThemeLibrary::load(data_dir)?;
    let entry = library.install(compilation, &[], InstallMode::Snapshot)?;
    library.save(data_dir)?;
    let variants = library.entry(&entry).map(|e| e.family.variants.iter().map(|v| v.id.clone()).collect()).unwrap_or_default();
    Ok(Imported { entry, variants, skipped })
}

/// Refused for a theme that isn't yours.
fn not_yours(data_dir: &Path, id: &str) -> anyhow::Error {
    let described = describe_dir(data_dir);
    let known = described["entries"].as_array().into_iter().flatten().find(|e| e["id"] == id);
    match known.and_then(|e| e["source"].as_str()) {
        Some("builtin") => anyhow!("{id} is built in, so it can't be changed. Make a copy: get_theme {id}, then create_theme with its colours and a new name."),
        Some("template") => anyhow!("{id} is a template's own theme, so it can't be changed here. Make a copy: get_theme {id}, then create_theme with its colours and a new name."),
        _ => anyhow!("There's no theme of yours {id}. list_themes names them."),
    }
}

/// Theme `id` of yours changed: renamed (`name`), and a scheme's palette
/// given (`light`, `dark`) or taken away (null). Its ids stay, so a theme
/// in use restyles where it's worn.
pub fn update(data_dir: &Path, id: &str, name: Option<&str>, light: Option<&Json>, dark: Option<&Json>) -> Result<Saved> {
    let asked = palettes_asked(light, dark)?;
    let name = name.map(a_name).transpose()?;
    let mut library = CustomThemeLibrary::load(data_dir)?;
    let Some(entry) = library.entries.iter_mut().find(|e| e.id == id) else { return Err(not_yours(data_dir, id)) };
    if let Some(name) = &name {
        entry.name = name.clone();
        entry.family.name = name.clone();
    }
    let theme = entry.name.clone();
    for (scheme, palette) in &asked {
        let dark = *scheme == "dark";
        let at = entry.family.variants.iter().position(|v| v.appearance.is_dark() == dark);
        match (palette, at) {
            (None, Some(i)) => {
                let gone = entry.family.variants.remove(i);
                entry.reports.remove(&gone.id);
                entry.selected_variant_ids.retain(|v| *v != gone.id);
            }
            (None, None) => {}
            (Some(p), Some(i)) => {
                let old = &entry.family.variants[i];
                let made = p.variant(&entry.family.id, &old.id.clone(), &old.name.clone(), appearance_of(scheme));
                entry.reports.remove(&made.id);
                entry.family.variants[i] = made;
            }
            (Some(p), None) => {
                let base = format!("{}-{scheme}", entry.family.id);
                let mut vid = base.clone();
                let mut n = 2;
                while entry.family.variants.iter().any(|v| v.id == vid) {
                    vid = format!("{base}-{n}");
                    n += 1;
                }
                entry.family.variants.push(p.variant(&entry.family.id, &vid, "", appearance_of(scheme)));
                entry.selected_variant_ids.push(vid);
            }
        }
    }
    if entry.family.variants.is_empty() {
        bail!("A theme keeps a palette for light, dark or both: this would leave it with none.");
    }
    // Names follow: one variant alone takes the theme's; two say which.
    let alone = entry.family.variants.len() == 1;
    let made_here = |v: &ThemeVariant| v.source.format == "zeron-palette";
    for v in &mut entry.family.variants {
        let scheme = if v.appearance.is_dark() { "dark" } else { "light" };
        if alone || made_here(v) || v.name.is_empty() {
            v.name = variant_name(&theme, scheme, alone);
        }
    }
    entry.family.variants.sort_by_key(|v| v.appearance.is_dark());
    entry.status = CustomThemeStatus::Ready;
    let out = saved(entry);
    library.save(data_dir)?;
    // A scheme taken away: a slot it filled goes back.
    let kept: Vec<String> = out.variants.values().cloned().collect();
    let gone: Vec<String> = asked.iter().filter(|(_, p)| p.is_none()).map(|(s, _)| format!("{id}-{s}")).collect();
    if !gone.is_empty() {
        let mut choice = chosen(data_dir);
        for scheme in SCHEMES {
            let slot = choice.slot(scheme).expect("a scheme");
            if slot.as_ref().is_some_and(|v| v.starts_with(id) && !kept.contains(v)) {
                *slot = None;
            }
        }
        save_choice(data_dir, &choice)?;
    }
    Ok(out)
}

/// Theme `id` of yours gone, and any slot one of its variants filled let go:
/// whether it was there.
pub fn remove(data_dir: &Path, id: &str) -> Result<bool> {
    let mut library = CustomThemeLibrary::load(data_dir)?;
    let variants: Vec<String> = library.entry(id).map(|e| e.family.variants.iter().map(|v| v.id.clone()).collect()).unwrap_or_default();
    let removed = library.remove(id);
    if removed {
        library.save(data_dir)?;
        let mut choice = chosen(data_dir);
        let before = choice.clone();
        for scheme in SCHEMES {
            let slot = choice.slot(scheme).expect("a scheme");
            if slot.as_ref().is_some_and(|v| variants.contains(v)) {
                *slot = None;
            }
        }
        if choice != before {
            save_choice(data_dir, &choice)?;
        }
    }
    Ok(removed)
}

/// [`remove`], for a tool: refused, saying why, for one that isn't yours.
pub fn delete(data_dir: &Path, id: &str) -> Result<()> {
    if remove(data_dir, id)? { Ok(()) } else { Err(not_yours(data_dir, id)) }
}

/// Theme `id` of yours named `name`: its family, and a variant alone in it,
/// take the name too.
pub fn rename(data_dir: &Path, id: &str, name: &str) -> Result<()> {
    let name = a_name(name)?;
    let mut library = CustomThemeLibrary::load(data_dir)?;
    let entry = library.entries.iter_mut().find(|e| e.id == id).ok_or_else(|| anyhow!("There's no imported theme {id}"))?;
    entry.name = name.clone();
    entry.family.name = name.clone();
    if let [only] = entry.family.variants.as_mut_slice() {
        only.name = name;
    }
    library.save(data_dir)
}

/// Theme `id` (any theme's, or one of its variants') worn for `slot`:
/// "light", "dark", or "both" (each scheme it has). What's chosen after.
pub fn use_theme(data_dir: &Path, id: &str, slot: &str) -> Result<Json> {
    let described = describe_dir(data_dir);
    let list = described["list"].as_array().cloned().unwrap_or_default();
    let kind_of = |v: &str| list.iter().find(|p| p["id"] == v).and_then(|p| p["kind"].as_str().map(str::to_string));
    let entry = described["entries"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|e| e["id"] == id || e["variants"].as_array().is_some_and(|v| v.iter().any(|x| x == id)))
        .ok_or_else(|| anyhow!("There's no theme {id}. list_themes names them."))?;
    let name = entry["name"].as_str().unwrap_or(id);
    let variants: Vec<String> = if entry["id"] == id {
        entry["variants"].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_string)).collect()
    } else {
        vec![id.to_string()]
    };
    let of = |scheme: &str| variants.iter().find(|v| kind_of(v).as_deref() == Some(scheme)).cloned();
    let schemes: Vec<&str> = match slot {
        "both" | "" => SCHEMES.to_vec(),
        "light" | "dark" => vec![slot],
        other => bail!("`slot` is \"light\", \"dark\" or \"both\", not {other:?}."),
    };
    let mut asked = Map::new();
    for scheme in &schemes {
        match of(scheme) {
            Some(v) => {
                asked.insert((*scheme).into(), json!(v));
            }
            None if slot == "light" || slot == "dark" => {
                let other = if *scheme == "dark" { "light" } else { "dark" };
                bail!("{name} has no {scheme} version. Use it for {other}, or pick another theme for {scheme}.");
            }
            None => {}
        }
    }
    if asked.is_empty() {
        bail!("{name} has no palette to wear.");
    }
    let choice = choose(data_dir, &Json::Object(asked.clone()))?;
    Ok(json!({ "theme": entry["id"], "name": name, "worn_for": asked.keys().collect::<Vec<_>>(), "chosen": choice.to_json() }))
}

/// Where a theme's library lives, for the window's watch.
pub fn library_path(data_dir: &Path) -> PathBuf {
    CustomThemeLibrary::path(data_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn palette(seed: &str) -> Json {
        let v = builtin_registry().families.iter().flat_map(|f| &f.variants).find(|v| v.id == seed).unwrap();
        Palette::of(v).to_json(None)
    }

    #[test]
    fn a_palette_reads_back_as_it_was_written() {
        let given = palette("gruvbox-dark");
        let p = Palette::from_json(&given, "dark").unwrap();
        assert_eq!(p.to_json(None), given);
        let v = p.variant("custom-x", "custom-x-dark", "X", Appearance::Dark);
        assert_eq!(Palette::of(&v), p, "the variant it makes reads back the same");
        // The fewest fields: the rest grown from them, and stable after.
        let mut least = given.clone();
        for k in ["shell", "card", "terminal", "ansi"] {
            least.as_object_mut().unwrap().remove(k);
        }
        let p = Palette::from_json(&least, "dark").unwrap();
        let back = Palette::of(&p.variant("custom-x", "custom-x-dark", "X", Appearance::Dark));
        assert_eq!(back.background, p.background);
        assert_eq!(back.shell, Some(p.background));
        assert_eq!(Palette::of(&back.variant("custom-x", "custom-x-dark", "X", Appearance::Dark)), back);
    }

    #[test]
    fn a_bad_palette_says_every_problem() {
        let mut bad = palette("nord");
        bad["text"] = json!("blue");
        bad["accent"] = json!(3);
        bad.as_object_mut().unwrap().remove("muted");
        bad["syntax"] = json!(["#ffffff"]);
        bad["colour"] = json!("#000000");
        bad["ansi"][2] = json!("nope");
        let errors = Palette::from_json(&bad, "dark").unwrap_err();
        let all = errors.join("\n");
        for said in [
            "`dark.text`: \"blue\" isn't a colour",
            "`dark.accent`: 3 isn't a colour",
            "`dark.muted` is missing",
            "`dark.syntax` needs 12 colours, in this order: comment, keyword",
            "`dark.colour` isn't part of a palette",
            "`dark.ansi[3]` (green)",
        ] {
            assert!(all.contains(said), "{said} in:\n{all}");
        }
        assert!(Palette::from_json(&json!("#000"), "light").unwrap_err()[0].contains("must be a palette"));
        // What a palette read back carries is fine.
        let mut labelled = palette("nord");
        labelled["id"] = json!("nord");
        labelled["kind"] = json!("dark");
        assert!(Palette::from_json(&labelled, "dark").is_ok());
    }

    #[test]
    fn made_got_changed_used_and_deleted() {
        let d = data();
        let dir = d.path();
        let made = create(dir, "Ink", Some(&palette("github-light")), Some(&palette("tokyo-night"))).unwrap();
        assert_eq!(made.id, "custom-ink");
        assert_eq!(made.variants["light"], "custom-ink-light");
        assert_eq!(made.variants["dark"], "custom-ink-dark");
        let got = get(dir, "custom-ink").unwrap();
        assert_eq!((got["editable"].as_bool(), got["source"].as_str()), (Some(true), Some("yours")));
        assert_eq!(got["dark"], palette("tokyo-night"), "colours as given");
        // Listed, under Yours.
        let listed = list(dir);
        let ink = listed["themes"].as_array().unwrap().iter().find(|t| t["id"] == "custom-ink").unwrap();
        assert_eq!(ink["variants"], json!({ "light": "custom-ink-light", "dark": "custom-ink-dark" }));
        assert_eq!(listed["wearing"]["dark"], json!({ "theme": "zeron", "variant": "zeron-dark" }));
        // Worn for dark; its colours changed: the same id, so it restyles.
        let used = use_theme(dir, "custom-ink", "dark").unwrap();
        assert_eq!(used["chosen"], json!({ "dark": "custom-ink-dark" }));
        let mut changed = got["dark"].clone();
        changed["accent"] = json!("#ff00aa");
        let saved = update(dir, "custom-ink", Some("Ink Two"), None, Some(&changed)).unwrap();
        assert_eq!(saved.variants["dark"], "custom-ink-dark");
        assert_eq!(get(dir, "custom-ink").unwrap()["dark"]["accent"], "#ff00aa");
        assert_eq!(get(dir, "custom-ink").unwrap()["name"], "Ink Two");
        assert_eq!(chosen(dir).dark.as_deref(), Some("custom-ink-dark"));
        let listed = list(dir);
        assert_eq!(listed["wearing"]["dark"], json!({ "theme": "custom-ink", "variant": "custom-ink-dark" }));
        // Its light taken away; its dark stays; then it goes, and so does dark's choice.
        update(dir, "custom-ink", None, Some(&Json::Null), None).unwrap();
        assert!(get(dir, "custom-ink").unwrap().get("light").is_none());
        assert!(update(dir, "custom-ink", None, None, Some(&Json::Null)).is_err(), "never none");
        delete(dir, "custom-ink").unwrap();
        assert!(chosen(dir).is_empty());
        assert!(delete(dir, "custom-ink").is_err());
    }

    #[test]
    fn built_in_and_template_themes_are_read_only_but_can_be_copied() {
        let d = data();
        let dir = d.path();
        let err = update(dir, "gruvbox", None, Some(&palette("nord")), None).unwrap_err().to_string();
        assert!(err.contains("built in") && err.contains("create_theme"), "{err}");
        assert!(delete(dir, "dracula").unwrap_err().to_string().contains("built in"));
        // A template's own theme.
        let t = dir.join("templates/ledger");
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join("zeron-template.json"), r#"{"name":"Ledger"}"#).unwrap();
        std::fs::write(t.join("theme.luau"), "return { name = \"Ledger Ink\", dark = { background = \"#2e3440\" } }").unwrap();
        std::fs::write(dir.join("templates/.active"), "ledger").unwrap();
        assert!(delete(dir, "template-ledger").unwrap_err().to_string().contains("template's own"));
        let listed = list(dir);
        assert_eq!(listed["wearing"]["dark"]["theme"], "template-ledger", "nothing chosen: the running template's");
        // A copy from a built-in's colours is yours.
        let dracula = get(dir, "dracula").unwrap();
        assert_eq!(dracula["editable"], false);
        assert!(dracula.get("light").is_none() && dracula["note"].as_str().unwrap().contains("create_theme"));
        let copy = create(dir, "My Dracula", None, Some(&dracula["dark"])).unwrap();
        assert_eq!(get(dir, &copy.id).unwrap()["dark"], dracula["dark"]);
        assert_eq!(copy.adjusted, get(dir, "dracula").unwrap()["adjusted"].as_array().unwrap().clone());
    }

    #[test]
    fn use_says_which_schemes_a_theme_has() {
        let d = data();
        let dir = d.path();
        let both = use_theme(dir, "gruvbox", "both").unwrap();
        assert_eq!(both["chosen"], json!({ "light": "gruvbox-light", "dark": "gruvbox-dark" }));
        let err = use_theme(dir, "dracula", "light").unwrap_err().to_string();
        assert!(err.contains("no light version"), "{err}");
        use_theme(dir, "dracula", "both").unwrap();
        assert_eq!(chosen(dir), Choice { light: Some("gruvbox-light".into()), dark: Some("dracula".into()) });
        use_theme(dir, "github-light", "both").unwrap();
        assert_eq!(chosen(dir).light.as_deref(), Some("github-light"), "a variant's id works too");
        assert!(use_theme(dir, "gruvbox", "sideways").is_err());
        assert!(use_theme(dir, "nope", "both").is_err());
    }

    #[test]
    fn an_illegible_ink_is_said_as_the_window_moves_it() {
        let mut faint = palette("github-light");
        faint["text"] = json!("#eeeeee");
        let p = Palette::from_json(&faint, "light").unwrap();
        let fixes = legibility(&p, "light");
        let text = fixes.iter().find(|f| f.role == "text.primary").unwrap();
        assert_eq!(text.from, "#eeeeee");
        assert!(contrast(&text.to, "#ffffff") >= 4.5);
        let saved = create(data().path(), "Faint", Some(&faint), None).unwrap();
        assert!(saved.adjusted.iter().any(|f| f["role"] == "text.primary" && f["scheme"] == "light"));
    }

    #[test]
    fn a_vs_code_theme_is_taken_inline() {
        let d = data();
        let dir = d.path();
        let theme = json!({
            "name": "Harbor Night", "type": "dark",
            "colors": { "editor.background": "#14181f", "editor.foreground": "#d8dee9", "focusBorder": "#7aa2f7" },
            "tokenColors": [{ "scope": "comment", "settings": { "foreground": "#5c6773" } }],
        });
        let saved = create_from_vscode(dir, None, &theme).unwrap();
        assert_eq!(saved.name, "Harbor Night");
        assert!(saved.variants.contains_key("dark"));
        assert_eq!(get(dir, &saved.id).unwrap()["dark"]["background"], "#14181f");
        assert!(create_from_vscode(dir, None, &json!({ "include": "./other.json" })).is_err());
        assert!(create_from_vscode(dir, None, &json!(3)).is_err());
        assert!(std::fs::read_dir(dir).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with(".theme-import")), "nothing left behind");
    }

    #[test]
    fn names_and_choices_are_checked() {
        let d = data();
        let dir = d.path();
        assert!(create(dir, "  ", Some(&palette("nord")), None).is_err());
        assert!(create(dir, &"x".repeat(81), Some(&palette("nord")), None).is_err());
        assert!(create(dir, "Line\nbreak", Some(&palette("nord")), None).is_err());
        assert!(create(dir, "None", None, None).is_err());
        assert!(choose(dir, &json!({ "dark": "../../etc" })).is_err());
        std::fs::write(dir.join(CHOICE_FILE), "not json").unwrap();
        assert!(chosen(dir).is_empty());
        assert_eq!(slug("Harbor Night!"), "harbor-night");
        assert_eq!(source_name(Path::new("/x/my-ext/package.json")), "my-ext");
    }

    #[test]
    fn every_built_in_family_is_listed_light_and_dark_together_and_one_variant_alone() {
        let all = builtin();
        assert_eq!(all[0].id, "zeron");
        let ayu = find_builtin("ayu").unwrap();
        assert_eq!(ayu.variants.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["ayu-light", "ayu-dark"]);
        assert_eq!(find_builtin("ayu-mirage").unwrap().variants.len(), 1);
        let variants: usize = builtin_registry().families.iter().map(|f| f.variants.len()).sum();
        assert_eq!(all.iter().map(|b| b.variants.len()).sum::<usize>(), variants);
        let described = describe(&CustomThemeLibrary::default(), &[], &Choice::default(), None);
        assert_eq!(described["list"].as_array().unwrap().len(), variants);
    }
}
