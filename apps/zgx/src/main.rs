//! Zeron's interface in zgx, starting with onboarding: the intro plays over
//! the desktop (zgx's `agents_intro`), then setup opens in a window of its
//! own.
//!
//! The hand-off is seamless: once the intro's way in shows, setup's window
//! opens hidden (out of Mission Control too) and builds its interface; the
//! click shows it under the scene, opaque there, so its first frame and
//! macOS's opening play before anything uncovers it. Over it the intro
//! decodes setup's first screen (sign-in) out of characters, the scene
//! closes around it, and that screen fades onto the window, which draws the
//! same: setup holds it to the intro's frame until the host tells it the
//! window is its own (`onboarding.arrived`).
//!
//! The interface is Luau under `ui/`, read from the source tree and
//! reloaded as it is saved, keeping its state. This host only makes the
//! windows, registers the fonts and answers the scripts' actions.
//!
//! Once setup is finished and the account is signed in, a launch opens the
//! main window instead (sessions, chats, files: `ui/screens/shell`), and so
//! does finishing setup; signed out, or new, it is setup again.
//!
//! Run: cargo run --release -- [intro | onboarding | main | gallery] [--inspect]
//! `ZERON_ZGX_STEP=<step>` starts onboarding at a step (signin, appearance,
//! username, card, org, invite, github, providers, done).
//! `ZERON_ZGX_REPLAY=1` shows every page, as if none were finished.
//! `--inspect` (or `ZGX_INSPECT_SOCKET=1`) serves devtools to `zgx inspect`.

mod about;
mod acp_registry;
mod agents;
mod appshots;
mod auth;
mod chat;
mod cloud;
mod dev;
mod completion;
mod engine;
mod engine_update;
mod editor_menu;
mod favicons;
mod files;
mod github;
mod haptics;
mod heap;
mod images;
mod links;
mod pasted;
mod math;
mod menu_bar;
mod plugins;
mod previews;
mod projects;
mod relaunch;
mod session_home;
mod sessions;
mod settings;
mod terminal;
mod tool_details;
mod themes;
mod visuals;
#[cfg(test)]
mod audit_shots;
#[cfg(test)]
mod menu_tests;
#[cfg(test)]
mod shell_tests;
#[cfg(test)]
mod split_tests;
#[cfg(test)]
mod route_motion_tests;
#[cfg(test)]
mod hit_sweep_tests;
#[cfg(test)]
mod hit_tests;
#[cfg(test)]
mod shortcut_tests;
#[cfg(test)]
mod menu_bar_tests;
#[cfg(test)]
mod focus_tests;
#[cfg(test)]
mod palette_focus_tests;
#[cfg(test)]
mod popups_tests;
#[cfg(test)]
mod placement_tests;
#[cfg(test)]
mod gif_tests;
#[cfg(test)]
mod picture_aspect_tests;
#[cfg(test)]
mod chip_copy_tests;
#[cfg(test)]
mod drop_tests;
#[cfg(test)]
mod fit_tests;
#[cfg(test)]
mod editor_scroll_tests;
#[cfg(test)]
mod i18n_tests;
#[cfg(test)]
mod visuals_tests;

use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use zgx::{compose::component, platform::PlatformService, scene::Rect, widgets::Ui};
use zgx_desktop::{
    Application, FontData, Glass, GlassMaterial, Titlebar, WindowBounds, WindowContext,
    WindowHandle, WindowLevel, WindowOptions,
};
use zgx_script::{ScriptHost, Shared, Value};

/// Where `ui.persist` keeps its files: the Zeron app's (the beta), apart from
/// development builds', which are staging's.
const APP_ID: &str = if cfg!(feature = "beta") { "zeron" } else { "zeron-zgx" };

/// Setup's window, in points: room for a step and its footer, no more.
const ONBOARDING_SIZE: (f64, f64) = (680., 500.);

