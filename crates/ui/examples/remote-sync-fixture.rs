//! A chat hosted on another device, rendered by the production shell for
//! review screenshots: opened cold, revisited while its room rejoins, during
//! an outage with and without a draft, a side chat opened cold, and a fork
//! on a slow host.
//!
//! A real engine runs underneath with isolated data, and the shell attaches
//! to it as at launch. The fixture plays the edge and the host over it: the
//! chats and devices, each room's connectivity, each transcript's opening
//! frame, and a fork that takes its time. No agent runs and nothing leaves
//! this machine.
//!
//! ```sh
//! cargo run -p zeron-ui --features appshots-fixture --example remote-sync-fixture -- <dir> <scene>
//! ```
//!
//! Scenes: `cold-open`, `revisit`, `outage`, `outage-draft`, `side-chat`,
//! `fork` (two captures: `fork-waiting`, `fork-failed`).
use futures::{StreamExt, stream};
use gpui::{AppContext, AsyncApp, Bounds, WindowBounds, WindowOptions, px, size};
use serde_json::{Value, json};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use zeron_proto::{ChatConnectivity, ChatSyncState, Connectivity, ConnectivityState};
use zeron_rpc::{RpcError, RpcReply, RpcService, methods};
use zeron_ui::*;

const HOST: &str = "omarchy";
const CHAT: &str = "remote-chat";
const SIDE_CHAT: &str = "remote-side-chat";
/// How long the host takes to answer a fork.
const FORK_HOLD: Duration = Duration::from_secs(6);

#[derive(Clone, Copy, PartialEq)]
enum Scene {
    ColdOpen,
    Revisit,
    Outage,
    OutageDraft,
    SideChat,
    Fork,
}

impl Scene {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "cold-open" => Self::ColdOpen,
            "revisit" => Self::Revisit,
            "outage" => Self::Outage,
            "outage-draft" => Self::OutageDraft,
            "side-chat" => Self::SideChat,
            "fork" => Self::Fork,
            _ => return None,
        })
    }

    /// What the edge reports: the registry, and each open chat's room.
    fn connectivity(self) -> Connectivity {
        let room = |chat: &str, sync_state| {
            let live = sync_state == ChatSyncState::Synced;
            ChatConnectivity {
                chat_id: chat.into(),
                sync_state,
                connected: live,
                delivery_live: live,
                pending_pushes: 0,
            }
        };
        let (state, chats) = match self {
            // The room dial is under way: the host has not answered yet.
            Self::ColdOpen | Self::Revisit => (
                ConnectivityState::Connected,
                vec![room(CHAT, ChatSyncState::Connecting)],
            ),
            // This device lost the edge: nothing delivers.
            Self::Outage | Self::OutageDraft => (
                ConnectivityState::Reconnecting,
                vec![room(CHAT, ChatSyncState::Connecting)],
            ),
            Self::SideChat => (
                ConnectivityState::Connected,
                vec![
                    room(CHAT, ChatSyncState::Synced),
                    room(SIDE_CHAT, ChatSyncState::Connecting),
                ],
            ),
            Self::Fork => (
                ConnectivityState::Connected,
                vec![room(CHAT, ChatSyncState::Synced)],
            ),
        };
        Connectivity {
            state,
            chats,
            ..Default::default()
        }
    }

    /// Each chat's opening transcript frame: this device's copy, empty for a
    /// chat never synced here.
    fn transcripts(self, conversation: Value) -> HashMap<String, Value> {
        let empty = json!({ "reset": [] });
        let cached = json!({ "reset": conversation });
        match self {
            Self::ColdOpen => HashMap::from([(CHAT.into(), empty)]),
            Self::SideChat => HashMap::from([(CHAT.into(), cached), (SIDE_CHAT.into(), empty)]),
            _ => HashMap::from([(CHAT.into(), cached)]),
        }
    }
}

/// The real engine, with the edge and the host scripted over it.
struct Scripted {
    engine: Arc<dyn RpcService>,
    chats: Value,
    devices: Value,
    spaces: Value,
    connectivity: Value,
    transcripts: HashMap<String, Value>,
}

