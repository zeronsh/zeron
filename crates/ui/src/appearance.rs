//! Light/dark switching: what the user asked for, what the OS reports, and the
//! plumbing that turns a change in either into a repaint.
//!
//! Three pieces, following the pattern zed uses (`crates/theme/src/theme.rs`
//! `SystemAppearance` + `reload_theme` + `cx.refresh_windows`):
//!
//! 1. [`AppearanceMode`] — the persisted user choice: follow the OS, or pin one.
//! 2. [`AppearanceState`] — a gpui global holding that choice alongside the last
//!    appearance the OS reported, so [`resolve`] can combine them.
//! 3. [`observe_window`] — subscribes to the platform's appearance notification
//!    (macOS `viewDidChangeEffectiveAppearance`) and re-applies.
//!
//! # Why `refresh_windows` and not `notify`
//!
//! Colors are read *imperatively* (`Theme::of(cx).text`) at paint time, not
//! through a reactive binding, so no view knows its colors went stale — a
//! `notify()` on some entity would repaint that entity and nothing else.
//! [`App::refresh_windows`] marks every window dirty *and* disables gpui's
//! per-view prepaint cache for the frame, which is the only thing that forces
//! already-laid-out elements to re-run their paint with the new palette.

use gpui::{App, Global, Subscription, Window};
use serde::{Deserialize, Serialize};
use zeron_theme::{AccentSelection, SurfacePreference, ThemeSelection};

use crate::settings::{self, SavePolicy};
use crate::theme::{Appearance, Theme};

/// The user's appearance preference. Persisted in `ui-settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppearanceMode {
    /// Follow the OS. The default — matches every other native app on the
    /// machine, including when the user has macOS set to switch at sunset.
    #[default]
    System,
    Light,
    Dark,
}

impl AppearanceMode {
    /// Menu/label text.
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// Shared glyph for appearance controls throughout the app.
    pub fn icon(self) -> &'static str {
        match self {
            Self::System => crate::icons::MONITOR,
            Self::Light => crate::icons::SUN,
            Self::Dark => crate::icons::MOON,
        }
    }

    pub const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];
}

/// Global state behind the current theme: what the user chose, and what the OS
/// last said. Kept separate from [`Theme`] itself so that flipping the OS
/// appearance while the user has pinned Light still records the new system value
/// (and takes effect the moment they switch back to `System`).
pub struct AppearanceState {
    pub mode: AppearanceMode,
    pub system: Appearance,
    pub themes: ThemeSelection,
    pub accent: AccentSelection,
    pub surface: SurfacePreference,
}

impl Global for AppearanceState {}

/// Combine the user's choice with the OS state.
pub fn resolve(mode: AppearanceMode, system: Appearance) -> Appearance {
    match mode {
        AppearanceMode::System => system,
        AppearanceMode::Light => Appearance::Light,
        AppearanceMode::Dark => Appearance::Dark,
    }
}

/// Install the appearance globals and the matching theme. Call once at boot,
/// before any window opens, so the first frame is already the right palette
/// (installing later produces a visible dark-to-light flash).
pub fn init(
    mode: AppearanceMode,
    themes: ThemeSelection,
    accent: AccentSelection,
    surface: SurfacePreference,
    cx: &mut App,
) {
    let system = Appearance::from_window(cx.window_appearance());
    tracing::debug!(?mode, ?system, "appearance: initial");
    cx.set_global(AppearanceState {
        mode,
        system,
        themes: themes.clone(),
        accent,
        surface,
    });
    sync_ns_appearance(mode);
    let appearance = resolve(mode, system);
    Theme::install_selection(
        appearance,
        themes.variant_id(model_appearance(appearance)),
        accent,
        surface,
        cx,
    );
    settings::wallpaper_colors::ensure_color(cx);
}

/// The mode currently in effect (defaults to `System` before [`init`]).
pub fn mode(cx: &App) -> AppearanceMode {
    cx.try_global::<AppearanceState>()
        .map(|s| s.mode)
        .unwrap_or_default()
}

pub fn themes(cx: &App) -> ThemeSelection {
    cx.try_global::<AppearanceState>()
        .map(|state| state.themes.clone())
        .unwrap_or_default()
}

pub fn accent(cx: &App) -> AccentSelection {
    cx.try_global::<AppearanceState>()
        .map(|state| state.accent)
        .unwrap_or_default()
}

