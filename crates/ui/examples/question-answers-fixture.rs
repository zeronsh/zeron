//! What each of an agent's questions received, and the open one closed
//! unanswered, end to end. Every question-chip state is seeded into a real
//! engine chat and drawn by the production `Shell`. Escape on the untouched
//! panel then goes through the window's real key dispatch (the panel's ✕ runs
//! the same `wizard_dismiss`), and the engine's record is checked. Synthetic
//! transcript, temporary engine and settings; the chat runs on the mock
//! harness, so no agent ever starts.
//!
//! ```bash
//! cargo run -p zeron-ui --features appshots-fixture \
//!   --example question-answers-fixture -- <output-dir>
//! ```
//!
//! Writes `panel.png` (the open question), `closed.png` (after Escape),
//! `chips.png` (every chip state) and `result.txt` (the command ledger and each
//! question part as the doc holds it), and prints PASS once the close reached
//! the doc as a decline.
use gpui::{
    AppContext, Bounds, KeyDownEvent, Keystroke, PlatformInput, WindowBounds, WindowOptions, px,
    size,
};
use serde_json::json;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionCommandStatus,
    SessionMessageEntry,
};
use zeron_proto::{UserInputAnswer, UserInputQuestion};
use zeron_ui::*;

const CHAT: &str = "questions";
/// The question left open for the panel.
const OPEN: &str = "r4";

fn port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut gpui::AsyncApp,
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

