//! The window's own controls where the platform's aren't drawn (Linux and
//! Windows: the window is `Titlebar::Hidden`, `zeron_window`): the title
//! bar draws close, minimize and maximize as macOS's traffic lights stand
//! (ds/title_bar.luau), and the host tells it what it needs and does what
//! they ask.
//!
//!   window.chrome       nil where the platform draws its own (macOS's
//!                       traffic lights); else { tiling = bool }: whether
//!                       a tiling compositor arranges the window (Hyprland,
//!                       sway, i3, niri), where Automatic shows none
//!   window.maximized    whether the window fills its display's work area
//!                       (the maximize control shows restore then)
//!   window.minimize     as the window manager's own buttons do
//!   window.maximize     maximize, or restore a maximized window
//!   window.close        as the window manager's close: the window asked
//!
//! Whatever a template draws (or doesn't), the window stays closable: the
//! window manager's close and Alt+F4 reach the host as the window's close
//! request (zgx-desktop's `WindowEvent::CloseRequested`), not the scripts.

use zgx_desktop::WindowContext;
use zgx_script::{ScriptHost, Value};

thread_local! {
    /// The effects keeping each window's `window.maximized` current.
    static FOLLOWING: std::cell::RefCell<Vec<zgx::reactive::Effect>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Whether the window draws its own controls here: everywhere but macOS,
/// whose traffic lights are the system's.
pub fn drawn() -> bool {
    !cfg!(target_os = "macos")
}

/// Whether the session's compositor tiles its windows, from its
/// environment (`var`): Hyprland, sway, i3 or niri. There a window has no
/// buttons of its own to press.
pub fn tiling(var: impl Fn(&str) -> Option<String>) -> bool {
    let set = |name: &str| var(name).is_some_and(|v| !v.is_empty());
    if ["HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK", "I3SOCK", "NIRI_SOCKET"].into_iter().any(set) {
        return true;
    }
    ["XDG_CURRENT_DESKTOP", "XDG_SESSION_DESKTOP", "DESKTOP_SESSION"].into_iter().filter_map(&var).any(|desktop| {
        desktop.split(':').any(|d| ["hyprland", "sway", "i3", "niri"].contains(&d.trim().to_ascii_lowercase().as_str()))
    })
}

/// Whether a window `size` fills a display of `display` (both physical),
/// short of a panel's room: maximized, as zgx keeps no state of its own
/// for it.
pub fn fills(size: (u32, u32), display: Option<(u32, u32)>) -> bool {
    display.is_some_and(|d| size.0 >= d.0.saturating_sub(64) && size.1 >= d.1.saturating_sub(128))
}

/// The window's controls for `host`'s scripts: what they're told and the
/// actions they call.
pub fn serve(cx: &mut WindowContext, host: &ScriptHost) {
    let chrome = if drawn() { Value::record([("tiling", Value::Bool(tiling(|name| std::env::var(name).ok())))]) } else { Value::Nil };
    host.shared("window.chrome", chrome);
    let info = cx.window_info.clone();
    let maximized = host.shared("window.maximized", false);
    let follow = cx.ui.runtime.effect_now(move || {
        let size = info.bounds.with(|b| b.size);
        let display = info.current_display.get().and_then(|i| info.displays.with(|d| d.get(i).map(|d| d.size)));
        maximized.set(Value::Bool(fills(size, display)));
    });
    FOLLOWING.with(|all| all.borrow_mut().push(follow));
    let window = cx.window.clone();
    host.action("window.minimize", move |_| window.set_minimized(true));
    let (window, info) = (cx.window.clone(), cx.window_info.clone());
    host.action("window.maximize", move |_| {
        let size = info.bounds.with_untracked(|b| b.size);
        let display = info.current_display.with_untracked(|i| *i).and_then(|i| info.displays.with_untracked(|d| d.get(i).map(|d| d.size)));
        window.set_maximized(!fills(size, display));
    });
    let window = cx.window.clone();
    host.action("window.close", move |_| window.request_close());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| pairs.iter().find(|(k, _)| *k == name).map(|(_, v)| v.to_string())
    }

    #[test]
    fn tiling_compositors_are_known_by_their_environment() {
        assert!(tiling(env(&[("HYPRLAND_INSTANCE_SIGNATURE", "abc_123")])));
        assert!(tiling(env(&[("XDG_CURRENT_DESKTOP", "Hyprland")])));
        assert!(tiling(env(&[("SWAYSOCK", "/run/user/1000/sway-ipc.sock")])));
        assert!(tiling(env(&[("I3SOCK", "/tmp/i3")])));
        assert!(tiling(env(&[("NIRI_SOCKET", "/run/niri.sock")])));
        assert!(tiling(env(&[("XDG_CURRENT_DESKTOP", "niri")])));
        assert!(!tiling(env(&[("XDG_CURRENT_DESKTOP", "ubuntu:GNOME")])));
        assert!(!tiling(env(&[("XDG_CURRENT_DESKTOP", "KDE")])));
        assert!(!tiling(env(&[("HYPRLAND_INSTANCE_SIGNATURE", "")])), "set but empty");
        assert!(!tiling(env(&[])));
    }

    #[test]
    fn a_window_filling_its_display_is_maximized() {
        assert!(fills((2560, 1400), Some((2560, 1440))), "a panel's room short");
        assert!(!fills((1280, 800), Some((2560, 1440))));
        assert!(!fills((2560, 1440), None), "no display known");
    }
}
