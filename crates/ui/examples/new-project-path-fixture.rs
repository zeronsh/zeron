//! New project → typed path, end to end, filmed. The real Shell drives a
//! real engine (in process, served over IPC like the background engine)
//! that lists a scratch home folder. Every step is a window input event —
//! the keys and clicks a person uses — and every folder on screen is a real
//! directory. Typed text arrives through the focused field's input handler,
//! where the platform delivers typing.
//!
//!     cargo build -p zeron-ui --example new-project-path-fixture \
//!         --features project-path-fixture
//!     target/debug/examples/new-project-path-fixture /tmp/new-project-path
//!
//! Writes PNG stills, `new-project-path.mp4` (needs `ffmpeg` on PATH) and
//! `result.txt`, the checks made against the palette and the engine's
//! registry. The window is shown and activated for the run, about a minute.
//! The scratch home is `/tmp/zeron-demo/ana`, and only its folders are
//! created; the Locations step lists the boot volume alone (see
//! [`BootVolumeOnly`]). The engine has no agent harnesses, so no installed
//! agent CLI is started.
use anyhow::Context as _;
use gpui::{
    AnyWindowHandle, AppContext, AsyncApp, Bounds, Entity, EntityInputHandler, MouseButton, Pixels,
    PlatformInput, Point, SharedString, WindowBounds, WindowOptions, prelude::*, px, size,
};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    rc::Rc,
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};
use zeron_ui::*;

const FPS: u32 = 30;
const HOME: &str = "/tmp/zeron-demo/ana";
const WINDOW: (f32, f32) = (1100.0, 760.0);