/// A real key press through the window's dispatch.
fn press(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, keystroke: &str) {
    let keystroke = Keystroke::parse(keystroke).expect("keystroke");
    let _ = window.update(cx, |_, w, cx| {
        w.dispatch_event(
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
    });
}

fn question(
    id: &str,
    header: &str,
    text: &str,
    options: &[&str],
    multi: bool,
) -> UserInputQuestion {
    UserInputQuestion {
        id: id.into(),
        header: header.into(),
        question: text.into(),
        options: options.iter().map(|o| (*o).to_owned()).collect(),
        multi_select: multi,
        prefill: None,
        multiline: false,
    }
}

fn answer(id: &str, labels: &[&str]) -> UserInputAnswer {
    UserInputAnswer {
        question_id: id.into(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
    }
}

fn input(
    id: &str,
    questions: Vec<UserInputQuestion>,
    resolved: bool,
    answers: Option<Vec<UserInputAnswer>>,
) -> MessagePart {
    MessagePart::Input {
        id: id.into(),
        request_id: id.into(),
        questions,
        resolved,
        answers,
    }
}

fn text(id: &str, text: &str) -> MessagePart {
    MessagePart::Text {
        id: id.into(),
        text: text.into(),
    }
}

/// One entry per chip state: unrecorded (resolved before answers were
/// recorded), one answer, several (picked, typed, skipped), closed unanswered,
/// and the open question.
fn transcript(device: &str) -> Vec<SessionMessageEntry> {
    let entry =
        |id: &str, role: MessageRole, at: i64, parts: Vec<MessagePart>| SessionMessageEntry {
            id: id.into(),
            role,
            parts,
            created_at: 1_788_900_000_000 + at * 60_000,
            device_id: device.into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        };
    vec![
        entry(
            "u1",
            MessageRole::User,
            0,
            vec![text(
                "u1t",
                "Audit the sync engine and propose a migration plan.",
            )],
        ),
        entry(
            "a0",
            MessageRole::Assistant,
            1,
            vec![
                text(
                    "a0t",
                    "An older question, saved before answers were recorded:",
                ),
                input(
                    "r0",
                    vec![question(
                        "approach",
                        "Approach",
                        "Incremental or rewrite?",
                        &["Incremental", "Rewrite"],
                        false,
                    )],
                    true,
                    None,
                ),
            ],
        ),
        entry(
            "a1",
            MessageRole::Assistant,
            2,
            vec![
                text("a1t", "One question before I start."),
                input(
                    "r1",
                    vec![question(
                        "depth",
                        "Research",
                        "How deep should the research go?",
                        &["Quick scan", "Deep dive"],
                        false,
                    )],
                    true,
                    Some(vec![answer("depth", &["Deep dive"])]),
                ),
                text("a1u", "Going deep: doc host, segment writer, journal."),
            ],
        ),
        entry(
            "a2",
            MessageRole::Assistant,
            3,
            vec![
                text("a2t", "Three choices before I write the plan:"),
                input(
                    "r2",
                    vec![
                        question(
                            "scope",
                            "Scope",
                            "Which surfaces should the plan cover?",
                            &["Desktop", "iOS", "Edge"],
                            true,
                        ),
                        question(
                            "format",
                            "Format",
                            "How should I deliver it?",
                            &["Table", "Prose"],
                            false,
                        ),
                        question(
                            "deadline",
                            "Deadline",
                            "Any deadline?",
                            &["This week", "Next sprint"],
                            false,
                        ),
                    ],
                    true,
                    Some(vec![
                        answer("scope", &["Desktop", "iOS"]),
                        answer("format", &["A short table, then a diagram"]),
                        answer("deadline", &[]),
                    ]),
                ),
                text("a2u", "Plan drafted for desktop and iOS."),
            ],
        ),
        entry(
            "a3",
            MessageRole::Assistant,
            4,
            vec![
                text("a3t", "Open the PR on a new branch?"),
                input(
                    "r3",
                    vec![question(
                        "branch",
                        "Branch",
                        "Create a new branch for the PR?",
                        &["Yes", "No"],
                        false,
                    )],
                    true,
                    Some(vec![]),
                ),
                text("a3u", "No answer, so I left the branch as it is."),
            ],
        ),
        entry(
            "a4",
            MessageRole::Assistant,
            5,
            vec![
                text("a4t", "Last one:"),
                input(
                    OPEN,
                    vec![question(
                        "ship",
                        "Ship",
                        "Ship the migration behind a flag?",
                        &["Behind a flag", "Straight to main"],
                        false,
                    )],
                    false,
                    None,
                ),
            ],
        ),
    ]
}

async fn drive(
    window: gpui::AnyWindowHandle,
    out: &Path,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    // The engine attach, the transcript watch and the panel settle.
    pause(cx, 3000).await;
    capture(window, cx, out, "panel")?;
    press(window, cx, "escape");
    pause(cx, 3000).await;
    capture(window, cx, out, "closed")?;
    // Back to the top: the older chips scrolled out above.
    window.update(cx, |_, w, cx| {
        w.dispatch_event(
            PlatformInput::ScrollWheel(gpui::ScrollWheelEvent {
                position: gpui::point(px(700.), px(300.)),
                delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(2000.))),
                modifiers: gpui::Modifiers::default(),
                touch_phase: gpui::TouchPhase::Moved,
            }),
            cx,
        );
    })?;
    pause(cx, 1200).await;
    capture(window, cx, out, "chips")?;
    Ok(())
}

