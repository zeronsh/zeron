//! Keep the existing GPUI platform instance to use its public cursor hiding API.
//! The platform restores its cursor before every motion/leave; we hide it again
//! only over a desktop that paints a remote cursor (including the hidden shape).
use gpui::{App, Global, Platform};
use std::rc::Rc;
pub struct CursorPlatform(Rc<dyn Platform>);
impl Global for CursorPlatform {}
pub fn init(platform: Rc<dyn Platform>, cx: &mut App) {
    cx.set_global(CursorPlatform(platform));
}
pub(super) fn hide(cx: &App) {
    if let Some(platform) = cx.try_global::<CursorPlatform>() {
        platform.0.hide_cursor_until_mouse_moves();
    }
}
