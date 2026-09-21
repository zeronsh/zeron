//! Isolated visual and keyboard evidence for native composer interactions.
use async_trait::async_trait;
use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, Focusable, Render, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use zeron_ui::*;

#[derive(Clone)]
struct SyntheticGoalHost {
    inner: Arc<Mutex<SyntheticGoalState>>,
}

struct SyntheticGoalState {
    goal: Option<zeron_proto::GoalState>,
    actions: Vec<zeron_proto::GoalAction>,
    delay_next_ms: u64,
    fail_next: Option<String>,
}

impl SyntheticGoalHost {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SyntheticGoalState {
                goal: None,
                actions: Vec::new(),
                delay_next_ms: 0,
                fail_next: None,
            })),
        }
    }

    fn reset(&self, status: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.goal = Some(zeron_proto::GoalState {
            objective: "Test the goal tool.".into(),
            status: status.into(),
            tokens_used: 4_200,
            token_budget: Some(24_000),
        });
        inner.actions.clear();
        inner.delay_next_ms = 0;
        inner.fail_next = None;
    }

    fn actions(&self) -> Vec<zeron_proto::GoalAction> {
        self.inner.lock().unwrap().actions.clone()
    }

    fn goal(&self) -> Option<zeron_proto::GoalState> {
        self.inner.lock().unwrap().goal.clone()
    }

    fn delay_next(&self, milliseconds: u64) {
        self.inner.lock().unwrap().delay_next_ms = milliseconds;
    }

    fn fail_next(&self, message: &str) {
        self.inner.lock().unwrap().fail_next = Some(message.into());
    }
}

#[async_trait]
impl zeron_rpc::RpcService for SyntheticGoalHost {
    async fn handle(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
        if method != zeron_rpc::methods::SET_GOAL {
            return Err(zeron_rpc::RpcError::UnknownMethod(method.into()));
        }
        if params.get("chatId").and_then(serde_json::Value::as_str) != Some("composer-fixture")
            || params
                .get("targetDeviceId")
                .and_then(serde_json::Value::as_str)
                != Some("fixture-device")
        {
            return Err(zeron_rpc::RpcError::BadParams(
                "synthetic goal host only accepts its fixture chat and device".into(),
            ));
        }
        let action: zeron_proto::GoalAction = serde_json::from_value(
            params
                .get("action")
                .cloned()
                .ok_or_else(|| zeron_rpc::RpcError::BadParams("missing action".into()))?,
        )
        .map_err(|error| zeron_rpc::RpcError::BadParams(error.to_string()))?;
        let objective = params
            .get("objective")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        let (delay, failure) = {
            let mut inner = self.inner.lock().unwrap();
            (
                std::mem::take(&mut inner.delay_next_ms),
                inner.fail_next.take(),
            )
        };
        if delay > 0 {
            tokio::time::sleep(Duration::from_millis(delay)).await;
        }
        if let Some(failure) = failure {
            return Err(zeron_rpc::RpcError::Failed(failure));
        }
        let mut inner = self.inner.lock().unwrap();
        match action {
            zeron_proto::GoalAction::Pause => {
                inner
                    .goal
                    .as_mut()
                    .ok_or_else(|| zeron_rpc::RpcError::Failed("no active goal".into()))?
                    .status = "paused".into();
            }
            zeron_proto::GoalAction::Resume => {
                inner
                    .goal
                    .as_mut()
                    .ok_or_else(|| zeron_rpc::RpcError::Failed("no paused goal".into()))?
                    .status = "active".into();
            }
            zeron_proto::GoalAction::Edit => {
                inner
                    .goal
                    .as_mut()
                    .ok_or_else(|| zeron_rpc::RpcError::Failed("no goal to edit".into()))?
                    .objective = objective.ok_or_else(|| {
                    zeron_rpc::RpcError::BadParams("edit requires a non-empty objective".into())
                })?;
            }
            zeron_proto::GoalAction::Clear => inner.goal = None,
        }
        inner.actions.push(action);
        Ok(zeron_rpc::RpcReply::Value(serde_json::json!({
            "goal": inner.goal.clone()
        })))
    }
}

struct Fixture {
    composer: Entity<composer::Composer>,
    title: &'static str,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::Theme::of(cx).clone();
        div()
            .id("fixture")
            .size_full()
            .on_key_down(|event, window, cx| {
                let key = &event.keystroke;
                if key.key == "tab"
                    && !key.modifiers.control
                    && !key.modifiers.alt
                    && !key.modifiers.platform
                {
                    if key.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            })
            .bg(theme.surface)
            .text_color(theme.text)
            .font_family(theme.font_sans.clone())
            .p(px(24.0))
            .child(
                div()
                    .h_full()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap(px(20.0))
                    .child(settings::widgets::page_header(&theme, self.title, None))
                    .child(self.composer.clone()),
            )
    }
}

