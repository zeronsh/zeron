//! The consent windows as they're drawn: every line of every text in view has
//! its ink, and an id is drawn to its last letter. (Boxes alone passed with a
//! card's rows drawn away, and with an id's end lost on a line it didn't have.)

use std::path::PathBuf;

use zgx::scene::Rect;
use zgx_gpu::GpuRenderer;
use zgx_script::{ScriptHost, testing::TestApp};

const W: f32 = 460.;
/// Each window's height, the host's (`SIZE`).
const STORE: f32 = 580.;
const TEMPLATE: f32 = 540.;
/// The code face's letter at the ids' size.
const LETTER: f32 = 6.6;

struct Drawn {
    height: f32,
    texts: Vec<(String, Rect)>,
    rgba: Vec<u8>,
}

impl Drawn {
    /// Whether anything is drawn in `r`: its brightest and darkest apart.
    fn ink(&self, r: Rect) -> bool {
        let (mut low, mut high) = (255u8, 0u8);
        for y in r.y.max(0.) as usize..(r.y + r.height).min(self.height) as usize {
            for x in r.x.max(0.) as usize..(r.x + r.width).min(W) as usize {
                let g = self.rgba[(y * W as usize + x) * 4 + 1];
                (low, high) = (low.min(g), high.max(g));
            }
        }
        high.saturating_sub(low) > 30
    }
}

fn draw(fixture: &str, height: f32, command: &str) -> Drawn {
    // (One drawing at a time, as transcript_paint_tests says.)
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _gpu = GPU.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui").join(fixture);
    let mut app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, script);
        host.action("appearance", |_| {});
        host
    });
    app.ui.scene.borrow_mut().resize(W, height);
    let mut gpu = GpuRenderer::new(W as u32, height as u32).unwrap();
    for font in crate::fonts() {
        font.unwrap().install(&mut gpu.text_system().borrow_mut());
    }
    gpu.install_text(&mut app.ui.scene.borrow_mut());
    app.ui.refresh_text_geometry();
    app.frames(10);
    assert!(app.host.run_command("dark"));
    assert!(app.host.run_command(command), "{command}");
    app.settle();
    app.frames(60);
    app.ui.prepare_frame();
    app.ui.scene.borrow_mut().flush();
    gpu.render(&app.ui.scene.borrow(), &[Rect::new(0., 0., W, height)]).unwrap();
    let rgba = gpu.readback().unwrap();
    // `@12 text "words" x,y wxh`, whatever's in its words.
    let mut view = height;
    let mut texts = Vec::new();
    for line in app.tree().lines() {
        let line = line.trim();
        let Some((head, size)) = line.rsplit_once(' ') else { continue };
        let Some((head, at)) = head.rsplit_once(' ') else { continue };
        let pair = |s: &str, by: char| s.split_once(by).and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?)));
        let (Some((x, y)), Some((w, h))) = (pair(at, ','), pair(size, 'x')) else { continue };
        if head.contains(" scroll") {
            view = y + h - 12.;
        }
        if let Some((_, words)) = head.split_once(" text \"") {
            texts.push((words.trim_end_matches('"').to_owned(), Rect::new(x, y, w, h)));
        }
    }
    // (What's in view, whole: under the scroll's fade nothing need be.)
    texts.retain(|(_, r)| r.y + r.height <= view);
    Drawn { height, texts, rgba }
}

#[test]
fn every_text_of_a_consent_window_in_view_is_drawn() {
    for (fixture, height, commands) in [
        ("store_consent_fixture.luau", STORE, &["publish", "unyank", "token", "token_both", "mixed", "many", "hostile", "busy"][..]),
        ("template_consent_fixture.luau", TEMPLATE, &["store", "token", "changed", "long", "plain_update", "store_said", "sleek"][..]),
    ] {
        for command in commands {
            let drawn = draw(fixture, height, command);
            // (Why it's asked is in view whole, over the fade, the sign-in's
            // line shown or not; a scope on many lines scrolls.)
            if ["publish", "unyank", "token", "token_both", "busy"].contains(command) && height == STORE {
                assert!(drawn.texts.iter().any(|(words, _)| words.starts_with("A template that isn't Zeron's has run")), "{command}: {:?}", drawn.texts);
            }
            assert!(drawn.texts.len() >= 4, "{fixture} {command}: {:?}", drawn.texts);
            for (words, r) in &drawn.texts {
                let lines = (r.height / 18.).round().max(1.);
                let line = r.height / lines;
                for n in 0..lines as usize {
                    assert!(drawn.ink(Rect::new(r.x, r.y + n as f32 * line, r.width, line)), "{fixture} {command}: line {} of {words:?} isn't drawn ({r:?})", n + 1);
                }
            }
        }
    }
}

#[test]
fn a_token_s_ids_are_drawn_to_their_last_letter() {
    let drawn = draw("store_consent_fixture.luau", STORE, "many");
    let ids: Vec<_> = drawn.texts.iter().filter(|(words, _)| words.starts_with("a-plugin-with")).collect();
    assert!(ids.len() >= 5, "{:?}", drawn.texts);
    for (id, r) in ids {
        assert_eq!((id.chars().count(), r.height), (40, 16.), "{id}");
        // The last two letters, each a number.
        for n in [38., 39.] {
            assert!(drawn.ink(Rect::new(r.x + n * LETTER, r.y, LETTER, r.height)), "{id}: letter {n} isn't drawn");
        }
    }
}
