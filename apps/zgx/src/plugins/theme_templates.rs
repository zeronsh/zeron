//! Zeron's built-in themes as templates: each of the shipping app's colour
//! themes (`zeron_theme::builtin_registry`, crates/theme builtins.rs) as a
//! template that recolours Zeron and changes nothing else.
//!
//! A template that recolours is data, not a copy of the app. Its folder holds
//! its manifest and `theme.luau`, a palette for each scheme it has
//! (`{ name, dark = Palette?, light = Palette? }`, in the shape src/themes.rs
//! gives imported themes). Every other file is inherited from Zeron's own, and
//! `ds/palettes` maps the palette onto every colour token. Any template can do
//! the same: Zeron's own `theme.luau` returns nil, and a template's own
//! `theme.luau` replaces it.
//!
//! The built-in ones ship with the app and are official Zeron templates,
//! listed as Zeron's own is: each launch writes them into
//! `<data>/templates/theme-<key>/` (`install_all`, rewriting only what an
//! update changed). They're marked in their manifest (`builtin_theme`), so a
//! folder that isn't marked is never written over. A template of your own
//! with that id keeps it, and the built-in one takes the next free id.
//!
//! Only families with a light and a dark variant ship, each as one template
//! that follows the system's appearance, as Zeron's own does (`choices`).
//! Zeron's own family is the built-in `zeron` template, not one of these.

use std::path::{Path, PathBuf};

use serde_json::{Value as Json, json};
use zeron_theme::{ThemeVariant, builtin_registry};

use super::live;

/// The manifest field that marks a folder as a built-in theme's (its key).
pub const MARK: &str = "builtin_theme";
/// The file a template's palette is in, at its root.
pub const FILE: &str = "theme.luau";

/// One built-in theme a template can be: its key (`dracula`, `ayu-mirage`),
/// its name, and its variants (a light, a dark, or both).
#[derive(Clone, Debug)]
pub struct Choice {
    pub key: String,
    pub name: String,
    pub variants: Vec<&'static ThemeVariant>,
}

impl Choice {
    /// The id it installs as (unless that's someone else's: `install`).
    pub fn id(&self) -> String {
        format!("theme-{}", self.key)
    }

    #[cfg(test)]
    pub fn variant(&self, dark: bool) -> Option<&'static ThemeVariant> {
        self.variants.iter().copied().find(|v| v.appearance.is_dark() == dark)
    }
}

/// Every built-in theme a template can be, in the shipping app's order:
/// each family that has a light and a dark variant, as one template that
/// follows the system's appearance (its first light and its first dark:
/// Ayu's light and dark, not its Mirage). A family with one appearance
/// isn't shipped. Zeron's own family is the `zeron` template.
pub fn choices() -> Vec<Choice> {
    let mut out = Vec::new();
    for family in &builtin_registry().families {
        if family.id == live::BUILTIN {
            continue;
        }
        let light = family.variants.iter().find(|v| !v.appearance.is_dark());
        let dark = family.variants.iter().find(|v| v.appearance.is_dark());
        if let (Some(light), Some(dark)) = (light, dark) {
            out.push(Choice { key: family.id.clone(), name: family.name.clone(), variants: vec![light, dark] });
        }
    }
    out
}

pub fn find(key: &str) -> Option<Choice> {
    choices().into_iter().find(|c| c.key == key)
}

/// A JSON value as a Luau literal (palettes are strings, lists of them and
/// tables of those).
fn luau(value: &Json, indent: usize) -> String {
    let pad = "\t".repeat(indent + 1);
    match value {
        Json::String(s) => format!("{s:?}"),
        Json::Array(items) => {
            let parts: Vec<String> = items.iter().map(|v| luau(v, indent + 1)).collect();
            format!("{{ {} }}", parts.join(", "))
        }
        Json::Object(map) => {
            let mut lines = Vec::new();
            for (k, v) in map {
                lines.push(format!("{pad}{k} = {},", luau(v, indent + 1)));
            }
            format!("{{\n{}\n{}}}", lines.join("\n"), "\t".repeat(indent))
        }
        Json::Bool(b) => b.to_string(),
        Json::Number(n) => n.to_string(),
        Json::Null => "nil".into(),
    }
}

