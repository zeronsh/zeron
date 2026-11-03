//! Themes: the colours Zeron wears, chosen apart from the template it runs
//! (docs/templates.md, "Themes"). A theme is data, a palette for light, for
//! dark or both, in the shape `ds/palettes` dresses a window from, so it
//! needs no consent and runs no code. Three kinds are listed:
//!
//! - Zeron's built-in themes, the shipping app's (`zeron_theme::builtin_registry`,
//!   crates/theme builtins.rs): each family's first light and first dark as
//!   one theme, any other variant as one of its own (Ayu Mirage);
//! - a template's own `theme.luau`, read as data (src/plugins/looks.rs), told
//!   here by the templates' bridge as the templates change (`set_templates`);
//! - your imported VS Code colour themes (a `.json` file, comments allowed, or
//!   an extension's folder), compiled and kept by the shipping app's theme
//!   crate (its importer, light-or-dark detection, library file and error
//!   words).
//!
//! What you chose is one theme for light and one for dark (`Choice`), kept in
//! `<data>/theme-choice.json`. A slot left unchosen wears the running
//! template's own theme for it, else Zeron's.
//!
//! ```text
//! shared  themes.library   { state, list = { Palette… }, entries = { Entry… }, chosen = { light?, dark? }, warning? }
//!         Entry            { id, name, source = "builtin" | "template" | "imported", variants = { id… },
//!                            template? (its id), status? = { ok, message? } }
//! call    themes.import    path → { entry, variants = { id… }, skipped = { "name: why"… } }, or its error
//! invoke  themes.remove    entry id (a slot it fills is let go)
//! call    themes.rename    { id, name }
//! call    themes.choose    { light?, dark? }: each slot named takes that theme's id, or "" to let it go
//! ```
//!
//! The library is `<data>/theme-library.json` (the engine's data dir), as the
//! shipping app keeps its own: this device's, not synced. Imports are
//! snapshots: the file can move or go afterwards.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{Value as Json, json};
use zeron_theme::{CustomThemeLibrary, CustomThemeStatus, InstallMode, ThemeVariant, builtin_registry};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::plugins::json_value;

/// The syntax colours a palette carries, in `ds/palettes`' order.
const SYNTAX: [&str; 12] = [
    "comment", "keyword", "string", "number", "type", "function", "property", "variable", "punctuation", "tag", "attribute", "invalid",
];

/// The schemes a theme can have, in the order its variants are listed.
pub const SCHEMES: [&str; 2] = ["light", "dark"];

/// Where the choice is kept, in the engine's data dir.
pub const CHOICE: &str = "theme-choice.json";

/// A variant as a palette (`ds/palettes`): its surfaces (the window, its
/// chrome, raised and card), its three inks, its accent and tones, its
/// code's twelve colours, and its terminal's (background and the sixteen
/// ANSI colours), with the theme it's listed under (`entry`).
pub fn palette(variant: &ThemeVariant, entry: &str) -> Json {
    let c = &variant.colors;
    let hex = |color: zeron_theme::Color| color.to_string();
    let syntax: Vec<String> = SYNTAX
        .iter()
        .map(|name| variant.syntax.get(*name).map(|color| hex(*color)).unwrap_or_else(|| hex(c.text)))
        .collect();
    json!({
        "id": variant.id,
        "name": variant.name,
        "kind": if variant.appearance.is_dark() { "dark" } else { "light" },
        "entry": entry,
        "background": hex(c.background),
        "shell": hex(c.shell),
        "raised": hex(c.raised),
        "card": hex(c.card),
        "text": hex(c.text),
        "muted": hex(c.text_muted),
        "faint": hex(c.text_faint),
        "accent": hex(variant.accent.primary),
        "danger": hex(c.danger),
        "warning": hex(c.warning),
        "success": hex(c.success),
        "syntax": syntax,
        "terminal": hex(variant.terminal.background),
        "ansi": variant.terminal.ansi.iter().map(|color| hex(*color)).collect::<Vec<_>>(),
    })
}

// ── Built in ────────────────────────────────────────────────────────────────

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

/// The built-in theme `id`.
pub fn find_builtin(id: &str) -> Option<Builtin> {
    builtin().into_iter().find(|b| b.id == id)
}

/// Built-in theme `id` chosen for every scheme it has.
pub fn choice_of(theme: &Builtin) -> Choice {
    Choice { light: theme.variant(false).map(|v| v.id.clone()), dark: theme.variant(true).map(|v| v.id.clone()) }
}

