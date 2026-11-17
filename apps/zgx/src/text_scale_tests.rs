//! The system's text size (GNOME's `text-scaling-factor`, Windows' Text
//! size) reaches the app's own text as it does the interface's. zgx scales
//! `font.*` by it, and the tokens ds/tokens names (`M.TEXT`:
//! `ui.theme.scale_with_text`). Before, it scaled `font.*` alone, and ds/look
//! read the scaled `font.size.md` as the theme's, so the conversation shrank
//! by the system's factor where it should have grown (Nov 15, Linux at
//! GNOME's 1.2: a reply drawn smaller than the sidebar at every Interface
//! text size). Hard-coded paragraph lines (`line_height = 20`) stayed too,
//! beside text that grew, and Settings laid its rows out for lines that
//! didn't grow.

use zgx::scene::NodeKind;
use zgx_script::{ScriptHost, testing::TestApp};

fn mount() -> TestApp {
    let path =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/text_scale_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(800., 600.);
    app.settle();
    assert_eq!(app.error(), None, "the fixture failed");
    app
}

/// The font size of the text in `id` (the id is the text's box; its glyphs
/// are a node in it).
fn size(app: &mut TestApp, id: &str) -> f32 {
    let node = app.find(id).unwrap_or_else(|| panic!("no {id}"));
    let scene = app.ui.scene.borrow();
    let mut todo = vec![node];
    while let Some(n) = todo.pop() {
        if let NodeKind::Text { font_size, .. } = scene.kind(n) {
            return *font_size;
        }
        todo.extend_from_slice(scene.children(n));
    }
    panic!("no text in {id}")
}

struct Sizes {
    interface: f32,
    conversation: f32,
    visual: f32,
    code: f32,
    /// A one-line paragraph's height: its line.
    paragraph: f32,
    /// Settings' two-line row, as it lays it out (ds/settings
    /// `truncated_row_h`): 12 above and below, two lines 2 apart.
    row: f32,
}

fn sizes(app: &mut TestApp, text_size: f64, system: f32) -> Sizes {
    app.host
        .set_state("text_size", zgx_script::Value::from(text_size))
        .expect("the pref");
    app.ui.theme().set_system_text_scale(system);
    app.frames(4);
    Sizes {
        interface: size(app, "#interface"),
        conversation: size(app, "#conversation"),
        visual: size(app, "#visual"),
        code: size(app, "#code"),
        paragraph: app.bounds("#paragraph").height,
        row: app.text("#row").parse().expect("a number"),
    }
}

#[test]
fn the_system_text_size_scales_the_apps_own_text_with_the_interface() {
    let mut app = mount();
    for text_size in [13., 14., 16.] {
        // At the system's own size: the interface at the size chosen, the
        // conversation its step up (15 at 13), a visual's body the
        // conversation's, inline code its 13.5, a paragraph's line 20 at 13,
        // the last three as the theme sets them whatever the size chosen
        // (only the interface and the conversation follow it).
        let at = sizes(&mut app, text_size, 1.);
        assert_eq!(
            at.interface, text_size as f32,
            "the interface at {text_size}"
        );
        let prose = (15. * text_size as f32 / 13. * 2.).round() / 2.;
        assert_eq!(at.conversation, prose, "the conversation at {text_size}");
        assert_eq!(at.visual, prose, "a visual's body at {text_size}");
        assert_eq!(at.code, 13.5, "inline code at {text_size}");
        let line = (20. * text_size as f32 / 13.).round();
        assert_eq!(at.paragraph, line, "a paragraph's line at {text_size}");
        // A row's padding and gap stay; its two lines grow.
        let words = at.row - 26.;
        assert_eq!(
            words,
            ((18. * text_size as f32 / 13.).round() + (16. * text_size as f32 / 13.).round()),
            "a row's lines at {text_size}"
        );
        for system in [1.2, 1.5] {
            let scaled = sizes(&mut app, text_size, system);
            let near = |got: f32, want: f32, what: &str| {
                assert!(
                    (got - want).abs() <= 0.01,
                    "{what} at {text_size}, system ×{system}: {got}, not {want} (the interface {})",
                    scaled.interface
                );
            };
            near(scaled.interface, at.interface * system, "the interface");
            near(
                scaled.conversation,
                at.conversation * system,
                "the conversation",
            );
            near(scaled.visual, at.visual * system, "a visual's body");
            near(scaled.code, at.code * system, "inline code");
            near(scaled.row, 26. + words * system, "a Settings row");
            // (A box's height lands on whole points or close to.)
            assert!(
                (scaled.paragraph - at.paragraph * system).abs() <= 1.,
                "a paragraph's line at {text_size}, system ×{system}: {}, not about {}",
                scaled.paragraph,
                at.paragraph * system
            );
        }
    }
}