pub fn surface(cx: &App) -> SurfacePreference {
    cx.try_global::<AppearanceState>()
        .map(|state| state.surface)
        .unwrap_or_default()
}

/// Change the user's preference, repaint if that changed the palette, and write
/// the choice to disk.
pub fn set_mode(mode: AppearanceMode, cx: &mut App) {
    if !cx.has_global::<AppearanceState>() {
        return;
    }
    let state = cx.global_mut::<AppearanceState>();
    if state.mode == mode {
        return;
    }
    state.mode = mode;
    apply(cx);
    settings::update(SavePolicy::Immediate, cx, |settings| {
        settings.appearance = mode;
    });
}

/// Change the interactive accent without changing any semantic theme roles.
pub fn set_theme(appearance: Appearance, variant_id: impl Into<String>, cx: &mut App) {
    if !cx.has_global::<AppearanceState>() {
        return;
    }
    let variant_id = variant_id.into();
    let state = cx.global_mut::<AppearanceState>();
    if state.themes.variant_id(model_appearance(appearance)) == variant_id {
        return;
    }
    state
        .themes
        .set_variant(model_appearance(appearance), variant_id);
    let themes = state.themes.clone();
    apply(cx);
    settings::update(SavePolicy::Immediate, cx, |settings| {
        settings.theme_selection = themes;
    });
}

pub fn set_accent(accent: AccentSelection, cx: &mut App) {
    if !cx.has_global::<AppearanceState>() {
        return;
    }
    let state = cx.global_mut::<AppearanceState>();
    if state.accent == accent {
        return;
    }
    state.accent = accent;
    apply(cx);
    settings::update(SavePolicy::Immediate, cx, |settings| {
        settings.accent = accent;
    });
}

/// Change glass independently from appearance, theme, and accent selections.
pub fn set_surface(surface: SurfacePreference, cx: &mut App) {
    if !cx.has_global::<AppearanceState>() {
        return;
    }
    let state = cx.global_mut::<AppearanceState>();
    if state.surface == surface {
        return;
    }
    state.surface = surface;
    apply(cx);
    settings::update(SavePolicy::Immediate, cx, |settings| {
        settings.surface = surface;
    });
}

/// Subscribe a window to OS appearance changes. The returned [`Subscription`]
/// must outlive the window — callers typically `.detach()` it.
///
/// The notification is *per window*, but the appearance it reports is a system
/// setting, so any one window is enough to learn about the change; re-applying
/// is idempotent when several fire.
pub fn observe_window(window: &mut Window, cx: &mut App) -> Subscription {
    // Reconcile against the *window's* appearance before subscribing.
    //
    // [`init`] runs before any window exists and can only ask the platform
    // (`App::window_appearance`), which on macOS reads `NSApp.effectiveAppearance`
    // — and that is not reliably populated that early in launch. When it guesses
    // wrong the app paints the wrong palette until some unrelated event happens to
    // fire the appearance notification, which reads as "it booted dark and fixed
    // itself when I clicked something". The window knows for certain, so ask it.
    sync(Appearance::from_window(window.appearance()), cx);
    window.observe_window_appearance(|window, cx| {
        sync(Appearance::from_window(window.appearance()), cx);
    })
}

/// Record the OS appearance and re-apply if it moved.
fn sync(system: Appearance, cx: &mut App) {
    if !cx.has_global::<AppearanceState>() {
        return;
    }
    let state = cx.global_mut::<AppearanceState>();
    if state.system == system {
        return;
    }
    tracing::debug!(?system, "appearance: system changed");
    state.system = system;
    apply(cx);
}

/// Re-resolve the palette and, if it moved, swap the theme and force a full
/// repaint. A no-op when the resolved appearance is unchanged — the OS fires the
/// notification for vibrancy and accent-color changes too, and repainting every
/// window for those would be a visible hitch for nothing.
pub fn apply(cx: &mut App) {
    let Some(state) = cx.try_global::<AppearanceState>() else {
        return;
    };
    sync_ns_appearance(state.mode);
    let wanted = resolve(state.mode, state.system);
    let accent = state.accent;
    let surface = state.surface;
    let variant_id = state.themes.variant_id(model_appearance(wanted)).to_owned();
    let changed = !cx.try_global::<Theme>().is_some_and(|theme| {
        theme.appearance == wanted
            && theme.variant_id.as_ref() == variant_id
            && theme.accent_selection == accent
            && theme.surface_preference == surface
            && theme.wallpaper_color == settings::wallpaper_colors::active(cx)
    });
    if changed {
        tracing::debug!(?wanted, %variant_id, "appearance: installing palette");
        Theme::install_selection(wanted, &variant_id, accent, surface, cx);
        cx.refresh_windows();
    }
    // Unconditional, even when the palette did not move: this is the only thing
    // that keeps macOS vibrancy alive. gpui's macOS backend removes the
    // `NSVisualEffectView` from the window the moment the background appearance
    // is anything but `Blurred`, and nothing puts it back on its own — so a
    // single missed re-apply leaves the sidebar and tab strip permanently
    // opaque, which is exactly how the frost died. zed runs the same loop on
    // every settings change (`crates/zed/src/main.rs`).
    reapply_window_background(cx);
}

