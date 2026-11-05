//! Right-panel reference stills: the production Shell, a real in-process
//! engine and a scratch git checkout, driven by the clicks and shortcuts a
//! person uses. Writes, into the output directory:
//!
//! - `right-empty.png` / `right-empty-660.png`: the right pane open on its
//!   empty (new tab) page, at the even split and dragged to 660px
//! - `right-file-explorer.png`: a file tab active with the explorer docked
//! - `right-tabs.png` / `right-tabs-660.png`: terminal + file tabs, explorer
//!   undocked; `right-tabs-hover-{inactive,active,plus}.png` hover states
//! - `result.txt`: window size, scale factor and the run's notes
//!
//!     cargo build -p zeron-ui --example right-panel-fixture --features appshots-fixture
//!     target/debug/examples/right-panel-fixture <out-dir>
//!
//! ZERON_PALETTE_LIGHT=1 renders the light theme. Clicks land at computed
//! logical positions; override with ZERON_RIGHT_TERMINAL=x,y and
//! ZERON_RIGHT_TREE=x,y0 (the explorer's first row) when a layout moves.
use anyhow::Context as _;
use gpui::{AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use zeron_ui::*;

const WINDOW: (f32, f32) = (1440.0, 900.0);
const SIDEBAR: f32 = 256.0;
const RIGHT_PANE: f32 = 660.0;

const FILES_RS: &str = r#"//! Workspace file browsing for a chat's checkout.
use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

/// One row of a directory listing, as the explorer draws it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
}

/// List `dir` relative to `root`, directories first, then by name.
pub fn list(root: &Path, dir: &str) -> anyhow::Result<Vec<Entry>> {
    let base = root.join(dir);
    let mut entries = Vec::new();
    for item in std::fs::read_dir(&base).with_context(|| format!("reading {dir}"))? {
        let item = item?;
        let meta = item.metadata()?;
        let name = item.file_name().to_string_lossy().into_owned();
        let kind = if meta.is_dir() {
            EntryKind::Directory
        } else if meta.file_type().is_symlink() {
            EntryKind::Symlink
        } else {
            EntryKind::File
        };
        entries.push(Entry {
            path: join(dir, &name),
            size: (kind == EntryKind::File).then(|| meta.len()),
            name,
            kind,
        });
    }
    entries.sort_by(|a, b| {
        (a.kind != EntryKind::Directory)
            .cmp(&(b.kind != EntryKind::Directory))
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(entries)
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{dir}/{name}")
    }
}

/// Resolve a workspace-relative path, refusing anything outside `root`.
pub fn resolve(root: &Path, relative: &str) -> Option<PathBuf> {
    let candidate = root.join(relative).canonicalize().ok()?;
    candidate.starts_with(root).then_some(candidate)
}
"#;

const MAIN_RS: &str = "fn main() {\n    println!(\"hello\");\n}\n";
const SHELL_RS: &str = "pub struct Shell;\n";
const CARGO_TOML: &str = "[package]\nname = \"zeron\"\nversion = \"0.1.0\"\nedition = \"2024\"\n";
const README: &str = "# zeron\n\nScratch checkout for the right-panel stills.\n";

fn xy(var: &str, default: (f32, f32)) -> (f32, f32) {
    std::env::var(var)
        .ok()
        .and_then(|value| {
            let (x, y) = value.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .unwrap_or(default)
}

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(std::time::Duration::from_millis(ms))
        .await;
}

fn capture(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, dir: &Path, name: &str) {
    window
        .update(cx, |_, w, cx| {
            w.draw(cx).clear();
            if let Ok(image) = w.render_to_image() {
                image.save(dir.join(format!("{name}.png"))).ok();
            }
        })
        .ok();
}

fn input(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, event: gpui::PlatformInput) {
    window
        .update(cx, |_, w, cx| {
            w.dispatch_event(event, cx);
        })
        .ok();
}

fn hover(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, (x, y): (f32, f32)) {
    input(
        window,
        cx,
        gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
            position: gpui::point(px(x), px(y)),
            pressed_button: None,
            modifiers: Default::default(),
        }),
    );
}