/// A watch that publishes once and stays open, as a quiet engine watch does.
fn standing(value: Value) -> RpcReply {
    RpcReply::Stream(
        stream::once(async move { value })
            .chain(stream::pending())
            .boxed(),
    )
}

#[async_trait::async_trait]
impl RpcService for Scripted {
    async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
        let chat = params.get("chatId").and_then(Value::as_str);
        let remote = chat.is_some_and(|chat| chat == CHAT || chat == SIDE_CHAT);
        match method {
            // The workspace the other fixtures render.
            methods::ENGINE_INFO => match self.engine.handle(method, params).await? {
                RpcReply::Value(mut info) => {
                    info["workspaceScope"] = json!("development");
                    Ok(RpcReply::Value(info))
                }
                reply => Ok(reply),
            },
            methods::AUTH_STATUS => Ok(RpcReply::Stream(stream::pending().boxed())),
            methods::WATCH_CHATS => Ok(standing(self.chats.clone())),
            methods::WATCH_DEVICES => Ok(standing(self.devices.clone())),
            methods::WATCH_SPACES => Ok(standing(self.spaces.clone())),
            methods::WATCH_CONNECTIVITY => Ok(standing(self.connectivity.clone())),
            methods::WATCH_DOC_MESSAGES if remote => {
                Ok(match chat.and_then(|chat| self.transcripts.get(chat)) {
                    Some(frame) => standing(frame.clone()),
                    None => RpcReply::Stream(stream::pending().boxed()),
                })
            }
            methods::WATCH_QUEUE if remote => Ok(standing(json!({ "items": [] }))),
            methods::FOCUS_CHAT if remote => Ok(RpcReply::Value(json!({}))),
            // A slow host: the answer takes its time, then this engine, which
            // has no edge, fails to route it.
            methods::FORK_SIDE_CHAT => {
                tokio::time::sleep(FORK_HOLD).await;
                self.engine.handle(method, params).await
            }
            _ => self.engine.handle(method, params).await,
        }
    }
}

