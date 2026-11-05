//! What a pointer reaches at a point (the scene's own hit test, as a
//! person's click or scroll goes): the page under the transcript, the
//! title bar's controls, the composer, each its own, with nothing laid over
//! them taking the pointer. On the shell's fixture (ui/shell_fixture.luau).

use zgx_script::{ScriptHost, testing::TestApp};

const W: f32 = 1200.;
const H: f32 = 800.;

fn mount() -> TestApp {
    // (`ZGX_HIT_UI`: another copy of ui/ to try, for a bisection.)
    let ui = std::env::var("ZGX_HIT_UI").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"));
    let path = ui.join("shell_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(30);
    app
}

/// Whether the pointer at `(x, y)` reaches `inside` (it, or something in it).
fn reaches(app: &mut TestApp, (x, y): (f32, f32), inside: &str) -> Result<(), String> {
    let want = app.target(inside)?;
    app.ui.prepare_frame();
    let scene = app.ui.scene.borrow();
    let hit = app.ui.input.pointer_target(&scene, x, y).ok_or_else(|| format!("nothing at {x},{y}"))?;
    if hit == want || scene.ancestors(hit).any(|a| a == want) {
        return Ok(());
    }
    let chain: Vec<String> = std::iter::once(hit).chain(scene.ancestors(hit)).map(|id| format!("{id:?} {:?}", scene.bounds(id))).collect();
    Err(format!("at {x},{y} the pointer reaches {hit:?}, not {inside}: {}", chain.join(" < ")))
}

fn middle(app: &mut TestApp, sel: &str) -> (f32, f32) {
    let b = app.bounds(sel);
    (b.x + b.width / 2., b.y + b.height / 2.)
}

/// Each place in `checks` (a selector) reached at its middle by the
/// pointer; what isn't, said.
fn check(app: &mut TestApp, when: &str, checks: &[&str], wrong: &mut Vec<String>) {
    for sel in checks {
        if app.find(sel).is_none() {
            wrong.push(format!("{when}: no {sel}"));
            continue;
        }
        let at = middle(app, sel);
        if let Err(e) = reaches(app, at, sel) {
            wrong.push(format!("{when}: {e}"));
        }
    }
}

#[test]
fn the_pointer_reaches_the_page_the_title_bar_and_the_composer() {
    let mut app = mount();
    let mut wrong = Vec::new();
    let head = [
        "#titlebar >> role:button \"Toggle sidebar\"",
        "#shell-sidebar >> role:button \"Sidebar view options\"",
        "#shell-sidebar >> role:button \"New chat\"",
        "#shell-sidebar >> role:button \"Search\"",
    ];
    check(&mut app, "a new chat", &head, &mut wrong);
    check(&mut app, "a new chat", &["role:textbox \"Message\""], &mut wrong);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    // The page's middle, where the transcript scrolls.
    let page = app.bounds("#shell-page");
    let at = (page.x + page.width / 2., page.y + page.height / 3.);
    if let Err(e) = reaches(&mut app, at, "#shell-page") {
        wrong.push(format!("a session: {e}"));
    }
    check(&mut app, "a session", &head, &mut wrong);
    // (Back once there's somewhere to go back to.)
    check(&mut app, "a session", &["#titlebar >> role:button \"Panel\"", "#titlebar >> role:button \"Back\"", "role:textbox \"Message\""], &mut wrong);
    // Its panel open: its own controls, and the page's still.
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    let panel = app.bounds("#shell-files");
    let at = (panel.x + panel.width / 2., panel.y + panel.height / 2.);
    if let Err(e) = reaches(&mut app, at, "#shell-files") {
        wrong.push(format!("the panel open: {e}"));
    }
    check(&mut app, "the panel open", &head, &mut wrong);
    check(
        &mut app,
        "the panel open",
        &["#titlebar >> role:button \"Panel\"", "role:textbox \"Message\"", "#panel-expand", "#panel-open-files"],
        &mut wrong,
    );
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
