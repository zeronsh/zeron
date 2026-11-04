//! Themes in the window: the colours Zeron wears, chosen apart from the
//! template it runs (docs/themes.md). What a theme is, where each kind comes
//! from and how the choice is kept are the shared catalogue's
//! (`zeron_theme::catalog`), which the engine's theme tools use too: this is
//! the window's side of it, handing the list to the scripts and doing what
//! the Themes page asks.
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
//! An agent's theme tools write the same files through the engine, which
//! says so (`WatchThemes`): the list is read again, and a theme worn as it
//! changes restyles the window in place. The templates' own themes come
//! from the templates' bridge as they change (`set_templates`), read with
//! its cache.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use serde_json::json;
use zeron_theme::CustomThemeLibrary;
pub use zeron_theme::catalog::{Choice, TemplateTheme, chosen, choice_of, find_builtin, save_choice};
use zeron_theme::catalog::{self, describe};
use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Shared, Value};

use crate::auth::text;
use crate::plugins::json_value;

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
    /// The engine's word that its theme tools changed something.
    static WATCH: RefCell<Option<crate::engine::rpc::Watch>> = const { RefCell::new(None) };
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

fn said(error: impl std::fmt::Display) -> String {
    error.to_string()
}

pub fn install(_cx: &mut WindowContext, host: &ScriptHost) {
    let shared = Rc::new(host.shared("themes.library", Value::record([("state", Value::from("loading"))])));
    LIBRARY.with(|l| *l.borrow_mut() = Some(shared));
    publish();
    // Each connection to the engine: told when its tools change a theme (and
    // read again at once, for what changed while there was none).
    crate::engine::get().on_connect(|rpc| {
        let watch = rpc.watch("WatchThemes", json!({}), |_| publish());
        WATCH.with(|w| *w.borrow_mut() = Some(watch));
        publish();
    });
    host.async_action("themes.import", move |arg| async move {
        let dir = data_dir().ok_or("Themes aren't available right now")?;
        let path = PathBuf::from(text(&arg));
        if path.as_os_str().is_empty() {
            return Err("Choose a theme file or an extension's folder.".into());
        }
        let imported = off_thread(move || catalog::import(&dir, &path).map_err(said)).await.ok_or("The import stopped")??;
        publish();
        Ok(json_value(&json!({ "entry": imported.entry, "variants": imported.variants, "skipped": imported.skipped })))
    });
    host.action("themes.remove", move |arg| {
        let Some(dir) = data_dir() else { return };
        if let Err(error) = catalog::remove(&dir, &text(&arg)) {
            eprintln!("zeron-zgx: themes: {error}");
        }
        publish();
    });
    host.async_action("themes.rename", move |arg| async move {
        let dir = data_dir().ok_or("Themes aren't available right now")?;
        let id = arg.field("id").map(text).unwrap_or_default();
        let name = arg.field("name").map(text).unwrap_or_default();
        catalog::rename(&dir, &id, &name).map_err(said)?;
        publish();
        Ok(Value::Nil)
    });
    host.async_action("themes.choose", move |arg| async move {
        let dir = data_dir().ok_or("Themes aren't available right now")?;
        catalog::choose(&dir, &crate::plugins::value_json(&arg)).map_err(said)?;
        publish();
        Ok(Value::Nil)
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use serde_json::Value as Json;
    use zeron_theme::ThemeVariant;
    use zeron_theme::catalog::{Builtin, Palette, builtin, import, legibility, remove, rename};

    /// A variant's palette, as the window is handed it.
    fn palette(v: &ThemeVariant, entry: &str) -> Json {
        let kind = if v.appearance.is_dark() { "dark" } else { "light" };
        Palette::of(v).to_json(Some((&v.id, &v.name, kind, entry)))
    }

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
        let error = import(&dir, &fixture("Broken.json")).unwrap_err().to_string();
        assert!(error.starts_with("could not parse JSONC theme"), "{error}");
        let missing = import(&dir, &fixture("nowhere.json")).unwrap_err().to_string();
        assert!(missing.starts_with("could not resolve"), "{missing}");
        assert!(library(&dir).entries.is_empty());
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
                // What the window moved is what the engine's tools say it moves.
                let said: std::collections::BTreeSet<String> = legibility(&Palette::of(v), kind).iter().map(|f| format!("{} {} → {}", f.role, f.from, f.to)).collect();
                let moved: std::collections::BTreeSet<String> = report["fixes"].as_array().into_iter().flatten().map(|f| format!("{} {} → {}", f["role"].as_str().unwrap_or(""), f["from"].as_str().unwrap_or(""), f["to"].as_str().unwrap_or(""))).collect();
                if said != moved {
                    failures.push(format!("{what}: the window moved {moved:?}, catalog::legibility says {said:?}"));
                }
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