async fn click(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, at: (f32, f32)) {
    hover(window, cx, at);
    pause(cx, 80).await;
    let position = gpui::point(px(at.0), px(at.1));
    input(
        window,
        cx,
        gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
            position,
            button: gpui::MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
    );
    input(
        window,
        cx,
        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
            position,
            button: gpui::MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
    );
}

fn action(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, action: Box<dyn gpui::Action>) {
    window
        .update(cx, |_, w, cx| w.dispatch_action(action, cx))
        .ok();
}

/// Drag the surface pane's seam (the real resize grabber) to `to_x`.
async fn drag_seam(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, from_x: f32, to_x: f32) {
    let y = 450.0;
    hover(window, cx, (from_x, y));
    pause(cx, 120).await;
    input(
        window,
        cx,
        gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
            position: gpui::point(px(from_x), px(y)),
            button: gpui::MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
    );
    let steps = 12;
    for step in 1..=steps {
        let x = from_x + (to_x - from_x) * step as f32 / steps as f32;
        input(
            window,
            cx,
            gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                position: gpui::point(px(x), px(y)),
                pressed_button: Some(gpui::MouseButton::Left),
                modifiers: Default::default(),
            }),
        );
        pause(cx, 30).await;
    }
    input(
        window,
        cx,
        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
            position: gpui::point(px(to_x), px(y)),
            button: gpui::MouseButton::Left,
            click_count: 1,
            ..Default::default()
        }),
    );
    pause(cx, 100).await;
    hover(window, cx, (500.0, 600.0));
    pause(cx, 700).await;
}

async fn hover_capture(
    window: gpui::AnyWindowHandle,
    cx: &mut gpui::AsyncApp,
    dir: &Path,
    name: &str,
    at: (f32, f32),
) {
    hover(window, cx, at);
    pause(cx, 120).await;
    hover(window, cx, (at.0 + 1.0, at.1));
    pause(cx, 600).await;
    capture(window, cx, dir, name);
}