async fn pause(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    directory: &Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, w, cx| {
        w.draw(cx).clear();
        w.render_to_image()?
            .save(directory.join(format!("{name}.png")))?;
        Ok(())
    })?
}

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let mut args = std::env::args().skip(1);
    let output = std::path::PathBuf::from(args.next().expect("output directory"));
    let name = args.next().expect("scene");
    let scene = Scene::parse(&name).ok_or_else(|| anyhow::anyhow!("unknown scene {name}"))?;
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let core = runtime.block_on(async {
        zeron_engine::EngineCore::assemble(
            &temp.path().join("engine"),
            Arc::new(zeron_engine::default_registry()),
            zeron_proto::HarnessId::ClaudeCode,
            None,
        )
    })?;
    let device = core.device_id.clone();
    let now = chrono::Utc::now();
    let at = |minutes: i64| now - chrono::Duration::minutes(minutes);
    let config = json!({"harness": "claude-code", "model": null, "reasoning": null, "sandbox": "workspace-write"});
    let chat = |id: &str, title: &str, parent: Option<&str>, last: i64, preview: &str| {
        json!({"id": id, "deviceId": HOST, "title": title, "archived": false,
            "cwd": "/home/dev/zeron", "branch": "main", "checkoutId": null, "config": config,
            "lastMessagePreview": preview, "lastMessageAt": at(last), "createdAt": at(last + 30),
            "lastSeenAt": at(last), "spaceId": "zeron", "roomGen": 2, "parentChatId": parent})
    };
    let chats = json!([
        chat(
            CHAT,
            "Retry the release upload",
            None,
            12,
            "Added a retry with exponential backoff around the upload."
        ),
        chat(
            SIDE_CHAT,
            "Check the backoff timings",
            Some(CHAT),
            9,
            "The waits add up to 14 s, inside the job's timeout."
        ),
        chat(
            "remote-older",
            "Profile the sidebar scroll",
            None,
            180,
            "Most of the frame goes to text shaping."
        ),
    ]);
    let devices = json!([
        {"id": device, "name": "MacBook Pro", "platform": "macos", "lastSeenAt": now},
        {"id": HOST, "name": HOST, "platform": "linux", "lastSeenAt": now},
    ]);
    let spaces = json!([
        {"id": "zeron", "deviceId": HOST, "path": "/home/dev/zeron", "name": "zeron",
         "gitDetected": true, "createdAt": at(60 * 24)},
    ]);
    let conversation = json!([
        {"id": "u1", "role": "user", "createdAt": at(14).timestamp_millis(), "deviceId": device,
         "parts": [{"id": "t", "kind": "text", "text":
            "Can you add retries to the release upload? It failed twice on a flaky network last night."}]},
        {"id": "a1", "role": "assistant", "createdAt": at(12).timestamp_millis(), "deviceId": HOST,
         "status": "complete", "parts": [{"id": "t", "kind": "text", "text":
            "Added a retry with exponential backoff around the upload in `scripts/release.sh`: three attempts, waiting 2 s, then 4 s, then 8 s. Each failed attempt logs its HTTP status, and the last one exits non-zero so the job fails.\n\nAgainst a local server that drops the first two requests, the upload went through on the third."}]},
    ]);
    let service = Arc::new(Scripted {
        engine: core.rpc_service(),
        chats,
        devices,
        spaces,
        connectivity: serde_json::to_value(scene.connectivity())?,
        transcripts: scene.transcripts(conversation),
    });
    let ipc_port = port();
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, &data, service))?;
    let boot = EngineBootConfig {
        data_dir: data.clone(),
        ipc_port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: zeron_proto::HarnessId::ClaudeCode,
    };
    let failure = Arc::new(std::sync::Mutex::new(None));
    let result = failure.clone();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let settings = settings::UiSettings::default();
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
            let state = cx.new(|_| state::AppState::new());
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(40.), px(40.)),
                            size(px(1280.), px(800.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot.clone(), cx)),
                )
                .unwrap();
            // Attach as at launch: the engine's watches fill the state.
            state::AppState::bootstrap(state.clone(), boot, cx);
            cx.activate(true);
            cx.spawn(async move |cx| {
                let run: anyhow::Result<()> = async {
                    let mut waited = 0;
                    while !state.read_with(cx, |s, _| {
                        matches!(s.connection, zeron_proto::view::ConnectionStatus::Ready)
                            && s.chats_synced
                    }) {
                        anyhow::ensure!(waited < 20_000, "the engine never attached");
                        pause(cx, 100).await;
                        waited += 100;
                    }
                    window.update(cx, |shell, _, cx| shell.fixture_open_chat(CHAT, cx))?;
                    match scene {
                        // Past the composer's four-second hold.
                        Scene::ColdOpen | Scene::Revisit => {
                            pause(cx, 5_000).await;
                            capture(window.into(), cx, &output, &name)?;
                        }
                        Scene::Outage => {
                            pause(cx, 3_000).await;
                            capture(window.into(), cx, &output, &name)?;
                        }
                        Scene::OutageDraft => {
                            let composer = window
                                .update(cx, |shell, _, _| shell.fixture_appshots_composer())?;
                            composer.update(cx, |composer, cx| {
                                composer.fixture_draft("Also retry the checksum upload.", cx)
                            });
                            pause(cx, 3_000).await;
                            capture(window.into(), cx, &output, &name)?;
                        }
                        Scene::SideChat => {
                            pause(cx, 1_500).await;
                            window.update(cx, |shell, _, cx| {
                                shell.fixture_open_side_chat(SIDE_CHAT, cx)
                            })?;
                            pause(cx, 5_000).await;
                            capture(window.into(), cx, &output, &name)?;
                        }
                        Scene::Fork => {
                            pause(cx, 1_500).await;
                            window.update(cx, |shell, _, cx| shell.fixture_fork_chat(cx))?;
                            pause(cx, 2_500).await;
                            capture(window.into(), cx, &output, "fork-waiting")?;
                            pause(cx, FORK_HOLD.as_millis() as u64).await;
                            capture(window.into(), cx, &output, "fork-failed")?;
                        }
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = run {
                    eprintln!("remote sync fixture failed: {error:#}");
                    *result.lock().unwrap() = Some(format!("{error:#}"));
                }
                let _ = window.update(cx, |_, w, _| w.remove_window());
                pause(cx, 100).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    runtime.block_on(core.shutdown());
    if let Some(error) = failure.lock().unwrap().take() {
        anyhow::bail!(error);
    }
    Ok(())
}
