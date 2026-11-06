//! The `imgui.windows` gate, at both layers it exists.
//!
//! The VM layer is structural: the ImGui host action is exposed behind
//! `zeron.imgui.windows`, so a plugin not granted it cannot even reach the
//! action, and `windows` (zgx's declarative windows) is a different
//! capability. The per-call layer is [`permission::allowed`], which also
//! weighs the plugin's status and what the engine now allows, because a VM's
//! grant is as old as its last load.
//!
//! These run headless: no window is opened, and none is created by the gate.

use serde_json::json;
use zgx_script::testing::TestApp;
use zgx_script::{Plugin, ScriptHost, Value};

use crate::plugins::imgui::permission;
use crate::plugins::{Item, grants};

/// A plugin's listing, as the engine would send it. `allowed` is what the
/// engine allowed (absent in a dev plugin's made-up listing).
fn listing(status: &str, asked: &[&str], allowed: Option<&[&str]>) -> Item {
    let mut json = json!({ "id": "plug", "status": status, "permissions": asked });
    if let Some(allowed) = allowed {
        json["allowed"] = json!(allowed);
    }
    Item { json }
}

/// The plugin's own code: it asks the ImGui action for a window, and says
/// what happened to a text node the test reads. A VM without the capability
/// fails before the action runs; a granted one reaches the gate.
const ASKING: &str = r#"
local out = ui.state("out", "nothing")
local ok, failed = pcall(function()
	local answer = ui.call_sync("plugin.plug.imgui", {})
	if answer.ok then
		out:set("reached")
	else
		out:set(answer.error)
	end
end)
if not ok then
	out:set(tostring(failed))
end
ui.contribute("bar", ui.text { id = "out", function() return out:get() end })
"#;

