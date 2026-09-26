//! zeron-ui — the gpui viewport. Shell, sidebar, conversation, composer, terminal,
//! diff pane.
//!
//! Design: ARCHITECTURE.md §4; animation catalog docs/research/feature-inventory.md
//! §1.12; virtualization/markdown techniques docs/research/mugen-pretext.md.
//!
//! M3a foundation:
//! - [`theme`] — always-dark monochrome theme (oklch-derived neutrals), a gpui Global;
//! - [`motion`] — the zeron animation catalog over gpui `Animation` + cubic-bezier;
//! - [`state`] — `AppState` entity + `EngineHandle` (connect-or-embed engine);
//! - [`settings`] — persisted pane widths/collapse flags;
//! - [`shell`] — sidebar + main panel + right-pane scaffold + gate;
//! - [`loaders`] — zeron pulse loader, gradient spinner, boot splash.

mod account_usage;
pub mod app_menus;
mod app_runtime;
pub mod appearance;
pub mod appshots;
pub mod attachments;
pub mod badges;
pub mod browser;
pub mod change_requests;
pub mod changes;
mod chat_store;
mod comment_ui;
pub mod comments;
pub mod composer;
mod composer_dock;
mod composer_markdown;
mod context_usage;
pub mod edge_fade;
pub mod file_icons;
pub mod files;
pub mod frost;
pub mod gui_instance;
pub mod history;
pub mod icons;
pub(crate) mod image_media;
pub(crate) mod image_viewer;
mod lifecycle;
pub mod links;
pub mod loaders;
pub mod markdown;
pub mod motion;
mod new_thread_background_effects;
mod new_thread_background_image;
mod new_thread_background_mask;
mod notice;
mod notification_service;
pub mod notify;
pub mod pickers;
pub mod popover;
pub mod project_actions;
pub mod queue;
pub mod rail;
pub mod settings;
pub mod shell;
pub mod sound;
pub mod state;
pub(crate) mod surface_chrome;
pub mod syntax_cache;
pub mod terminal;
pub mod theme;
pub mod theme_library;
pub mod transcript;
pub mod typography;
#[cfg(feature = "multi-window-fixture")]
mod window_fixture;
mod window_manager;
mod workspace_links;

use std::path::PathBuf;

use futures::{FutureExt as _, StreamExt as _};
use gpui::{App, AppContext as _, Bounds, TitlebarOptions, WindowBounds, WindowOptions, px, size};

pub use state::EngineBootConfig;
pub use zeron_proto::HarnessId;

/// Everything the headed binary passes in (config/env resolution lives in
/// `apps/zeron`, not here).
#[derive(Debug, Clone)]
pub struct UiConfig {
    /// Data directory — engine stores + `ui-settings.json`.
    pub data_dir: PathBuf,
    /// Localhost IPC port: connect if an engine daemon is listening, embed if not.
    pub ipc_port: u16,
    /// Edge base URL for the embedded engine.
    pub edge_url: String,
    /// Edge bearer; `None` runs offline.
    pub edge_token: Option<String>,
    /// Workspace org override for explicit dev-mode runs.
    pub org_id: Option<String>,
    /// WorkOS client id; `Some` makes the embedded headed engine require a
    /// production session before opening identity-scoped stores.
    pub workos_client_id: Option<String>,
    /// Harness for doc-command runs until per-chat config lands (M4).
    pub default_harness: HarnessId,
    /// Conversation URL passed by the OS on a cold launch.
    pub initial_url: Option<String>,
    /// A cold `--new-window` launch starts on the blank canvas.
    pub initial_new_window: bool,
}

impl UiConfig {
    fn boot(&self) -> EngineBootConfig {
        EngineBootConfig {
            data_dir: self.data_dir.clone(),
            ipc_port: self.ipc_port,
            edge_url: self.edge_url.clone(),
            edge_token: self.edge_token.clone(),
            org_id: self.org_id.clone(),
            workos_client_id: self.workos_client_id.clone(),
            default_harness: self.default_harness,
        }
    }
}

