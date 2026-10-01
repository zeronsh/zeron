//! Device-local, whole-window zoom. Font preferences remain independent.
use gpui::{App, KeyBinding, Keystroke, Window, actions};

use crate::settings::{self, SavePolicy};

pub const MIN_PERCENT: u16 = 50;
pub const MAX_PERCENT: u16 = 200;
pub const DEFAULT_PERCENT: u16 = 100;
pub const STEP_PERCENT: u16 = 10;

actions!(ui_scale, [Increase, Decrease, Reset]);

pub fn clamp(percent: u16) -> u16 {
    percent.clamp(MIN_PERCENT, MAX_PERCENT)
}

pub fn factor(percent: u16) -> f32 {
    f32::from(clamp(percent)) / 100.0
}

pub fn stepped(percent: u16, direction: i16) -> u16 {
    (i32::from(clamp(percent)) + i32::from(direction) * i32::from(STEP_PERCENT))
        .clamp(i32::from(MIN_PERCENT), i32::from(MAX_PERCENT)) as u16
}

pub fn apply_to_window(percent: u16, window: &mut Window) {
    let scale = factor(percent);
    window.set_ui_scale(scale);
    // AppKit keeps the traffic lights at native size. Align their centers
    // with the scaled titlebar controls, including when the UI shrinks.
    #[cfg(target_os = "macos")]
    window.set_traffic_light_position(gpui::point(gpui::px(14.0), gpui::px(21.0 * scale - 7.0)));
}

pub fn set(percent: u16, window: &mut Window, cx: &mut App) {
    let percent = clamp(percent);
    settings::update(SavePolicy::Immediate, cx, |settings| {
        settings.ui_scale_percent = percent;
    });
    apply_to_window(percent, window);
    // The active window is already borrowed. Update other open windows without
    // trying to reenter it, so they share the same device-local preference.
    for handle in cx.windows() {
        if handle != window.window_handle() {
            let _ = handle.update(cx, |_, window, _| apply_to_window(percent, window));
        }
    }
    cx.refresh_windows();
}

pub fn step(direction: i16, window: &mut Window, cx: &mut App) {
    set(
        stepped(settings::current(cx).ui_scale_percent, direction),
        window,
        cx,
    );
}

// Backends can report either the physical punctuation key or its shifted
// character. Accept both, and preserve the requested Ctrl chords on macOS
// alongside the usual Command equivalents.
pub(crate) fn combos() -> Vec<(&'static str, u8)> {
    let mut combos = vec![
        ("ctrl-shift-=", 0),
        ("ctrl-shift-+", 0),
        ("ctrl-shift--", 1),
        ("ctrl-shift-_", 1),
        ("ctrl-shift-0", 2),
        ("ctrl-shift-)", 2),
    ];
    if cfg!(target_os = "macos") {
        combos.extend([
            ("cmd-shift-=", 0),
            ("cmd-shift-+", 0),
            ("cmd-shift--", 1),
            ("cmd-shift-_", 1),
            ("cmd-shift-0", 2),
            ("cmd-shift-)", 2),
        ]);
    }
    combos
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys(combos().into_iter().map(|(combo, action)| match action {
        0 => KeyBinding::new(combo, Increase, None),
        1 => KeyBinding::new(combo, Decrease, None),
        _ => KeyBinding::new(combo, Reset, None),
    }));
}

pub fn is_reserved_combo(combo: &str) -> bool {
    let Ok(candidate) = Keystroke::parse(&settings::platform_combo(combo)) else {
        return false;
    };
    combos()
        .into_iter()
        .any(|(combo, _)| Keystroke::parse(combo).is_ok_and(|key| key == candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct ScaleProbe {
        clicks: usize,
        scroll: gpui::Point<gpui::Pixels>,
    }

    impl gpui::Render for ScaleProbe {
        fn render(
            &mut self,
            _: &mut Window,
            cx: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            use gpui::prelude::*;
            gpui::div().size_full().child(
                gpui::div()
                    .absolute()
                    .left(gpui::px(20.))
                    .top(gpui::px(20.))
                    .w(gpui::px(100.))
                    .h(gpui::px(80.))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, _| this.clicks += 1),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, _| {
                        this.scroll = event.delta.pixel_delta(gpui::px(16.));
                    })),
            )
        }
    }

    #[gpui::test]
    fn ui_scale_native_hit_testing_and_scroll_stay_aligned(cx: &mut gpui::TestAppContext) {
        let (view, cx) = cx.add_window_view(|window, _| {
            window.set_ui_scale(1.5);
            ScaleProbe {
                clicks: 0,
                scroll: Default::default(),
            }
        });
        cx.simulate_event(gpui::MouseDownEvent {
            button: gpui::MouseButton::Left,
            position: gpui::point(gpui::px(150.), gpui::px(90.)),
            ..Default::default()
        });
        assert_eq!(view.read_with(cx, |view, _| view.clicks), 1);
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(gpui::px(150.), gpui::px(90.)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(30.), gpui::px(60.))),
            ..Default::default()
        });
        assert_eq!(
            view.read_with(cx, |view, _| view.scroll),
            gpui::point(gpui::px(20.), gpui::px(40.))
        );
    }

    #[test]
    fn scale_steps_clamp_without_overflow_and_reset_to_default() {
        assert_eq!(stepped(100, 1), 110);
        assert_eq!(stepped(100, -1), 90);
        assert_eq!(stepped(200, 1), 200);
        assert_eq!(stepped(50, -1), 50);
        assert_eq!(stepped(u16::MAX, i16::MAX), 200);
        assert_eq!(stepped(0, i16::MIN), 50);
        assert_eq!(factor(DEFAULT_PERCENT), 1.0);
    }

    #[test]
    fn scaling_shortcuts_parse_and_are_reserved() {
        for (combo, _) in combos() {
            Keystroke::parse(combo).unwrap();
            assert!(is_reserved_combo(combo));
        }
        assert!(!is_reserved_combo("mod-s"));
        assert!(!is_reserved_combo("ctrl-+"));
    }

    #[test]
    fn ui_scale_reserved_shortcuts_heal_only_colliding_preferences() {
        let mut saved = settings::UiSettings::default();
        saved.keymap.random_wallpaper = "ctrl-shift-+".into();
        saved.keymap.save_file = "mod-alt-s".into();
        let healed = saved.clamped();
        assert_eq!(
            healed.keymap.random_wallpaper,
            settings::ShortcutId::RandomWallpaper.default_combo()
        );
        assert_eq!(healed.keymap.save_file, "mod-alt-s");
    }

    #[test]
    fn scale_defaults_migrates_clamps_and_round_trips() {
        let legacy: settings::UiSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.ui_scale_percent, 100);
        let dir = tempfile::tempdir().unwrap();
        let mut settings = settings::UiSettings::default();
        settings.ui_scale_percent = 150;
        settings.save(dir.path()).unwrap();
        let restored = settings::UiSettings::load(dir.path());
        assert_eq!(restored.ui_scale_percent, 150);
        assert_eq!(restored.ui_font_size, settings.ui_font_size);
        settings.ui_scale_percent = 0;
        assert_eq!(settings.clone().clamped().ui_scale_percent, 50);
        settings.ui_scale_percent = u16::MAX;
        assert_eq!(settings.clamped().ui_scale_percent, 200);
    }
}
