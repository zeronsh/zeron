//! Visual evidence for experimental transcript editing: the editor and its
//! Save bar over a synthetic Claude Code history served by an in-memory host.
use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, Render, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, size,
};
use std::{path::Path, path::PathBuf, sync::Arc, time::Duration};
use zeron_proto::native_history::{
    NativeHistory, NativeHistoryItem, NativeItemKind, NativeRole, ReasoningSeal,
};
use zeron_ui::*;

const COLUMN: f32 = 640.0;

struct Fixture {
    editor: Entity<transcript_edit::TranscriptEditor>,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::Theme::of(cx).clone();
        let bar = transcript_edit::TranscriptEditor::render_bar(&self.editor, COLUMN, cx);
        div()
            .id("transcript-edit-fixture")
            .size_full()
            .relative()
            .bg(theme.surface)
            .text_color(theme.text)
            .font_family(theme.font_sans.clone())
            .child(self.editor.clone())
            .child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(px(20.0))
                    .child(bar),
            )
    }
}

fn item(id: &str, role: NativeRole, turn: u32, kind: NativeItemKind) -> NativeHistoryItem {
    NativeHistoryItem {
        id: id.into(),
        role,
        turn,
        editable: matches!(kind, NativeItemKind::Text { .. }),
        removable: matches!(kind, NativeItemKind::Reasoning { .. }),
        kind,
    }
}

fn history() -> NativeHistory {
    use NativeItemKind::*;
    use NativeRole::*;
    NativeHistory {
        chat_id: "fixture".into(),
        harness: "claude-code".into(),
        session_id: "session".into(),
        revision: "r1".into(),
        items: vec![
            item("u0", User, 0, Text { text: "Why does the sync test fail on CI but not locally?".into() }),
            item("r0", Assistant, 0, Reasoning {
                text: Some("The CI runner is slower, so the 50 ms debounce probably fires after the assertion. Check the timer in registry tests first.".into()),
                seal: ReasoningSeal::Signed,
            }),
            item("t0", Assistant, 0, ToolCall { name: "Bash".into(), summary: "cargo test -p zeron-sync registry::debounce -- --nocapture".into() }),
            item("o0", User, 0, ToolResult { summary: "test registry::debounce ... FAILED (expected 3 pushes, got 2)".into(), is_error: true }),
            item("r1", Assistant, 0, Reasoning { text: None, seal: ReasoningSeal::Encrypted }),
            item("a0", Assistant, 0, Text { text: "The test races its own debounce: on a slow runner the third push lands after the assertion. Waiting on the flush notification instead of a fixed sleep makes it deterministic.".into() }),
            item("u1", User, 1, Text { text: "Do that, and keep the timeout short.".into() }),
            item("a1", Assistant, 1, Text { text: "Done. The test now awaits `flushed()` with a 2 s ceiling; it passes 200 runs in a row locally.".into() }),
        ],
        compacted: false,
        can_undo: true,
    }
}

struct Host;

#[async_trait::async_trait]
impl zeron_rpc::RpcService for Host {
    async fn handle(
        &self,
        _: &str,
        _: serde_json::Value,
    ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
        zeron_rpc::RpcReply::value(&history())
    }
}

async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
}

/// Save the window as `name`. Platforms without in-process capture (Linux)
/// announce the scene on stdout and hold it for an outside screenshot.
async fn capture(
    window: gpui::AnyWindowHandle,
    cx: &mut AsyncApp,
    output: &Path,
    name: &str,
) -> anyhow::Result<()> {
    let saved = window.update(cx, |_, window, cx| {
        window.draw(cx).clear();
        window
            .render_to_image()
            .and_then(|image| Ok(image.save(output.join(name))?))
    })?;
    if saved.is_err() {
        println!("CAPTURE {}", output.join(name).display());
        cx.background_executor()
            .timer(Duration::from_millis(1500))
            .await;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    let _guard = runtime.enter();
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_tokio::init(cx);
            gpui_base::init(cx);
            let prefs = settings::UiSettings::default();
            settings::init(prefs.clone(), data.clone(), cx);
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
            theme_library::init(data, cx);
            appearance::init(
                appearance::AppearanceMode::Dark,
                prefs.theme_selection,
                prefs.accent,
                prefs.surface,
                cx,
            );
            composer::init(cx, prefs.composer_send_behavior);
            let engine = state::EngineHandle::from_test_client(zeron_rpc::memory_client(Arc::new(Host)));
            let editor = cx.new(|cx| {
                let mut editor = transcript_edit::TranscriptEditor::new(
                    "fixture".into(),
                    "host".into(),
                    zeron_proto::HarnessId::ClaudeCode,
                    engine,
                    None,
                    cx,
                );
                editor.set_column_width(COLUMN, cx);
                editor
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(40.0), px(40.0)),
                            size(px(900.0), px(900.0)),
                        ))),
                        ..Default::default()
                    },
                    |_, cx| cx.new(|_| Fixture { editor: editor.clone() }),
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let result: anyhow::Result<()> = async {
                    for _ in 0..50 {
                        pause(cx).await;
                        if editor.read_with(cx, |editor, _| editor.is_loaded()) {
                            break;
                        }
                    }
                    pause(cx).await;
                    capture(window.into(), cx, &output, "edit-at-rest-dark.png").await?;
                    editor.update(cx, |editor, cx| {
                        editor.fixture_expand("r0", cx);
                        editor.fixture_remove("r1", cx);
                        editor.fixture_replace(
                            "a0",
                            "The test races its own debounce. Await the flush notification instead of sleeping.",
                            cx,
                        );
                    });
                    pause(cx).await;
                    capture(window.into(), cx, &output, "edit-pending-dark.png").await?;
                    cx.update(|cx| appearance::set_mode(appearance::AppearanceMode::Light, cx));
                    pause(cx).await;
                    capture(window.into(), cx, &output, "edit-pending-light.png").await?;
                    Ok(())
                }
                .await;
                if let Err(err) = result {
                    eprintln!("fixture failed: {err:#}");
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