/// Rebuild the current palette after the installed custom registry changes.
/// Unlike [`apply`], this deliberately reinstalls even when the selected id is
/// unchanged because a linked theme may have recompiled under that same id.
pub fn apply_registry_change(cx: &mut App) {
    let Some(state) = cx.try_global::<AppearanceState>() else {
        return;
    };
    let wanted = resolve(state.mode, state.system);
    let accent = state.accent;
    let surface = state.surface;
    let variant_id = state.themes.variant_id(model_appearance(wanted)).to_owned();
    Theme::reinstall_selection(wanted, &variant_id, accent, surface, cx);
    cx.refresh_windows();
    reapply_window_background(cx);
}

fn model_appearance(appearance: Appearance) -> zeron_theme::Appearance {
    match appearance {
        Appearance::Dark => zeron_theme::Appearance::Dark,
        Appearance::Light => zeron_theme::Appearance::Light,
    }
}

/// Tell AppKit which appearance the app's windows use, so the chrome *it*
/// draws — the traffic lights above all — matches the palette *we* paint.
/// gpui never sets `NSAppearance`, so before this a pinned in-app theme left
/// the window chrome following the OS setting: a light window rendered
/// dark-appearance inactive traffic lights when the system was dark (user
/// report). Pinned modes name the appearance explicitly; `System` clears the
/// override (`setAppearance: nil`) so AppKit follows the OS again — resolving
/// to a name there too would freeze the chrome across OS sunset switches
/// until our own notification round-trip repainted it.
#[cfg(target_os = "macos")]
fn sync_ns_appearance(mode: AppearanceMode) {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    // NSAppearanceName constants are NSStrings whose value equals the
    // constant's own name (AppKit documents them as stable identifiers), so
    // building them from literals avoids linking the extern statics.
    let name = match mode {
        AppearanceMode::System => None,
        AppearanceMode::Light => Some(c"NSAppearanceNameAqua"),
        AppearanceMode::Dark => Some(c"NSAppearanceNameDarkAqua"),
    };
    unsafe {
        let appearance: *mut Object = match name {
            None => std::ptr::null_mut(),
            Some(name) => {
                let name: *mut Object =
                    msg_send![class!(NSString), stringWithUTF8String: name.as_ptr()];
                msg_send![class!(NSAppearance), appearanceNamed: name]
            }
        };
        let app: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![app, setAppearance: appearance];
    }
}

#[cfg(not(target_os = "macos"))]
fn sync_ns_appearance(_mode: AppearanceMode) {}

/// Push the theme's window background appearance onto every open window.
pub fn reapply_window_background(cx: &mut App) {
    // The window handling a settings click is temporarily taken out of App,
    // so updating it synchronously fails — silently, leaving its native
    // backdrop (macOS blur view, Windows Acrylic) on the old surface until a
    // restart. Defer until every window is back, on every platform.
    cx.defer(apply_window_background);
}

/// What `window` should composite with. Like native macOS apps, a frosted
/// (blurred) window drops to opaque compositing while it is not key: the
/// window server stops sampling and blurring the desktop behind it, and
/// [`window_base_fill`] paints the solid tone the frost would read as. Every
/// other theme keeps its own choice, active or not.
pub fn window_background_for(theme: &Theme, active: bool) -> gpui::WindowBackgroundAppearance {
    let wanted = theme.window_background_appearance();
    if wanted == gpui::WindowBackgroundAppearance::Blurred && !active {
        gpui::WindowBackgroundAppearance::Opaque
    } else {
        wanted
    }
}

