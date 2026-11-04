//! What the host draws in the main window itself, outside the scripts'
//! view, so no template can take it away:
//!
//! - while a template runs as a preview (`preview`): "Agent template ·
//!   <name>", with Keep… (Zeron's own keep sheet) and Revert (Zeron's
//!   template, in safe mode);
//! - a notice when the template changed without you choosing it here (an
//!   agent's `use_template`, an update, a file written by hand);
//! - on Linux and Windows, which have no system menu bar, the app's menu
//!   bar (host_menu.rs: the title bar's ☰ and Alt, or the host's own on
//!   Alt under a template): View → Use Zeron's Template is always in it,
//!   the host's own item.
//!
//! It sits above the scripts' view in the window's own layout. (A script's
//! modal or layer mounts at the window's top and may still paint over it:
//! zgx's host-owned top layer will hold it above them.)

use std::cell::RefCell;

use zgx::compose::{View, button, column, row, switch, text};
use zgx::scene::Color;
use zgx::style::Styled;
use zgx_desktop::WindowContext;

use super::preview;

thread_local! {
    static NOTICE: RefCell<Option<zgx::reactive::Signal<Option<String>>>> = const { RefCell::new(None) };
}

/// How long a notice stays.
const NOTICE_FOR: std::time::Duration = std::time::Duration::from_secs(12);

/// A notice in the host's strip: told for a while, then gone.
pub fn notice(words: String) {
    eprintln!("zeron-zgx: notice: {words}");
    let Some(signal) = NOTICE.with(|n| n.borrow().clone()) else { return };
    signal.set(Some(words.clone()));
    let later = signal.clone();
    super::spawn_on_bridge(async move {
        zgx::timer::sleep(NOTICE_FOR).await;
        if later.with_untracked(|now| now.as_deref() == Some(words.as_str())) {
            later.set(None);
        }
    });
}

/// The strip above the scripts' view, and the menus' layer where there's
/// no system menu bar (taking no room).
pub fn view(cx: &mut WindowContext) -> View {
    let strip = strip(&cx.ui.runtime);
    column().child(strip).children(super::host_menu::view(cx))
}

/// The main window's root: the host's `chrome` above the scripts' view,
/// which takes the rest of the window, its full width and every point the
/// chrome leaves (the scripts' root, `#shell`, sizes itself to it).
pub fn compose(chrome: View, scripts: View) -> View {
    column().w_full().h_full().child(chrome).child(scripts.grow().min_h(0.))
}

/// The preview's bar and the notice, as they come and go.
fn strip(runtime: &zgx::reactive::Runtime) -> View {
    let now = runtime.signal(preview::current());
    let heard = now.clone();
    preview::on_change(move |p| {
        heard.set(p.cloned());
    });
    let notice_ = runtime.signal(None::<String>);
    NOTICE.with(|n| *n.borrow_mut() = Some(notice_.clone()));
    switch(
        move || (now.get(), notice_.get()),
        |(preview, notice), _| {
            let mut out = column();
            if let Some(p) = preview {
                out = out.child(
                    row()
                        .p(6.)
                        .gap(10.)
                        .bg(Color(0x5c, 0x3d, 0x00, 0xff))
                        .child(text(format!("Agent template · {} is a preview: it can't run programs, use your account, or change plugins, templates or files until you keep it.", p.name)).text_color(Color(0xff, 0xf4, 0xd6, 0xff)).grow())
                        .child(button().child("Keep…").on_click(|| super::host_command(super::KEEP_TEMPLATE)))
                        .child(button().child("Revert").on_click(|| super::host_command(super::USE_ZERON_TEMPLATE))),
                );
            }
            if let Some(words) = notice {
                out = out.child(
                    row()
                        .p(6.)
                        .gap(10.)
                        .bg(Color(0x1f, 0x2a, 0x44, 0xff))
                        .child(text(words).text_color(Color(0xe8, 0xee, 0xff, 0xff)).grow())
                        .child(button().child("Use Zeron's Template").on_click(|| super::host_command(super::USE_ZERON_TEMPLATE))),
                );
            }
            out
        },
    )
}

