//! What the host draws in the main window itself, outside the scripts'
//! view, so no template can take it away:
//!
//! - while a template runs as a preview (`preview`): "Agent template ·
//!   <name>", with Keep… (Zeron's own keep sheet) and Revert (Zeron's
//!   template, in safe mode);
//! - a notice when the template changed without you choosing it here (an
//!   agent's `use_template`, an update, a file written by hand);
//! - on Linux and Windows, which have no system menu bar, the app's menu
//!   bar (zgx's `app_menu_bar`): View → Use Zeron's Template is always in
//!   it, the host's own item.
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

/// The strip, and the menu bar where there's no system one: above the
/// scripts' view.
pub fn view(cx: &mut WindowContext) -> View {
    let now = cx.ui.runtime.signal(preview::current());
    let heard = now.clone();
    preview::on_change(move |p| {
        heard.set(p.cloned());
    });
    let notice_ = cx.ui.runtime.signal(None::<String>);
    NOTICE.with(|n| *n.borrow_mut() = Some(notice_.clone()));
    let strip = switch(
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
    );
    column().child(strip).children((!zgx_desktop::has_native_app_menu()).then(|| cx.app_menu_bar()))
}