/// The scratch home: everyday folders and a few repositories (`.git`).
const TREE: &[&str] = &[
    "Desktop",
    "Documents",
    "Downloads",
    "Music",
    "Pictures",
    "notes",
    "code/opensource/zeron/.git",
    "code/opensource/zeron/apps",
    "code/opensource/zeron/crates",
    "code/opensource/zeron/docs",
    "code/opensource/gpui/.git",
    "code/opensource/helix/.git",
    "code/work/api/.git",
    "code/work/dashboard/.git",
    "code/work/dashboard/src",
];

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).context("output directory")?);
    std::fs::create_dir_all(&output)?;
    for folder in TREE {
        std::fs::create_dir_all(Path::new(HOME).join(folder))?;
    }
    // SAFETY: no other thread exists yet. The engine lists this home under a
    // neutral device name.
    unsafe {
        std::env::set_var("HOME", HOME);
        std::env::set_var("ZERON_DEVICE_NAME", "Studio Mac");
    }
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zeron_engine::HarnessRegistry::new()),
            zeron_proto::HarnessId::Mock,
            None,
        )
    })?;
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    let ipc_port = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    // Publishes the IPC bearer where the attaching viewport looks for it.
    let service = Arc::new(BootVolumeOnly(core.rpc_service()));
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, &data, service))?;
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
    let device = core.device_id.clone();
    let registry = core.workspace.clone();
    let outcome: Arc<Mutex<Option<anyhow::Result<Vec<String>>>>> = Arc::default();
    let report = outcome.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut settings = settings::UiSettings::default();
            settings.surface = zeron_theme::SurfacePreference::Opaque;
            settings.github_star_banner_dismissed = true;
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
            appearance::init(
                appearance::AppearanceMode::Dark,
                settings.theme_selection,
                settings.accent,
                settings.surface,
                cx,
            );
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
            // Every folder and project request goes to the engine; the
            // registry watches stay off, so the run never touches agent
            // accounts (and their Keychain items).
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.fixture_attachment_engine(handle);
                s.connection = zeron_proto::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Development);
                s.local_device_id = Some(device.clone());
                s.devices = serde_json::from_value(serde_json::json!([
                    {"id": device, "name": "Studio Mac", "platform": "macos", "lastSeenAt": null}
                ]))
                .unwrap();
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s
            });
            let shell = cx.new(|cx| shell::Shell::new(state.clone(), boot, cx));
            let stage = cx.new(|_| Stage::new(shell.clone()));
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(40.0), px(60.0)),
                            size(px(WINDOW.0), px(WINDOW.1)),
                        ))),
                        titlebar: Some(gpui::TitlebarOptions {
                            title: None,
                            appears_transparent: true,
                            traffic_light_position: Some(gpui::point(px(14.0), px(14.0))),
                        }),
                        app_owns_titlebar_drag: true,
                        ..Default::default()
                    },
                    |_, _| stage.clone(),
                )
                .unwrap();
            cx.activate(true);
            let run = Run {
                window: window.into(),
                stage,
                shell,
                state,
                output: output.clone(),
                checks: Default::default(),
            };
            cx.spawn(async move |cx| {
                let result = run.film(cx, &registry, &device).await;
                let _ = run.window.update(cx, |_, window, _| window.remove_window());
                pause(cx, 100).await;
                // AppKit's terminate exits without returning from `run`:
                // report here, and fail before taking that path.
                match &result {
                    Ok(checks) => checks.iter().for_each(|check| println!("✓ {check}")),
                    Err(error) => {
                        eprintln!("New project path fixture failed: {error:#}");
                        std::process::exit(1);
                    }
                }
                *report.lock().unwrap() = Some(result);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    outcome
        .lock()
        .unwrap()
        .take()
        .context("the run never finished")??;
    Ok(())
}

/// The engine as the window sees it, except that the Locations step lists
/// the boot volume alone: the film and stills don't depend on, or show,
/// what the capturing machine has mounted.
struct BootVolumeOnly(Arc<dyn zeron_rpc::RpcService>);

#[async_trait::async_trait]
impl zeron_rpc::RpcService for BootVolumeOnly {
    async fn handle(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
        Ok(match self.0.handle(method, params).await? {
            zeron_rpc::RpcReply::Value(mut listing)
                if method == zeron_rpc::methods::LIST_DRIVES =>
            {
                if let Some(drives) = listing["drives"].as_array_mut() {
                    drives.retain(|drive| drive["path"] == "/");
                }
                zeron_rpc::RpcReply::Value(listing)
            }
            reply => reply,
        })
    }
}

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

/// The Shell, with what a viewer of the film needs and the screen doesn't
/// show: the keys pressed, the text typed, the pointer and the scene.
struct Stage {
    shell: Entity<shell::Shell>,
    hud: bool,
    caption: Option<SharedString>,
    keys: Option<(SharedString, Instant)>,
    pointer: Option<Point<Pixels>>,
    clicked: Option<Instant>,
}

impl Stage {
    fn new(shell: Entity<shell::Shell>) -> Self {
        Self {
            shell,
            hud: true,
            caption: None,
            keys: None,
            pointer: None,
            clicked: None,
        }
    }
}

impl Render for Stage {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let viewport = window.viewport_size();
        let keys = self.keys.as_ref().and_then(|(keys, at)| {
            let age = at.elapsed().as_secs_f32();
            (age < 1.6).then(|| (keys.clone(), (1.6 - age).min(0.3) / 0.3))
        });
        let ring = self
            .clicked
            .map(|at| at.elapsed().as_secs_f32() / 0.45)
            .filter(|progress| *progress < 1.0);
        let pill = |text: SharedString, size: f32| {
            gpui::div()
                .px(px(size * 0.8))
                .py(px(size * 0.42))
                .rounded(px(size * 0.65))
                .bg(gpui::hsla(0.0, 0.0, 0.06, 0.88))
                .border_1()
                .border_color(gpui::hsla(0.0, 0.0, 1.0, 0.14))
                .text_size(px(size))
                .text_color(gpui::hsla(0.0, 0.0, 1.0, 0.94))
                .font_family(".SystemUIFont")
                .child(text)
        };
        let overlay = gpui::div()
            .w(viewport.width)
            .h(viewport.height)
            .relative()
            .when_some(self.caption.clone(), |el, caption| {
                el.child(
                    gpui::div()
                        .absolute()
                        .top(px(14.0))
                        .left(px(0.0))
                        .w(viewport.width)
                        .flex()
                        .justify_center()
                        .child(pill(caption, 13.0)),
                )
            })
            .when_some(keys, |el, (keys, opacity)| {
                el.child(
                    gpui::div()
                        .absolute()
                        .bottom(px(26.0))
                        .left(px(0.0))
                        .w(viewport.width)
                        .flex()
                        .justify_center()
                        .child(pill(keys, 19.0).opacity(opacity)),
                )
            })
            .when_some(self.pointer, |el, at| {
                let dot = 16.0;
                el.when_some(ring, |el, progress| {
                    let size = dot + 26.0 * progress;
                    el.child(
                        gpui::div()
                            .absolute()
                            .left(at.x - px(size / 2.0))
                            .top(at.y - px(size / 2.0))
                            .size(px(size))
                            .rounded_full()
                            .border_2()
                            .border_color(gpui::hsla(0.58, 0.9, 0.65, 1.0 - progress)),
                    )
                })
                .child(
                    gpui::div()
                        .absolute()
                        .left(at.x - px(dot / 2.0))
                        .top(at.y - px(dot / 2.0))
                        .size(px(dot))
                        .rounded_full()
                        .bg(gpui::hsla(0.0, 0.0, 1.0, 0.85))
                        .border_2()
                        .border_color(gpui::hsla(0.0, 0.0, 0.0, 0.55)),
                )
            });
        gpui::div()
            .size_full()
            .child(self.shell.clone())
            .when(self.hud, |el| {
                el.child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.0), px(0.0)))
                            .child(overlay),
                    )
                    .priority(10),
                )
            })
    }
}