/// Run the headed app: tokio bridge up, engine bootstrap kicked off (probe →
/// connect-or-embed), 1320×880 window (min 900×600) with [`shell::Shell`] as the
/// root view, boot splash overlaid until the engine reports ready.
pub fn run_app(config: UiConfig, instance: gui_instance::GuiInstance) {
    run_application(config, instance, None);
}

fn run_application(
    config: UiConfig,
    mut instance: gui_instance::GuiInstance,
    on_start: Option<Box<dyn FnOnce(&mut App)>>,
) {
    let mut launches = instance.incoming.take().expect("GUI launch receiver");
    // Retain ownership for the whole application lifetime. The bridge's
    // default runtime has only two workers, insufficient for a desktop engine.
    let runtime = tokio::runtime::Runtime::new().expect("desktop Tokio runtime");
    let runtime_handle = runtime.handle().clone();
    let app = gpui_platform::application().with_assets(icons::Assets);
    let (url_tx, mut url_rx) = futures::channel::mpsc::unbounded::<String>();
    let callback_tx = url_tx.clone();
    app.on_open_urls(move |urls| {
        for url in urls {
            let _ = callback_tx.unbounded_send(url);
        }
    });
    if let Some(url) = config.initial_url.clone() {
        let _ = url_tx.unbounded_send(url);
    }
    app.on_reopen(|cx| {
        window_manager::activate(cx);
    });
    app.run(move |cx: &mut App| {
        gpui_tokio::init_from_handle(cx, runtime_handle);
        gpui_base::init(cx);
        let data_dir = config.boot().data_dir.clone();
        let ui_settings = settings::UiSettings::load(&data_dir);
        settings::init(ui_settings.clone(), data_dir.clone(), cx);
        let font_availability = typography::register_fonts(cx);
        // Typography first: theme installation reads the effective family, so
        // the first frame has the final font and palette without a flash.
        typography::init(
            ui_settings.ui_font_family.clone(),
            ui_settings.ui_font_size,
            ui_settings.terminal_font_family.clone(),
            ui_settings.terminal_font_size,
            ui_settings.code_font_family.clone(),
            ui_settings.code_font_size,
            font_availability,
            cx,
        );
        theme_library::init(data_dir.clone(), cx);
        appearance::init(
            ui_settings.appearance,
            ui_settings.theme_selection,
            ui_settings.accent,
            ui_settings.surface,
            cx,
        );
        history::init(
            ui_settings.git_history_columns,
            ui_settings.git_history_column_widths,
            ui_settings.git_history_column_order,
            ui_settings.git_history_author_display,
            cx,
        );
        composer::init(cx, ui_settings.composer_send_behavior);
        appshots::set_enabled(ui_settings.appshots_enabled);
        terminal::panel::init(cx);
        app_menus::init(cx);
        if on_start.is_none() {
            cx.register_url_scheme("zeron").detach();
        }

        let owner = app_runtime::init(config.boot(), cx);
        cx.spawn(async move |cx| {
            while let Some(request) = launches.next().await {
                let _ = cx.update(|cx| match request {
                    gui_instance::LaunchRequest::Activate => {
                        window_manager::activate(cx);
                    }
                    gui_instance::LaunchRequest::NewWindow => {
                        window_manager::open(window_manager::Open::Blank, cx);
                    }
                    gui_instance::LaunchRequest::OpenUrl(url) => window_manager::deep_link(url, cx),
                });
            }
        })
        .detach();
        lifecycle::init(cx);
        window_manager::init(cx);
        shell::apply_keymap(cx, &ui_settings.keymap, ui_settings.composer_send_behavior);
        cx.spawn(async move |cx| {
            while let Some(url) = url_rx.next().await {
                let _ = cx.update(|cx| window_manager::deep_link(url, cx));
            }
        })
        .detach();
        // Banner clicks land on the notified chat. The AppKit delegate fires
        // mid-event, so hop through a channel rather than updating inline.
        let (click_tx, mut click_rx) = futures::channel::mpsc::unbounded::<String>();
        notify::on_click(move |chat_id| {
            let _ = click_tx.unbounded_send(chat_id);
        });
        cx.spawn(async move |cx| {
            while let Some(chat_id) = click_rx.next().await {
                let _ = cx.update(|cx| window_manager::notified_chat(chat_id, cx));
            }
        })
        .detach();
        state::AppState::bootstrap(owner, config.boot(), cx);
        window_manager::open(
            if config.initial_new_window {
                window_manager::Open::Blank
            } else {
                window_manager::Open::Restore
            },
            cx,
        );
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if on_start.is_none() {
            start_appshot_service(config.boot().data_dir, cx);
        }
        // Native menu bar — macOS gets the standard app menu (About/Services/
        // Hide/Quit ⌘Q), Edit clipboard verbs routed to the focused input, and
        // a Window menu (⌘M/⌘W). Without this, `NSApp.mainMenu` stays nil: no
        // Cmd+Q, and nothing for the system menu bar to show. Set after
        // `open_main_window` because `Shell::new` ran `apply_keymap`
        // synchronously, so `set_menus` reads the final bindings for the ⌘-key
        // equivalents (gpui snapshots the keymap at set time).
        cx.set_menus(app_menus::app_menus());
        // Keep the Dock action available for the application's lifetime,
        // including after the last macOS window closes.
        #[cfg(target_os = "macos")]
        cx.set_dock_menu(vec![gpui::MenuItem::action(
            "New Window",
            app_menus::NewWindow,
        )]);
        cx.activate(true);
        if let Some(on_start) = on_start {
            on_start(cx);
        }
    });
    drop(instance);
}

