//! Your imported themes: VS Code colour themes (a `.json` file, comments
//! allowed, or an extension's folder) compiled and kept by the shipping app's
//! own theme crate (`zeron-theme`: its importer, light-or-dark detection,
//! library file and error words), and each variant handed to the scripts in
//! the palette shape `ds/palettes` dresses a window from.
//!
//! ```text
//! shared  themes.library   { state, list = { Palette… }, entries = { { id, name, status, variants } }, warning? }
//! call    themes.import    path → { entry, variants = { id… }, skipped = { "name: why"… } }, or its error
//! invoke  themes.remove    entry id
//! call    themes.rename    { id, name }
//! ```
//!
//! The library is `<data>/theme-library.json` (the engine's data dir), as the
//! shipping app keeps its own: this device's, not synced. Imports are
//! snapshots: the file can move or go afterwards.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use serde_json::{Value as Json, json};
use zeron_theme::{CustomThemeLibrary, CustomThemeStatus, InstallMode, ThemeVariant};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::plugins::json_value;

/// The syntax colours a palette carries, in `ds/palettes`' order.
const SYNTAX: [&str; 12] = [
    "comment", "keyword", "string", "number", "type", "function", "property", "variable", "punctuation", "tag", "attribute", "invalid",
];

/// A variant as a palette (`ds/palettes`): its surfaces (the window, its
/// chrome, raised and card), its three inks, its accent and tones, its
/// code's twelve colours, and its terminal's (background and the sixteen
/// ANSI colours). `entry` is an imported one's library entry ("" for one
/// built in).
pub fn palette(variant: &ThemeVariant, entry: &str) -> Json {
    let c = &variant.colors;
    let hex = |color: zeron_theme::Color| color.to_string();
    let syntax: Vec<String> = SYNTAX
        .iter()
        .map(|name| variant.syntax.get(*name).map(|color| hex(*color)).unwrap_or_else(|| hex(c.text)))
        .collect();
    let mut out = json!({
        "id": variant.id,
        "name": variant.name,
        "kind": if variant.appearance.is_dark() { "dark" } else { "light" },
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
    });
    if !entry.is_empty() {
        out["entry"] = json!(entry);
    }
    out
}

/// What the scripts are told of `library`.
pub fn describe(library: &CustomThemeLibrary, warning: Option<&str>) -> Json {
    let mut list = Vec::new();
    let mut entries = Vec::new();
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
            "status": status,
            "variants": entry.family.variants.iter().map(|v| v.id.clone()).collect::<Vec<_>>(),
        }));
    }
    let mut out = json!({ "state": "ready", "list": list, "entries": entries });
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