/// Streams captures to ffmpeg at a constant rate: each film frame is the
/// newest capture at or before its time, so the film keeps the run's real
/// timing when captures come unevenly.
struct Film {
    frames: Option<mpsc::SyncSender<(Duration, Vec<u8>)>>,
    writer: Option<std::thread::JoinHandle<anyhow::Result<()>>>,
    started: Instant,
    size: (u32, u32),
}

impl Film {
    fn start(path: &Path, size: (u32, u32)) -> anyhow::Result<Self> {
        let mut ffmpeg = Command::new("ffmpeg")
            .args([
                "-y",
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
            ])
            .args(["-s", &format!("{}x{}", size.0, size.1)])
            .args(["-r", &FPS.to_string(), "-i", "-"])
            .args(["-vf", "scale=1650:-2:flags=lanczos", "-c:v", "libx264"])
            .args(["-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"])
            .args(["-movflags", "+faststart"])
            .arg(path)
            .stdin(Stdio::piped())
            .spawn()
            .context("ffmpeg must be on PATH to film the run")?;
        let mut stdin = ffmpeg.stdin.take().context("ffmpeg stdin")?;
        let (frames, captured) = mpsc::sync_channel::<(Duration, Vec<u8>)>(3);
        let writer = std::thread::spawn(move || -> anyhow::Result<()> {
            let due = |frame: u64| Duration::from_secs_f64(frame as f64 / f64::from(FPS));
            let mut last: Option<Vec<u8>> = None;
            let mut written = 0u64;
            for (at, frame) in captured {
                if let Some(last) = &last {
                    while due(written) < at {
                        stdin.write_all(last)?;
                        written += 1;
                    }
                }
                last = Some(frame);
            }
            // Hold the closing frame for a second.
            for _ in 0..FPS {
                if let Some(last) = &last {
                    stdin.write_all(last)?;
                }
            }
            drop(stdin);
            anyhow::ensure!(ffmpeg.wait()?.success(), "ffmpeg failed to encode the film");
            Ok(())
        });
        Ok(Self {
            frames: Some(frames),
            writer: Some(writer),
            started: Instant::now(),
            size,
        })
    }