// ── Templates' ──────────────────────────────────────────────────────────────

/// A template's own theme: its `theme.luau` read as data (`{ name?, light?,
/// dark? }`, each a palette), with the template's id and name.
#[derive(Clone, Debug, PartialEq)]
pub struct TemplateTheme {
    pub template: String,
    pub name: String,
    pub file: Json,
}

/// The id a template's theme is listed under, and its variant's for `scheme`.
pub fn template_entry(template: &str) -> String {
    format!("template-{template}")
}

fn template_variant(template: &str, scheme: &str) -> String {
    format!("template-{template}-{scheme}")
}

/// A template's theme as the page lists it: its palettes (each scheme its
/// file has, named as the theme and marked as its kind) and its entry. None
/// for a file with no palette.
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

/// The theme chosen for each scheme: a theme variant's id, or none (the
/// running template's own theme for it, else Zeron's).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Choice {
    pub light: Option<String>,
    pub dark: Option<String>,
}

impl Choice {
    pub fn is_empty(&self) -> bool {
        self.light.is_none() && self.dark.is_none()
    }

    fn slot(&mut self, scheme: &str) -> Option<&mut Option<String>> {
        match scheme {
            "light" => Some(&mut self.light),
            "dark" => Some(&mut self.dark),
            _ => None,
        }
    }

    fn to_json(&self) -> Json {
        let mut out = serde_json::Map::new();
        for (scheme, id) in [("light", &self.light), ("dark", &self.dark)] {
            if let Some(id) = id {
                out.insert(scheme.into(), json!(id));
            }
        }
        Json::Object(out)
    }
}

/// A theme variant's id as a choice keeps it: short, of the characters ids
/// are made of.
fn an_id(value: &str) -> Option<String> {
    let value = value.trim();
    let ok = !value.is_empty() && value.len() <= 128 && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then(|| value.to_string())
}