#[derive(Clone)]
enum Screen {
    Intro,
    Onboarding,
    Main,
    Gallery,
    /// A fixture under ui/ (development).
    Fixture(String),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // A terminal tab's bridge (src/terminal.rs): this binary, run by zgx's
    // terminal, passing a shell's bytes to and from the engine.
    let argv: Vec<String> = std::env::args().collect();
    if argv.get(1).map(String::as_str) == Some("terminal-bridge") {
        std::process::exit(terminal::bridge(&argv[2..]));
    }
    // This launch as it started, for Reload Zeron (src/relaunch.rs), before
    // anything below sets a variable; started by one, the launch it
    // replaces waited out first.
    relaunch::remember();
    // No type check after each reload unless asked (`ZGX_CHECK=1`): a debug
    // zgx host runs `luau-lsp analyze` over the whole app on every reload,
    // on a thread of its own that a newer reload doesn't stop. With several
    // chats saving under ui/ that piled up hundreds of luau-lsp processes
    // (Nov 1). `zgx check ui` is the check; run it on purpose.
    if std::env::var_os("ZGX_CHECK").is_none() {
        // SAFETY: first thing in main, before any thread is started.
        unsafe { std::env::set_var("ZGX_CHECK", "0") };
    }
    // The browser tab's pages on Linux (zgx-desktop's `webkitgtk`) are drawn
    // in GTK 3 offscreen windows, which can't make a GL context. Where
    // WebKitGTK (2.42 and later) shares a page's frames as GPU buffers
    // (DMA-BUF: a GPU it can render with, as on NVIDIA), it needs one in
    // this process to show them and aborts the app without it ("GDK is not
    // able to create a GL context"). Shared memory instead: the web process
    // still renders, and its frames come back as pixels zgx copies anyway.
    #[cfg(target_os = "linux")]
    if std::env::var_os("WEBKIT_DMABUF_RENDERER_FORCE_SHM").is_none() {
        // SAFETY: first thing in main, before any thread is started.
        unsafe { std::env::set_var("WEBKIT_DMABUF_RENDERER_FORCE_SHM", "1") };
    }
    // `zeron`'s command line (crates/cli), as the Zeron app's: help,
    // `--version`, `update`, `daemon` here; `headless`, `login`, `status`,
    // `plugin` and the engine's other subcommands in the engine helper beside
    // this app (which doesn't link the engine), with the same arguments. A
    // development build keeps its own launch words (intro, gallery, a
    // fixture, --inspect) and its bare launch, which would otherwise log as
    // the Zeron app does (in ~/.zeron).
    let mut open_url = None;
    // (This app's own version for crates/update, before `zeron update` or
    // the app's updater compares releases with it: a beta's isn't the
    // workspace's.)
    zeron_update::set_app_version(about::APP_VERSION);
    if cfg!(feature = "beta") || !dev_launch(&argv[1..]) {
        match zeron_cli::run(about::APP_VERSION, zeron_cli::Engines::Helper) {
            zeron_cli::Launch::Done(result) => {
                // (As a `main` returning it would: the error, exit status 1.)
                if let Err(error) = result {
                    eprintln!("Error: {error:?}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            zeron_cli::Launch::Window { open_url: url } => open_url = url,
            zeron_cli::Launch::Engine(_) => unreachable!("the helper runs the engine's commands"),
            #[cfg(target_os = "linux")]
            zeron_cli::Launch::Appshot => return Err("Appshots from the command line aren't in this app yet".into()),
        }
    }
    let mut screen = Screen::Intro;
    let mut asked_for_intro = false;
    let mut inspect = false;
    for arg in std::env::args().skip(1).filter(|_| !cfg!(feature = "beta") && open_url.is_none()) {
        match arg.as_str() {
            "intro" => {
                screen = Screen::Intro;
                asked_for_intro = true;
            }
            "onboarding" => screen = Screen::Onboarding,
            "main" => screen = Screen::Main,
            "gallery" => screen = Screen::Gallery,
            "--inspect" => inspect = true,
            // Zeron's own template, whatever was in use (src/plugins/live.rs):
            // for one that breaks before its window can switch back.
            "--zeron-template" => plugins::ask_zeron_template(),
            // (Development: a fixture under ui/ in a window of its own, as
            // the gallery's; with `ZERON_ZGX_DEV_PLUGINS`, plugins in it.)
            other if other.starts_with("fixture=") && other.ends_with(".luau") => {
                screen = Screen::Fixture(other["fixture=".len()..].to_string());
            }
            other => {
                return Err(format!(
                    "unknown argument `{other}`: expected intro, onboarding, main, gallery, --inspect or --zeron-template"
                )
                .into());
            }
        }
    }
    // (`ZGX_INSPECT_SOCKET=1`: debug builds alone, src/dev.rs.)
    inspect |= dev::inspect_asked();
    if inspect {
        // Devtools label every node with the script line that made it, and
        // turn the layout warnings on as they start. Asked for from the
        // first load, the scripts record those lines as they first mount;
        // otherwise zgx loads them all a second time just after the first
        // frame, so the window comes up twice (focus, scroll and anything
        // still loading lost on every launch). Nothing else changes: the
        // warnings were on under devtools already.
        if std::env::var_os("ZGX_LAYOUT_WARNINGS").is_none() {
            // SAFETY: no other thread runs yet (arguments only, above).
            unsafe { std::env::set_var("ZGX_LAYOUT_WARNINGS", "1") };
        }
    }
    // Signed in on an earlier run: no intro, setup at once, where sign-in
    // leads (src/auth.rs). Asking for `intro` still plays it.
    if matches!(screen, Screen::Intro) && !asked_for_intro && auth::session_exists() {
        screen = Screen::Onboarding;
        // Setup finished on this Mac before: the main window.
        if onboarded() {
            screen = Screen::Main;
        }
    }

    // The scripts this launch runs, compiled on a worker while AppKit, the
    // GPU and the window are set up (zgx's `compile_ahead`): the first load's
    // requires find them compiled. (After the last `set_var` above: it
    // starts a thread.)
    compile_ahead(&screen);

    let mut application = Application::new();
    for font in fonts() {
        application = application.font(font?);
    }
    // The Zeron app's links (`zeron://`: a clicked banner, a chat's link),
    // delivered to the app as Zeron.app's are (src/links.rs).
    if cfg!(feature = "beta") {
        application = application
            .application_id("sh.zeron.app")?
            .on_application_event(|event, _| {
                if let zgx_desktop::ApplicationEvent::OpenUrls(urls) = event {
                    for url in urls {
                        links::open(&url);
                    }
                }
            });
    }
    let options = match screen {
        Screen::Intro => intro_window(),
        Screen::Onboarding => onboarding_window(),
        Screen::Main => main_window(),
        Screen::Gallery | Screen::Fixture(_) => gallery_window(),
    };
    application.window(options).run(move |cx| {
        match &screen {
            Screen::Intro => intro(cx, inspect),
            Screen::Onboarding => open(cx, "app.luau", inspect),
            Screen::Main => open_main(cx, inspect),
            Screen::Gallery => open(cx, "gallery.luau", inspect),
            Screen::Fixture(entry) => open(cx, entry, inspect),
        }
        // Launched on a link (`zeron <url>`).
        if let Some(url) = &open_url {
            links::open(url);
        }
    })?;
    // Quitting: an update downloaded and never restarted for installs now,
    // so the next launch is on it (src/about.rs).
    about::install_on_quit();
    // Reload Zeron: the windows closed, a fresh launch in this one's place.
    relaunch::after_quit();
    Ok(())
}

/// Whether `args` are a development launch's own words (a screen, a
/// fixture, devtools), or none: not `zeron`'s command line.
fn dev_launch(args: &[String]) -> bool {
    args.iter().all(|arg| {
        matches!(arg.as_str(), "intro" | "onboarding" | "main" | "gallery" | "--inspect" | "--zeron-template")
            || (arg.starts_with("fixture=") && arg.ends_with(".luau"))
    })
}

/// What `screen` loads first, compiled ahead with the design system it
/// lends (`ds`): the intro, then setup's `app.luau` it leads to; setup; the
/// main window's `app.luau` in `.live` as the last launch laid it (laid
/// again as the window opens: a file that changes then compiles at its
/// require, as before), else ui/'s; the gallery or a fixture.
fn compile_ahead(screen: &Screen) {
    let ui = ui_dir();
    let ahead = |dir: &std::path::Path, entry: &str| {
        let ds = dir.join("ds");
        zgx_script::compile_ahead(&dir.join(entry), &[("ds", ds.as_path())]);
    };
    match screen {
        Screen::Intro => {
            ahead(&ui, "intro.luau");
            ahead(&ui, "app.luau");
        }
        Screen::Onboarding => ahead(&ui, "app.luau"),
        Screen::Main => {
            let live = plugins::templates_root()
                .map(|root| root.join(plugins::live::LIVE))
                .filter(|live| live.join("app.luau").is_file());
            ahead(live.as_deref().unwrap_or(&ui), "app.luau");
        }
        Screen::Gallery => ahead(&ui, "gallery.luau"),
        Screen::Fixture(entry) => ahead(&ui, entry),
    }
}

/// The intro: a transparent, frameless window over the whole display, above
/// other windows, letting the pointer through until the script asks for it.
fn intro(cx: &mut WindowContext, inspect: bool) {
    cx.window.set_pointer_passthrough(true);
    let host = script_host(cx, "intro.luau");
    host.shared("onboarding.width", ONBOARDING_SIZE.0);
    host.shared("onboarding.height", ONBOARDING_SIZE.1);
    // Setup's window's corner radius, which the card ends on: the window
    // reports it once it exists.
    let radius = host.shared("onboarding.radius", 0.);
    // The intro's copy of setup's screen is never the window: it holds the
    // frame the window holds until `done`.
    host.shared("onboarding.arrived", false);
    // Setup's window is made hidden (`prepare`): the intro may ask for it at
    // launch, over the desktop.
    host.shared("onboarding.prepare_hidden", true);
    let setup: Rc<RefCell<Option<WindowHandle>>> = Rc::default();
    // Setup's `onboarding.arrived`, set once the window is its own.
    let arrival: Rc<RefCell<Option<Shared>>> = Rc::default();
    // Setup's `onboarding.epoch`: when the way in was taken, which its card
    // wall reckons its drift from, as the intro's copy of it does.
    let epoch: Rc<RefCell<Option<Shared>>> = Rc::default();

    let window = cx.window.clone();
    host.action("interactive", move |_| {
        window.set_pointer_passthrough(false);
        window.request_focus();
    });
    // `prepare { x, y, w, h }`: setup's window at that rect (points from the
    // overlay's top left, where the intro draws its screen over it), once,
    // made hidden (its screen built and its first frame drawn meanwhile, so
    // the intro can ask for it at launch, over the desktop) until `reveal`
    // shows it whole.
    let (overlay, windows, opened) = (cx.window.clone(), cx.windows.clone(), setup.clone());
    let (bounds, viewport) = (cx.window_info.bounds.clone(), cx.viewport.clone());
    let (arriving, epoch_slot) = (arrival.clone(), epoch.clone());
    host.action("prepare", move |rect| {
        if opened.borrow().is_some() {
            return;
        }
        let at = |key| rect.field(key).and_then(Value::as_f64).unwrap_or(0.);
        let (origin, physical_width) =
            bounds.with_untracked(|b| (b.position.unwrap_or((0, 0)), f64::from(b.size.0)));
        let scale = physical_width / viewport.with_untracked(|v| f64::from(v.0)).max(1.);
        let px = |points: f64| (points * scale).round();
        let options = WindowOptions {
            bounds: Some(WindowBounds {
                position: Some((origin.0 + px(at("x")) as i32, origin.1 + px(at("y")) as i32)),
                size: (px(at("w")) as u32, px(at("h")) as u32),
            }),
            // Not a window yet, to the user or to Mission Control: shown by
            // `reveal`, under the scene.
            visible: false,
            ..onboarding_window()
        };
        let (overlay, radius, arriving, epoch_slot) =
            (overlay.clone(), radius.clone(), arriving.clone(), epoch_slot.clone());
        let handle = windows.open(options, move |cx| {
            radius.set(cx.window_info.corner_radius.with_untracked(|r| *r).into());
            // Devtools are one socket a process: the intro keeps them (run
            // `onboarding` to inspect setup).
            let host = script_host(cx, "app.luau");
            *arriving.borrow_mut() = Some(host.shared("onboarding.arrived", false));
            *epoch_slot.borrow_mut() = Some(host.shared("onboarding.epoch", 0.));
            serve(cx, host, "app.luau", false);
            // Keys stay with the intro until it hands over.
            overlay.request_focus();
        });
        *opened.borrow_mut() = Some(handle);
    });
    // The way in taken (at `ui.now()`'s time, the argument): setup's window
    // shows, under the scene's picture, its card wall reckoned from then.
    let shown = setup.clone();
    host.action("reveal", move |at| {
        if let (Some(epoch), Some(at)) = (&*epoch.borrow(), at.as_f64()) {
            epoch.set(Value::Num(at));
        }
        if let Some(window) = &*shown.borrow() {
            window.set_visible(true);
        }
    });
    // The card has faded over setup's window: it takes over.
    let (overlay, windows) = (cx.window.clone(), cx.windows.clone());
    host.action("done", move |_| {
        if let Some(arrived) = &*arrival.borrow() {
            arrived.set(Value::Bool(true));
        }
        match &*setup.borrow() {
            Some(window) => {
                window.set_visible(true);
                window.request_focus();
            }
            None => {
                windows.open(onboarding_window(), move |cx| open(cx, "app.luau", false));
            }
        }
        overlay.close();
    });
    serve(cx, host, "intro.luau", inspect);
}

/// The script `entry` under `ui/` in the window.
fn open(cx: &mut WindowContext, entry: &str, inspect: bool) {
    let host = script_host(cx, entry);
    // (Development: `ZERON_ZGX_DEV_PLUGINS` loads plugin folders into a
    // fixture's window, no engine. src/plugins `dev_plugins`.)
    plugins::dev_plugins(&host);
    // (Development: `ZERON_ZGX_HOST_TOASTS=1` lays the host's toasts over a
    // fixture, a template in preview and a notice of each kind.)
    let chrome = plugins::host_bar::dev_toasts(cx);
    let floor = chrome.is_some().then(|| host.shared("toast_floor", 16.));
    serve_with(cx, host, entry, inspect, chrome, menu_bar::Role::Bar);
    if let Some(floor) = floor {
        plugins::host_bar::follow(floor);
    }
}

/// The main window: app.luau as the shell (`window` = "main"), where setup
/// is app.luau as onboarding.
fn open_main(cx: &mut WindowContext, inspect: bool) {
    // The active template laid over Zeron's own, in `<data>/templates/.live/`
    // (src/plugins/live.rs): the host's file is fixed as it's made, so it's
    // that folder's, whichever template is in use.
    let host = script_host_at(cx, plugins::prepare_live(), "app.luau");
    host.shared("window", "main");
    // Plugins and your templates (src/plugins): the main window's alone.
    plugins::install(cx, &host);
    // The host's own toasts over it (a template in preview, a switch you
    // didn't make, signing keys, safe mode at launch) and the menu bar
    // where the system has none.
    let chrome = plugins::host_bar::view(cx);
    // Where the app's toasts stand, over the host's (host_bar.rs `follow`).
    let floor = host.shared("toast_floor", 16.);
    let view = serve_with(cx, host, "app.luau", inspect, Some(chrome), menu_bar::Role::Bar);
    plugins::host_bar::follow(floor);
    // What zgx reports of the window's elements, for plugins' places.
    plugins::set_view(view);
    // Launch's allocator peak handed back once it has settled (src/heap.rs).
    heap::install(cx);
}

/// Where finishing setup on this Mac is remembered: a launch signed in
/// opens the main window once it's there.
fn onboarded_marker() -> Option<PathBuf> {
    zgx_script::app_data_dir(APP_ID).map(|dir| dir.join("onboarded"))
}

fn onboarded() -> bool {
    onboarded_marker().is_some_and(|path| path.is_file()) || (cfg!(feature = "beta") && finished_in_zeron_app())
}

/// Whether setup was finished in the Zeron app's own interface before this
/// one replaced it (its `onboardingCompleted` in the data dir's
/// ui-settings.json, crates/ui settings.rs): someone who has used Zeron isn't
/// walked through it again.
fn finished_in_zeron_app() -> bool {
    engine::data_dir()
        .and_then(|dir| std::fs::read(dir.join("ui-settings.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .is_some_and(|settings| settings["onboardingCompleted"].as_bool() == Some(true))
}

/// A host for the script `entry` under `ui/`, with what every window gives
/// its scripts.
fn script_host(cx: &mut WindowContext, entry: &str) -> ScriptHost {
    script_host_at(cx, ui_dir().join(entry), entry)
}

/// `script_host`, its script at `path` (the main window's, in its template's
/// folder).
fn script_host_at(cx: &mut WindowContext, path: PathBuf, entry: &str) -> ScriptHost {
    // Zeron's design system, lent to the window's plugins (`@ds`).
    let lent = path.clone();
    let host = plugins::lend_design_system(ScriptHost::new(&cx.ui.runtime, path).app_id(APP_ID), &lent);
    // The network, for `ui.fetch` and remote images: the app's scripts, and
    // plugins granted "network" (zgx's own transport; a plugin without the
    // grant reaches none).
    zgx_run::install_fetcher(&host);
    host.window_chrome(
        cx.window_info.titlebar_area.clone(),
        cx.window_info.controls.clone(),
    );
    host.window_zoom(&cx.ui);
    // `ui.window.size` (the lightbox fills the window with it).
    host.window_size(cx.viewport.clone());
    if let Ok(step) = std::env::var("ZERON_ZGX_STEP") {
        host.shared("onboarding.start", step.as_str());
    }
    // Every page again, whatever the account and this Mac have finished.
    if std::env::var_os("ZERON_ZGX_REPLAY").is_some() {
        host.shared("onboarding.replay", true);
    }
    // ds/transcript's trace (`ZERON_ZGX_TRACE_TRANSCRIPT=1`): the hover lane
    // and the rows' heights as they change, a line each on stdout.
    if std::env::var("ZERON_ZGX_TRACE_TRANSCRIPT").is_ok_and(|v| v == "1") {
        host.shared("debug.trace_transcript", true);
    }
    let window = cx.window.clone();
    host.action("close", move |_| window.close());
    // Reload Zeron: the app quits and starts again, the same launch
    // (src/relaunch.rs).
    let windows = cx.windows.clone();
    host.action("app.relaunch", move |_| relaunch::ask(|| windows.quit()));
    // Setup finished: remembered, and the main window opens in its place.
    let (window, windows) = (cx.window.clone(), cx.windows.clone());
    host.action("app.open_main", move |_| {
        if let Some(path) = onboarded_marker() {
            let _ = path.parent().map(std::fs::create_dir_all);
            if let Err(error) = std::fs::write(&path, b"") {
                eprintln!("zeron-zgx: setup's end wasn't remembered: {error}");
            }
        }
        windows.open(main_window(), move |cx| open_main(cx, false));
        window.close();
    });
    // Signed out in the main window: setup again, in its window.
    let (window, windows) = (cx.window.clone(), cx.windows.clone());
    host.action("app.onboard", move |_| {
        windows.open(onboarding_window(), move |cx| open(cx, "app.luau", false));
        window.close();
    });
    // A web page in the browser (a post's draft on X, a link's Open), or a
    // `mailto:` in the mail app: what a link's `href` opens. Anything else
    // is ignored.
    host.action("open_url", |url| {
        if let Value::Str(url) = url
            && (url.starts_with("https://") || url.starts_with("http://") || url.starts_with("mailto:"))
            && !url.chars().any(char::is_control)
            && let Err(error) = open::that_detached(&*url)
        {
            eprintln!("zeron-zgx: the browser didn't open: {error}");
        }
    });
    // Scripts can't reach the text selection (zgx gives Luau no API for it):
    // `read_selection` puts it in the shared state `selection`, and
    // `clear_selection` ends it.
    let selection = host.shared("selection", Value::Nil);
    let ui = cx.ui.downgrade();
    host.action("read_selection", move |_| {
        if let Some(ui) = ui.upgrade() {
            selection.set(selected(&ui));
        }
    });
    let ui = cx.ui.downgrade();
    host.action("clear_selection", move |_| {
        if let Some(ui) = ui.upgrade() {
            ui.clear_text_selection();
        }
    });
    // `appearance(dark)`: the windows' own chrome (the traffic lights, the
    // title bar's material) in the theme's appearance, `true` dark, `false`
    // light, nil the system's. zgx themes the content only.
    host.action("appearance", |value| {
        set_appearance(match value {
            Value::Bool(dark) => Some(dark),
            _ => None,
        })
    });
    // LaTeX math laid out for `ds/math` (src/math.rs): every window.
    math::install(&host);
    // A slider's detents tapped under the trackpad (src/haptics.rs).
    haptics::install(&host);
    // The engine's account (src/auth.rs), this Mac's agents (src/agents.rs)
    // and GitHub for Cloud (src/github.rs): every window but the intro's.
    if entry != "intro.luau" {
        auth::install(cx, &host);
        agents::install(cx, &host);
        acp_registry::install(cx, &host);
        github::install(cx, &host);
        // Cloud and Billing (src/cloud.rs): Settings' pages, the new chat's
        // Cloud choice.
        cloud::install(cx, &host);
        // The sessions behind the main window (src/sessions.rs).
        sessions::install(cx, &host);
        // Adding a project, removing one (src/projects.rs).
        projects::install(&host);
        // A session's files (src/files.rs): the files pane and editor.
        files::install(cx, &host);
        // The composer's `/` commands and `$` skills, from the engine.
        completion::install(&host);
        // What a tool row opens to: its whole output or diff, fetched.
        tool_details::install(&host);
        // Appshots: the frontmost window, captured on its shortcut.
        appshots::install(cx, &host);
        // The panel's terminal tabs (src/terminal.rs).
        terminal::install(cx, &host);
        // The panel's home: run actions, the branch's totals (src/session_home.rs).
        session_home::install(&host);
        engine_update::install(cx, &host);
        // A chat: its transcript, sending, approvals (src/chat.rs).
        chat::install(cx, &host);
        // Agent visuals in a transcript (src/visuals.rs).
        visuals::install(cx, &host);
        // Settings' account, organization and engine calls (src/settings.rs).
        settings::install(cx, &host);
        // Settings → About & updates: versions and the engine's updater
        // (src/about.rs).
        about::install(cx, &host);
        // `zeron://` links the system handed the app (src/links.rs).
        links::install(&host);
        // Your imported VS Code themes (src/themes.rs).
        themes::install(cx, &host);
    }
    host
}

/// The selection for scripts: its text, and its first and last lines as they
/// show, in the window's points (`top`, `bottom`). Either is missing while the
/// row holding that end is scrolled away. Nil with nothing selected.
fn selected(ui: &Ui) -> Value {
    if !ui.has_text_selection() {
        return Value::Nil;
    }
    let text = ui.selected_text();
    let mut lines: Vec<Rect> = Vec::new();
    if let Some((anchor, focus)) = ui.text_selection() {
        let scene = ui.scene.borrow();
        for node in [anchor.node, focus.node] {
            if !scene.contains(node) {
                continue;
            }
            let at = scene.bounds(node);
            if let Some(highlight) = scene.text_highlight(node) {
                lines.extend(highlight.rects.iter().map(|r| Rect {
                    x: at.x + r.x,
                    y: at.y + r.y,
                    ..*r
                }));
            }
        }
    }
    let line = |r: &Rect| {
        let n = |v: f32| Value::from(f64::from(v));
        Value::record([
            ("x", n(r.x)),
            ("y", n(r.y)),
            ("w", n(r.width)),
            ("h", n(r.height)),
        ])
    };
    let mut fields = vec![("text", Value::from(text.as_str()))];
    if let Some(top) = lines.iter().min_by(|a, b| a.y.total_cmp(&b.y)) {
        fields.push(("top", line(top)));
    }
    if let Some(bottom) = lines.iter().max_by(|a, b| a.y.total_cmp(&b.y)) {
        fields.push(("bottom", line(bottom)));
    }
    Value::record(fields)
}

/// Every window's native appearance: dark, light, or (`None`) the system's.
#[cfg(target_os = "macos")]
fn set_appearance(dark: Option<bool>) {
    use objc2::{class, msg_send, runtime::AnyObject};
    use objc2_foundation::NSString;
    // SAFETY: `+[NSApplication sharedApplication]` and `+[NSAppearance
    // appearanceNamed:]` return objects AppKit keeps (unretained, used at
    // once); `-[NSApplication setAppearance:]` takes one or nil. On the main
    // thread, as every host action runs.
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let appearance: *mut AnyObject = match dark {
            Some(dark) => {
                let name = NSString::from_str(if dark {
                    "NSAppearanceNameDarkAqua"
                } else {
                    "NSAppearanceNameAqua"
                });
                msg_send![class!(NSAppearance), appearanceNamed: &*name]
            }
            None => std::ptr::null_mut(),
        };
        let _: () = msg_send![app, setAppearance: appearance];
    }
}

#[cfg(not(target_os = "macos"))]
fn set_appearance(_dark: Option<bool>) {}

/// What a menu over selected text does (zgx keeps its own menu's actions from
/// scripts): `copy_selection`, as rich text and plain; `look_up_selection`;
/// `select_all`, in the region the selection is in. The platform they go
/// through is the caller's to fill in.
fn selection_platform(cx: &mut WindowContext, host: &ScriptHost) -> Rc<RefCell<Option<PlatformService>>> {
    let platform: Rc<RefCell<Option<PlatformService>>> = Rc::default();
    let (ui, service) = (cx.ui.downgrade(), platform.clone());
    host.action("copy_selection", move |_| {
        if let (Some(ui), Some(service)) = (ui.upgrade(), &*service.borrow()) {
            service.0.set_clipboard_rich_text(ui.selected_text(), ui.selected_html());
        }
    });
    let (ui, service) = (cx.ui.downgrade(), platform.clone());
    host.action("look_up_selection", move |_| {
        if let (Some(ui), Some(service)) = (ui.upgrade(), &*service.borrow()) {
            ui.look_up_selection(service);
        }
    });
    let ui = cx.ui.downgrade();
    host.action("select_all", move |_| {
        if let Some(ui) = ui.upgrade()
            && let Some((anchor, _)) = ui.text_selection()
        {
            ui.select_all_text(anchor.node);
        }
    });
    platform
}

/// Loads `host`'s scripts into the window, reloading them as they change,
/// with devtools when asked.
fn serve(cx: &mut WindowContext, host: ScriptHost, entry: &str, inspect: bool) -> zgx::compose::ViewHandle {
    serve_with(cx, host, entry, inspect, None, menu_bar::Role::Bar)
}

/// `serve`, for a sheet in a window of its own (a plugin's or a template's
/// consent): its commands, if any, are never the app's menu bar, which
/// stays the window's that has it (src/menu_bar.rs).
fn serve_sheet(cx: &mut WindowContext, host: ScriptHost, entry: &str, inspect: bool) -> zgx::compose::ViewHandle {
    serve_with(cx, host, entry, inspect, None, menu_bar::Role::Sheet)
}

/// `serve`, with what the host draws above the scripts' view (`chrome`: the
/// main window's own bar, src/plugins/host_bar.rs), outside anything the
/// scripts build.
fn serve_with(
    cx: &mut WindowContext,
    host: ScriptHost,
    entry: &str,
    inspect: bool,
    chrome: Option<zgx::compose::View>,
    role: menu_bar::Role,
) -> zgx::compose::ViewHandle {
    let _ = host.reload();
    // (The scripts' garbage is collected as the window goes idle, by zgx
    // itself: `zgx::idle`. The app once looked for quiet twice a second.)
    cx.tasks.spawn(host.watch(Duration::from_millis(250)));
    if inspect {
        zgx_script::inspect::prepare();
    }
    // The platform, which the selection's Copy and Look Up go through, is
    // found as the view mounts.
    let platform = selection_platform(cx, &host);
    // An editor's menu (ds/editor_menu): Cut, Copy, Paste, Select All, Look Up.
    editor_menu::install(&host, cx.ui.downgrade(), platform.clone());
    // A picture copied or shown in Finder (the lightbox's).
    images::install(&host, platform.clone());
    // A picture pasted into the composer, written out to send.
    pasted::install(&host, platform.clone());
    // A page's icon, for the panel's browser tab.
    favicons::install(&host);
    // The project's dev servers, for the browser tab's start page.
    previews::install(&host);
    let looks_up = host.shared("looks_up", false);
    let view = host.view();
    let handle = cx.render(component(move |cx| {
        let found = cx.try_service::<PlatformService>().map(|p| (*p).clone());
        looks_up.set(Value::from(found.as_ref().is_some_and(|p| p.0.looks_up())));
        *platform.borrow_mut() = found;
        match chrome {
            Some(chrome) => plugins::host_bar::compose(chrome, view),
            None => view,
        }
    }));
    let devtools = if inspect {
        let session = zgx_script::inspect::Session::new(
            &cx.ui,
            handle.clone(),
            &host,
            Some(cx.inspector.clone()),
        );
        let tasks = cx.tasks.clone();
        match zgx_script::inspect::serve(session, entry, move |task| tasks.spawn(task)) {
            Ok(server) => {
                eprintln!("zeron-zgx: devtools for {entry} at {}", server.path().display());
                Some(server)
            }
            Err(error) => {
                eprintln!("zeron-zgx: devtools: {error}");
                None
            }
        }
    } else {
        None
    };
    let commands = serve_commands(cx, &host, role);
    // Reload interface: every script evaluated again and the window's
    // interface mounted anew, as a save under ui/ does (state kept), on the
    // next turn rather than inside the command that asks. (The host is held
    // only while the window is open.)
    let reloading: Rc<RefCell<Option<ScriptHost>>> = Rc::new(RefCell::new(Some(host.clone())));
    let (slot, tasks) = (reloading.clone(), cx.tasks.clone());
    host.action("app.reload_interface", move |_| {
        let slot = slot.clone();
        tasks.spawn(async move {
            let host = slot.borrow().clone();
            if let Some(host) = host
                && let Err(error) = host.reload()
            {
                eprintln!("zeron-zgx: reload interface: {error}");
            }
        });
    });
    let keep = RefCell::new(Some((host, devtools, commands)));
    cx.on_closed(move || {
        reloading.borrow_mut().take();
        if let Some((host, devtools, commands)) = keep.borrow_mut().take() {
            host.save();
            drop(devtools);
            drop(commands);
        }
    });
    handle
}

/// The scripts' commands as the app's menu bar while this window has it
/// (src/menu_bar.rs: one bar for the whole app, the last opened window's
/// that has one; a sheet never takes it), and each item it shows chosen
/// here run in this window's scripts. Keep what it returns while the window
/// lives: dropped, the bar is the next window's again.
fn serve_commands(cx: &mut WindowContext, host: &ScriptHost, role: menu_bar::Role) -> menu_bar::Served {
    // The host's own items follow a template going in and out of preview.
    let preview = cx.ui.runtime.signal(plugins::preview::current());
    let heard = preview.clone();
    plugins::preview::on_change(move |now| {
        heard.set(now.cloned());
    });
    let served = menu_bar::of(&cx.windows).serve(&cx.ui.runtime, host, preview, role);
    let run = host.clone();
    cx.on_menu_action(move |action| {
        // The host's own items (no script's: `zeron.` is the host's, and the
        // bar leaves the scripts' out): a way back from any template, and
        // keeping the one in preview, from any window.
        if action.0.starts_with(menu_bar::HOST_COMMANDS) {
            plugins::host_command(&action.0);
            return;
        }
        // A sheet's bar is the window's under it: its items aren't the
        // sheet's to run.
        if role == menu_bar::Role::Sheet {
            return;
        }
        if !run.run_command(&action.0) {
            eprintln!("zeron-zgx: menus: {} chosen, but no command of this window runs it", action.0);
        }
    });
    served
}

fn intro_window() -> WindowOptions {
    WindowOptions {
        title: "Zeron".into(),
        transparent: true,
        decorations: false,
        resizable: false,
        covers_display: true,
        level: WindowLevel::AlwaysOnTop,
        shadow: false,
        ..Default::default()
    }
}

/// Zeron's window: an inset title bar over the colourless under-window
/// blur, which the theme's `background` tints (the shipping app's glass).
fn zeron_window(width: f64, height: f64) -> WindowOptions {
    WindowOptions {
        title: "Zeron".into(),
        width,
        height,
        decorations: cfg!(target_os = "macos"),
        titlebar: Titlebar::Inset,
        glass: Some(Glass::new(GlassMaterial::UnderWindowBackground).plain()),
        ..Default::default()
    }
}

/// Setup: a fixed size, placed by the intro's hand-off (or the platform),
/// on the glass as every window is; its frame (ds.Onboarding.Frame) covers
/// it with its own colour when the app is opaque.
fn onboarding_window() -> WindowOptions {
    WindowOptions {
        resizable: false,
        ..zeron_window(ONBOARDING_SIZE.0, ONBOARDING_SIZE.1)
    }
}

/// The main window: resizable, its frame kept across launches.
fn main_window() -> WindowOptions {
    placed(WindowOptions {
        min_size: Some((760., 500.)),
        frame_autosave: Some("zeron-zgx-main".into()),
        ..zeron_window(1240., 820.)
    })
}

/// A window put where `ZERON_ZGX_FRAME` says (`x,y,w,h,scale`: its corner
/// and size in physical pixels, and the display's scale), its frame neither
/// read from nor kept for the person's own window: scripts/smoke.sh's small
/// corner window.
fn placed(mut options: WindowOptions) -> WindowOptions {
    let Some(frame) = std::env::var("ZERON_ZGX_FRAME").ok() else {
        return options;
    };
    let n: Vec<f64> = frame.split(',').filter_map(|v| v.trim().parse().ok()).collect();
    if let [x, y, w, h, scale] = n[..] {
        options.width = w / scale.max(1.);
        options.height = h / scale.max(1.);
        options.frame_autosave = None;
        options.bounds = Some(WindowBounds {
            position: Some((x as i32, y as i32)),
            size: (w as u32, h as u32),
        });
    }
    options
}

fn gallery_window() -> WindowOptions {
    WindowOptions {
        min_size: Some((900., 560.)),
        frame_autosave: Some("zeron-zgx-gallery".into()),
        ..zeron_window(1240., 820.)
    }
}

/// The scripts: this folder's `ui/`, or `ZERON_ZGX_UI` (scripts/smoke.sh's
/// frozen copy, which other saves under ui/ can't reload mid-run), or, in
/// an app bundle (the Zeron app: the beta), the copy in its `Contents/Resources/ui`
/// (with `plugins-builtin` beside it).
fn ui_dir() -> PathBuf {
    match dev::smoke_switch("ZERON_ZGX_UI") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => bundled_ui().unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")),
    }
}

/// `Contents/Resources/ui` of the bundle this executable runs from, if any.
fn bundled_ui() -> Option<PathBuf> {
    static BUNDLED: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    BUNDLED
        .get_or_init(|| {
            let exe = std::env::current_exe().ok()?;
            let ui = exe.parent()?.parent()?.join("Resources").join("ui");
            ui.join("app.luau").is_file().then_some(ui)
        })
        .clone()
}

/// Geist (the iOS app's, copied into `fonts/` so this folder builds on its
/// own) for the interface, Instrument Serif (zeron.sh's) for display type.
fn fonts() -> impl Iterator<Item = Result<FontData, Box<dyn std::error::Error>>> {
    [
        &include_bytes!("../fonts/Geist.ttf")[..],
        include_bytes!("../fonts/Geist-Italic.ttf"),
        include_bytes!("../fonts/Geist-Medium.ttf"),
        include_bytes!("../fonts/Geist-SemiBold.ttf"),
        include_bytes!("../fonts/Geist-Bold.ttf"),
        include_bytes!("../fonts/GeistMono.ttf"),
        include_bytes!("../fonts/GeistMono-Medium.ttf"),
        include_bytes!("../fonts/InstrumentSerif-Regular.ttf"),
        include_bytes!("../fonts/InstrumentSerif-Italic.ttf"),
    ]
    .into_iter()
    .map(|bytes| FontData::new(Arc::<[u8]>::from(bytes)).map_err(Into::into))
}