    /// A capture behind the encoder is dropped, never waited for.
    fn offer(&self, image: image::RgbaImage) {
        if image.dimensions() == self.size
            && let Some(frames) = &self.frames
        {
            let _ = frames.try_send((self.started.elapsed(), image.into_raw()));
        }
    }

    fn finish(mut self) -> anyhow::Result<()> {
        self.frames = None;
        self.writer
            .take()
            .context("film writer")?
            .join()
            .map_err(|_| anyhow::anyhow!("film writer panicked"))?
    }
}

struct Run {
    window: AnyWindowHandle,
    stage: Entity<Stage>,
    shell: Entity<shell::Shell>,
    state: Entity<state::AppState>,
    output: PathBuf,
    checks: std::cell::RefCell<Vec<String>>,
}

impl Run {
    async fn film(
        &self,
        cx: &mut AsyncApp,
        registry: &zeron_engine::WorkspaceHost,
        device: &str,
    ) -> anyhow::Result<Vec<String>> {
        pause(cx, 1500).await;
        let size = self.capture(cx)?.dimensions();
        let film = Rc::new(Film::start(
            &self.output.join("new-project-path.mp4"),
            size,
        )?);
        let filming = Rc::new(std::cell::Cell::new(true));
        let camera = cx.spawn({
            let (film, filming, window, stage) = (
                film.clone(),
                filming.clone(),
                self.window,
                self.stage.clone(),
            );
            async move |cx| {
                while filming.get() {
                    // Keep the overlay's fades moving between input events.
                    stage.update(cx, |_, cx| cx.notify());
                    if let Ok(Ok(image)) = window.update(cx, |_, window, cx| {
                        window.draw(cx).clear();
                        window.render_to_image()
                    }) {
                        film.offer(image);
                    }
                    pause(cx, u64::from(1000 / FPS)).await;
                }
            }
        });
        let scenes = self.scenes(cx, registry, device).await;
        filming.set(false);
        camera.await;
        Rc::try_unwrap(film)
            .map_err(|_| anyhow::anyhow!("film still shared"))?
            .finish()?;
        scenes?;
        let checks = self.checks.take();
        std::fs::write(
            self.output.join("result.txt"),
            format!(
                "New project typed path, run against a real in-process engine listing {HOME}.\n\
                 Input: window key/mouse events; text through the focused field's input handler.\n\n{}\n",
                checks
                    .iter()
                    .map(|check| format!("✓ {check}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        )?;
        Ok(checks)
    }

    async fn scenes(
        &self,
        cx: &mut AsyncApp,
        registry: &zeron_engine::WorkspaceHost,
        device: &str,
    ) -> anyhow::Result<()> {
        let home = HOME;
        // 1 · A path you know, typed with completion and added in one go.
        self.caption(cx, "Add a project by typing the path you know");
        pause(cx, 900).await;
        self.press(cx, "cmd-shift-n", "⇧⌘N")?;
        pause(cx, 900).await;
        self.expect(cx, "⇧⌘N opens New project on its devices", "step=Devices")?;
        self.press(cx, "cmd-l", "⌘L")?;
        pause(cx, 900).await;
        self.expect(
            cx,
            "⌘L on the device list types a path on the highlighted device",
            "step=Locations open=None field=Some(\"~/\")",
        )?;
        self.still(cx, "01-path-field-on-device")?;
        self.type_text(cx, "~/co").await?;
        pause(cx, 600).await;
        self.still(cx, "02-completion-ghost")?;
        self.press(cx, "tab", "⇥")?;
        pause(cx, 450).await;
        self.type_text(cx, "op").await?;
        pause(cx, 350).await;
        self.press(cx, "tab", "⇥")?;
        pause(cx, 450).await;
        self.type_text(cx, "ze").await?;
        pause(cx, 350).await;
        self.press(cx, "tab", "⇥")?;
        pause(cx, 700).await;
        self.expect(
            cx,
            "⇥ completes each folder from the device's real listing",
            "field=Some(\"~/code/opensource/zeron/\")",
        )?;
        self.still(cx, "03-completed-path")?;
        self.press(cx, "cmd-enter", "⌘↵")?;
        pause(cx, 1600).await;
        let zeron = format!("{home}/code/opensource/zeron");
        self.expect_closed(cx, "⌘↵ opens the typed folder and adds it as a project")?;
        self.expect_project(registry, device, &zeron)?;
        self.expect_selected(cx, &zeron)?;
        self.still(cx, "04-project-added")?;

        // 2 · Browsing, then editing the open folder's path.
        self.caption(cx, "Browse, then edit the open folder's path");
        pause(cx, 900).await;
        self.press(cx, "cmd-shift-n", "⇧⌘N")?;
        pause(cx, 700).await;
        self.press(cx, "enter", "↵")?;
        pause(cx, 700).await;
        self.press(cx, "enter", "↵")?;
        pause(cx, 900).await;
        self.expect(
            cx,
            "Home lists the device's real home",
            &format!("open=Some({home:?})"),
        )?;
        self.press(cx, "enter", "↵")?;
        pause(cx, 800).await;
        self.press(cx, "down", "↓")?;
        pause(cx, 400).await;
        self.press(cx, "enter", "↵")?;
        pause(cx, 900).await;
        let work = format!("{home}/code/work");
        self.expect(
            cx,
            "the folder rows still browse as before",
            &format!("open=Some({work:?})"),
        )?;
        self.still(cx, "05-crumbs-with-edit")?;
        let viewport = size(px(WINDOW.0), px(WINDOW.1));
        self.click(cx, pen_center(viewport)).await?;
        pause(cx, 900).await;
        self.expect(
            cx,
            "the pen turns the crumbs into the open folder's path, from ~",
            "field=Some(\"~/code/work/\")",
        )?;
        self.still(cx, "06-path-field-selected")?;
        self.press(cx, "right", "→")?;
        pause(cx, 400).await;
        self.type_text(cx, "da").await?;
        pause(cx, 450).await;
        self.press(cx, "tab", "⇥")?;
        pause(cx, 600).await;
        self.press(cx, "enter", "↵")?;
        pause(cx, 1100).await;
        let dashboard = format!("{home}/code/work/dashboard");
        self.expect(
            cx,
            "↵ opens the edited path; the crumbs come back",
            &format!("open=Some({dashboard:?}) field=None error=None"),
        )?;
        self.still(cx, "07-edited-path-opened")?;
        self.press(cx, "cmd-enter", "⌘↵")?;
        pause(cx, 1500).await;
        self.expect_closed(cx, "⌘↵ adds the open folder, as before")?;
        self.expect_project(registry, device, &dashboard)?;

        // 3 · A typo keeps the field and the folder that was open.
        self.caption(cx, "A path that isn't there stays editable");
        pause(cx, 900).await;
        self.press(cx, "cmd-shift-n", "⇧⌘N")?;
        pause(cx, 600).await;
        self.press(cx, "cmd-l", "⌘L")?;
        pause(cx, 600).await;
        self.type_text(cx, "~/code/opensorce/helix").await?;
        pause(cx, 400).await;
        self.press(cx, "enter", "↵")?;
        pause(cx, 1200).await;
        self.expect(
            cx,
            "a missing folder says so and keeps the typed path",
            "field=Some(\"~/code/opensorce/helix\") error=Some(\"No folder at ~/code/opensorce/helix\")",
        )?;
        self.still(cx, "08-missing-folder")?;
        self.press(cx, "escape", "esc")?;
        pause(cx, 700).await;
        self.expect(
            cx,
            "Esc gives the trail back and keeps New project open",
            "step=Locations open=None field=None error=None",
        )?;
        self.press(cx, "cmd-l", "⌘L")?;
        pause(cx, 400).await;
        self.type_text(cx, "~/code/opensource/helix").await?;
        pause(cx, 400).await;
        self.press(cx, "cmd-enter", "⌘↵")?;
        pause(cx, 1600).await;
        let helix = format!("{home}/code/opensource/helix");
        self.expect_closed(cx, "the corrected path opens and is added")?;
        self.expect_project(registry, device, &helix)?;
        self.caption_off(cx);
        pause(cx, 1200).await;
        Ok(())
    }

    fn capture(&self, cx: &mut AsyncApp) -> anyhow::Result<image::RgbaImage> {
        self.window.update(cx, |_, window, cx| {
            window.draw(cx).clear();
            window.render_to_image()
        })?
    }

    /// A still without the film's overlay: the whole window, and the
    /// palette area cropped.
    fn still(&self, cx: &mut AsyncApp, name: &str) -> anyhow::Result<()> {
        self.stage.update(cx, |stage, cx| {
            stage.hud = false;
            cx.notify();
        });
        let image = self.capture(cx);
        self.stage.update(cx, |stage, cx| {
            stage.hud = true;
            cx.notify();
        });
        let image = image?;
        image.save(self.output.join(format!("{name}.png")))?;
        // The card is 560pt wide and centered; keep a margin around it.
        let scale = image.width() as f32 / WINDOW.0;
        let (x, w) = (
            ((WINDOW.0 - 600.0) / 2.0 * scale) as u32,
            (600.0 * scale) as u32,
        );
        let (y, h) = ((120.0 * scale) as u32, (560.0 * scale) as u32);
        let h = h.min(image.height() - y);
        image::imageops::crop_imm(&image, x, y, w, h)
            .to_image()
            .save(self.output.join(format!("{name}-palette.png")))?;
        Ok(())
    }

    fn caption(&self, cx: &mut AsyncApp, text: &'static str) {
        self.stage.update(cx, |stage, cx| {
            stage.caption = Some(text.into());
            cx.notify();
        });
    }

    fn caption_off(&self, cx: &mut AsyncApp) {
        self.stage.update(cx, |stage, cx| {
            stage.caption = None;
            cx.notify();
        });
    }

    fn show_keys(&self, cx: &mut AsyncApp, keys: impl Into<SharedString>) {
        let keys = keys.into();
        self.stage.update(cx, |stage, cx| {
            stage.keys = Some((keys, Instant::now()));
            cx.notify();
        });
    }

    fn press(&self, cx: &mut AsyncApp, keystroke: &str, shown: &'static str) -> anyhow::Result<()> {
        self.show_keys(cx, shown);
        let keystroke = gpui::Keystroke::parse(keystroke)?;
        self.window.update(cx, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::KeyDown(gpui::KeyDownEvent {
                    keystroke: keystroke.clone(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.dispatch_event(PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }), cx);
        })?;
        Ok(())
    }

    /// Types `text` a character at a time into the focused palette field.
    async fn type_text(&self, cx: &mut AsyncApp, text: &str) -> anyhow::Result<()> {
        let mut typed = String::new();
        for ch in text.chars() {
            typed.push(ch);
            self.show_keys(cx, typed.clone());
            let shell = self.shell.clone();
            self.window
                .update(cx, |_, window, cx| -> anyhow::Result<()> {
                    let input = shell
                        .read(cx)
                        .fixture_project_input(window, cx)
                        .context("no New project field has the keyboard")?;
                    input.update(cx, |input, cx| {
                        input.replace_text_in_range(None, &ch.to_string(), window, cx)
                    });
                    Ok(())
                })??;
            pause(cx, 75).await;
        }
        Ok(())
    }

    /// Moves the pointer there over a few frames, then clicks.
    async fn click(&self, cx: &mut AsyncApp, at: Point<Pixels>) -> anyhow::Result<()> {
        let from = self
            .stage
            .read_with(cx, |stage, _| stage.pointer)
            .unwrap_or(gpui::point(px(WINDOW.0 * 0.62), px(WINDOW.1 * 0.8)));
        for step in 1..=16 {
            let t = step as f32 / 16.0;
            let ease = 1.0 - (1.0 - t).powi(3);
            let position = gpui::point(
                from.x + (at.x - from.x) * ease,
                from.y + (at.y - from.y) * ease,
            );
            self.stage.update(cx, |stage, cx| {
                stage.pointer = Some(position);
                cx.notify();
            });
            self.window.update(cx, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(gpui::MouseMoveEvent {
                        position,
                        ..Default::default()
                    }),
                    cx,
                );
            })?;
            pause(cx, 28).await;
        }
        pause(cx, 250).await;
        self.stage.update(cx, |stage, cx| {
            stage.clicked = Some(Instant::now());
            cx.notify();
        });
        self.window.update(cx, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::MouseDown(gpui::MouseDownEvent {
                    button: MouseButton::Left,
                    position: at,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
            window.dispatch_event(
                PlatformInput::MouseUp(gpui::MouseUpEvent {
                    button: MouseButton::Left,
                    position: at,
                    click_count: 1,
                    ..Default::default()
                }),
                cx,
            );
        })?;
        Ok(())
    }

    fn summary(&self, cx: &mut AsyncApp) -> Option<String> {
        self.shell
            .read_with(cx, |shell, cx| shell.fixture_project_summary(cx))
    }

    fn expect(&self, cx: &mut AsyncApp, check: &str, needle: &str) -> anyhow::Result<()> {
        let summary = self.summary(cx).unwrap_or_else(|| "closed".into());
        anyhow::ensure!(
            summary.contains(needle),
            "{check}: expected `{needle}` in `{summary}`"
        );
        self.checks.borrow_mut().push(check.to_string());
        Ok(())
    }

    fn expect_closed(&self, cx: &mut AsyncApp, check: &str) -> anyhow::Result<()> {
        if let Some(summary) = self.summary(cx) {
            anyhow::bail!("{check}: the palette is still open: {summary}");
        }
        self.checks.borrow_mut().push(check.to_string());
        Ok(())
    }

    /// The engine's own registry holds the project: the device and folder.
    fn expect_project(
        &self,
        registry: &zeron_engine::WorkspaceHost,
        device: &str,
        path: &str,
    ) -> anyhow::Result<()> {
        let spaces = registry.read_spaces()?;
        anyhow::ensure!(
            spaces
                .iter()
                .any(|space| space.device_id == device && space.path == path),
            "the engine has no project at {path}: {:?}",
            spaces.iter().map(|space| &space.path).collect::<Vec<_>>()
        );
        self.checks
            .borrow_mut()
            .push(format!("the engine's registry holds the project {path}"));
        Ok(())
    }

    /// The app landed on the new project.
    fn expect_selected(&self, cx: &mut AsyncApp, path: &str) -> anyhow::Result<()> {
        let selected = self.state.read_with(cx, |state, _| {
            state.selected_space_row().map(|space| space.path.clone())
        });
        anyhow::ensure!(
            selected.as_deref() == Some(path),
            "the app should land on {path}, not {selected:?}"
        );
        self.checks
            .borrow_mut()
            .push(format!("the app lands on the new project {path}"));
        Ok(())
    }
}

/// The crumb row's pen: the palette card (560pt, centered, `palette_top`),
/// its 44pt search header, then the 36pt crumb row whose last 24pt control
/// sits 12pt from the card's edge.
fn pen_center(viewport: gpui::Size<Pixels>) -> Point<Pixels> {
    let (width, height) = (f32::from(viewport.width), f32::from(viewport.height));
    let results = (height - 180.0).clamp(100.0, 360.0);
    let top = ((height - (results + 120.0)) / 2.0).max(theme::Theme::TITLEBAR_HEIGHT + 16.0);
    let card_right = (width + 560.0_f32.min(width - 32.0)) / 2.0;
    gpui::point(
        px(card_right - 1.0 - 12.0 - 12.0),
        px(top + 1.0 + 44.0 + 18.0),
    )
}
