//! The design lint finds nothing: no hard-coded colour, spacing, radius,
//! type size or shadow anywhere the app builds (README, "The design
//! system's rules", 4).
//!
//! The lint runs as nodes are built (`ui.theme.strict("warn")`), so it sees
//! only what mounts. This mounts the gallery and opens every page of it,
//! the main window, and setup at each of its steps, every host action a
//! stub, and gathers what `ScriptHost::warnings()` kept.

use std::{collections::BTreeSet, path::PathBuf};

use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// Every host action the scripts name (`ui.call("…")`, `ui.invoke("…")`,
/// or through a wrapper's `call("…")`), read from them, so a new one needs
/// no list kept here. Each is stubbed both ways (calls and invokes are kept
/// apart, so one name can be both).
fn actions() -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut dirs = vec![ui_dir()];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                dirs.push(path);
                continue;
            }
            if path.extension().is_none_or(|e| e != "luau") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            for needle in ["call(\"", "invoke(\""] {
                for (at, _) in text.match_indices(needle) {
                    let rest = &text[at + needle.len()..];
                    let Some(end) = rest.find('"') else { continue };
                    let name = &rest[..end];
                    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                        names.insert(name.to_owned());
                    }
                }
            }
        }
    }
    // The host's own window actions, which scripts reach in other ways.
    for name in ["appearance", "close", "interactive", "prepare", "reveal", "done"] {
        names.insert(name.to_owned());
    }
    names
}

/// What the stubs answer where nothing won't do (a list the script walks).
const ANSWERS: &[(&str, &str)] = &[("account.orgs", "[]"), ("account.onboarding", "[]")];

/// `entry` mounted headless, every action a stub answering nothing.
fn mount(entry: &str, shared: &[(&'static str, &'static str)]) -> TestApp {
    let entry = entry.to_owned();
    let shared = shared.to_vec();
    TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join(&entry));
        for (key, value) in &shared {
            host.shared(key, *value);
        }
        host.shared("onboarding.arrived", true);
        for name in actions() {
            host.action(name.clone(), |_| {});
            let answer = ANSWERS
                .iter()
                .find(|(n, _)| *n == name)
                .and_then(|(_, json)| Value::from_json(json))
                .unwrap_or(Value::Nil);
            host.async_action(name, move |_| {
                let answer = answer.clone();
                async move { Ok(answer) }
            });
        }
        host
    })
}

/// The window made `w` by `h` points, as a person's would be: an action
/// acts where a person would, so a target off the window does nothing.
fn resize(app: &TestApp, w: f32, h: f32) {
    app.ui.scene.borrow_mut().resize(w, h);
    app.frames(2);
}

/// The design lint's findings so far, a line each.
fn findings(app: &TestApp) -> Vec<String> {
    app.host.warnings().into_iter().filter(|w| w.contains("design lint")).collect()
}

/// The gallery's pages, by their names in its sidebar.
fn gallery_pages(app: &mut TestApp) -> Vec<String> {
    let tree = app.tree();
    let mut names = Vec::new();
    let mut in_nav = false;
    for line in tree.lines() {
        let depth = line.len() - line.trim_start().len();
        // Its sidebar: the first scroll under the root, a button a page.
        if !in_nav && line.trim_start().contains(" scroll ") && depth <= 4 {
            in_nav = true;
            continue;
        }
        if in_nav {
            if depth <= 2 {
                break;
            }
            let line = line.trim_start();
            if let Some(at) = line.find(" button \"") {
                let rest = &line[at + 9..];
                if let Some(end) = rest.find('"') {
                    names.push(rest[..end].to_owned());
                }
            }
        }
    }
    names
}

/// The options of the radio group named `group`, as the tree shows them.
fn radios(app: &mut TestApp, group: &str) -> Vec<String> {
    let tree = app.tree();
    let mut out = Vec::new();
    let mut depth: Option<usize> = None;
    for line in tree.lines() {
        let d = line.len() - line.trim_start().len();
        let line = line.trim_start();
        match depth {
            None if line.contains(&format!(" radiogroup \"{group}\"")) => depth = Some(d),
            Some(g) if d <= g => break,
            Some(_) => {
                if let Some(at) = line.find(" radio \"") {
                    let rest = &line[at + 8..];
                    if let Some(end) = rest.find('"') {
                        out.push(rest[..end].to_owned());
                    }
                }
            }
            None => {}
        }
    }
    out
}

#[test]
fn nothing_hard_codes_a_style() {
    let mut all: BTreeSet<String> = BTreeSet::new();

    // The gallery: every page opened.
    let mut gallery = mount("gallery.luau", &[]);
    // Tall enough for every page's name in its sidebar to be on the window.
    resize(&gallery, 1400., 2800.);
    gallery.frames(30);
    let pages = gallery_pages(&mut gallery);
    assert!(pages.len() > 20, "the gallery's pages weren't found: {pages:?}");
    for page in &pages {
        gallery.click(&format!("role:button \"{page}\"[1]"));
        gallery.frames(20);
        assert!(gallery.error().is_none(), "the gallery's {page} page failed: {:?}", gallery.error());
        // (`ZERON_ZGX_TEST_DUMP=1`: each page's laid-out tree, to compare
        // two copies of the scripts.)
        if std::env::var_os("ZERON_ZGX_TEST_DUMP").is_some() {
            eprintln!("PAGE {page}\n{}", gallery.tree());
        }
        // An example's states (its "State" switch), each shown in turn.
        for state in radios(&mut gallery, "State") {
            gallery.click(&format!("role:radiogroup \"State\" >> role:radio \"{state}\""));
            gallery.frames(10);
        }
    }
    all.extend(findings(&gallery));

    // The main window, and setup at each of its steps.
    let main = mount("app.luau", &[("window", "main")]);
    resize(&main, 1280., 800.);
    main.frames(60);
    assert_eq!(main.error(), None, "the main window failed to build, so the lint saw only part of it");
    all.extend(findings(&main));
    for step in ["signin", "appearance", "username", "card", "org", "invite", "github", "providers", "done"] {
        let setup = mount("app.luau", &[("window", "onboarding"), ("onboarding.start", step)]);
        resize(&setup, 680., 500.);
        setup.frames(60);
        assert_eq!(setup.error(), None, "setup at {step} failed to build");
        all.extend(findings(&setup));
    }

    assert!(all.is_empty(), "{} design lint findings:\n{}", all.len(), all.into_iter().collect::<Vec<_>>().join("\n"));
}