/// A headless app whose plugin `plug` holds `granted` in its VM while its
/// listing is `listing`.
fn asking_app(name: &str, listing: &Item, granted: &[&str]) -> TestApp {
    let dir = std::env::temp_dir().join(format!(
        "zgx-imgui-permission-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let entry = dir.join("init.luau");
    std::fs::write(&entry, ASKING).unwrap();
    let listing = listing.clone();
    let asked: Vec<String> = granted.iter().map(|p| p.to_string()).collect();
    let grants = grants::capabilities("plug", &asked, &[], &[]);
    TestApp::new(move |runtime| {
        let host = ScriptHost::inline(runtime, "app", r#"return ui.column { ui.slot("bar") }"#);
        permission::expose_action(&host, "plugin.plug.imgui");
        let item = listing.clone();
        host.function(
            "plugin.plug.imgui",
            move |_| match permission::open_window(Some(&item)) {
                Ok(()) => Value::record([("ok", Value::from(true))]),
                Err(sentence) => Value::record([
                    ("ok", Value::from(false)),
                    ("error", Value::from(sentence.as_str())),
                ]),
            },
        );
        host.set_plugins(vec![Plugin {
            name: "plug".into(),
            entry,
            grants,
        }]);
        host
    })
}

/// What the plugin's script ended up saying.
fn plugin_said(app: &mut TestApp) -> String {
    app.frames(30);
    assert!(
        app.error().is_none(),
        "the plugin didn't load: {:?}",
        app.error()
    );
    app.text("out")
}

#[test]
fn a_plugin_without_the_grant_cannot_reach_the_imgui_action() {
    for (name, asked) in [("nothing", vec![]), ("declarative", vec!["windows"])] {
        let item = listing("on", &asked, None);
        let mut app = asking_app(name, &item, &asked);
        let said = plugin_said(&mut app);
        assert!(said.contains("zeron.imgui.windows"), "{name}: {said}");
        assert!(
            !said.contains("reached"),
            "{name}: the host action ran without the grant: {said}"
        );
    }
}

#[test]
fn only_the_imgui_grant_opens_the_action_and_the_gate() {
    let item = listing("on", &["imgui.windows"], Some(&["imgui.windows"]));
    let mut app = asking_app("granted", &item, &["imgui.windows"]);
    assert_eq!(plugin_said(&mut app), "reached");
}

#[test]
fn a_plugin_asking_for_windows_alone_reaches_nothing_of_imgui() {
    // The manifest was edited to "windows" alone: the listing asks for it,
    // and that is all the VM holds. Neither the VM nor the gate accepts it.
    let item = listing("on", &["windows"], Some(&["windows"]));
    let mut app = asking_app("bypass", &item, &["windows"]);
    let said = plugin_said(&mut app);
    assert!(said.contains("zeron.imgui.windows"), "{said}");
    assert!(!said.contains("reached"), "{said}");
    assert!(
        permission::open_window(Some(&item)).is_err(),
        "the gate refuses it too"
    );
}

#[test]
fn a_granted_plugin_that_is_not_on_is_refused_on_every_call() {
    for status in ["off", "pending", "tampered", "broken"] {
        // The VM still holds what it was granted at load; the status is
        // checked again on the call, so no new window can start.
        let item = listing(status, &["imgui.windows"], Some(&["imgui.windows"]));
        let mut app = asking_app(status, &item, &["imgui.windows"]);
        let said = plugin_said(&mut app);
        assert!(said.contains("isn't on"), "{status}: {said}");
        assert!(
            said.contains("imgui.windows"),
            "{status}: the refusal names the permission: {said}"
        );
        assert!(!said.contains("reached"), "{status}: {said}");
    }
}

#[test]
fn a_grant_taken_back_refuses_the_next_window_even_while_on() {
    let taken = listing("on", &["imgui.windows", "ui.add"], Some(&["ui.add"]));
    let mut app = asking_app("revoked", &taken, &["imgui.windows", "ui.add"]);
    let said = plugin_said(&mut app);
    assert!(said.contains("wasn't allowed"), "{said}");
    assert!(said.contains("imgui.windows"), "{said}");
    assert!(!said.contains("reached"), "{said}");

    // The same listing with the grant still allowed opens it.
    let kept = listing(
        "on",
        &["imgui.windows", "ui.add"],
        Some(&["imgui.windows", "ui.add"]),
    );
    let mut app = asking_app("kept", &kept, &["imgui.windows", "ui.add"]);
    assert_eq!(plugin_said(&mut app), "reached");
}

#[test]
fn every_refusal_names_the_permission_and_where_to_allow_it() {
    for item in [
        listing("on", &[], None),
        listing("on", &["windows"], None),
        listing("off", &["imgui.windows"], Some(&["imgui.windows"])),
        listing("on", &["imgui.windows"], Some(&[])),
        Item::default(),
    ] {
        let refused = permission::open_window(Some(&item)).expect_err("refused");
        assert!(refused.contains("imgui.windows"), "{refused}");
        assert!(
            refused.contains("Zeron"),
            "it says where to allow it: {refused}"
        );
        assert!(!refused.contains("windows\" alone"), "{refused}");
    }
    let absent = permission::open_window(None).expect_err("no listing, no window");
    assert!(
        absent.contains("imgui.windows") && absent.contains("Zeron"),
        "{absent}"
    );
}

#[test]
fn the_capability_is_the_imgui_one_and_not_zgx_s_windows() {
    use zgx_script::Capability;
    assert_eq!(
        permission::capability(),
        Capability::Named(permission::CAPABILITY.to_string())
    );
    assert_eq!(permission::CAPABILITY, "zeron.imgui.windows");
    assert_ne!(permission::CAPABILITY, "zeron.windows");
    assert_eq!(permission::PERMISSION, "imgui.windows");
}

/// The consent sheet says a plugin asks to create native application
/// windows, in words of its own, and never as zgx's declarative windows.
#[test]
fn the_consent_sheet_says_native_application_windows() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/consent_fixture.luau");
    let mut app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, path.clone());
        crate::plugins::lend_design_system(host, &path)
    });
    app.frames(20);
    assert_eq!(app.error(), None, "the sheet failed: {:?}", app.error());
    app.host.run_command("imgui");
    app.frames(20);
    let tree = app.tree();
    assert!(tree.contains("Create native application windows"), "{tree}");
    assert!(
        tree.contains("Dear ImGui"),
        "the detail says what it draws: {tree}"
    );
    assert!(
        !tree.contains("Open windows of its own"),
        "never zgx's \"windows\" words: {tree}"
    );

    // A plugin asking for both shows two entries with different words.
    app.host.run_command("long");
    app.frames(20);
    let both = app.tree();
    let imgui = both
        .find("Create native application windows")
        .expect("the ImGui row");
    let windows = both
        .find("Open windows of its own")
        .expect("zgx's windows row");
    assert_ne!(imgui, windows, "two rows, each its own");
}
