//! The sidebar's words keep the scheme's ink as the scheme switches (Light,
//! Dark, Light, System, Dark, Light), in every state a row takes: a session
//! at rest and one under the pointer in `text.secondary`; working, asking,
//! done (unseen) and selected in `text.primary`; times and the "Projects"
//! heading in `text.tertiary`; a group's header in `text.primary`. Before zgx
//! d81d2c33 (#1439) a style re-run that saw nothing of its own change kept a
//! text's colour from the first switch: a light theme after a dark one left
//! the rows at rest in the dark scheme's ink (white on the light sidebar),
//! as the app showed it.
//!
//! Read off the scene, where zgx paints it (the Lua test kit can't see
//! colours): every shown text node in the sidebar with a row's words,
//! against the window's token, on the shell's own fixture
//! (ui/shell_fixture.luau). (A row under the pointer shows its action in
//! its corner, not its time: the times read are the others'.)

use std::path::PathBuf;

use zgx::scene::{Color, NodeKind};
use zgx::theme::{Appearance, ColorToken, tok};
use zgx_script::{ScriptHost, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The colour of every shown text node in the sidebar reading exactly
/// `words` (not the hover card a row under the pointer opens).
fn inks(app: &mut TestApp, words: &str) -> Vec<Color> {
    let sidebar = app
        .find("sidebar")
        .unwrap_or_else(|| panic!("no #sidebar:\n{}", app.tree()));
    let scene = app.ui.scene.borrow();
    let mut out = vec![];
    let mut stack = vec![sidebar];
    while let Some(id) = stack.pop() {
        if let NodeKind::Text { text, color, .. } = scene.kind(id)
            && text.as_ref() == words
            && scene.visible_bounds(id).is_some()
        {
            out.push(*color);
        }
        stack.extend(scene.children(id).iter().copied());
    }
    out
}

/// The scheme picked as setup and Settings pick it (screens/onboarding
/// actions `apply_scheme`): a theme, or the system's, here dark.
fn scheme(app: &mut TestApp, which: &str) {
    let themes = app.ui.theme().clone();
    match which {
        "System" => {
            themes.follow_system(Some(("zeron-dark", "zeron-light")));
            themes.set_system_appearance(Appearance::Dark);
        }
        name => {
            themes.follow_system(None);
            let theme = if name == "Light" {
                "zeron-light"
            } else {
                "zeron-dark"
            };
            themes.set(theme).unwrap();
        }
    }
    app.frames(30);
}

#[test]
fn sidebar_rows_wear_each_schemes_ink_in_every_state() {
    let mut app =
        TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("shell_fixture.luau")));
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(60);
    assert_eq!(app.error(), None, "the shell failed to load");
    // Selected: "Tidy the tests" picked. Under the pointer: "Answer the
    // review". At rest: "Write the docs". Working: "Fix the build" (done,
    // unseen, from the fourth switch on). Asking: "Ship the release".
    app.click("\"Tidy the tests\"");
    app.frames(30);
    app.hover("\"Answer the review\"");
    app.frames(30);
    let mut fix_the_build = "working";
    // Every miss, each switch's and state's, said at the end.
    let mut wrong: Vec<String> = vec![];
    for (i, which) in ["Light", "Dark", "Light", "System", "Dark", "Light"]
        .into_iter()
        .enumerate()
    {
        if i == 3 {
            assert!(app.host.run_command("finish"), "no `finish` in the fixture");
            app.frames(30);
            fix_the_build = "done";
        }
        scheme(&mut app, which);
        let root = app.ui.theme().root().clone();
        let expect: [(&str, &str, ColorToken); 9] = [
            ("Write the docs", "at rest", tok::TEXT_SECONDARY),
            (
                "Answer the review",
                "under the pointer",
                tok::TEXT_SECONDARY,
            ),
            ("Tidy the tests", "selected", tok::TEXT_PRIMARY),
            ("Fix the build", fix_the_build, tok::TEXT_PRIMARY),
            ("Ship the release", "asking", tok::TEXT_PRIMARY),
            ("Borealis", "a group's header", tok::TEXT_PRIMARY),
            ("Projects", "the heading", tok::TEXT_TERTIARY),
            ("1h", "a time", tok::TEXT_TERTIARY),
            ("2h", "a time, its row selected", tok::TEXT_TERTIARY),
        ];
        for (words, state, token) in expect {
            let want = root.color_untracked(token);
            let got = inks(&mut app, words);
            if got.is_empty() || got.iter().any(|c| *c != want) {
                wrong.push(format!(
                    "after {which} (switch {}): \"{words}\" ({state}) painted {got:?}, its token is {want:?}",
                    i + 1
                ));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} misses:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
}