/// What's chosen in `data_dir` (none where nothing is, or the file is
/// unreadable).
pub fn chosen(data_dir: &Path) -> Choice {
    let file: Json = std::fs::read(data_dir.join(CHOICE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let pick = |scheme: &str| file[scheme].as_str().and_then(an_id);
    Choice { light: pick("light"), dark: pick("dark") }
}

pub fn save_choice(data_dir: &Path, choice: &Choice) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(&choice.to_json()).unwrap_or_default();
    crate::plugins::live::put(&data_dir.join(CHOICE), &bytes).map(|_| ()).map_err(|e| format!("Couldn't keep the theme: {e}"))
}

/// The slots `asked` names, changed in what's chosen in `data_dir`: each to
/// a theme's id, or let go for "".
pub fn choose(data_dir: &Path, asked: &Json) -> Result<Choice, String> {
    let mut choice = chosen(data_dir);
    for scheme in SCHEMES {
        let Some(want) = asked.get(scheme) else { continue };
        let want = want.as_str().ok_or("A theme is chosen by its id")?;
        let slot = choice.slot(scheme).expect("a scheme");
        *slot = if want.is_empty() { None } else { Some(an_id(want).ok_or("That isn't a theme")?) };
    }
    save_choice(data_dir, &choice)?;
    Ok(choice)
}

// ── The library ─────────────────────────────────────────────────────────────

/// What the scripts are told: every theme (built in, the templates', then
/// imported) and its palettes, and what's chosen.
pub fn describe(library: &CustomThemeLibrary, templates: &[TemplateTheme], choice: &Choice, warning: Option<&str>) -> Json {
    let mut list = Vec::new();
    let mut entries = Vec::new();
    for b in builtin() {
        list.extend(b.variants.iter().map(|v| palette(v, &b.id)));
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
        for variant in &entry.family.variants {
            list.push(palette(variant, &entry.id));
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

/// A source's name, as the shipping app names it: the file's stem, or the
/// extension's folder for its `package.json`.
pub fn source_name(path: &Path) -> String {
    let path = if path.file_name().and_then(|n| n.to_str()) == Some("package.json") { path.parent().unwrap_or(path) } else { path };
    path.file_stem().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or("Custom theme").to_owned()
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

/// What an import adds: the entry's id, its variants' ids, and those that
/// didn't compile (a package's other variants still come in).
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub entry: String,
    pub variants: Vec<String>,
    pub skipped: Vec<String>,
}

/// `path` compiled and added to the library in `data_dir`, saved. The error
/// is the shipping app's words for what's wrong ("could not parse JSONC
/// theme …").
pub fn import(data_dir: &Path, path: &Path) -> Result<Imported, String> {
    let name = source_name(path);
    let compilation = CustomThemeLibrary::compile(path, &format!("custom-{}", slug(&name)), &name).map_err(|e| e.to_string())?;
    let skipped: Vec<String> = compilation.failures.iter().map(|f| format!("{}: {}", f.name, f.message)).collect();
    let mut library = CustomThemeLibrary::load(data_dir).map_err(|e| e.to_string())?;
    let entry = library.install(compilation, &[], InstallMode::Snapshot).map_err(|e| e.to_string())?;
    library.save(data_dir).map_err(|e| e.to_string())?;
    let variants = library.entry(&entry).map(|e| e.family.variants.iter().map(|v| v.id.clone()).collect()).unwrap_or_default();
    Ok(Imported { entry, variants, skipped })
}

/// The entry `id` gone from the library, saved, and any slot one of its
/// variants filled let go: whether it was there.
pub fn remove(data_dir: &Path, id: &str) -> Result<bool, String> {
    let mut library = CustomThemeLibrary::load(data_dir).map_err(|e| e.to_string())?;
    let variants: Vec<String> = library.entry(id).map(|e| e.family.variants.iter().map(|v| v.id.clone()).collect()).unwrap_or_default();
    let removed = library.remove(id);
    if removed {
        library.save(data_dir).map_err(|e| e.to_string())?;
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

/// The entry `id` named `name`: its family, and a variant alone in it, take
/// the name too (a package's variants keep theirs: "Dark", "Light").
pub fn rename(data_dir: &Path, id: &str, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("A theme needs a name".into());
    }
    let mut library = CustomThemeLibrary::load(data_dir).map_err(|e| e.to_string())?;
    let entry = library.entries.iter_mut().find(|e| e.id == id).ok_or_else(|| format!("There's no imported theme {id}"))?;
    entry.name = name.to_owned();
    entry.family.name = name.to_owned();
    if let [only] = entry.family.variants.as_mut_slice() {
        only.name = name.to_owned();
    }
    library.save(data_dir).map_err(|e| e.to_string())
}

// ── The host ────────────────────────────────────────────────────────────────

/// `work` on a thread of its own, its result once done (none if it panicked):
/// compiling a theme reads and parses files.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tell, mut heard) = zgx::task::latest::<T>();
    std::thread::spawn(move || {
        tell.send(work());
    });
    heard.recv().await
}

fn data_dir() -> Option<PathBuf> {
    crate::engine::data_dir()
}

thread_local! {
    /// The main window's library, once installed.
    static LIBRARY: RefCell<Option<Rc<Shared>>> = const { RefCell::new(None) };
    /// The templates' own themes, as their bridge last told them.
    static TEMPLATES: RefCell<Vec<TemplateTheme>> = const { RefCell::new(Vec::new()) };
}

/// The library as the scripts see it, read again from disk.
fn publish() {
    let Some(shared) = LIBRARY.with(|l| l.borrow().clone()) else { return };
    let Some(dir) = data_dir() else { return };
    let (library, warning) = match CustomThemeLibrary::load(&dir) {
        Ok(library) => (library, None),
        Err(error) => (CustomThemeLibrary::default(), Some(error.to_string())),
    };
    let described = TEMPLATES.with(|t| describe(&library, &t.borrow(), &chosen(&dir), warning.as_deref()));
    shared.set(json_value(&described));
}

/// The templates' own themes now (the templates' bridge, as they change):
/// listed again where they changed.
pub fn set_templates(themes: Vec<TemplateTheme>) {
    let changed = TEMPLATES.with(|t| {
        let mut t = t.borrow_mut();
        let changed = *t != themes;
        *t = themes;
        changed
    });
    if changed {
        publish();
    }
}

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    let shared = Rc::new(host.shared("themes.library", Value::record([("state", Value::from("loading"))])));
    LIBRARY.with(|l| *l.borrow_mut() = Some(shared));
    publish();
    host.async_action("themes.import", move |arg| async move {
        let dir = data_dir().ok_or("Themes aren't available right now")?;
        let path = PathBuf::from(text(&arg));
        if path.as_os_str().is_empty() {
            return Err("Choose a theme file or an extension's folder.".into());
        }
        let imported = off_thread(move || import(&dir, &path)).await.ok_or("The import stopped")??;
        publish();
        Ok(json_value(&json!({ "entry": imported.entry, "variants": imported.variants, "skipped": imported.skipped })))
    });
    host.action("themes.remove", move |arg| {
        let Some(dir) = data_dir() else { return };
        if let Err(error) = remove(&dir, &text(&arg)) {
            eprintln!("zeron-zgx: themes: {error}");
        }
        publish();
    });
    host.async_action("themes.rename", move |arg| async move {
        let dir = data_dir().ok_or("Themes aren't available right now")?;
        let id = arg.field("id").map(text).unwrap_or_default();
        let name = arg.field("name").map(text).unwrap_or_default();
        rename(&dir, &id, &name)?;
        publish();
        Ok(Value::Nil)
    });
    host.async_action("themes.choose", move |arg| async move {
        let dir = data_dir().ok_or("Themes aren't available right now")?;
        choose(&dir, &crate::plugins::value_json(&arg))?;
        publish();
        Ok(Value::Nil)
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/tests/fixtures/themes").join(name)
    }

    fn data(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-themes-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn library(dir: &Path) -> CustomThemeLibrary {
        CustomThemeLibrary::load(dir).unwrap()
    }

    #[test]
    fn a_vs_code_theme_with_comments_imports_as_a_palette() {
        let dir = data("import");
        let imported = import(&dir, &fixture("Harbor Night.json")).unwrap();
        assert_eq!(imported.variants.len(), 1, "{imported:?}");
        let described = describe(&library(&dir), &[], &Choice::default(), None);
        let p = described["list"].as_array().unwrap().iter().find(|p| p["entry"] == imported.entry).unwrap();
        assert_eq!(p["kind"], "dark", "type: dark");
        assert_eq!(p["name"], "Harbor Night");
        // The editor's own colours, as the shipping app maps them.
        assert_eq!(p["background"], "#14181f");
        assert_eq!(p["text"], "#d8dee9");
        assert_eq!(p["syntax"].as_array().unwrap().len(), 12);
        assert_eq!(p["syntax"][0], "#5c6773", "comments, from tokenColors");
        assert_eq!(p["syntax"][1], "#c792ea", "keywords");
        let entry = described["entries"].as_array().unwrap().last().unwrap();
        assert_eq!((entry["name"].as_str(), entry["source"].as_str()), (Some("Harbor Night"), Some("imported")));
    }

    #[test]
    fn a_light_theme_is_told_light() {
        let dir = data("light");
        let imported = import(&dir, &fixture("Paper Mill.json")).unwrap();
        let described = describe(&library(&dir), &[], &Choice::default(), None);
        let p = described["list"].as_array().unwrap().iter().find(|p| p["entry"] == imported.entry).unwrap();
        assert_eq!(p["kind"], "light");
    }

    #[test]
    fn an_extension_folder_imports_too() {
        let dir = data("folder");
        let ext = dir.join("harbor-ext");
        std::fs::create_dir_all(ext.join("themes")).unwrap();
        std::fs::copy(fixture("Harbor Night.json"), ext.join("themes/harbor.json")).unwrap();
        std::fs::write(
            ext.join("package.json"),
            r#"{ "name": "harbor-ext", "contributes": { "themes": [{ "label": "Harbor Night", "uiTheme": "vs-dark", "path": "./themes/harbor.json" }] } }"#,
        )
        .unwrap();
        let imported = import(&dir, &ext).unwrap();
        assert_eq!(imported.variants.len(), 1, "{imported:?}");
        assert_eq!(library(&dir).entry(&imported.entry).unwrap().name, "harbor-ext");
    }

    #[test]
    fn imports_rename_and_remove_and_are_kept() {
        let dir = data("keep");
        let first = import(&dir, &fixture("Harbor Night.json")).unwrap();
        let again = import(&dir, &fixture("Harbor Night.json")).unwrap();
        assert_ne!(first.entry, again.entry, "a second import is a second entry");
        rename(&dir, &first.entry, "Harbor").unwrap();
        assert_eq!(library(&dir).entry(&first.entry).unwrap().name, "Harbor");
        let described = describe(&library(&dir), &[], &Choice::default(), None);
        assert!(described["list"].as_array().unwrap().iter().any(|p| p["entry"] == first.entry && p["name"] == "Harbor"));
        assert!(rename(&dir, &first.entry, "  ").is_err());
        assert!(remove(&dir, &first.entry).unwrap());
        assert!(!remove(&dir, &first.entry).unwrap());
        assert_eq!(library(&dir).entries.len(), 1);
    }

    #[test]
    fn a_broken_file_says_what_s_wrong_and_changes_nothing() {
        let dir = data("broken");
        let error = import(&dir, &fixture("Broken.json")).unwrap_err();
        assert!(error.starts_with("could not parse JSONC theme"), "{error}");
        let missing = import(&dir, &fixture("nowhere.json")).unwrap_err();
        assert!(missing.starts_with("could not resolve"), "{missing}");
        assert!(library(&dir).entries.is_empty());
    }

    #[test]
    fn names_and_ids_as_the_shipping_app_makes_them() {
        assert_eq!(source_name(Path::new("/x/Harbor Night.json")), "Harbor Night");
        assert_eq!(source_name(Path::new("/x/my-ext/package.json")), "my-ext");
        assert_eq!(source_name(Path::new("/x/my-ext")), "my-ext");
        assert_eq!(slug("Harbor Night!"), "harbor-night");
        assert_eq!(slug("!!"), "theme");
    }

    #[test]
    fn every_built_in_family_is_listed_light_and_dark_together_and_one_variant_alone() {
        let all = builtin();
        let ids: Vec<&str> = all.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids[0], "zeron", "Zeron's first");
        let zeron = &all[0];
        assert_eq!((zeron.variant(false).unwrap().id.as_str(), zeron.variant(true).unwrap().id.as_str()), ("zeron-light", "zeron-dark"));
        // Ayu's light and dark together, its Mirage a theme of its own.
        let ayu = find_builtin("ayu").unwrap();
        assert_eq!(ayu.variants.iter().map(|v| v.id.as_str()).collect::<Vec<_>>(), ["ayu-light", "ayu-dark"]);
        let mirage = find_builtin("ayu-mirage").unwrap();
        assert_eq!((mirage.name.as_str(), mirage.variants.len()), ("Ayu Mirage", 1));
        assert!(mirage.variant(false).is_none() && mirage.variant(true).is_some());
        // A family with one variant is listed, as its one.
        let dracula = find_builtin("dracula").unwrap();
        assert_eq!((dracula.variant(false), dracula.variant(true).map(|v| v.id.as_str())), (None, Some("dracula")));
        // Every variant of every family, once.
        let variants: usize = builtin_registry().families.iter().map(|f| f.variants.len()).sum();
        assert_eq!(all.iter().map(|b| b.variants.len()).sum::<usize>(), variants);
        // Listed so: each palette under its theme.
        let described = describe(&CustomThemeLibrary::default(), &[], &Choice::default(), None);
        assert_eq!(described["list"].as_array().unwrap().len(), variants);
        let gruvbox = described["entries"].as_array().unwrap().iter().find(|e| e["id"] == "gruvbox").unwrap();
        assert_eq!(gruvbox["variants"], json!(["gruvbox-light", "gruvbox-dark"]));
        assert_eq!(gruvbox["source"], "builtin");
        assert_eq!(choice_of(&find_builtin("gruvbox").unwrap()), Choice { light: Some("gruvbox-light".into()), dark: Some("gruvbox-dark".into()) });
    }

    #[test]
    fn a_template_s_theme_is_listed_as_its_own_and_a_file_without_one_is_not() {
        let palette_of = |id: &str| {
            let v = builtin_registry().families.iter().flat_map(|f| &f.variants).find(|v| v.id == id).unwrap();
            palette(v, "")
        };
        let themes = vec![
            TemplateTheme { template: "ledger".into(), name: "Ledger".into(), file: json!({ "name": "Ledger Ink", "dark": palette_of("nord") }) },
            TemplateTheme { template: "plain".into(), name: "Plain".into(), file: Json::Null },
        ];
        let described = describe(&CustomThemeLibrary::default(), &themes, &Choice::default(), None);
        let entries = described["entries"].as_array().unwrap();
        let ledger = entries.iter().find(|e| e["source"] == "template").unwrap();
        assert_eq!(ledger["id"], "template-ledger");
        assert_eq!((ledger["name"].as_str(), ledger["template"].as_str()), (Some("Ledger Ink"), Some("ledger")));
        assert_eq!(ledger["variants"], json!(["template-ledger-dark"]));
        assert_eq!(entries.iter().filter(|e| e["source"] == "template").count(), 1, "no theme, not listed");
        let p = described["list"].as_array().unwrap().iter().find(|p| p["id"] == "template-ledger-dark").unwrap();
        assert_eq!((p["kind"].as_str(), p["entry"].as_str(), p["background"].as_str()), (Some("dark"), Some("template-ledger"), Some("#2e3440")));
    }

    #[test]
    fn a_choice_is_kept_slot_by_slot_and_a_removed_theme_lets_its_slot_go() {
        let dir = data("choice");
        assert!(chosen(&dir).is_empty());
        choose(&dir, &json!({ "light": "gruvbox-light", "dark": "gruvbox-dark" })).unwrap();
        choose(&dir, &json!({ "dark": "dracula" })).unwrap();
        assert_eq!(chosen(&dir), Choice { light: Some("gruvbox-light".into()), dark: Some("dracula".into()) });
        choose(&dir, &json!({ "light": "" })).unwrap();
        assert_eq!(chosen(&dir), Choice { light: None, dark: Some("dracula".into()) });
        assert!(choose(&dir, &json!({ "dark": "../../etc" })).is_err());
        assert!(choose(&dir, &json!({ "dark": 3 })).is_err());
        assert_eq!(describe(&CustomThemeLibrary::default(), &[], &chosen(&dir), None)["chosen"], json!({ "dark": "dracula" }));
        // An imported theme in use, removed: its slot goes back; the other stays.
        let harbor = import(&dir, &fixture("Harbor Night.json")).unwrap();
        choose(&dir, &json!({ "dark": harbor.variants[0], "light": "github-light" })).unwrap();
        remove(&dir, &harbor.entry).unwrap();
        assert_eq!(chosen(&dir), Choice { light: Some("github-light".into()), dark: None });
        // A file that isn't a choice reads as none.
        std::fs::write(dir.join(CHOICE), "not json").unwrap();
        assert!(chosen(&dir).is_empty());
    }

    // -- Each theme worn (ui/theme_check_fixture.luau) ---------------------

    use zeron_theme::Color;
    use zgx_script::testing::TestApp;

    /// The roles every theme sets (surfaces and glass, text, hairlines,
    /// accent, focus, selection, code and Markdown, diffs, git, statuses, the
    /// composer and the sidebar's rows), on top of whatever else the mapping
    /// writes.
    const ROLES: &[&str] = &[
        "zeron.window", "background", "glass.tint", "popover.bg", "surface.sunken", "surface.base", "surface.raised", "surface.overlay",
        "text.primary", "text.secondary", "text.tertiary", "text.disabled", "border", "border.subtle", "border.strong", "zeron.edge",
        "hover", "pressed", "selection", "selection.item", "focus.ring", "accent", "accent.hover", "accent.text", "zeron.accents.default",
        "danger", "warning", "success", "info", "zeron.selected.flat.bg", "zeron.selected.bg", "zeron.menu.tint", "zeron.menu.solid",
        "zeron.composer.tint", "zeron.composer.solid", "zeron.composer.edge", "zeron.primary.bg", "zeron.primary.text",
        "zeron.destructive.bg", "zeron.destructive.text", "zeron.diff.added", "zeron.diff.removed", "zeron.git.modified",
        "zeron.git.added", "zeron.git.deleted", "zeron.pr.open", "zeron.pr.merged", "zeron.status.approval", "zeron.status.dot.done",
        "zeron.status.failed", "zeron.tone.accent.ink", "zeron.tone.danger.ink", "zeron.tone.success.ink", "zeron.tone.warning.ink",
        "zeron.tone.info.ink", "zeron.visual.series.teal",
    ];
    /// Code's and Markdown's roles (zeron.syntax.*): all of them.
    const SYNTAX_ROLES: &[&str] = &[
        "text", "keyword", "string", "escape", "regex", "number", "constant", "comment", "function", "macro", "type", "variable",
        "property", "attribute", "label", "operator", "punctuation", "tag", "heading", "link", "url", "emphasis", "strong", "quote",
        "inserted", "deleted", "hunk", "meta",
    ];
    /// Code's quieter roles, read at 3:1 as faint text is.
    const QUIET: &[&str] = &["comment", "url", "punctuation"];

    fn ui() -> PathBuf {
        std::env::var_os("ZERON_ZGX_TEST_UI").map(PathBuf::from).unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("ui"))
    }

    struct Rig {
        app: TestApp,
    }

    impl Rig {
        fn new() -> Self {
            let entry = ui().join("theme_check_fixture.luau");
            let app = TestApp::new(|runtime| ScriptHost::new(runtime, entry));
            app.frames(4);
            let keys: Vec<Json> = ROLES.iter().map(|k| json!(k)).chain(SYNTAX_ROLES.iter().map(|k| json!(format!("zeron.syntax.{k}")))).collect();
            app.host.shared("theme_check.keys", Value::Nil).set(json_value(&Json::Array(keys)));
            Rig { app }
        }

        fn run(&self, command: &str) -> Json {
            assert!(self.app.host.run_command(command), "no command {command}");
            self.app.frames(3);
            assert!(self.app.host.run_command("read"));
            self.app.frames(2);
            crate::plugins::value_json(&self.app.host.shared("theme_check.report", Value::Nil).signal().with_untracked(Value::clone))
        }

        /// `slots` (`{ light?, dark? }`, palettes) worn, `kind` showing.
        fn wear(&self, slots: &Json, kind: &str) -> Json {
            self.app.host.shared("theme_check.slots", Value::Nil).set(json_value(slots));
            self.app.host.shared("theme_check.kind", Value::Nil).set(Value::from(kind));
            self.run("wear")
        }
    }

    fn color(value: &Json) -> Color {
        let text = value.as_str().unwrap_or_else(|| panic!("not a colour: {value}"));
        text.parse().unwrap_or_else(|_| panic!("not a colour: {text}"))
    }

    /// `#rrggbb` and `#rrggbbff` are one colour.
    fn same(a: &str, b: &str) -> bool {
        let norm = |s: &str| {
            let s = s.to_ascii_lowercase();
            if s.len() == 9 && s.ends_with("ff") { s[..7].to_string() } else { s }
        };
        norm(a) == norm(b)
    }

    fn slots_of(theme: &Builtin) -> Json {
        let mut out = json!({});
        for v in &theme.variants {
            out[if v.appearance.is_dark() { "dark" } else { "light" }] = palette(v, &theme.id);
        }
        out
    }

    #[test]
    fn every_theme_sets_every_role_and_reads() {
        let rig = Rig::new();
        let mut worn = 0;
        let mut fixes = Vec::new();
        let mut failures = Vec::new();
        for theme in builtin().into_iter().filter(|b| b.id != "zeron") {
            let slots = slots_of(&theme);
            for v in &theme.variants {
                let kind = if v.appearance.is_dark() { "dark" } else { "light" };
                let report = rig.wear(&slots, kind);
                let what = format!("{} ({kind})", v.name);
                assert_eq!(report["current"], format!("theme-{kind}"), "{what}: {report}");
                worn += 1;
                let resolved = &report["resolved"];
                let written = report["written"].as_object().unwrap_or_else(|| panic!("{what}: nothing written: {report}"));
                // Every role the mapping promises, and every one it wrote,
                // resolved as written.
                for role in ROLES.iter().map(|r| r.to_string()).chain(SYNTAX_ROLES.iter().map(|s| format!("zeron.syntax.{s}"))) {
                    if !written.contains_key(&role) {
                        failures.push(format!("{what}: {role} isn't set"));
                    }
                }
                for (role, value) in written {
                    let got = resolved[role].as_str().unwrap_or("");
                    if got.is_empty() {
                        failures.push(format!("{what}: {role} doesn't resolve"));
                    } else if let Some(hex) = value.as_str().filter(|s| s.starts_with('#'))
                        && !same(hex, got)
                    {
                        failures.push(format!("{what}: {role} wrote {hex}, resolves {got}"));
                    }
                }
                // What follows the accent follows it.
                let accent = color(&resolved["accent"]);
                let ring = color(&resolved["focus.ring"]);
                if (ring.r, ring.g, ring.b) != (accent.r, accent.g, accent.b) {
                    failures.push(format!("{what}: the focus ring isn't the accent's"));
                }
                // Legible: on the window and on a menu.
                let window = color(&resolved["zeron.window"]);
                let menu = color(&resolved["zeron.menu.solid"]);
                let mut check = |role: &str, on: Color, min: f32| {
                    let c = color(&resolved[role]).contrast(on);
                    if c < min - 0.01 {
                        failures.push(format!("{what}: {role} reads {c:.2}:1 (wants {min})"));
                    }
                };
                for under in [window, menu] {
                    check("text.primary", under, 4.5);
                    check("text.secondary", under, 4.5);
                    check("text.tertiary", under, 3.0);
                }
                check("accent", window, 3.0);
                for role in [
                    "zeron.diff.added", "zeron.diff.removed", "zeron.git.modified", "zeron.git.added", "zeron.git.deleted",
                    "zeron.pr.open", "zeron.pr.merged", "zeron.status.approval", "zeron.status.failed", "zeron.status.dot.done",
                    "zeron.tone.accent.ink", "zeron.tone.danger.ink", "zeron.tone.success.ink", "zeron.tone.warning.ink", "zeron.tone.info.ink",
                ] {
                    check(role, window, 4.5);
                }
                for s in SYNTAX_ROLES {
                    check(&format!("zeron.syntax.{s}"), window, if QUIET.contains(s) { 3.0 } else { 4.5 });
                }
                check("zeron.primary.text", color(&resolved["zeron.primary.bg"]), 4.5);
                check("accent.text", accent, 3.0);
                check("zeron.destructive.text", color(&resolved["zeron.destructive.bg"]), 3.0);
                for fix in report["fixes"].as_array().into_iter().flatten() {
                    fixes.push(format!("{what}: {} {} → {} (to {}:1)", fix["role"].as_str().unwrap_or(""), fix["from"].as_str().unwrap_or(""), fix["to"].as_str().unwrap_or(""), fix["min"]));
                }
            }
        }
        let variants: usize = builtin().iter().filter(|b| b.id != "zeron").map(|b| b.variants.len()).sum();
        assert_eq!(worn, variants);
        // What was moved from the source to stay legible (the deviations).
        eprintln!("{} inks moved to stay legible:\n{}", fixes.len(), fixes.join("\n"));
        if let Some(path) = std::env::var_os("ZGX_THEME_FIXES") {
            let _ = std::fs::write(path, fixes.join("\n"));
        }
        assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    }

    #[test]
    fn each_scheme_wears_its_own_slot_and_an_empty_one_is_zeron_s() {
        let rig = Rig::new();
        let zeron = rig.wear(&json!({}), "dark");
        assert_eq!(zeron["names"], json!({ "dark": "theme-dark", "light": "theme-light" }), "the names never change");
        // Gruvbox for light, Tokyo Night for dark: each scheme its own.
        let slots = json!({ "light": palette(find_builtin("gruvbox").unwrap().variant(false).unwrap(), "gruvbox"),
                            "dark": palette(find_builtin("tokyo-night").unwrap().variant(true).unwrap(), "tokyo-night") });
        let light = rig.wear(&slots, "light");
        assert!(same(light["resolved"]["zeron.window"].as_str().unwrap(), "#fbf1c7"), "{}", light["resolved"]["zeron.window"]);
        let dark = rig.wear(&slots, "dark");
        assert!(same(dark["resolved"]["zeron.window"].as_str().unwrap(), "#1a1b26"));
        // One with only a dark theme: light is Zeron's, every role as it was.
        let dracula = rig.wear(&slots_of(&find_builtin("dracula").unwrap()), "light");
        assert_eq!(dracula["current"], "theme-light");
        let own_light = rig.wear(&json!({}), "light");
        assert_eq!(dracula["asked"], own_light["asked"], "an empty slot is Zeron's");
        assert_ne!(rig.wear(&slots_of(&find_builtin("dracula").unwrap()), "dark")["asked"], zeron["asked"]);
    }

    #[test]
    fn wearing_a_theme_and_going_back_restores_zeron_s_colours() {
        let rig = Rig::new();
        for kind in ["dark", "light"] {
            let before = rig.wear(&json!({}), kind);
            let theme = rig.wear(&slots_of(&find_builtin("github").unwrap()), kind);
            assert_ne!(theme["asked"]["text.primary"], before["asked"]["text.primary"], "GitHub's text isn't Zeron's");
            assert_ne!(theme["asked"]["zeron.syntax.keyword"], before["asked"]["zeron.syntax.keyword"]);
            let after = rig.wear(&json!({}), kind);
            assert_eq!(after["asked"], before["asked"], "every role back as Zeron's ({kind})");
        }
    }
}