/// A choice's palettes, by scheme.
pub fn palettes(choice: &Choice) -> Json {
    let mut out = json!({ "name": choice.name });
    for v in &choice.variants {
        let scheme = if v.appearance.is_dark() { "dark" } else { "light" };
        out[scheme] = crate::themes::palette(v, "");
    }
    out
}

/// A choice's `theme.luau`: its palettes, as data.
pub fn theme_file(choice: &Choice) -> String {
    format!(
        "-- {name}: Zeron's built-in theme's colours, the shipping app's own\n\
         -- (crates/theme builtins.rs). Zeron's interface wears them through\n\
         -- ds/palettes, which maps each palette onto every colour token. To\n\
         -- change them, duplicate this template and edit the copy.\n\
         {credit}\
         return {body}\n",
        name = choice.name,
        credit = credit(choice),
        body = luau(&palettes(choice), 0),
    )
}

/// Where a choice's colours were adapted from, as comment lines: each
/// upstream project once, with its revision and licence (THIRD_PARTY_NOTICES.md
/// carries the notices).
fn credit(choice: &Choice) -> String {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = String::new();
    for v in &choice.variants {
        let s = &v.source;
        if s.url.is_empty() || seen.contains(&s.url.as_str()) {
            continue;
        }
        seen.push(&s.url);
        let rev = if s.revision.is_empty() { String::new() } else { format!(" at {}", s.revision) };
        let license = if s.license.is_empty() { String::new() } else { format!(", {}", s.license) };
        out.push_str(&format!("-- Adapted from {}{rev}{license}.\n", s.url));
    }
    if !out.is_empty() {
        out.push_str("-- Zeron's THIRD_PARTY_NOTICES.md has its copyright and licence notice.\n");
    }
    out
}

pub fn manifest(choice: &Choice, id: &str) -> Json {
    json!({
        "id": id,
        "name": choice.name,
        "description": format!("{}'s colours on Zeron's interface.", choice.name),
        "author": "Zeron",
        "version": live::ZERON_VERSION,
        "zeron_version": live::ZERON_VERSION,
        MARK: choice.key,
    })
}

fn read_json(path: &Path) -> Json {
    std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Whose folder `dir` is: none (free), the built-in theme `key`'s, or
/// someone else's. A built-in's holds what Zeron writes and nothing else
/// (`theme_shaped`): a folder that only says `builtin_theme` (an imported
/// package's, or one an agent wrote) is someone else's, never written over.
fn holds(dir: &Path, key: &str) -> Option<bool> {
    if std::fs::symlink_metadata(dir).is_err() {
        return None;
    }
    Some(theme_shaped(dir).as_deref() == Some(key))
}

/// The built-in theme key a folder is shaped as Zeron writes one: a real
/// folder holding exactly its manifest (marked) and `theme.luau`, both
/// regular files, nothing else (no screens, no `.imported.json`). Whether
/// their bytes are this build's is `verified`'s.
fn theme_shaped(dir: &Path) -> Option<String> {
    if !std::fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // (`live::put`'s temporary files, mid-write.)
        if name.starts_with('.') && name.ends_with(".tmp") {
            continue;
        }
        if !entry.file_type().is_ok_and(|k| k.is_file()) {
            return None;
        }
        names.push(name);
    }
    names.sort();
    if names != [FILE, "zeron-template.json"] {
        return None;
    }
    read_json(&dir.join("zeron-template.json"))[MARK].as_str().map(str::to_string).filter(|k| !k.is_empty())
}