async fn film(
    window: gpui::AnyWindowHandle,
    dir: PathBuf,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<String> {
    let mut notes = String::new();
    let scale = window.update(cx, |_, w, _| w.scale_factor())?;
    // Opening a pane splits the room right of the sidebar evenly
    // (`split_panel_widths` / `content_split`); a seam drag holds a width.
    let split = (WINDOW.0 - SIDEBAR) / 2.0;
    let split_left = WINDOW.0 - split;
    let target_left = WINDOW.0 - RIGHT_PANE;
    notes.push_str(&format!(
        "window {}x{} logical, scale {scale}, sidebar {SIDEBAR}, even split pane {split} (x={split_left}), dragged pane {RIGHT_PANE} (x={target_left})\n",
        WINDOW.0, WINDOW.1
    ));
    // Park the pointer over the conversation.
    hover(window, cx, (500.0, 600.0));
    action(window, cx, Box::new(shell::ToggleChanges));
    pause(cx, 1200).await;
    capture(window, cx, &dir, "right-empty");
    drag_seam(window, cx, split_left, target_left).await;
    capture(window, cx, &dir, "right-empty-660");
    // The picker's rows (Browser, Terminal, Diffs, History in a git
    // project: 44px rows, 8px gaps) centre in the pane below the titlebar.
    let content_mid = theme::Theme::TITLEBAR_HEIGHT + (WINDOW.1 - theme::Theme::TITLEBAR_HEIGHT) / 2.0;
    let terminal_at = xy(
        "ZERON_RIGHT_TERMINAL",
        (target_left + RIGHT_PANE / 2.0, content_mid - 26.0),
    );
    notes.push_str(&format!("terminal row click {terminal_at:?}\n"));
    click(window, cx, terminal_at).await;
    pause(cx, 1500).await;
    action(window, cx, Box::new(shell::ToggleFiles));
    pause(cx, 1500).await;
    let files_left = WINDOW.0 - settings::FILES_PANEL_DEFAULT;
    let (tree_x, tree_y0) = xy("ZERON_RIGHT_TREE", (files_left + 60.0, 113.5));
    notes.push_str(&format!("tree row0 click ({tree_x}, {tree_y0})\n"));
    click(window, cx, (tree_x, tree_y0)).await;
    pause(cx, 1000).await;
    click(window, cx, (tree_x, tree_y0 + 27.0)).await;
    pause(cx, 1800).await;
    hover(window, cx, (500.0, 600.0));
    pause(cx, 400).await;
    capture(window, cx, &dir, "right-file-explorer");
    action(window, cx, Box::new(shell::ToggleFiles));
    pause(cx, 1500).await;
    hover(window, cx, (500.0, 600.0));
    pause(cx, 400).await;
    capture(window, cx, &dir, "right-tabs");
    // Chips start 9px into the pane (8px content pad + 1px seam); 112 wide,
    // 4px apart, centred on y≈21 in the 38px titlebar.
    let chip0 = split_left + 9.0 + 56.0;
    hover_capture(window, cx, &dir, "right-tabs-hover-inactive", (chip0, 21.0)).await;
    hover_capture(window, cx, &dir, "right-tabs-hover-active", (chip0 + 116.0, 21.0)).await;
    hover_capture(window, cx, &dir, "right-tabs-hover-plus", (split_left + 9.0 + 232.0 + 12.0, 21.0)).await;
    hover(window, cx, (500.0, 600.0));
    pause(cx, 400).await;
    drag_seam(window, cx, split_left, target_left).await;
    capture(window, cx, &dir, "right-tabs-660");
    Ok(notes)
}

fn write_checkout(root: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::write(root.join("src/files.rs"), FILES_RS)?;
    std::fs::write(root.join("src/main.rs"), MAIN_RS)?;
    std::fs::write(root.join("src/shell.rs"), SHELL_RS)?;
    std::fs::write(root.join("Cargo.toml"), CARGO_TOML)?;
    std::fs::write(root.join("README.md"), README)?;
    if !root.join(".git").exists() {
        let status = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(root)
            .status()?;
        anyhow::ensure!(status.success(), "git init failed");
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).context("output directory")?);
    std::fs::create_dir_all(&output)?;
    // SAFETY: no other thread exists yet. A plain sh never retitles its
    // tab, so the terminal chip keeps the app's own "Terminal 1".
    unsafe {
        std::env::set_var("SHELL", "/bin/sh");
        std::env::set_var("PS1", "$ ");
    }
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let checkout = temp.path().join("zeron");
    write_checkout(&checkout)?;
    let checkout = checkout.canonicalize()?;
    let checkout_str = checkout.to_string_lossy().into_owned();

    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zeron_engine::HarnessRegistry::new()),
            zeron_proto::HarnessId::Mock,
            None,
        )
    })?;
    let device = core.device_id.clone();
    let space: zeron_proto::Space = serde_json::from_value(json!({
        "id": "prod-space", "deviceId": device, "path": checkout_str, "name": "zeron",
        "gitDetected": true, "createdAt": "2026-10-30T00:00:00Z"
    }))?;
    let chat: zeron_proto::Chat = serde_json::from_value(json!({
        "id": "prod-chat", "deviceId": device, "title": "Right panel parity",
        "archived": false, "cwd": checkout_str, "spaceId": "prod-space",
        "createdAt": "2026-10-30T00:00:00Z",
        "config": {"harness": "claude-code", "model": "claude-sonnet-4-6", "reasoning": null, "sandbox": "workspace-write"}
    }))?;
    core.workspace.import_space_row(&space)?;
    core.workspace.import_chat_row(&chat)?;

    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    let ipc_port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, &data, core.rpc_service()))?;
    let boot = EngineBootConfig {
        data_dir: data.clone(),
        ipc_port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: zeron_proto::HarnessId::Mock,
    };
    let handle = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;

    let light = std::env::var_os("ZERON_PALETTE_LIGHT").is_some();
    let glass = std::env::var_os("ZERON_RIGHT_GLASS").is_some();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            settings.sidebar_width = SIDEBAR;
            settings.right_pane_width = RIGHT_PANE;
            settings.github_star_banner_dismissed = true;
            if !glass {
                settings.surface = zeron_theme::SurfacePreference::Opaque;
            }
            settings.save(&data).unwrap();
            settings::init(settings.clone(), data.clone(), cx);
            let fonts = typography::register_fonts(cx);
            typography::init(
                settings.ui_font_family.clone(),
                settings.ui_font_size,
                settings.terminal_font_family.clone(),
                settings.terminal_font_size,
                settings.code_font_family.clone(),
                settings.code_font_size,
                fonts,
                cx,
            );
            theme_library::init(data.clone(), cx);
            let mode = if light {
                appearance::AppearanceMode::Light
            } else {
                appearance::AppearanceMode::Dark
            };
            appearance::init(mode, settings.theme_selection, settings.accent, settings.surface, cx);
            history::init(
                settings.git_history_columns,
                settings.git_history_column_widths,
                settings.git_history_column_order,
                settings.git_history_author_display,
                cx,
            );
            composer::init(cx, settings.composer_send_behavior);
            terminal::panel::init(cx);
            app_menus::init(cx);
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.fixture_attachment_engine(handle);
                s.connection = zeron_proto::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                s.local_device_id = Some(device.clone());
                s.devices = serde_json::from_value(json!([
                    {"id": device, "name": "Studio Mac", "platform": "macos", "lastSeenAt": null}
                ]))
                .unwrap();
                s.spaces = vec![space.clone()];
                s.chats = vec![chat.clone()];
                s.selected_space = Some("prod-space".into());
                s.selected_chat = Some("prod-chat".into());
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(40.), px(40.)),
                            size(px(WINDOW.0), px(WINDOW.1)),
                        ))),
                        titlebar: Some(gpui::TitlebarOptions {
                            title: None,
                            appears_transparent: true,
                            traffic_light_position: Some(gpui::point(px(14.), px(14.))),
                        }),
                        app_owns_titlebar_drag: true,
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            appearance::reapply_window_background(cx);
            state.update(cx, |_, cx| cx.notify());
            cx.activate(true);
            let window: gpui::AnyWindowHandle = window.into();
            let entries = json!([
                {"id": "u0", "role": "user", "createdAt": 1_788_900_000_000_i64, "deviceId": device,
                 "parts": [{"id": "t", "kind": "text", "text": "Open the files module and a terminal beside the chat."}]},
                {"id": "a0", "role": "assistant", "createdAt": 1_788_900_001_000_i64, "deviceId": device, "status": "complete",
                 "parts": [{"id": "t", "kind": "text", "text": "Opened `src/files.rs` in the right panel, with a terminal tab next to it."}]}
            ]);
            cx.spawn(async move |cx| {
                pause(cx, 800).await;
                state.update(cx, |s, cx| {
                    s.receive_transcript_frame(
                        zeron_doc::TranscriptFrame::Reset {
                            reset: serde_json::from_value(entries).unwrap(),
                        },
                        cx,
                    )
                    .ok();
                    cx.notify();
                });
                pause(cx, 1200).await;
                let result = film(window, output.clone(), cx).await;
                let text = match result {
                    Ok(notes) => notes,
                    Err(error) => format!("FAILED: {error:#}\n"),
                };
                print!("{text}");
                std::fs::write(output.join("result.txt"), text).ok();
                let _ = window.update(cx, |_, window, _| window.remove_window());
                pause(cx, 100).await;
                // AppKit's terminate exits without returning from `run`.
                std::process::exit(0);
            })
            .detach();
        });
    Ok(())
}