/// The entry `id` gone from the library, saved: whether it was there.
pub fn remove(data_dir: &Path, id: &str) -> Result<bool, String> {
    let mut library = CustomThemeLibrary::load(data_dir).map_err(|e| e.to_string())?;
    let removed = library.remove(id);
    if removed {
        library.save(data_dir).map_err(|e| e.to_string())?;
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

/// The library as the scripts see it, read again from disk.
fn publish(shared: &Shared, dir: &Path) {
    let (library, warning) = match CustomThemeLibrary::load(dir) {
        Ok(library) => (library, None),
        Err(error) => (CustomThemeLibrary::default(), Some(error.to_string())),
    };
    shared.set(json_value(&describe(&library, warning.as_deref())));
}

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    let shared = host.shared("themes.library", Value::record([("state", Value::from("loading"))]));
    let shared = Rc::new(RefCell::new(shared));
    if let Some(dir) = data_dir() {
        publish(&shared.borrow(), &dir);
    }
    host.async_action("themes.import", {
        let shared = shared.clone();
        move |arg| {
            let shared = shared.clone();
            async move {
                let dir = data_dir().ok_or("Themes aren't available right now")?;
                let path = PathBuf::from(text(&arg));
                if path.as_os_str().is_empty() {
                    return Err("Choose a local theme file or extension folder.".into());
                }
                let at = dir.clone();
                let imported = off_thread(move || import(&at, &path)).await.ok_or("The import stopped")??;
                publish(&shared.borrow(), &dir);
                Ok(json_value(&json!({ "entry": imported.entry, "variants": imported.variants, "skipped": imported.skipped })))
            }
        }
    });
    host.action("themes.remove", {
        let shared = shared.clone();
        move |arg| {
            let Some(dir) = data_dir() else { return };
            if let Err(error) = remove(&dir, &text(&arg)) {
                eprintln!("zeron-zgx: themes: {error}");
            }
            publish(&shared.borrow(), &dir);
        }
    });
    host.async_action("themes.rename", {
        let shared = shared.clone();
        move |arg| {
            let shared = shared.clone();
            async move {
                let dir = data_dir().ok_or("Themes aren't available right now")?;
                let id = arg.field("id").map(text).unwrap_or_default();
                let name = arg.field("name").map(text).unwrap_or_default();
                rename(&dir, &id, &name)?;
                publish(&shared.borrow(), &dir);
                Ok(Value::Nil)
            }
        }
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

    #[test]
    fn a_vs_code_theme_with_comments_imports_as_a_palette() {
        let dir = data("import");
        let imported = import(&dir, &fixture("Harbor Night.json")).unwrap();
        assert_eq!(imported.variants.len(), 1, "{imported:?}");
        let library = CustomThemeLibrary::load(&dir).unwrap();
        let described = describe(&library, None);
        let p = &described["list"][0];
        assert_eq!(p["kind"], "dark", "type: dark");
        assert_eq!(p["name"], "Harbor Night");
        assert_eq!(p["entry"], imported.entry);
        // The editor's own colours, as the shipping app maps them.
        assert_eq!(p["background"], "#14181f");
        assert_eq!(p["text"], "#d8dee9");
        assert_eq!(p["syntax"].as_array().unwrap().len(), 12);
        assert_eq!(p["syntax"][0], "#5c6773", "comments, from tokenColors");
        assert_eq!(p["syntax"][1], "#c792ea", "keywords");
        assert_eq!(described["entries"][0]["name"], "Harbor Night");
    }

    #[test]
    fn a_light_theme_is_told_light() {
        let dir = data("light");
        import(&dir, &fixture("Paper Mill.json")).unwrap();
        let described = describe(&CustomThemeLibrary::load(&dir).unwrap(), None);
        assert_eq!(described["list"][0]["kind"], "light");
    }

    #[test]
    fn imports_rename_and_remove_and_are_kept() {
        let dir = data("keep");
        let first = import(&dir, &fixture("Harbor Night.json")).unwrap();
        let again = import(&dir, &fixture("Harbor Night.json")).unwrap();
        assert_ne!(first.entry, again.entry, "a second import is a second entry");
        rename(&dir, &first.entry, "Harbor").unwrap();
        let library = CustomThemeLibrary::load(&dir).unwrap();
        assert_eq!(library.entry(&first.entry).unwrap().name, "Harbor");
        assert_eq!(describe(&library, None)["list"][0]["name"], "Harbor");
        assert!(rename(&dir, &first.entry, "  ").is_err());
        assert!(remove(&dir, &first.entry).unwrap());
        assert!(!remove(&dir, &first.entry).unwrap());
        assert_eq!(CustomThemeLibrary::load(&dir).unwrap().entries.len(), 1);
    }

    #[test]
    fn a_broken_file_says_what_s_wrong_and_changes_nothing() {
        let dir = data("broken");
        let error = import(&dir, &fixture("Broken.json")).unwrap_err();
        assert!(error.starts_with("could not parse JSONC theme"), "{error}");
        let missing = import(&dir, &fixture("nowhere.json")).unwrap_err();
        assert!(missing.starts_with("could not resolve"), "{missing}");
        assert!(CustomThemeLibrary::load(&dir).unwrap().entries.is_empty());
    }

    #[test]
    fn names_and_ids_as_the_shipping_app_makes_them() {
        assert_eq!(source_name(Path::new("/x/Harbor Night.json")), "Harbor Night");
        assert_eq!(source_name(Path::new("/x/my-ext/package.json")), "my-ext");
        assert_eq!(slug("Harbor Night!"), "harbor-night");
        assert_eq!(slug("!!"), "theme");
    }
}