/// The built-in theme `dir` is, if this build wrote it as it stands: shaped
/// as one (`theme_shaped`), at its own id, its manifest and palette byte for
/// byte what `install` writes. Only such a folder shows as Zeron's (and
/// can't be deleted); any other that says `builtin_theme` is an ordinary
/// template.
pub fn verified(dir: &Path) -> Option<String> {
    let key = theme_shaped(dir)?;
    let choice = find(&key)?;
    let id = dir.file_name()?.to_str()?;
    let ours = id == choice.id() || id.strip_prefix(&format!("{}-", choice.id())).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    let manifest = serde_json::to_vec_pretty(&manifest(&choice, id)).unwrap_or_default();
    let same = |rel: &str, want: &[u8]| std::fs::read(dir.join(rel)).is_ok_and(|b| b == want);
    (ours && same(FILE, theme_file(&choice).as_bytes()) && same("zeron-template.json", &manifest)).then_some(key)
}

/// Where the built-in theme `key` is (or would be) installed: its own id
/// where that's free or already its, else the first `theme-<key>-N` that is.
pub fn place(templates: &Path, choice: &Choice) -> (String, PathBuf) {
    let base = choice.id();
    let mut id = base.clone();
    let mut n = 2;
    loop {
        let dir = templates.join(&id);
        if holds(&dir, &choice.key) != Some(false) {
            return (id, dir);
        }
        id = format!("{base}-{n}");
        n += 1;
    }
}

/// The built-in theme `key` written into `templates` (or brought up to
/// date there, where an earlier Zeron wrote it): its id. Only its own
/// manifest and `theme.luau` are written, and never in a folder that isn't
/// marked as its own.
pub fn install(templates: &Path, key: &str) -> Result<String, String> {
    let choice = find(key).ok_or_else(|| format!("There's no built-in theme {key}"))?;
    let (id, dir) = place(templates, &choice);
    let pretty = serde_json::to_vec_pretty(&manifest(&choice, &id)).unwrap_or_default();
    let write = |rel: &str, bytes: &[u8]| live::put(&dir.join(rel), bytes).map_err(|e| format!("Couldn't write the theme's {rel}: {e}"));
    // (The palette first: a folder with a manifest is a template.)
    write(FILE, theme_file(&choice).as_bytes())?;
    write("zeron-template.json", &pretty)?;
    Ok(id)
}

/// Every built-in theme written into `templates` (or brought up to date
/// there), as Zeron's own templates are kept: each one's id. What failed is
/// told and skipped.
pub fn install_all(templates: &Path) -> Vec<String> {
    remove_unshipped(templates);
    let mut ids = Vec::new();
    for choice in choices() {
        match install(templates, &choice.key) {
            Ok(id) => ids.push(id),
            Err(error) => eprintln!("zeron-zgx: the {} template wasn't installed: {error}", choice.name),
        }
    }
    ids
}

