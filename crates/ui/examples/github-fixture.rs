//! Interactive offline preview of the real native GitHub surface.
//! cargo run -p zeron-ui --example github-fixture --features github-fixture
use gpui::{
    AppContext, Bounds, Context, Focusable, Render, Window, WindowBounds, WindowOptions, div,
    point, prelude::*, px, size,
};
use zeron_ui::{
    appearance, github::GitHubSurface, icons, popover, settings, state::AppState, theme::Theme,
    theme_library, typography,
};

struct Preview {
    view: gpui::AnyView,
}
impl Render for Preview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .size_full()
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(theme.font_sans_fixed.clone())
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(44.0))
                    .px(px(12.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .child("Zeron · GitHub viewer · Sample data"),
                    )
                    .children(
                        [
                            (appearance::AppearanceMode::Light, "Light"),
                            (appearance::AppearanceMode::Dark, "Dark"),
                        ]
                        .into_iter()
                        .map(|(mode, label)| {
                            popover::btn_ghost(theme, label, format!("demo-{label}"))
                                .id(label)
                                .role(gpui::Role::Button)
                                .aria_label(label)
                                .on_click(move |_, _, cx| appearance::set_mode(mode, cx))
                        }),
                    ),
            )
            .child(div().flex_1().min_h_0().child(self.view.clone()))
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<_> = std::env::args().collect();
    let files = args.iter().any(|arg| arg == "--files");
    let commit = args.iter().any(|arg| arg == "--commit");
    let light = args.iter().any(|arg| arg == "--light");
    let before = args.iter().any(|arg| arg == "--before");
    let surface = if args.iter().any(|arg| arg == "--frosted") {
        zeron_theme::SurfacePreference::Frosted
    } else if args.iter().any(|arg| arg == "--opaque") {
        zeron_theme::SurfacePreference::Opaque
    } else {
        zeron_theme::SurfacePreference::ThemeDefault
    };
    let width = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--width=")?.parse::<f32>().ok())
        .unwrap_or(800.0);
    let data = std::env::temp_dir().join(format!("zeron-github-preview-{}", std::process::id()));
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
            theme_library::init(data, cx);
            appearance::init(
                if light {
                    appearance::AppearanceMode::Light
                } else {
                    appearance::AppearanceMode::Dark
                },
                settings.theme_selection,
                settings.accent,
                surface,
                cx,
            );
            let state = cx.new(|_| AppState::new());
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(120.0), px(80.0)),
                            size(px(width), px(700.0)),
                        ))),
                        titlebar: Some(gpui::TitlebarOptions {
                            title: Some("Zeron — Native GitHub preview".into()),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    |window, cx| {
                        let view: gpui::AnyView = if before {
                            let browser = cx.new(|cx| {
                                zeron_ui::browser::BrowserSurface::new(
                                    Default::default(),
                                    false,
                                    window,
                                    cx,
                                )
                            });
                            browser.update(cx, |browser, cx| {
                                browser.navigate(
                                    "https://github.com/acme/project/pull/42",
                                    window,
                                    cx,
                                );
                            });
                            window.focus(&browser.read(cx).focus_handle(cx), cx);
                            browser.into()
                        } else {
                            let view = cx.new(|cx| GitHubSurface::demo(state, files, commit, cx));
                            window.focus(&view.read(cx).focus_handle(cx), cx);
                            view.into()
                        };
                        cx.new(|_| Preview { view })
                    },
                )
                .expect("open native preview");
            cx.on_window_closed(move |cx, _| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            let _ = window;
            cx.activate(true);
        });
    Ok(())
}