/// Whether `window` currently shows its frost as a flat fill (a frosted theme
/// in a window that is not key). In-app backdrop blurs follow the same switch
/// (see `frost::frosted`), so an unfocused window's redraws — a streaming
/// reply, a spinner — never pay for a blur pass either.
pub fn frost_flattened(theme: &Theme, window: &Window) -> bool {
    theme.window_background_appearance() == gpui::WindowBackgroundAppearance::Blurred
        && !window.is_window_active()
}

/// The shell's base fill. Over the live blur it is the translucent glass; with
/// the blur off it is that same glass composited over the theme's surface
/// tone — the color the frost settles to over a neutral backdrop, and exactly
/// what opaque platforms already paint — so deactivation changes no layout
/// and keeps every translucent overlay (panels, cards) composing as before.
pub fn window_base_fill(theme: &Theme, window: &Window) -> gpui::Hsla {
    if frost_flattened(theme, window) {
        crate::theme::flatten(theme.glass(), theme.surface)
    } else {
        theme.glass()
    }
}

/// A window gained or lost key status (called from its activation observer).
///
/// Ordering avoids any visible flash. Activating restores the blur first —
/// the frame on screen is still the opaque one, so inserting the blur view
/// under it changes nothing — and gpui's activation refresh then repaints the
/// translucent frost. Deactivating repaints the opaque base first (that same
/// refresh), and only one frame later switches compositing to opaque, so the
/// layer never shows translucent content without the blur behind it. A quick
/// re-activation before then cancels the switch.
///
/// gpui's macOS backend removes the `NSVisualEffectView` whenever the value is
/// not `Blurred` and re-creates it on the next `Blurred` with the content
/// view's current bounds, pinned below every subview with autoresizing — so
/// resizes or fullscreen changes while inactive cannot leave it misplaced.
pub fn window_activation_changed(window: &mut Window, cx: &mut App) {
    let Some(theme) = cx.try_global::<Theme>() else {
        return;
    };
    let wanted = window_background_for(theme, window.is_window_active());
    if theme.window_background_appearance() != gpui::WindowBackgroundAppearance::Blurred {
        return;
    }
    if wanted == gpui::WindowBackgroundAppearance::Blurred {
        set_window_background(window, wanted);
        return;
    }
    window.on_next_frame(|window, _| {
        window.on_next_frame(|window, cx| {
            let Some(theme) = cx.try_global::<Theme>() else {
                return;
            };
            if !window.is_window_active() {
                set_window_background(window, window_background_for(theme, false));
            }
        });
    });
}