async fn pause(cx: &mut AsyncApp) {
    pause_for(cx, 260).await;
}

async fn pause_for(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(Duration::from_millis(ms))
        .await;
}

fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &Path,
    name: &str,
) -> anyhow::Result<()> {
    window.update(cx, |_, window, cx| {
        window.draw(cx).clear();
        window.render_to_image()?.save(output.join(name))?;
        Ok(())
    })?
}

fn press(window: gpui::AnyWindowHandle, cx: &mut AsyncApp, key: &str) -> anyhow::Result<()> {
    window.update(cx, |_, window, cx| {
        assert!(window.dispatch_keystroke(gpui::Keystroke::parse(key).unwrap(), cx));
    })?;
    Ok(())
}

async fn wait_for_goal_actions(
    host: &SyntheticGoalHost,
    expected: &[zeron_proto::GoalAction],
    cx: &mut AsyncApp,
) {
    for _ in 0..80 {
        if host.actions() == expected {
            return;
        }
        pause_for(cx, 25).await;
    }
    panic!("timed out waiting for goal actions: {:?}", host.actions());
}

fn question() -> zeron_proto::UserInputQuestion {
    zeron_proto::UserInputQuestion {
        id: "release-path".into(),
        header: "Release".into(),
        question: "Which checks should run before publishing?".into(),
        options: vec![
            "Unit tests".into(),
            "Integration tests".into(),
            "Visual regression".into(),
            "Accessibility audit".into(),
            "Cross-device smoke".into(),
            "Packaging verification".into(),
        ],
        option_descriptions: vec![
            "Fast package-level checks".into(),
            "Exercise the native RPC boundary".into(),
            "Capture dense tray states".into(),
            "Verify focus and keyboard flow".into(),
            "Check remote host capability gates".into(),
            "Validate distributable artifacts".into(),
        ],
        multi_select: true,
        allow_custom: true,
        non_blocking: false,
    }
}

fn dense_calls() -> Vec<zeron_proto::ToolCall> {
    serde_json::from_value(serde_json::json!([
        {"kind":"plan","text":"## Release plan\n\n1. Validate native inputs.\n2. Run focused tests.\n3. Capture keyboard evidence.\n4. Review provider capability gates."},
        {"kind":"todo","items":[
            {"id":"1","text":"Map native mode options","done":true,"status":"completed"},
            {"id":"2","text":"Verify question draft recovery","done":false,"status":"inProgress"},
            {"id":"3","text":"Exercise dense tray scrolling","done":false,"status":"pending"},
            {"id":"4","text":"Capture goal control roundtrip","done":false,"status":"blocked"}
        ]},
        {"kind":"goal","objective":"Ship native composer interactions","status":"active","tokensUsed":4200,"tokenBudget":24000}
    ]))
    .unwrap()
}

#[derive(Clone, Copy)]
struct FixtureVariant {
    appearance: appearance::AppearanceMode,
    appearance_name: &'static str,
    surface: zeron_theme::SurfacePreference,
    surface_name: &'static str,
    reduced_motion: bool,
}