thread_local! {
    /// The host's layers (the menus'): kept over the scripts'.
    static ON_TOP: RefCell<Vec<zgx::scene::NodeId>> = const { RefCell::new(Vec::new()) };
}

/// Keeps what holds `handle` (one of the host's layers) among the window
/// root's last children, drawn over everything and hit first: as it mounts
/// and after every layout that changes (a script's modal or layer opening
/// raises itself to the end, which lays the root out again). The host's
/// layers keep their order among themselves, so two never trade places.
pub(super) fn keep_on_top(handle: zgx::compose::NodeHandle) {
    let Some(scene) = handle.scene() else { return };
    let node = handle.node();
    ON_TOP.with(|t| {
        let mut t = t.borrow_mut();
        if !t.contains(&node) {
            t.push(node);
        }
    });
    let raise = move || {
        let Ok(mut scene) = scene.try_borrow_mut() else { return };
        let root = scene.root();
        // Each of the host's layers, as the root's child that holds it.
        let ours: Vec<zgx::scene::NodeId> = ON_TOP.with(|t| {
            let mut t = t.borrow_mut();
            t.retain(|n| scene.contains(*n));
            t.iter()
                .filter_map(|&n| {
                    let mut at = n;
                    loop {
                        match scene.parent(at) {
                            Some(parent) if parent == root => return Some(at),
                            Some(parent) => at = parent,
                            None => return None,
                        }
                    }
                })
                .collect()
        });
        let children = scene.children(root);
        let mut order: Vec<_> = children.iter().copied().filter(|c| !ours.contains(c)).collect();
        order.extend(children.iter().copied().filter(|c| ours.contains(c)));
        if order.as_slice() == children {
            return;
        }
        scene.reorder_children(root, &order);
    };
    let first = raise.clone();
    first();
    handle.on_layout(raise);
}

#[cfg(test)]
mod tests {
    use super::*;
    use zgx_script::{ScriptHost, testing::TestApp};

    /// The shell's fixture under the host's strip, as the main window
    /// composes them (`compose`).
    fn mount() -> TestApp {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
        let chrome: std::rc::Rc<RefCell<Option<View>>> = Default::default();
        let made = chrome.clone();
        let app = TestApp::wrapped(
            move |runtime| {
                *made.borrow_mut() = Some(strip(runtime));
                ScriptHost::new(runtime, path)
            },
            move |scripts| compose(chrome.borrow_mut().take().unwrap(), scripts),
        );
        app.frames(30);
        assert_eq!(app.error(), None, "the shell failed");
        app
    }

    fn bounds(app: &mut TestApp, selector: &str) -> zgx::scene::Rect {
        let node = app.find(selector).unwrap_or_else(|| panic!("no {selector}"));
        app.ui.scene.borrow().transformed_bounds(node)
    }

    /// The scripts' root fills the window under the host's strip: at
    /// launch, after a resize, with the strip shown (a template in preview)
    /// and hidden.
    #[test]
    fn the_shell_fills_the_window_under_the_host_s_strip() {
        preview::set(None);
        let mut app = mount();
        for (w, h) in [(1624., 836.), (900., 600.), (1240., 820.)] {
            app.ui.scene.borrow_mut().resize(w, h);
            app.frames(6);
            let shell = bounds(&mut app, "#shell");
            assert_eq!((shell.x, shell.y, shell.width, shell.height), (0., 0., w, h), "no strip, {w}×{h}");

            preview::set(Some(preview::Preview { id: "sleek".into(), name: "Sleek".into() }));
            app.frames(6);
            let shell = bounds(&mut app, "#shell");
            let strip = bounds(&mut app, "role:button \"Keep…\"");
            assert!(strip.y < shell.y && shell.y > 0., "the strip above the shell: {strip:?} {shell:?}");
            assert_eq!((shell.x, shell.width), (0., w), "full width under the strip, {w}×{h}");
            assert!((shell.y + shell.height - h).abs() < 0.5, "down to the window's foot, {w}×{h}: {shell:?}");

            preview::set(None);
            app.frames(6);
            let shell = bounds(&mut app, "#shell");
            assert_eq!((shell.y, shell.height), (0., h), "the strip gone, the shell whole again, {w}×{h}");
        }
    }
}