#[cfg(test)]
thread_local! {
    static APPLIED_BACKGROUNDS: std::cell::RefCell<Vec<gpui::WindowBackgroundAppearance>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn set_window_background(window: &Window, appearance: gpui::WindowBackgroundAppearance) {
    // The test platform discards the value; record what reached it.
    #[cfg(test)]
    APPLIED_BACKGROUNDS.with(|applied| applied.borrow_mut().push(appearance));
    window.set_background_appearance(appearance);
}

fn apply_window_background(cx: &mut App) {
    let Some(theme) = cx.try_global::<Theme>().cloned() else {
        return;
    };
    for window in cx.windows() {
        if let Err(error) = window.update(cx, |_, window, _| {
            let wanted = window_background_for(&theme, window.is_window_active());
            set_window_background(window, wanted);
        }) {
            tracing::warn!(%error, "appearance: window background not applied");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn themes() -> (Theme, Theme) {
        let theme = |surface| {
            Theme::for_selection(
                Appearance::Dark,
                "zeron-dark",
                AccentSelection::ThemeDefault,
                surface,
            )
        };
        (
            theme(SurfacePreference::Frosted),
            theme(SurfacePreference::Opaque),
        )
    }

    #[test]
    fn frosted_windows_drop_the_blur_only_while_inactive() {
        use gpui::WindowBackgroundAppearance::{Blurred, Opaque};
        let (frosted, opaque) = themes();
        let wanted = frosted.window_background_appearance();
        assert_eq!(
            window_background_for(&frosted, true),
            wanted,
            "active keeps the choice"
        );
        if wanted == Blurred {
            assert_eq!(window_background_for(&frosted, false), Opaque);
        } else {
            // Platforms without window blur (Linux) are unaffected.
            assert_eq!(window_background_for(&frosted, false), wanted);
        }
        for active in [true, false] {
            assert_eq!(
                window_background_for(&opaque, active),
                opaque.window_background_appearance(),
                "non-frosted themes never change"
            );
        }
        // The flat base is fully opaque and is the frost's own hue.
        let flat = crate::theme::flatten(frosted.glass(), frosted.surface);
        assert_eq!(flat.a, 1.0);
        assert!((flat.h - frosted.surface.h).abs() < 1e-3 || frosted.surface.s < 1e-3);
    }

    #[gpui::test]
    fn activation_switches_compositing_without_a_flash(cx: &mut gpui::TestAppContext) {
        use gpui::WindowBackgroundAppearance::{Blurred, Opaque};
        let (frosted, opaque) = themes();
        let blurs = frosted.window_background_appearance() == Blurred;
        let take =
            || APPLIED_BACKGROUNDS.with(|applied| std::mem::take(&mut *applied.borrow_mut()));
        cx.update(|cx| cx.set_global(frosted));
        let (_, cx) = cx.add_window_view(|_, _| gpui::Empty);
        take();
        let changed = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| window_activation_changed(window, cx))
        };
        let next_frame = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, cx| {
                window.simulate_next_frame(cx);
            })
        };
        let activate = |cx: &mut gpui::VisualTestContext| {
            cx.update(|window, _| window.activate_window());
            cx.run_until_parked();
        };
        // Inactive (test windows start without key status): the opaque base
        // paints first; compositing switches a frame later.
        changed(cx);
        assert!(
            take().is_empty(),
            "nothing switches before the opaque frame"
        );
        next_frame(cx);
        assert!(take().is_empty());
        next_frame(cx);
        assert_eq!(take(), if blurs { vec![Opaque] } else { vec![] });

        // Becoming key restores the blur at once.
        activate(cx);
        changed(cx);
        assert_eq!(take(), if blurs { vec![Blurred] } else { vec![] });

        // A quick blur → refocus before the deferred switch cancels it.
        cx.deactivate_window();
        changed(cx);
        activate(cx);
        changed(cx);
        next_frame(cx);
        next_frame(cx);
        assert!(
            take().iter().all(|applied| *applied != Opaque),
            "a window that is key again never drops its blur"
        );

        // Non-frosted themes are left alone in both directions.
        cx.update(|_, cx| cx.set_global(opaque));
        cx.deactivate_window();
        changed(cx);
        next_frame(cx);
        next_frame(cx);
        activate(cx);
        changed(cx);
        assert!(take().is_empty());
    }

    /// In-app backdrop blurs follow the same switch: an unfocused window's
    /// redraws paint the flat stand-in instead of a blur pass.
    #[gpui::test]
    fn floats_skip_backdrop_blur_while_inactive(cx: &mut gpui::TestAppContext) {
        let (frosted, _) = themes();
        let (_, cx) = cx.add_window_view(|_, _| gpui::Empty);
        let stand_in = cx.update(|window, _| crate::frost::backdrop_stand_in(&frosted, window));
        let fill = stand_in.expect("inactive: flat fill instead of blur");
        assert_eq!(fill.a, 1.0, "the stand-in hides what is behind the card");
        cx.update(|window, _| window.activate_window());
        cx.run_until_parked();
        let stand_in = cx.update(|window, _| crate::frost::backdrop_stand_in(&frosted, window));
        assert_eq!(stand_in, None, "key windows keep the blur");
    }

    #[test]
    fn system_mode_follows_the_os() {
        assert_eq!(
            resolve(AppearanceMode::System, Appearance::Light),
            Appearance::Light
        );
        assert_eq!(
            resolve(AppearanceMode::System, Appearance::Dark),
            Appearance::Dark
        );
    }

    #[test]
    fn pinned_modes_ignore_the_os() {
        for system in [Appearance::Light, Appearance::Dark] {
            assert_eq!(resolve(AppearanceMode::Light, system), Appearance::Light);
            assert_eq!(resolve(AppearanceMode::Dark, system), Appearance::Dark);
        }
    }

    #[test]
    fn default_mode_is_system() {
        assert_eq!(AppearanceMode::default(), AppearanceMode::System);
    }

    /// The setting round-trips through the settings file as a lowercase string.
    #[test]
    fn mode_serialises_stably() {
        for (mode, json) in [
            (AppearanceMode::System, "\"system\""),
            (AppearanceMode::Light, "\"light\""),
            (AppearanceMode::Dark, "\"dark\""),
        ] {
            assert_eq!(serde_json::to_string(&mode).unwrap(), json);
            assert_eq!(
                serde_json::from_str::<AppearanceMode>(json).unwrap(),
                mode,
                "{json} should parse back"
            );
        }
    }
}
