//! A template's `ds/tokens.luau` beside Zeron's own (`live::plan`): Zeron's
//! tokens declared first, then the template's, so a token it names is its
//! and every other one is the running Zeron's, those added after it was made
//! too. Run on the store's own tree, `.live` laid down as the app does.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use zgx_script::{ScriptHost, testing::TestApp};

use crate::plugins::live;

/// `ds/tokens.luau` as Zeron 0.3.20 shipped it: before the letter tiles
/// (`zeron.tile.*`) and the risky ask's ink (`zeron.asks.risky`).
const TOKENS_0_3_20: &str = include_str!("template_tokens_v0_3_20.luau.txt");

/// The fewest a template needs: one token of its own.
const MINIMAL: &str = r#"
return {
	setup = function()
		ui.theme.tokens { zeron = { list = { gap = 9 } } }
	end,
}
"#;

/// Tokens Zeron has that 0.3.20's file hadn't, and some it had.
const ADDED: [&str; 3] = ["zeron.tile.red", "zeron.tile.blue", "zeron.asks.risky"];
const NAMES: [&str; 8] = ["zeron.tile.red", "zeron.tile.blue", "zeron.asks.risky", "zeron.inset.gap", "zeron.space.tight", "zeron.list.gap", "menu.hover", "menu.bg"];

/// What the probe reads, by token, as the material and the scheme change;
/// `ds.setup()` run as `setup` says (`pcall`ed: what it threw, as `said`).
fn probe(guarded: bool) -> String {
    let names = NAMES.iter().map(|n| format!("{n:?}")).collect::<Vec<_>>().join(", ");
    let setup = if guarded {
        r#"local ok, why = pcall(ds.setup)
ui.state("tokens_said", ""):set(if ok then "" else tostring(why))"#
    } else {
        "ds.setup()"
    };
    format!(
        r#"
local ds = require("@ds")
{setup}
local material = require("@ds/material")
local NAMES = {{ {names} }}
local seen = ui.state("tokens_seen", {{}})
ui.effect(function()
	material.frosted:get()
	ui.theme.current:get()
	local out = {{}}
	for _, name in NAMES do
		out[name] = tostring(ui.theme.get(name))
	end
	seen:set(out)
end)
ui.command {{ id = "solid", title = "Solid", run = function()
	material.frosted:set(not material.frosted:peek())
end }}
for _, scheme in {{ "dark", "light" }} do
	ui.command {{ id = scheme, title = scheme, run = function()
		ui.theme.follow_system(false)
		ui.theme.current:set(`theme-{{scheme}}`)
	end }}
end
return ui.column {{ ui.text {{ "probe" }} }}
"#
    )
}