/// Folders an earlier Zeron installed for a built-in theme it no longer
/// ships (marked as that theme's, under its `theme-<key>` id): deleted, so
/// they aren't listed. A folder that isn't marked is never touched.
fn remove_unshipped(templates: &Path) {
    let shipped: Vec<String> = choices().into_iter().map(|c| c.key).collect();
    let Ok(entries) = std::fs::read_dir(templates) else { return };
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = id.strip_prefix("theme-") else { continue };
        if !entry.file_type().is_ok_and(|k| k.is_dir()) {
            continue;
        }
        // Only a folder shaped as Zeron wrote it: anything more is someone's.
        let Some(key) = theme_shaped(&entry.path()) else { continue };
        let ours = rest == key || rest.strip_prefix(&format!("{key}-")).is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if ours && !shipped.contains(&key) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// A built-in theme's place in the list: Zeron's order.
pub fn order(key: &str) -> usize {
    choices().iter().position(|c| c.key == key).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-theme-templates-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn only_families_with_a_light_and_a_dark_ship_one_template_each() {
        let keys: Vec<String> = choices().iter().map(|c| c.key.clone()).collect();
        assert_eq!(
            keys,
            ["vscode-default", "catppuccin", "tokyo-night", "github", "ayu", "gruvbox", "rose-pine", "night-owl", "winter-is-coming"],
            "the shipped set, in the shipping app's order"
        );
        // Each has both appearances, checked by appearance, not by count,
        // and each is the registry's own variant.
        for c in choices() {
            let (light, dark) = (c.variant(false).unwrap(), c.variant(true).unwrap());
            assert!(!light.appearance.is_dark() && dark.appearance.is_dark(), "{}", c.key);
            assert_eq!(c.variants.len(), 2, "{}", c.key);
            let family = builtin_registry().families.iter().find(|f| f.id == c.key).unwrap();
            assert!(family.variants.iter().any(|v| v.id == light.id) && family.variants.iter().any(|v| v.id == dark.id));
        }
        // Ayu as its light and dark; its Mirage (a second dark) isn't shipped.
        let ayu = find("ayu").unwrap();
        assert_eq!((ayu.variant(false).unwrap().id.as_str(), ayu.variant(true).unwrap().id.as_str()), ("ayu-light", "ayu-dark"));
        // Families with one appearance aren't, nor Zeron's own (the `zeron` template).
        for key in ["dracula", "nord", "andromeda", "atom-one-dark", "cobalt2", "one-dark-pro", "palenight", "shades-of-purple", "synthwave-84", "ayu-mirage", "zeron"] {
            assert!(find(key).is_none(), "{key} isn't shipped");
        }
        // Ids an engine takes: 2 to 40 lowercase letters, digits and dashes.
        for c in choices() {
            let (id, _) = place(&dir("ids"), &c);
            assert!(id.len() <= 40 && id.chars().all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'), "{id}");
        }
    }

    #[test]
    fn a_theme_installs_as_data_and_never_over_your_own() {
        let t = dir("install");
        // Your own template, by the same name and at the built-in's id.
        std::fs::create_dir_all(t.join("theme-gruvbox/screens")).unwrap();
        std::fs::write(t.join("theme-gruvbox/zeron-template.json"), r#"{"name":"Gruvbox","author":"you"}"#).unwrap();
        std::fs::write(t.join("theme-gruvbox/screens/x.luau"), "-- mine").unwrap();
        std::fs::create_dir_all(t.join("gruvbox")).unwrap();
        std::fs::write(t.join("gruvbox/zeron-template.json"), r#"{"name":"Gruvbox"}"#).unwrap();
        let id = install(&t, "gruvbox").unwrap();
        assert_eq!(id, "theme-gruvbox-2");
        assert_eq!(std::fs::read_to_string(t.join("theme-gruvbox/zeron-template.json")).unwrap(), r#"{"name":"Gruvbox","author":"you"}"#);
        assert_eq!(std::fs::read_to_string(t.join("theme-gruvbox/screens/x.luau")).unwrap(), "-- mine");
        assert_eq!(std::fs::read_to_string(t.join("gruvbox/zeron-template.json")).unwrap(), r#"{"name":"Gruvbox"}"#);
        // Its own: a manifest and its palette, nothing of Zeron's copied.
        let files: Vec<String> = live::files(&t.join(&id), |_| true).into_keys().collect();
        assert_eq!(files, vec![FILE.to_string(), "zeron-template.json".to_string()]);
        let m = read_json(&t.join(&id).join("zeron-template.json"));
        assert_eq!((m["name"].as_str(), m[MARK].as_str(), m["author"].as_str()), (Some("Gruvbox"), Some("gruvbox"), Some("Zeron")));
        let theme = std::fs::read_to_string(t.join(&id).join(FILE)).unwrap();
        assert!(theme.contains("dark = {") && theme.contains("light = {"), "both appearances");
        assert!(theme.contains("background = \"#fbf1c7\"") && theme.contains("background = \"#282828\"") && theme.contains("ansi = {"));
        // Again: the same place, nothing rewritten.
        let at = std::fs::metadata(t.join(&id).join(FILE)).unwrap().modified().unwrap();
        assert_eq!(install(&t, "gruvbox").unwrap(), id);
        assert_eq!(std::fs::metadata(t.join(&id).join(FILE)).unwrap().modified().unwrap(), at);
        assert!(install(&t, "no-such-theme").is_err() && install(&t, "dracula").is_err());
        // An earlier Zeron's built-in that's no longer shipped: deleted. Yours,
        // under such an id or marked by hand elsewhere, kept.
        std::fs::create_dir_all(t.join("theme-dracula")).unwrap();
        std::fs::write(t.join("theme-dracula/zeron-template.json"), r#"{"name":"Dracula","builtin_theme":"dracula"}"#).unwrap();
        std::fs::write(t.join("theme-dracula/theme.luau"), "return nil").unwrap();
        std::fs::create_dir_all(t.join("theme-nord")).unwrap();
        std::fs::write(t.join("theme-nord/zeron-template.json"), r#"{"name":"Nord","author":"you"}"#).unwrap();
        std::fs::create_dir_all(t.join("my-dracula")).unwrap();
        std::fs::write(t.join("my-dracula/zeron-template.json"), r#"{"name":"Mine","builtin_theme":"dracula"}"#).unwrap();
        // All of them, as a launch installs them: each once, yours kept.
        let all = install_all(&t);
        assert_eq!(all.len(), choices().len());
        assert!(all.contains(&id) && !all.contains(&"theme-gruvbox".to_string()));
        assert!(!t.join("theme-dracula").exists(), "a theme no longer shipped is deleted");
        assert!(t.join("theme-nord").is_dir() && t.join("my-dracula").is_dir(), "never what isn't ours");
        assert_eq!(install_all(&t), all, "again: the same places");
        assert_eq!(std::fs::read_to_string(t.join("theme-gruvbox/screens/x.luau")).unwrap(), "-- mine");
        // The engine and the page see it as a template, marked.
        let installed = live::installed(&t);
        let entry = installed.iter().find(|i| i.id == id).unwrap();
        assert_eq!(entry.theme.as_deref(), Some("gruvbox"));
        assert!(installed.iter().find(|i| i.id == "theme-gruvbox").unwrap().theme.is_none());
        assert_eq!(order("vscode-default"), 0);
    }

    /// R18: an imported template's manifest that says `builtin_theme` never
    /// shows as Zeron's (author Zeron, no Delete), nor is one rewritten
    /// into Zeron's name.
    #[test]
    fn an_imported_builtin_theme_never_shows_as_zeron_s() {
        let t = dir("posing");
        // A package from someone, installed by the engine as signed: its
        // manifest kept as it came, marked, with screens of its own.
        let posing = t.join("theme-catppuccin");
        std::fs::create_dir_all(posing.join("screens/shell")).unwrap();
        std::fs::write(posing.join("zeron-template.json"), r#"{"id":"theme-catppuccin","name":"Catppuccin","author":"Zeron","builtin_theme":"catppuccin"}"#).unwrap();
        std::fs::write(posing.join(FILE), "return nil").unwrap();
        std::fs::write(posing.join("screens/shell/sidebar.luau"), "-- takes over").unwrap();
        std::fs::write(posing.join(".imported.json"), r#"{"key":"k","author":{"kind":"unknown"}}"#).unwrap();
        // Shaped as a theme (just a manifest and a palette), but not this
        // build's palette: a palette is code too.
        let shaped = t.join("theme-ayu");
        std::fs::create_dir_all(&shaped).unwrap();
        std::fs::write(shaped.join("zeron-template.json"), serde_json::to_vec_pretty(&manifest(&find("ayu").unwrap(), "theme-ayu")).unwrap()).unwrap();
        std::fs::write(shaped.join(FILE), "ui.call('terminal.start', {}) return nil").unwrap();
        let listed = live::installed(&t);
        let theme_of = |id: &str| listed.iter().find(|i| i.id == id).map(|i| i.theme.clone());
        assert_eq!(theme_of("theme-catppuccin"), Some(None), "an ordinary template: its author's, deletable");
        assert_eq!(theme_of("theme-ayu"), Some(None), "not this build's bytes: not Zeron's");
        assert!(verified(&posing).is_none() && verified(&shaped).is_none());
        // Zeron's own built-in goes beside the posing one, never over it.
        let id = install(&t, "catppuccin").unwrap();
        assert_eq!(id, "theme-catppuccin-2");
        assert_eq!(std::fs::read_to_string(posing.join("screens/shell/sidebar.luau")).unwrap(), "-- takes over");
        assert_eq!(verified(&t.join(&id)).as_deref(), Some("catppuccin"));
        // The theme-shaped one is rewritten with Zeron's own palette, and
        // only then is it Zeron's.
        assert_eq!(install(&t, "ayu").unwrap(), "theme-ayu");
        assert_eq!(verified(&shaped).as_deref(), Some("ayu"));
    }

    #[test]
    fn a_theme_laid_over_zeron_brings_its_palette_and_back_brings_zeron_s() {
        let t = dir("compose");
        let z = t.join(live::ZERON);
        std::fs::create_dir_all(z.join("ds")).unwrap();
        std::fs::write(z.join("theme.luau"), "return nil\n").unwrap();
        std::fs::write(z.join("ds/tokens.luau"), "-- tokens").unwrap();
        std::fs::write(z.join("zeron-template.json"), "{}").unwrap();
        let id = install(&t, "tokyo-night").unwrap();
        let (_, overlay) = live::compose(&t, &id, &Default::default(), false).unwrap();
        let live_theme = std::fs::read_to_string(t.join(live::LIVE).join(FILE)).unwrap();
        assert!(live_theme.contains("Tokyo Night") && live_theme.contains("light = {") && live_theme.contains("dark = {"));
        assert_eq!(overlay.from_template.iter().cloned().collect::<Vec<_>>(), vec![FILE.to_string()], "colours only");
        assert_eq!(std::fs::read_to_string(t.join(live::LIVE).join("ds/tokens.luau")).unwrap(), "-- tokens");
        live::compose(&t, live::BUILTIN, &Default::default(), false).unwrap();
        assert_eq!(std::fs::read_to_string(t.join(live::LIVE).join(FILE)).unwrap(), "return nil\n");
    }

    // -- Each theme worn (ui/theme_templates_fixture.luau) -------------------

    use zeron_theme::Color;
    use zgx_script::{ScriptHost, Value, testing::TestApp};

    /// The roles every theme sets (the brief's: surfaces and glass, text,
    /// hairlines, accent, focus, selection, code and Markdown, diffs, git,
    /// statuses, the composer and the sidebar's rows), on top of whatever
    /// else the mapping writes.
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
    const SYNTAX: &[&str] = &[
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
            let entry = ui().join("theme_templates_fixture.luau");
            let app = TestApp::new(|runtime| ScriptHost::new(runtime, entry));
            app.frames(4);
            let keys: Vec<Json> = ROLES.iter().map(|k| json!(k)).chain(SYNTAX.iter().map(|k| json!(format!("zeron.syntax.{k}")))).collect();
            app.host.shared("theme_check.keys", Value::Nil).set(super::super::json_value(&Json::Array(keys)));
            Rig { app }
        }

        fn run(&self, command: &str) -> Json {
            assert!(self.app.host.run_command(command), "no command {command}");
            self.app.frames(3);
            assert!(self.app.host.run_command("read"));
            self.app.frames(2);
            super::super::value_json(&self.app.host.shared("theme_check.report", Value::Nil).signal().with_untracked(Value::clone))
        }

        fn wear(&self, family: &Json, kind: &str) -> Json {
            self.app.host.shared("theme_check.family", Value::Nil).set(super::super::json_value(family));
            self.app.host.shared("theme_check.kind", Value::Nil).set(Value::from(kind));
            self.run("wear")
        }

        fn zeron(&self, kind: &str) -> Json {
            self.app.host.shared("theme_check.kind", Value::Nil).set(Value::from(kind));
            self.run("zeron")
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

    #[test]
    fn every_theme_sets_every_role_and_reads() {
        let rig = Rig::new();
        let mut worn = 0;
        let mut fixes = Vec::new();
        let mut failures = Vec::new();
        for choice in choices() {
            let family = palettes(&choice);
            for v in &choice.variants {
                let kind = if v.appearance.is_dark() { "dark" } else { "light" };
                let report = rig.wear(&family, kind);
                let what = format!("{} ({kind})", v.name);
                assert_eq!(report["current"], format!("template-{kind}"), "{what}: {report}");
                worn += 1;
                let resolved = &report["resolved"];
                let written = report["written"].as_object().unwrap_or_else(|| panic!("{what}: nothing written: {report}"));
                // Every role the mapping promises, and every one it wrote,
                // resolved as written.
                for role in ROLES.iter().map(|r| r.to_string()).chain(SYNTAX.iter().map(|s| format!("zeron.syntax.{s}"))) {
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
                for s in SYNTAX {
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
        let variants: usize = choices().iter().map(|c| c.variants.len()).sum();
        assert_eq!(worn, variants);
        // What was moved from the source to stay legible (the deviations).
        eprintln!("{} inks moved to stay legible:\n{}", fixes.len(), fixes.join("\n"));
        if let Some(path) = std::env::var_os("ZGX_THEME_FIXES") {
            let _ = std::fs::write(path, fixes.join("\n"));
        }
        assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    }

    #[test]
    fn a_template_follows_the_system_and_one_palette_wears_it_in_both() {
        let rig = Rig::new();
        let tokyo = palettes(&find("tokyo-night").unwrap());
        let report = rig.wear(&tokyo, "light");
        assert_eq!(report["names"], json!({ "dark": "template-dark", "light": "template-light" }));
        assert_eq!(report["current"], "template-light");
        assert!(same(report["resolved"]["zeron.window"].as_str().unwrap(), "#d5d6db"), "{}", report["resolved"]["zeron.window"]);
        let report = rig.wear(&tokyo, "dark");
        assert_eq!(report["current"], "template-dark");
        assert!(same(report["resolved"]["zeron.window"].as_str().unwrap(), "#1a1b26"));
        // A template of yours with one palette (the capability is any
        // template's): worn in either scheme.
        let dark_only = builtin_registry().families.iter().flat_map(|f| &f.variants).find(|v| v.id == "dracula").unwrap();
        let mine = json!({ "name": "Mine", "dark": crate::themes::palette(dark_only, "") });
        let report = rig.wear(&mine, "light");
        assert_eq!(report["names"], json!({ "dark": "template-dark", "light": "template-dark" }));
        assert_eq!(report["current"], "template-dark");
    }

    #[test]
    fn wearing_a_theme_and_going_back_restores_zeron_s_own() {
        let rig = Rig::new();
        for kind in ["dark", "light"] {
            let before = rig.zeron(kind);
            assert_eq!(before["current"], format!("zeron-{kind}"));
            let theme = rig.wear(&palettes(&find("github").unwrap()), kind);
            assert_ne!(theme["asked"]["text.primary"], before["asked"]["text.primary"], "GitHub's text isn't Zeron's");
            assert_ne!(theme["asked"]["zeron.syntax.keyword"], before["asked"]["zeron.syntax.keyword"]);
            let after = rig.zeron(kind);
            assert_eq!(after["names"], json!({ "dark": "zeron-dark", "light": "zeron-light" }));
            assert_eq!(after["asked"], before["asked"], "every role back as Zeron's own ({kind})");
        }
    }

    /// Development only: each built-in theme's `theme.luau` written to
    /// `THEME_FILES_OUT/<key>.luau`, to lay over a copy of `ui/` for
    /// offscreen pictures of a theme (src/audit_shots.rs).
    #[test]
    #[ignore = "run on purpose: writes theme files for pictures"]
    fn write_theme_files() {
        let out = PathBuf::from(std::env::var("THEME_FILES_OUT").expect("THEME_FILES_OUT: where they go"));
        std::fs::create_dir_all(&out).unwrap();
        for c in choices() {
            std::fs::write(out.join(format!("{}.luau", c.key)), theme_file(&c)).unwrap();
        }
    }

    #[test]
    fn zeron_s_own_theme_file_wears_nothing() {
        let own = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("ui").join(FILE)).unwrap();
        assert!(own.lines().any(|l| l.trim() == "return nil"), "{own}");
    }
}
