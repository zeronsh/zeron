//! A toast's button outlives a load of the scripts, and what's waiting for
//! you to allow it stays said (ds/toast, screens/plugins/waiting): the half
//! of ui/tests/waiting_toasts.test.luau its kit can't do. The reported bug
//! (Nov 3): the host told the app a plugin was waiting and, in the same turn,
//! reloaded the scripts to load the others, as every launch does. The toast
//! (state) survived; what its button ran (a function in the old VM) didn't,
//! so it stood there with no button.
//!
//! `ZERON_ZGX_TEST_UI` points these at a scratch copy of ui/, to show them
//! failing on old code (as tests/reload.rs).

use std::path::{Path, PathBuf};

use zgx_script::{ScriptHost, testing::TestApp};

fn ui() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI").map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"), PathBuf::from)
}

/// The fixture, its `ui.persist` keeping to `storage`.
fn launch(storage: &Path) -> TestApp {
    let path = ui().join("waiting_toasts_fixture.luau");
    let storage = storage.to_path_buf();
    let app = TestApp::new(move |runtime| ScriptHost::new(runtime, path).storage(storage));
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(30);
    assert_eq!(app.error(), None);
    app
}

fn storage(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("zgx-waiting-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// How many times the tree says `words`.
fn count(app: &mut TestApp, words: &str) -> usize {
    app.tree().matches(words).count()
}

/// How many cards have a Review. (A card is a `status`, and the shell has no
/// other: the tree lists a card's words, then its buttons.)
fn review_count(app: &mut TestApp) -> usize {
    let tree = app.tree();
    let mut in_card = false;
    let mut n = 0;
    for line in tree.lines() {
        if line.trim_start().split_whitespace().nth(1) == Some("status") {
            in_card = true;
        } else if in_card && line.contains(" button \"Review\"") {
            n += 1;
            in_card = false;
        }
    }
    n
}

/// A card's words, then the first button so called after them.
fn button_in_card(app: &mut TestApp, words: &str, name: &str) -> String {
    let tree = app.tree();
    let mut seen = false;
    for line in tree.lines() {
        if line.contains(&format!("\"{words}\"")) {
            seen = true;
        } else if seen && line.contains(&format!(" button \"{name}\"")) {
            let at = line.trim_start().split_whitespace().next().unwrap();
            return at.to_string();
        }
    }
    panic!("no {name} in the card saying {words}:\n{tree}");
}

/// What Review asked of the host, as the fixture kept it.
fn calls(app: &TestApp) -> String {
    let found = app.host.states().into_iter().find(|(name, _)| name == "waiting_calls");
    found.map_or_else(String::new, |(_, v)| v.to_json())
}

#[test]
fn a_notice_shown_as_the_scripts_reload_keeps_its_review_and_it_opens_each() {
    let dir = storage("same-turn");
    let mut app = launch(&dir);
    // As `Bridge::heard` does: what's waiting, then the reload, in one turn.
    assert!(app.host.run_command("waiting.two"));
    app.host.reload().expect("the reload");
    app.frames(20);
    assert_eq!(app.error(), None);
    assert_eq!(review_count(&mut app), 2, "no Review after the reload:\n{}", app.tree());
    assert_eq!(app.tree().matches("is waiting for you to allow it").count(), 2, "a notice each, once:\n{}", app.tree());

    let cua = button_in_card(&mut app, "Cua is waiting for you to allow it", "Review");
    app.click(&cua);
    app.frames(10);
    assert!(calls(&app).contains("review:cua"), "{}", calls(&app));
    // (Still up: nothing's decided.)
    assert_eq!(review_count(&mut app), 2);
    let random = button_in_card(&mut app, "Random Number is waiting for you to allow it", "Review");
    app.click(&random);
    app.frames(10);
    assert!(calls(&app).contains("review:random-number"), "{}", calls(&app));
}

#[test]
fn a_notice_kept_across_reloads_is_the_same_card_where_it_stood_until_it_is_decided() {
    let dir = storage("kept");
    let mut app = launch(&dir);
    assert!(app.host.run_command("waiting.two"));
    app.frames(30);
    let target = "role:status >> \"Cua is waiting for you to allow it\"";
    let before = app.bounds(target);
    for _ in 0..3 {
        app.host.reload().expect("the reload");
        // At once: no sliding in again, no second card.
        app.frames(1);
        assert_eq!(app.bounds(target), before, "it moved with the reload");
        assert_eq!(review_count(&mut app), 2, "{}", app.tree());
    }
    // Decided, wherever: it goes, reloads or no.
    assert!(app.host.run_command("waiting.allow_cua"));
    app.host.reload().expect("the reload");
    app.frames(30);
    assert_eq!(count(&mut app, "Cua is waiting"), 0, "{}", app.tree());
    assert_eq!(review_count(&mut app), 1);
}

#[test]
fn what_you_put_away_stays_away_across_reloads_and_launches_until_it_changes() {
    let dir = storage("away");
    let mut app = launch(&dir);
    assert!(app.host.run_command("waiting.two"));
    app.frames(30);
    let dismiss = button_in_card(&mut app, "Cua is waiting for you to allow it", "Dismiss");
    app.click(&dismiss);
    app.frames(30);
    assert_eq!(count(&mut app, "Cua is waiting"), 0);

    app.host.reload().expect("the reload");
    app.frames(30);
    assert_eq!(count(&mut app, "Cua is waiting"), 0, "back after a reload");

    // The next launch: the one you put away isn't said again; the one you
    // didn't is still up.
    app.host.save();
    drop(app);
    let mut app = launch(&dir);
    assert!(app.host.run_command("waiting.two"));
    app.frames(30);
    assert_eq!(count(&mut app, "Cua is waiting"), 0, "back after a launch:\n{}", app.tree());
    assert_eq!(count(&mut app, "Random Number is waiting"), 1, "{}", app.tree());

    // A new version of it speaks again.
    assert!(app.host.run_command("waiting.change_cua"));
    app.frames(30);
    assert_eq!(count(&mut app, "Cua is waiting"), 1, "{}", app.tree());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_toast_whose_button_was_a_function_goes_with_its_load_and_never_stands_without_it() {
    let dir = storage("function");
    let mut app = launch(&dir);
    assert!(app.host.run_command("waiting.function_toast"));
    app.frames(20);
    assert!(app.find("role:status >> role:button \"Undo\"").is_some(), "{}", app.tree());
    app.host.reload().expect("the reload");
    app.frames(20);
    // Never the words without the button (the bug); here, gone altogether.
    assert_eq!(count(&mut app, "Branch deleted"), 0, "it stayed up with no button:\n{}", app.tree());
}