fn ui_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn templates(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("zgx-template-tokens-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let t = root.join("templates");
    live::sync_zeron(&ui_dir(), &t).unwrap();
    t
}

fn template(t: &Path, id: &str, tokens: &str) {
    let dir = t.join(id);
    std::fs::create_dir_all(dir.join("ds")).unwrap();
    std::fs::write(dir.join("zeron-template.json"), format!(r#"{{"id":"{id}","name":"{id}","zeron_version":"0.3.20"}}"#)).unwrap();
    std::fs::write(dir.join("ds/tokens.luau"), tokens).unwrap();
}

struct Probe {
    app: TestApp,
    overlay: live::Overlay,
}

impl Probe {
    /// `.live` for `id` (its `failed` files Zeron's), the probe run on it.
    fn on(t: &Path, id: &str, failed: &BTreeSet<String>, guarded: bool) -> Self {
        let (_, overlay) = live::compose(t, id, failed, false).unwrap();
        let entry = t.join(live::LIVE).join("tokens_probe.luau");
        std::fs::write(&entry, probe(guarded)).unwrap();
        let app = TestApp::new(move |runtime| ScriptHost::new(runtime, &entry));
        app.frames(4);
        Self { app, overlay }
    }

    fn of(t: &Path, id: &str) -> Self {
        let p = Self::on(t, id, &BTreeSet::new(), false);
        assert!(p.app.error().is_none(), "{id}: {:?}", p.app.error());
        p
    }

    fn state(&self, name: &str) -> serde_json::Value {
        self.app
            .host
            .states()
            .into_iter()
            .find(|(n, _)| n == name || n.ends_with(&format!("::{name}")))
            .map_or(serde_json::Value::Null, |(_, v)| serde_json::from_str(&v.to_json()).unwrap_or_default())
    }

    fn seen(&self) -> serde_json::Value {
        self.state("tokens_seen")
    }

    fn run(&self, command: &str) -> serde_json::Value {
        assert!(self.app.host.run_command(command), "{command}");
        self.app.frames(4);
        assert!(self.app.error().is_none(), "{command}: {:?}", self.app.error());
        self.seen()
    }
}

/// Every token of `NAMES` but `except` as Zeron's own reads it.
fn zeron_s_but(got: &serde_json::Value, zeron: &serde_json::Value, except: &[&str], what: &str) {
    for name in NAMES.iter().filter(|n| !except.contains(n)) {
        assert_eq!(got[*name], zeron[*name], "{what}: {name} is the running Zeron's");
    }
}

#[test]
fn an_old_template_s_tokens_keep_what_zeron_added_since_and_what_it_names_wins() {
    let t = templates("old");
    let zeron = Probe::of(&t, live::BUILTIN).seen();
    for name in ADDED {
        assert_ne!(zeron[name], "nil", "Zeron's own has {name}: {zeron}");
    }
    assert_eq!(zeron["zeron.space.tight"], "6");

    // 0.3.20's file, as a template made then has it, one value its own.
    assert!(!TOKENS_0_3_20.contains("risky") && !TOKENS_0_3_20.contains("tile = {"), "0.3.20 had neither");
    let named = TOKENS_0_3_20.replacen("tight = 6,", "tight = 7,", 1);
    assert_ne!(named, TOKENS_0_3_20);
    template(&t, "old", &named);
    let old = Probe::of(&t, "old").seen();
    zeron_s_but(&old, &zeron, &["zeron.space.tight"], "0.3.20's");
    assert_eq!(old["zeron.space.tight"], "7", "the template's own value wins: {old}");

    // Back to Zeron's own: its tokens as they were.
    assert_eq!(Probe::of(&t, live::BUILTIN).seen(), zeron);
}

/// A template that starts from Zeron's own file as it is now (which runs
/// the template's tokens at its end): laid as the template's, it runs once,
/// not itself again, and what it names wins.
#[test]
fn a_template_s_copy_of_zeron_s_tokens_runs_once() {
    let t = templates("copy");
    let zeron = Probe::of(&t, live::BUILTIN).seen();
    let own = std::fs::read_to_string(ui_dir().join("ds/tokens.luau")).unwrap();
    let copy = own.replacen("tight = 6,", "tight = 7,", 1);
    assert_ne!(copy, own);
    template(&t, "copy", &copy);
    let seen = Probe::of(&t, "copy").seen();
    assert_eq!(seen["zeron.space.tight"], "7", "{seen}");
    zeron_s_but(&seen, &zeron, &["zeron.space.tight"], "a copy of Zeron's own");
}

/// What the guide tells authors to write: only the tokens they change.
#[test]
fn a_template_naming_one_token_has_zeron_s_for_every_other() {
    let t = templates("minimal");
    let zeron = Probe::of(&t, live::BUILTIN).seen();
    assert_eq!(zeron["zeron.list.gap"], "2");
    template(&t, "one", MINIMAL);
    let one = Probe::of(&t, "one");
    let seen = one.seen();
    assert_eq!(seen["zeron.list.gap"], "9", "{seen}");
    zeron_s_but(&seen, &zeron, &["zeron.list.gap"], "one token's");
    // And so in either scheme, with either material.
    for command in ["light", "solid", "dark", "solid"] {
        let seen = one.run(command);
        assert_eq!(seen["zeron.list.gap"], "9", "after {command}: {seen}");
        assert_ne!(seen["zeron.tile.red"], "nil", "after {command}: {seen}");
    }
}

/// A template's `ds/tokens/` folder: Luau would take it for Zeron's
/// `ds/tokens.luau` (its init.luau, or none at all, an error), so it's never
/// laid, and Zeron's tokens are what run.
#[test]
fn a_template_s_tokens_folder_leaves_zeron_s_tokens_in_place() {
    let t = templates("folder");
    let zeron = Probe::of(&t, live::BUILTIN).seen();
    for (id, rel) in [("init", "ds/tokens/init.luau"), ("other", "ds/tokens/other.luau")] {
        let dir = t.join(id);
        std::fs::create_dir_all(dir.join("ds/tokens")).unwrap();
        std::fs::write(dir.join("zeron-template.json"), format!(r#"{{"id":"{id}","name":"{id}"}}"#)).unwrap();
        std::fs::write(dir.join(rel), "return { setup = function() end }").unwrap();
        let p = Probe::of(&t, id);
        assert!(!t.join(live::LIVE).join("ds/tokens").exists(), "{id}: never laid");
        assert_eq!(p.seen(), zeron, "{id}: Zeron's tokens");
    }
}

/// A `.lua` file beside the ones laid (`ds/tokens_template.lua`,
/// `ds/tokens.lua`): zgx requires a module by its `.luau` (or a folder's
/// init.luau), so they're never what runs.
#[test]
fn a_lua_file_beside_the_tokens_is_never_what_runs() {
    let t = templates("twin");
    let zeron = Probe::of(&t, live::BUILTIN).seen();
    template(&t, "twin", MINIMAL);
    for rel in ["ds/tokens_template.lua", "ds/tokens.lua"] {
        std::fs::write(t.join("twin").join(rel), "error(\"not this one\")\nreturn {}").unwrap();
    }
    let p = Probe::of(&t, "twin");
    assert!(t.join(live::LIVE).join("ds/tokens_template.lua").is_file(), "laid, and never required");
    let seen = p.seen();
    assert_eq!(seen["zeron.list.gap"], "9", "the laid .luau ran: {seen}");
    zeron_s_but(&seen, &zeron, &["zeron.list.gap"], "beside a .lua");
}

/// A template's tokens that don't load, or throw in `setup`: Zeron's are all
/// set (they ran first), the error is its `ds/tokens.luau`'s, and once that
/// falls back the window runs on Zeron's own.
#[test]
fn a_template_s_tokens_that_fail_leave_zeron_s_set_and_are_blamed() {
    let t = templates("fail");
    let zeron = Probe::of(&t, live::BUILTIN).seen();
    for (id, tokens) in [
        ("unparsed", "return { setup = function( end }"),
        ("thrown", "error(\"tokens broke on load\")\nreturn {}"),
        ("in-setup", "return { setup = function()\n\tui.theme.tokens { zeron = { list = { gap = 9 } } }\n\terror(\"tokens broke in setup\")\nend }"),
    ] {
        template(&t, id, tokens);
        // Caught: every one of Zeron's set.
        let caught = Probe::on(&t, id, &BTreeSet::new(), true);
        let said = caught.state("tokens_said");
        let said = said.as_str().unwrap_or_default();
        assert!(said.contains("tokens_template.luau"), "{id}: {said}");
        let seen = caught.seen();
        let named = if id == "in-setup" { &["zeron.list.gap"][..] } else { &[] };
        zeron_s_but(&seen, &zeron, named, id);

        // As the window has it: the error is the template's ds/tokens.luau.
        // (It fails to load: the window's first error.)
        let (_, overlay) = live::compose(&t, id, &BTreeSet::new(), false).unwrap();
        let entry = t.join(live::LIVE).join("tokens_probe.luau");
        std::fs::write(&entry, probe(false)).unwrap();
        let error = TestApp::try_new(move |runtime| ScriptHost::new(runtime, &entry)).err().unwrap_or_else(|| panic!("{id}: no error"));
        assert_eq!(live::blame(&error, &t, &overlay).as_deref(), Some("ds/tokens.luau"), "{id}: {error}");

        // Set aside, as the app does: Zeron's stub in its place, Zeron's own.
        assert_eq!(failed_over(&t, id), zeron, "{id}");
    }
}

/// The probe on `id` with its tokens set aside as failed.
fn failed_over(t: &Path, id: &str) -> serde_json::Value {
    let p = Probe::on(t, id, &BTreeSet::from(["ds/tokens.luau".to_string()]), false);
    assert!(p.app.error().is_none(), "{id}: {:?}", p.app.error());
    assert!(p.overlay.laid_as.is_empty());
    p.seen()
}

/// The menus' tint is made in an effect, in Zeron's file and in a copied
/// one: as the material changes either way, in either scheme, the
/// template's still wins (it runs after Zeron's).
#[test]
fn the_menus_tint_stays_the_template_s_as_the_material_and_scheme_change() {
    let t = templates("menus");
    let from = r#"bg = if through then "{zeron.menu.tint}" else "{zeron.menu.solid}","#;
    assert!(TOKENS_0_3_20.contains(from));
    template(&t, "menus", &TOKENS_0_3_20.replacen(from, r##"bg = if through then "#a1a1a1" else "#b2b2b2","##, 1));
    let zeron = Probe::of(&t, live::BUILTIN);
    let p = Probe::of(&t, "menus");
    for scheme in ["dark", "light"] {
        let z = zeron.run(scheme);
        let seen = p.run(scheme);
        assert_ne!(z["menu.bg"], seen["menu.bg"], "{scheme}: Zeron's differs, or nothing is shown");
        assert_eq!(seen["menu.bg"], "#a1a1a1ff", "{scheme}, frosted: {seen}");
        // Solid, then frosted again.
        let seen = p.run("solid");
        assert_eq!(seen["menu.bg"], "#b2b2b2ff", "{scheme}, solid: {seen}");
        assert_ne!(zeron.run("solid")["menu.bg"], seen["menu.bg"]);
        let seen = p.run("solid");
        assert_eq!(seen["menu.bg"], "#a1a1a1ff", "{scheme}, frosted again: {seen}");
        zeron.run("solid");
    }
    // Starting solid, and frosted after.
    let p = Probe::of(&t, "menus");
    assert_eq!(p.run("solid")["menu.bg"], "#b2b2b2ff");
    assert_eq!(p.run("light")["menu.bg"], "#b2b2b2ff");
    assert_eq!(p.run("solid")["menu.bg"], "#a1a1a1ff");
}

// ── A template's theme files ────────────────────────────────────────────────

/// What the dark scheme shows of two tokens Zeron's theme file sets, the
/// selected wash (one a palette makes too) and a tab's: on template `id`,
/// with `library` (the host's list, a palette you chose in it) if given.
fn theme_seen(t: &Path, id: &str, library: Option<&str>) -> serde_json::Value {
    const PROBE: &str = r#"
local ds = require("@ds")
ds.setup()
ui.theme.follow_system(false)
ui.theme.current:set("theme-dark")
local seen = ui.state("theme_seen", {})
ui.effect(function()
	ui.theme.current:get()
	seen:set({ flat = tostring(ui.theme.get("zeron.selected.flat.bg")), tab = tostring(ui.theme.get("zeron.tab.selected")) })
end)
return ui.column { ui.text { "probe" } }
"#;
    live::compose(t, id, &Default::default(), false).unwrap();
    let entry = t.join(live::LIVE).join("theme_probe.luau");
    std::fs::write(&entry, PROBE).unwrap();
    let library = library.and_then(zgx_script::Value::from_json);
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, &entry);
        if let Some(library) = library {
            host.shared("themes.library", library);
        }
        host
    });
    app.frames(4);
    assert!(app.error().is_none(), "{id}: {:?}", app.error());
    app.host
        .states()
        .into_iter()
        .find(|(n, _)| n.ends_with("theme_seen"))
        .map_or(serde_json::Value::Null, |(_, v)| serde_json::from_str(&v.to_json()).unwrap_or_default())
}

/// A template's own palette (its theme.luau), light text on near black.
const ITS_PALETTE: &str = r##"return {
	name = "Its own",
	dark = {
		id = "its-dark", name = "Its own", kind = "dark",
		background = "#101010", raised = "#1c1c1c", text = "#f0f0f0", muted = "#a0a0a0", faint = "#606060",
		accent = "#3366ff", danger = "#ff3333", warning = "#ffaa00", success = "#33cc66",
		syntax = { "#777777", "#cc66ff", "#66cc66", "#ffaa44", "#44aaff", "#ffcc44", "#66ccff", "#eeeeee", "#999999", "#ff6688", "#ffbb66", "#ff4444" },
	},
}"##;

/// A template's theme file naming one token.
const ONE_TOKEN: &str = r##"{
	// Only what it changes.
	"name": "zeron-dark",
	"type": "dark",
	"tokens": { "zeron": { "selected": { "flat": { "bg": "#123456" } } } }
}"##;

/// Zeron's own theme file, the template's own palette, the template's theme
/// file, a palette you chose, each over the one before: what a template's
/// file names wins over what its own palette makes of it; a palette you
/// chose wins over the template; a token the file doesn't name is what's
/// under it (its palette's, else Zeron's, those added since it was copied
/// too); with no template, nothing changes.
#[test]
fn a_template_s_theme_file_sits_over_its_palette_and_under_yours() {
    let t = templates("theme-order");
    let zeron = theme_seen(&t, live::BUILTIN, None);
    assert_eq!(zeron["tab"], "#ebebeb1a", "{zeron}");

    // Its own palette alone: the palette's wash.
    let dir = t.join("palette-only");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("zeron-template.json"), r#"{"id":"palette-only","name":"palette-only"}"#).unwrap();
    std::fs::write(dir.join("theme.luau"), ITS_PALETTE).unwrap();
    let palette = theme_seen(&t, "palette-only", None);
    assert_ne!(palette["flat"], zeron["flat"], "its palette makes the wash: {palette}");

    // With its theme file naming the wash: the file wins, the rest Zeron's.
    let dir = t.join("both");
    std::fs::create_dir_all(dir.join("ds/themes")).unwrap();
    std::fs::write(dir.join("zeron-template.json"), r#"{"id":"both","name":"both"}"#).unwrap();
    std::fs::write(dir.join("theme.luau"), ITS_PALETTE).unwrap();
    std::fs::write(dir.join("ds/themes/zeron-dark.json"), ONE_TOKEN).unwrap();
    let both = theme_seen(&t, "both", None);
    assert_eq!(both["flat"], "#123456ff", "its file over its palette: {both}");
    assert_eq!(both["tab"], palette["tab"], "what it doesn't name is its palette's, as without the file");

    // A palette you chose: over the template's file.
    let library = r##"{ "state": "ready", "entries": [], "chosen": { "dark": "yours" }, "list": [
        { "id": "yours", "name": "Yours", "kind": "dark", "background": "#000000", "raised": "#111111", "text": "#00ff00",
          "muted": "#00aa00", "faint": "#005500", "accent": "#ff00ff", "danger": "#ff0000", "warning": "#ffff00", "success": "#00ffff",
          "syntax": ["#777777", "#cc66ff", "#66cc66", "#ffaa44", "#44aaff", "#ffcc44", "#66ccff", "#eeeeee", "#999999", "#ff6688", "#ffbb66", "#ff4444"] } ] }"##;
    let yours = theme_seen(&t, "both", Some(library));
    assert_ne!(yours["flat"], "#123456ff", "yours over the template: {yours}");
    assert_ne!(yours["flat"], palette["flat"]);

    // An old full copy of Zeron's file, missing what was added since.
    let own = std::fs::read_to_string(ui_dir().join("ds/themes/zeron-dark.json")).unwrap();
    let line = own.lines().find(|l| l.trim_start().starts_with("\"tab\": {")).unwrap();
    let old = own.replacen(line, "", 1);
    assert_ne!(old, own);
    let dir = t.join("old-copy");
    std::fs::create_dir_all(dir.join("ds/themes")).unwrap();
    std::fs::write(dir.join("zeron-template.json"), r#"{"id":"old-copy","name":"old-copy"}"#).unwrap();
    std::fs::write(dir.join("ds/themes/zeron-dark.json"), old).unwrap();
    assert_eq!(theme_seen(&t, "old-copy", None)["tab"], zeron["tab"], "what it lacks is Zeron's");

    // No template: as before.
    assert_eq!(theme_seen(&t, live::BUILTIN, None), zeron);
}
