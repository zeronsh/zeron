//! Isolated visual and keyboard evidence for the compact model picker.
use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, Render, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, size,
};
use std::{path::Path, path::PathBuf, time::Duration};
use zeron_ui::*;

struct Fixture {
    pickers: Entity<pickers::Pickers>,
    title: &'static str,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme::Theme::of(cx).clone();
        div()
            .id("compact-picker-fixture")
            .size_full()
            .bg(theme.surface)
            .text_color(theme.text)
            .font_family(theme.font_sans.clone())
            .p(px(24.0))
            .child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .gap(px(16.0))
                    .child(settings::widgets::page_header(&theme, self.title, None))
                    .child(
                        div()
                            .relative()
                            .h(px(48.0))
                            .flex()
                            .items_center()
                            .child(self.pickers.clone()),
                    ),
            )
    }
}

async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(260))
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
        assert!(
            window.dispatch_keystroke(gpui::Keystroke::parse(key).unwrap(), cx),
            "compact picker did not handle {key}"
        );
    })?;
    Ok(())
}

async fn capture_matrix_scene(
    window: gpui::WindowHandle<Fixture>,
    cx: &mut AsyncApp,
    output: &Path,
    appearance: appearance::AppearanceMode,
    width: f32,
    label: &str,
) -> anyhow::Result<()> {
    window.update(cx, |view, window, cx| {
        appearance::set_mode(appearance, cx);
        window.resize(size(px(width), px(560.0)));
        view.title = if appearance == appearance::AppearanceMode::Light {
            "Compact picker · light"
        } else {
            "Compact picker · dark"
        };
        cx.notify();
    })?;
    pause(cx).await;
    capture(
        window.into(),
        cx,
        output,
        &format!("compact-panel-{label}.png"),
    )?;
    press(window.into(), cx, "enter")?;
    pause(cx).await;
    window.update(cx, |view, _, cx| {
        assert!(view.pickers.read(cx).fixture_compact_state(cx).model_list);
    })?;
    capture(
        window.into(),
        cx,
        output,
        &format!("compact-model-list-{label}.png"),
    )?;
    press(window.into(), cx, "escape")?;
    pause(cx).await;
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
            let mut prefs = settings::UiSettings::default();
            prefs.compact_model_picker = true;
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

            let state = cx.new(|_| state::AppState::new());
            let pickers = cx.new(|cx| {
                let mut pickers = pickers::Pickers::new(state, cx);
                pickers.fixture_compact_catalog(cx);
                pickers
            });
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(40.0), px(40.0)),
                            size(px(840.0), px(560.0)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        pickers.update(cx, |pickers, cx| {
                            pickers.open_model_menu(window, cx)
                        });
                        cx.new(|_| Fixture {
                            pickers,
                            title: "Compact picker · dark",
                        })
                    },
                )
                .unwrap();
            cx.activate(true);

            cx.spawn(async move |cx| {
                let result: anyhow::Result<()> = async {
                    pause(cx).await;
                    for (appearance, width, label) in [
                        (appearance::AppearanceMode::Dark, 840.0, "dark-wide"),
                        (appearance::AppearanceMode::Dark, 360.0, "dark-narrow"),
                        (appearance::AppearanceMode::Light, 840.0, "light-wide"),
                        (appearance::AppearanceMode::Light, 360.0, "light-narrow"),
                    ] {
                        capture_matrix_scene(window, cx, &output, appearance, width, label).await?;
                    }

                    window.update(cx, |view, window, cx| {
                        appearance::set_mode(appearance::AppearanceMode::Dark, cx);
                        window.resize(size(px(840.0), px(560.0)));
                        view.title = "Compact picker · keyboard controls";
                        cx.notify();
                    })?;
                    pause(cx).await;

                    // Model → Reset → Fast → Effort, then choose the ladder's
                    // maximum using the real keyboard path.
                    for key in ["down", "down", "down", "end"] {
                        press(window.into(), cx, key)?;
                    }
                    pause(cx).await;
                    window.update(cx, |view, _, cx| {
                        let state = view.pickers.read(cx).fixture_compact_state(cx);
                        assert_eq!(state.reasoning, Some(zeron_proto::ReasoningLevel::XHigh));
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "compact-effort-keyboard-xhigh.png",
                    )?;

                    for key in ["up", "up", "enter"] {
                        press(window.into(), cx, key)?;
                    }
                    pause(cx).await;
                    window.update(cx, |view, _, cx| {
                        let state = view.pickers.read(cx).fixture_compact_state(cx);
                        assert_eq!(state.explicit_reasoning, None);
                        assert_eq!(state.reasoning, Some(zeron_proto::ReasoningLevel::High));
                        assert!(!state.fast);
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "compact-reset-keyboard.png",
                    )?;

                    // Reset → Fast toggles the exact service-tier option.
                    for key in ["down", "enter"] {
                        press(window.into(), cx, key)?;
                    }
                    window.update(cx, |view, _, cx| {
                        assert!(view.pickers.read(cx).fixture_compact_state(cx).fast);
                    })?;
                    pause(cx).await;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "compact-fast-keyboard.png",
                    )?;

                    // Return to Model, enter the list, move by keyboard, and
                    // star the focused model without changing the selection.
                    for key in ["up", "up", "enter", "down", "cmd-shift-f"] {
                        press(window.into(), cx, key)?;
                    }
                    pause(cx).await;
                    window.update(cx, |view, _, cx| {
                        let state = view.pickers.read(cx).fixture_compact_state(cx);
                        assert!(state.model_list);
                        assert_eq!(state.focused_model.as_deref(), Some("gpt-5.3-codex"));
                        assert!(state.focused_model_favorite);
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "compact-favorite-keyboard.png",
                    )?;

                    press(window.into(), cx, "cmd-shift-f")?;
                    pause(cx).await;
                    window.update(cx, |view, _, cx| {
                        let state = view.pickers.read(cx).fixture_compact_state(cx);
                        assert_eq!(state.focused_model.as_deref(), Some("gpt-5.3-codex"));
                        assert!(!state.focused_model_favorite);
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "compact-favorite-removed-keyboard.png",
                    )?;

                    // Repeat the keyboard paths at the opposite visual and
                    // geometry extreme so the dense controls are exercised,
                    // rather than only photographed, in the narrow layout.
                    press(window.into(), cx, "escape")?;
                    window.update(cx, |view, window, cx| {
                        appearance::set_mode(appearance::AppearanceMode::Light, cx);
                        window.resize(size(px(360.0), px(560.0)));
                        view.title = "Compact picker · light narrow keyboard";
                        cx.notify();
                    })?;
                    for key in ["down", "enter", "down", "down", "home"] {
                        press(window.into(), cx, key)?;
                    }
                    pause(cx).await;
                    window.update(cx, |view, _, cx| {
                        assert_eq!(
                            view.pickers.read(cx).fixture_compact_state(cx).reasoning,
                            Some(zeron_proto::ReasoningLevel::Low)
                        );
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "compact-effort-keyboard-light-narrow.png",
                    )?;

                    for key in ["up", "up", "enter"] {
                        press(window.into(), cx, key)?;
                    }
                    pause(cx).await;
                    window.update(cx, |view, _, cx| {
                        let state = view.pickers.read(cx).fixture_compact_state(cx);
                        assert_eq!(state.explicit_reasoning, None);
                        assert_eq!(state.reasoning, Some(zeron_proto::ReasoningLevel::High));
                        assert!(!state.fast);
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "compact-reset-keyboard-light-narrow.png",
                    )?;

                    for key in ["up", "enter", "down", "cmd-shift-f"] {
                        press(window.into(), cx, key)?;
                    }
                    pause(cx).await;
                    window.update(cx, |view, _, cx| {
                        let state = view.pickers.read(cx).fixture_compact_state(cx);
                        assert!(state.model_list);
                        assert_eq!(state.focused_model.as_deref(), Some("gpt-5.3-codex"));
                        assert!(state.focused_model_favorite);
                    })?;
                    capture(
                        window.into(),
                        cx,
                        &output,
                        "compact-favorite-keyboard-light-narrow.png",
                    )?;

                    std::fs::write(
                        output.join("result.txt"),
                        "PASS: compact picker panel/list layout at dark/light and narrow/wide sizes; keyboard effort, reset, fast tier, and favorite identity behavior.\n",
                    )?;
                    Ok(())
                }
                .await;
                if let Err(error) = result {
                    eprintln!("compact picker fixture failed: {error:#}");
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