/// The command ledger and every question part, read straight from the chat's
/// doc, and whether the close landed as a decline: an applied `RespondInput`
/// with no answers, and the open question stamped closed (`Some([])`).
fn engine_record(core: &zeron_engine::EngineCore) -> anyhow::Result<(String, bool)> {
    let chat = core.doc_host.open(CHAT)?;
    let mut log = String::new();
    let mut declined = false;
    for command in chat.doc().read_commands()? {
        log += &format!(
            "command {:?} → {:?} {:?}\n",
            command.payload, command.status, command.resolution
        );
        declined |= matches!(
            &command.payload,
            SessionCommandPayload::RespondInput { request_id, answers }
                if request_id == OPEN && answers.is_empty()
        ) && matches!(command.status, SessionCommandStatus::Applied);
    }
    let mut stamped = false;
    for entry in chat.doc().read_entries()? {
        for part in &entry.parts {
            if let MessagePart::Input {
                request_id,
                resolved,
                answers,
                ..
            } = part
            {
                log += &format!("{request_id}: resolved={resolved} answers={answers:?}\n");
                stamped |= request_id == OPEN
                    && *resolved
                    && answers.as_ref().is_some_and(|answers| answers.is_empty());
            }
        }
    }
    Ok((log, declined && stamped))
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let out = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&out)?;
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
    let config: zeron_proto::ChatConfig = serde_json::from_value(json!({
        "harness": "mock", "model": "fable-5", "reasoning": null, "sandbox": "workspace-write"
    }))?;
    core.workspace.create_chat(
        CHAT,
        None,
        Some(&core.device_id),
        Some(config),
        Some(temp.path().to_string_lossy().into_owned()),
    )?;
    core.workspace.rename_chat(CHAT, "Sync engine migration")?;
    runtime.block_on(async {
        let chat = core.doc_host.open(CHAT)?;
        for entry in transcript(&core.device_id) {
            chat.doc().push_message(&entry)?;
        }
        anyhow::Ok(())
    })?;
    let ipc_port = port();
    let data = temp.path().join("ui");
    std::fs::create_dir(&data)?;
    // Publish the IPC bearer where the attaching viewport looks for it.
    let _ipc = runtime.block_on(zeron_engine::serve_ipc(ipc_port, &data, core.rpc_service()))?;
    let boot = EngineBootConfig {
        data_dir: data.clone(),
        ipc_port,
        edge_url: String::new(),
        edge_token: None,
        org_id: None,
        workos_client_id: None,
        default_harness: zeron_proto::HarnessId::ClaudeCode,
    };
    let handle = runtime.block_on(state::EngineHandle::bootstrap(boot.clone()))?;
    let chats = core.workspace.read_chats()?;
    let device = core.device_id.clone();
    let out_dir = out.clone();
    let core = Arc::new(core);
    let engine = core.clone();
    let tokio_handle = runtime.handle().clone();

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
            let state = cx.new(|_| {
                let mut s = state::AppState::new();
                s.fixture_attachment_engine(handle);
                s.connection = zeron_proto::view::ConnectionStatus::Ready;
                s.workspace_scope = Some(zeron_proto::WorkspaceScope::Development);
                s.local_device_id = Some(device.clone());
                s.devices = vec![
                    serde_json::from_value(json!({"id": device, "name": "This device",
                    "platform": std::env::consts::OS, "lastSeenAt": null}))
                    .unwrap(),
                ];
                s.chats = chats;
                s.auto_selected = true;
                s.chats_synced = true;
                s.spaces_synced = true;
                s.no_project = true;
                s
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(40.), px(40.)),
                            size(px(1200.), px(1000.)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
                )
                .unwrap();
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            // The real selection path: starts the transcript watch on the engine.
            state.update(cx, |s, cx| s.select_chat(Some(CHAT.into()), cx));
            cx.activate(true);
            cx.spawn(async move |cx| {
                let drove = drive(window.into(), &out_dir, cx).await;
                // Read before quitting: on macOS quit ends the process.
                let record = {
                    let _tokio = tokio_handle.enter();
                    engine_record(&engine)
                };
                let passed = match (drove, record) {
                    (Ok(()), Ok((log, declined))) => {
                        print!("{log}");
                        std::fs::write(out_dir.join("result.txt"), &log).ok();
                        declined
                    }
                    (Err(error), _) | (_, Err(error)) => {
                        eprintln!("fixture failed: {error:#}");
                        false
                    }
                };
                if !passed {
                    eprintln!("FAIL: the close did not reach the doc as a decline");
                    std::process::exit(1);
                }
                println!("PASS");
                cx.update(|cx| cx.quit());
            })
            .detach();
        });

    runtime.block_on(core.shutdown());
    Ok(())
}