fn fixture_options() -> anyhow::Result<(PathBuf, FixtureVariant)> {
    let appearance = std::env::var("ZERON_FIXTURE_APPEARANCE")
        .unwrap_or_else(|_| "dark".into())
        .to_ascii_lowercase();
    let surface = std::env::var("ZERON_FIXTURE_SURFACE")
        .unwrap_or_else(|_| "frosted".into())
        .to_ascii_lowercase();
    let mut variant = FixtureVariant {
        appearance: match appearance.as_str() {
            "dark" => appearance::AppearanceMode::Dark,
            "light" => appearance::AppearanceMode::Light,
            value => anyhow::bail!(
                "invalid ZERON_FIXTURE_APPEARANCE={value:?}; expected `dark` or `light`"
            ),
        },
        appearance_name: if appearance == "light" {
            "light"
        } else {
            "dark"
        },
        surface: match surface.as_str() {
            "frosted" => zeron_theme::SurfacePreference::Frosted,
            "opaque" => zeron_theme::SurfacePreference::Opaque,
            value => anyhow::bail!(
                "invalid ZERON_FIXTURE_SURFACE={value:?}; expected `frosted` or `opaque`"
            ),
        },
        surface_name: if surface == "opaque" {
            "opaque"
        } else {
            "frosted"
        },
        reduced_motion: std::env::var_os("ZERON_FIXTURE_REDUCE_MOTION").is_some(),
    };
    let mut output = None;
    for argument in std::env::args().skip(1) {
        match argument.as_str() {
            "--dark" => {
                variant.appearance = appearance::AppearanceMode::Dark;
                variant.appearance_name = "dark";
            }
            "--light" => {
                variant.appearance = appearance::AppearanceMode::Light;
                variant.appearance_name = "light";
            }
            "--frosted" => {
                variant.surface = zeron_theme::SurfacePreference::Frosted;
                variant.surface_name = "frosted";
            }
            "--opaque" => {
                variant.surface = zeron_theme::SurfacePreference::Opaque;
                variant.surface_name = "opaque";
            }
            "--reduce-motion" => variant.reduced_motion = true,
            "--standard-motion" => variant.reduced_motion = false,
            value if value.starts_with('-') => anyhow::bail!("unknown fixture option {value:?}"),
            value if output.is_none() => output = Some(PathBuf::from(value)),
            value => anyhow::bail!("unexpected second output directory {value:?}"),
        }
    }
    let output = output.ok_or_else(|| {
        anyhow::anyhow!(
            "usage: native-interactions-fixture OUTPUT_DIR [--dark|--light] [--frosted|--opaque] [--reduce-motion]"
        )
    })?;
    Ok((output, variant))
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let (output, variant) = fixture_options()?;
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let mut prefs = settings::UiSettings::default();
            prefs.surface = variant.surface;
            settings::init(prefs.clone(), data.clone(), cx);
            motion::set_reduced_motion(cx, variant.reduced_motion);
            let fonts = typography::register_fonts(cx);
            typography::init(
                prefs.ui_font_family.clone(),
                prefs.ui_font_size,
                prefs.terminal_font_family.clone(),
                prefs.terminal_font_size,
                prefs.code_font_family.clone(),
                prefs.code_font_size,
                fonts,
                cx,
            );
            theme_library::init(data.clone(), cx);
            appearance::init(
                variant.appearance,
                prefs.theme_selection,
                prefs.accent,
                prefs.surface,
                cx,
            );
            composer::init(cx, prefs.composer_send_behavior);

            let goal_host = SyntheticGoalHost::new();
            let goal_engine = state::EngineHandle::from_goal_fixture_client(
                zeron_rpc::memory_client(Arc::new(goal_host.clone())),
            );
            let state = cx.new(|_| state::AppState::new());
            state.update(cx, |state, _| state.set_goal_fixture_engine(goal_engine));
            let composer = cx.new(|cx| composer::Composer::new(state.clone(), cx));
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(40.0), px(40.0)),
                            size(px(840.0), px(740.0)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| {
                        cx.new(|_| Fixture {
                            composer,
                            title: "Native composer interactions",
                        })
                    },
                )
                .unwrap();
            cx.activate(true);

            cx.spawn(async move |cx| {
                let result: anyhow::Result<()> = async {
                    window.update(cx, |view, window, cx| {
                        view.title = "Dense native trays";
                        view.composer.update(cx, |composer, cx| {
                            composer.fixture_activity(dense_calls(), true, cx);
                            composer.fixture_queue(
                                &[
                                    "Run the focused checks after this answer",
                                    "Summarize remaining provider-specific gaps",
                                    "Prepare review notes without losing this queue",
                                ],
                                cx,
                            );
                            composer.fixture_question(question(), cx);
                            composer.fixture_harness(zeron_proto::HarnessId::Codex, cx);
                        });
                        window.resize(size(px(440.0), px(520.0)));
                        cx.notify();
                    })?;
                    pause(cx).await;
                    window.update(cx, |view, _, cx| {
                        assert!(
                            view.composer
                                .read(cx)
                                .fixture_has_pending_question("fixture-question", cx)
                        );
                    })?;
                    let dense_capture = format!(
                        "native-dense-stack-{}-440.png",
                        variant.appearance_name
                    );
                    capture(
                        window.into(),
                        cx,
                        &output,
                        &dense_capture,
                    )?;

                    let first_question = zeron_proto::UserInputQuestion {
                        id: "scope".into(),
                        header: "Scope".into(),
                        question: "What should this release focus on?".into(),
                        options: vec!["Native interactions".into(), "Provider adapters".into()],
                        option_descriptions: vec![
                            "Composer controls and state recovery".into(),
                            "Harness normalization and transport".into(),
                        ],
                        multi_select: false,
                        allow_custom: true,
                        non_blocking: false,
                    };
                    let mut second_question = question();
                    second_question.id = "checks".into();
                    window.update(cx, |view, _, cx| {
                        view.title = "Native question paging";
                        view.composer.update(cx, |composer, cx| {
                            composer.fixture_activity(Vec::new(), false, cx);
                            composer.fixture_paginated_question(
                                vec![first_question, second_question],
                                cx,
                            );
                        });
                        cx.notify();
                    })?;
                    pause(cx).await;
                    press(window.into(), cx, "1")?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-question-page-two-keyboard-selection.png",
                    )?;
                    window.update(cx, |view, _, cx| {
                        view.composer
                            .update(cx, |composer, cx| composer.fixture_question_back(cx));
                    })?;
                    pause(cx).await;
                    window.update(cx, |view, _, cx| {
                        assert_eq!(
                            view.composer.read(cx).fixture_message_draft(cx),
                            "Keep this typed answer when I return"
                        )
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-question-draft-restored.png",
                    )?;

                    window.update(cx, |view, window, cx| {
                        view.title = "Activity keyboard disclosure";
                        view.composer.update(cx, |composer, cx| {
                            composer.fixture_activity(dense_calls(), false, cx)
                        });
                        window.focus(&view.composer.focus_handle(cx), cx);
                        cx.notify();
                    })?;
                    pause(cx).await;
                    for _ in 0..32 {
                        let focused = window.update(cx, |view, window, cx| {
                            view.composer
                                .read(cx)
                                .fixture_activity_keyboard_state(window)
                                .0
                        })?;
                        if focused {
                            break;
                        }
                        press(window.into(), cx, "tab")?;
                    }
                    press(window.into(), cx, "enter")?;
                    pause_for(cx, 220).await;
                    window.update(cx, |view, window, cx| {
                        assert!(
                            view.composer
                                .read(cx)
                                .fixture_activity_keyboard_state(window)
                                .1
                        )
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-activity-keyboard-expanded.png",
                    )?;

                    const DRAFT: &str = "Keep this message draft while I manage the goal.";
                    goal_host.reset("active");
                    window.update(cx, |view, window, cx| {
                        view.title = "Goal controls · synthetic host";
                        view.composer.update(cx, |composer, cx| {
                            composer.fixture_activity(
                                vec![
                                    serde_json::from_value(serde_json::json!({
                                        "kind":"goal",
                                        "objective":"Test the goal tool.",
                                        "status":"active",
                                        "tokensUsed":4200,
                                        "tokenBudget":24000
                                    }))
                                    .unwrap(),
                                ],
                                false,
                                cx,
                            );
                            composer.fixture_harness(zeron_proto::HarnessId::Codex, cx);
                            composer.fixture_set_message_draft(DRAFT, cx);
                            assert!(composer.fixture_focus_goal_control(
                                "goal-lifecycle",
                                window,
                                cx
                            ));
                        });
                        window.resize(size(px(840.0), px(740.0)));
                        cx.notify();
                    })?;
                    pause(cx).await;
                    press(window.into(), cx, "enter")?;
                    wait_for_goal_actions(&goal_host, &[zeron_proto::GoalAction::Pause], cx).await;
                    window.update(cx, |view, _, cx| {
                        assert_eq!(view.composer.read(cx).fixture_message_draft(cx), DRAFT);
                    })?;
                    assert_eq!(goal_host.goal().unwrap().status, "paused");
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-goal-paused-synthetic-host.png",
                    )?;

                    // A delayed host reply leaves the native control visibly
                    // pending and disabled without borrowing the message draft.
                    goal_host.delay_next(350);
                    window.update(cx, |view, window, cx| {
                        assert!(view.composer.update(cx, |composer, cx| {
                            composer.fixture_focus_goal_control("goal-lifecycle", window, cx)
                        }));
                    })?;
                    press(window.into(), cx, "space")?;
                    pause_for(cx, 60).await;
                    assert_eq!(goal_host.actions(), vec![zeron_proto::GoalAction::Pause]);
                    window.update(cx, |view, _, cx| {
                        assert_eq!(view.composer.read(cx).fixture_message_draft(cx), DRAFT)
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-goal-resume-pending-synthetic-host.png",
                    )?;
                    wait_for_goal_actions(
                        &goal_host,
                        &[
                            zeron_proto::GoalAction::Pause,
                            zeron_proto::GoalAction::Resume,
                        ],
                        cx,
                    )
                    .await;
                    assert_eq!(goal_host.goal().unwrap().status, "active");

                    // Cancel remains local: it closes the objective field,
                    // keeps the ordinary draft, and never sends SetGoal.
                    window.update(cx, |view, window, cx| {
                        assert!(view.composer.update(cx, |composer, cx| {
                            composer.fixture_focus_goal_control("goal-edit", window, cx)
                        }));
                    })?;
                    press(window.into(), cx, "enter")?;
                    window.update(cx, |view, window, cx| {
                        let (editing, focused) =
                            view.composer.read(cx).fixture_goal_edit_state(window, cx);
                        assert!(editing && focused);
                        view.composer.update(cx, |composer, cx| {
                            composer.fixture_goal_edit_text("Discard this objective", cx)
                        });
                        assert!(view.composer.update(cx, |composer, cx| {
                            composer.fixture_focus_goal_control("goal-cancel", window, cx)
                        }));
                    })?;
                    press(window.into(), cx, "space")?;
                    pause(cx).await;
                    assert_eq!(
                        goal_host.actions(),
                        vec![
                            zeron_proto::GoalAction::Pause,
                            zeron_proto::GoalAction::Resume,
                        ]
                    );
                    window.update(cx, |view, window, cx| {
                        assert!(!view.composer.read(cx).fixture_goal_edit_state(window, cx).0);
                        assert_eq!(view.composer.read(cx).fixture_message_draft(cx), DRAFT);
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-goal-edit-cancelled-synthetic-host.png",
                    )?;

                    window.update(cx, |view, window, cx| {
                        assert!(view.composer.update(cx, |composer, cx| {
                            composer.fixture_focus_goal_control("goal-edit", window, cx)
                        }));
                    })?;
                    press(window.into(), cx, "enter")?;
                    window.update(cx, |view, window, cx| {
                        let (editing, focused) =
                            view.composer.read(cx).fixture_goal_edit_state(window, cx);
                        assert!(editing && focused);
                        view.composer.update(cx, |composer, cx| {
                            composer
                                .fixture_goal_edit_text("Ship the focused native interactions", cx)
                        });
                    })?;
                    press(window.into(), cx, "enter")?;
                    wait_for_goal_actions(
                        &goal_host,
                        &[
                            zeron_proto::GoalAction::Pause,
                            zeron_proto::GoalAction::Resume,
                            zeron_proto::GoalAction::Edit,
                        ],
                        cx,
                    )
                    .await;
                    assert_eq!(
                        goal_host.goal().unwrap().objective,
                        "Ship the focused native interactions"
                    );
                    window.update(cx, |view, _, cx| {
                        assert_eq!(view.composer.read(cx).fixture_message_draft(cx), DRAFT)
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-goal-edited-synthetic-host.png",
                    )?;

                    // A host rejection restores operability and reports the
                    // error while leaving both the provider goal and message
                    // draft untouched.
                    goal_host.fail_next("synthetic provider rejected the update");
                    window.update(cx, |view, window, cx| {
                        assert!(view.composer.update(cx, |composer, cx| {
                            composer.fixture_focus_goal_control("goal-lifecycle", window, cx)
                        }));
                    })?;
                    press(window.into(), cx, "enter")?;
                    pause(cx).await;
                    assert_eq!(
                        goal_host.actions(),
                        vec![
                            zeron_proto::GoalAction::Pause,
                            zeron_proto::GoalAction::Resume,
                            zeron_proto::GoalAction::Edit,
                        ]
                    );
                    window.update(cx, |view, _, cx| {
                        assert_eq!(view.composer.read(cx).fixture_message_draft(cx), DRAFT)
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-goal-error-synthetic-host.png",
                    )?;

                    window.update(cx, |view, window, cx| {
                        assert!(view.composer.update(cx, |composer, cx| {
                            composer.fixture_focus_goal_control("goal-clear", window, cx)
                        }));
                    })?;
                    press(window.into(), cx, "enter")?;
                    wait_for_goal_actions(
                        &goal_host,
                        &[
                            zeron_proto::GoalAction::Pause,
                            zeron_proto::GoalAction::Resume,
                            zeron_proto::GoalAction::Edit,
                            zeron_proto::GoalAction::Clear,
                        ],
                        cx,
                    )
                    .await;
                    assert!(goal_host.goal().is_none());
                    window.update(cx, |view, _, cx| {
                        assert_eq!(view.composer.read(cx).fixture_message_draft(cx), DRAFT)
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "native-goal-deleted-synthetic-host.png",
                    )?;
                    std::fs::write(
                        output.join("result.txt"),
                        format!(
                            "PASS: native composer interactions; appearance={}, surface={}, motion={}.\n",
                            variant.appearance_name,
                            variant.surface_name,
                            if variant.reduced_motion {
                                "reduced"
                            } else {
                                "standard"
                            }
                        ),
                    )?;
                    Ok(())
                }
                .await;
                if let Err(error) = result {
                    eprintln!("native interaction fixture failed: {error:#}");
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