fn restored_main_window_bounds(cx: &App) -> (Bounds<gpui::Pixels>, Option<gpui::DisplayId>) {
    let fallback = (Bounds::centered(None, size(px(1320.), px(880.)), cx), None);
    let Some(saved) = settings::current(cx).window_geometry else {
        return fallback;
    };
    let displays = cx.displays();
    let primary = cx
        .primary_display()
        .and_then(|primary| {
            displays
                .iter()
                .position(|display| display.id() == primary.id())
        })
        .unwrap_or(0);
    let geometries: Vec<_> = displays
        .iter()
        .map(|display| {
            let mut geometry = settings::WindowGeometry::from_bounds(display.visible_bounds());
            geometry.display_uuid = display.uuid().ok();
            geometry
        })
        .collect();
    saved
        .restore(&geometries, primary)
        .map_or(fallback, |(index, geometry)| {
            (geometry.bounds(), Some(displays[index].id()))
        })
}

fn open_main_window(
    state: gpui::Entity<state::AppState>,
    boot: EngineBootConfig,
    cx: &mut App,
) -> anyhow::Result<gpui::WindowHandle<shell::Shell>> {
    let is_main = state.read(cx).window_key.as_deref() == Some("main");
    let restored_bounds = state
        .read(cx)
        .window_key
        .as_deref()
        .and_then(|key| settings::current(cx).windows.get(key).cloned())
        .and_then(|saved| saved.geometry)
        .and_then(|geometry| geometry.restore(cx));
    // Fall back to the pre-multi-window geometry so upgrading does not reset
    // the main window. Per-window geometry takes over after the first render.
    let (legacy_bounds, legacy_display_id) = restored_main_window_bounds(cx);
    let (window_bounds, display_id) = restored_bounds.map_or_else(
        || (WindowBounds::Windowed(legacy_bounds), legacy_display_id),
        |bounds| (bounds, None),
    );
    if is_main && restored_bounds.is_none() && settings::current(cx).window_geometry.is_some() {
        // Migrate the pre-multi-window geometry while platform access is safe.
        // Window-bound observers maintain this entry from this point forward.
        let geometry = settings::windows::WindowGeometry::capture(window_bounds);
        settings::update(settings::SavePolicy::Debounced, cx, |settings| {
            let mut window = settings::windows::WindowSettings::from_ui(settings);
            window.geometry = Some(geometry);
            settings.windows.insert("main".into(), window);
        });
    }
    let handle = cx.open_window(
        WindowOptions {
            window_bounds: Some(window_bounds),
            display_id,
            window_min_size: Some(size(px(900.), px(600.))),
            // `kind` is deliberately left at its default `WindowKind::Normal`
            // (gpui platform.rs WindowOptions::default), which on macOS maps
            // to `NSNormalWindowLevel` (gpui_macos window.rs) — same as zed's
            // main window. Nothing here raises the window level or touches
            // presentation options; the "menu bar never appears" symptom came
            // from the missing `set_menus` call (nil `NSApp.mainMenu`), not
            // from window kind/level, and `appears_transparent` only affects
            // the titlebar, not the menu bar.
            // macOS: frameless-inset chrome like the original Electron app
            // (`titleBarStyle: "hiddenInset"`, traffic lights at 14,15 —
            // feature-inventory §1.1). The strip is custom-drawn. Windows
            // still needs a native title for taskbar previews and Alt+Tab. On
            // Linux/Windows `appears_transparent` hides the system titlebar
            // for our custom-drawn chrome; harmless where unsupported.
            titlebar: Some(TitlebarOptions {
                title: cfg!(target_os = "windows").then(|| "Zeron".into()),
                appears_transparent: true,
                // Native lights are 14px tall: top 14 → center 21, matching
                // the 38px titlebar row with 4px top-only content padding.
                traffic_light_position: Some(gpui::point(px(14.), px(14.))),
            }),
            // Our own titlebar strip drags the window (WindowControlArea::
            // Drag + start_window_move) — mark the content view app-owned
            // so AppKit neither dead-zones the strip nor delays clicks.
            app_owns_titlebar_drag: true,
            // Linux: request client-side decorations — zeron draws its own
            // unified titlebar and (under CSD) its own caption buttons
            // (shell.rs `render_linux_caption_controls`). Leaving this unset
            // requests SERVER decorations, which stacked a compositor
            // titlebar on top of the app's chrome under sway/KDE, while
            // compositors without SSD support (GNOME) went client-side
            // anyway — frameless, and before the shell drew caption buttons,
            // with no window controls at all. The compositor can still
            // override via xdg-decoration negotiation; the shell re-resolves
            // what to draw every frame.
            window_decorations: cfg!(target_os = "linux")
                .then_some(gpui::WindowDecorations::Client),
            // Frosted shell (macOS): blur the desktop behind the window; the
            // shell paints its frost surface translucent so the sidebar reads
            // as glass (shell.rs root). Elsewhere blur support is compositor
            // roulette — stay opaque.
            // One source of truth with the re-apply loop in `appearance::apply`
            // — if these two ever disagree, vibrancy dies on the first theme
            // change and never comes back.
            window_background: theme::Theme::of(cx).window_background_appearance(),
            app_id: Some("zeron".into()),
            ..Default::default()
        },
        move |window, cx| {
            window.set_rem_size(px(typography::font_size(cx).pixels()));
            // React to the user flipping macOS between light and dark. Detached:
            // the subscription lives as long as the window does, and the window
            // owns nothing that would drop it early.
            appearance::observe_window(window, cx).detach();
            let shell = cx.new(|cx| shell::Shell::new(state, boot, cx));
            let weak_shell = shell.downgrade();
            window.on_window_should_close(cx, move |_, cx| {
                let should_close = weak_shell
                    .update(cx, |shell, cx| shell.prepare_window_close(cx))
                    .unwrap_or(true);
                if should_close {
                    settings::flush(cx);
                }
                should_close
            });
            shell
        },
    )?;
    // Belt and braces: assert the blur once the window actually exists. The
    // `WindowOptions` value is applied during creation, before the view is
    // attached; re-pushing it here means a window is never left opaque.
    appearance::reapply_window_background(cx);
    Ok(handle)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_appshot_service(activation_dir: std::path::PathBuf, cx: &mut App) {
    let mut shortcuts = appshots::start_global_shortcut(activation_dir);
    cx.spawn(async move |cx| {
        while shortcuts.next().await.is_some() {
            if !appshots::capture_allowed() {
                continue;
            }
            let Some(capture) = cx.update(start_appshot_capture) else {
                continue;
            };
            let capture = capture.await;
            // Coalesce presses made while capture was in flight. Delivery
            // focuses Zeron; replaying old activations would capture the wrong
            // app or show a misleading self-capture error after success.
            while matches!(shortcuts.next().now_or_never(), Some(Some(()))) {}
            cx.update(|cx| deliver_appshot(capture, cx));
        }
    })
    .detach();
}

/// Check viewer focus on the UI thread before any native capture or portal
/// request. Portals do not identify the source window, so their backends cannot
/// reject Zeron after the picker or capture has already started.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn start_appshot_capture(
    cx: &mut App,
) -> Option<gpui::Task<Result<appshots::CapturedAppshot, appshots::CaptureError>>> {
    if cx.active_window().is_some() {
        return None;
    }
    Some(
        cx.background_executor()
            .spawn(async { appshots::capture_active_window().await }),
    )
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod appshot_activation_tests {
    use super::*;

    struct ViewerWindow;

    impl gpui::Render for ViewerWindow {
        fn render(
            &mut self,
            _: &mut gpui::Window,
            _: &mut gpui::Context<Self>,
        ) -> impl gpui::IntoElement {
            gpui::div()
        }
    }

    #[gpui::test]
    fn appshot_capture_skips_any_focused_viewer_window(cx: &mut gpui::TestAppContext) {
        // The guard must cover every Zeron window, not only a Shell/chat root.
        for _ in 0..2 {
            let window = cx.add_window(|_, _| ViewerWindow);
            window
                .update(cx, |_, window, _| window.activate_window())
                .unwrap();
            cx.run_until_parked();
            cx.update(|cx| {
                assert!(cx.active_window().is_some());
                assert!(start_appshot_capture(cx).is_none());
            });
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn deliver_appshot(
    result: Result<appshots::CapturedAppshot, appshots::CaptureError>,
    cx: &mut App,
) {
    use std::collections::VecDeque;
    use std::sync::{Mutex, OnceLock};

    fn pending() -> &'static Mutex<VecDeque<appshots::CapturedAppshot>> {
        static PENDING: OnceLock<Mutex<VecDeque<appshots::CapturedAppshot>>> = OnceLock::new();
        PENDING.get_or_init(|| Mutex::new(VecDeque::new()))
    }

    if matches!(
        result,
        Err(appshots::CaptureError::Cancelled | appshots::CaptureError::SelfCapture)
    ) {
        return;
    }
    let mut captures = pending()
        .lock()
        .map(|mut queue| queue.drain(..).collect::<VecDeque<_>>())
        .unwrap_or_default();
    let error = match result {
        Ok(appshot) => {
            captures.push_back(appshot);
            None
        }
        Err(error) => Some(error),
    };
    let handle = cx
        .window_stack()
        .unwrap_or_else(|| cx.windows())
        .into_iter()
        .find_map(|handle| handle.downcast::<shell::Shell>())
        .or_else(|| window_manager::activate(cx));
    let Some(handle) = handle else {
        if !captures.is_empty() {
            let count = captures.len();
            if let Ok(mut queue) = pending().lock() {
                queue.extend(captures);
            }
            tracing::warn!(
                count,
                "Appshot captured with no Zeron window; preserving it for the next delivery"
            );
        }
        return;
    };
    let captured = !captures.is_empty();
    cx.activate(true);
    let _ = handle.update(cx, |shell, window, cx| {
        window.activate_window();
        for appshot in captures {
            shell.receive_appshot(appshot, window, cx);
        }
        if let Some(error) = error {
            shell.show_appshot_error(error.to_string(), window, cx);
        }
    });
    if captured {
        appshots::foreground_after_capture();
    }
}

/// Native regression fixture; absent from production builds.
#[cfg(feature = "multi-window-fixture")]
pub fn run_multi_window_fixture(data_dir: PathBuf, output: PathBuf) -> anyhow::Result<()> {
    let gui_instance::Launch::Primary(instance) =
        gui_instance::GuiInstance::acquire(&data_dir, gui_instance::LaunchRequest::Activate)?
    else {
        anyhow::bail!("fixture profile already has a GUI");
    };
    let config = UiConfig {
        data_dir,
        ipc_port: 0,
        edge_url: "http://127.0.0.1:1".into(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: HarnessId::Mock,
        initial_url: None,
        initial_new_window: false,
    };
    run_application(
        config,
        instance,
        Some(Box::new(move |cx| window_fixture::start(output, cx))),
    );
    Ok(())
}
