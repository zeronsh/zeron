//! Agent visual review fixture: one inline visual (`dash`, `bars`, `misc`,
//! or `tabs`) rendered from its Luau script in an isolated, offline window, the
//! transcript's column width. Pick it with `--visual <name>`. With `--capture
//! <dir>` it saves the visual at rest (and, with ZERON_VISUAL_HOVER=x,y,
//! with the pointer there), then quits; ZERON_PALETTE_LIGHT=1 renders the
//! light theme. Where `render_to_image` isn't implemented (Linux), the
//! window stays up for ZERON_VISUAL_HOLD_MS (default 6000) so it can be
//! captured from the display.
use gpui::{
    AppContext, Bounds, IntoElement, ParentElement, Render, Styled, WindowBounds, WindowOptions,
    div, px, size,
};
use std::path::{Path, PathBuf};
use zeron_ui::*;

const WIDTH: f32 = 776.0;
const HEIGHT: f32 = 860.0;

/// A dashboard (stats, tabs, an area chart), bars beside a line, the
/// remaining chart kinds with a table, and conditional tab content.
const VISUALS: &[(&str, &str)] = &[
    (
        "dash",
        r#"
local days = { "Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun" }
return ui.column { gap = 16,
  ui.row { gap = 16,
    ui.stat { value = "158 ms", label = "p95 latency", delta = "-8%", tone = "success" },
    ui.stat { value = "99.95%", label = "uptime", delta = "+0.02%" },
    ui.stat { value = "4.2k", label = "req / min", delta = "+12%", tone = "danger" },
  },
  ui.tabs { options = { "7d", "30d" }, value = "7d", on_change = function() end },
  ui.chart { type = "area", height = 200, x = days, unit = "ms",
    series = { { name = "p95", values = { 182, 175, 190, 168, 160, 171, 158 } },
               { name = "p50", values = { 92, 88, 95, 84, 80, 86, 79 } } } },
}"#,
    ),
    (
        "bars",
        r#"
return ui.column { gap = 16,
  ui.text { "Revenue by region", size = "lg", weight = 600 },
  ui.chart { type = "bar", height = 200, x = { "NA", "EU", "APAC", "LATAM", "MEA" }, prefix = "$", unit = "M",
    series = { { name = "2025", values = { 42, 31, 24, 9, 6 } }, { name = "2026", values = { 51, 34, 30, 12, 7 } } } },
  ui.text { "Signups", size = "lg", weight = 600 },
  ui.chart { type = "line", height = 180,
    x = { "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec" },
    series = { { name = "Signups", values = { 120, 132, 128, 150, 171, 165, 190, 204, 198, 230, 251, 270 } } } },
}"#,
    ),
    (
        "misc",
        r#"
return ui.column { gap = 18,
  ui.chart { type = "bar", horizontal = true, height = 150, x = { "Rust", "TypeScript", "Swift", "Kotlin" }, unit = "%",
    series = { { name = "share", values = { 46, 31, 14, 9 } } } },
  ui.chart { type = "bar", stacked = true, height = 180, x = { "Q1", "Q2", "Q3", "Q4" },
    series = { { name = "Cloud", values = { 10, 14, 18, 23 } }, { name = "Desktop", values = { 8, 9, 9, 10 } },
               { name = "Mobile", values = { 3, 5, 8, 11 } } } },
  ui.chart { type = "scatter", height = 170, x_unit = "ms", unit = "MB",
    series = { { name = "runs", points = { { 12, 40 }, { 18, 52 }, { 25, 61 }, { 31, 58 }, { 40, 80 }, { 44, 92 }, { 52, 88 } } } } },
  ui.table { columns = { "Service", { label = "p95", align = "end" }, { label = "Errors", align = "end" } },
    rows = { { "api", "182 ms", "0.02%" }, { "auth", "96 ms", "0.00%" }, { "search", "240 ms", "0.31%" } } },
}"#,
    ),
    // Tabs as agents write them: one `cond and node or nil` child per tab,
    // opened on the second. Each unselected tab leaves a nil hole in the
    // children before the selected one.
    (
        "tabs",
        r#"
local s = ui.state { tab = "Costs" }
return function()
  return ui.column { gap = 16,
    ui.tabs { options = { "Overview", "Costs", "Team" }, value = s.tab, on_change = function(v) s.tab = v end },
    s.tab == "Overview" and ui.text "Spend is up 12% this quarter, driven by inference." or nil,
    s.tab == "Costs" and ui.chart { type = "bar", height = 200, x = { "Compute", "Storage", "Egress", "Seats" }, prefix = "$", unit = "k",
      series = { { name = "Q3", values = { 84, 22, 13, 31 } }, { name = "Q4", values = { 97, 24, 11, 35 } } } } or nil,
    s.tab == "Team" and ui.table { columns = { "Team", { label = "Spend", align = "end" } },
      rows = { { "Platform", "$61k" }, { "Research", "$88k" } } } or nil,
  }
end"#,
    ),
];

struct Page(gpui::Entity<visuals::VisualFrame>);

impl Render for Page {
    fn render(&mut self, _: &mut gpui::Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        // The transcript's reply column on an opaque window.
        div()
            .size_full()
            .bg(theme::Theme::of(cx).bg)
            .p(px(28.))
            .child(div().w(px(720.)).child(self.0.clone()))
    }
}

async fn pause(cx: &mut gpui::AsyncApp, ms: u64) {
    cx.background_executor()
        .timer(std::time::Duration::from_millis(ms))
        .await;
}

/// Save the window as `{name}.png`; false where the platform can't.
fn capture(window: gpui::AnyWindowHandle, cx: &mut gpui::AsyncApp, dir: &Path, name: &str) -> bool {
    window
        .update(cx, |_, w, cx| {
            w.draw(cx).clear();
            match w.render_to_image() {
                Ok(image) => image.save(dir.join(format!("{name}.png"))).is_ok(),
                Err(error) => {
                    eprintln!("{name}: capture failed: {error:#}");
                    false
                }
            }
        })
        .unwrap_or(false)
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("warn").init();
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    let arg = |flag: &str| std::env::args().skip_while(|a| a != flag).nth(1);
    let capture_dir = arg("--capture").map(PathBuf::from);
    let name = arg("--visual").unwrap_or_else(|| "dash".into());
    let Some(&(name, script)) = VISUALS.iter().find(|(n, _)| *n == name) else {
        anyhow::bail!("no visual named {name}; try dash, bars, misc, or tabs");
    };
    let hover = std::env::var("ZERON_VISUAL_HOVER").ok().and_then(|at| {
        let (x, y) = at.split_once(',')?;
        Some((x.parse::<f32>().ok()?, y.parse::<f32>().ok()?))
    });
    let hold = std::env::var("ZERON_VISUAL_HOLD_MS")
        .ok()
        .and_then(|ms| ms.parse().ok())
        .unwrap_or(6000);
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
            gpui_base::init(cx);
            let settings = settings::UiSettings::default();
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
            let mode = if std::env::var_os("ZERON_PALETTE_LIGHT").is_some() {
                appearance::AppearanceMode::Light
            } else {
                appearance::AppearanceMode::Dark
            };
            appearance::init(
                mode,
                settings.theme_selection,
                settings.accent,
                settings.surface,
                cx,
            );
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            gpui::point(px(0.), px(0.)),
                            size(px(WIDTH), px(HEIGHT)),
                        ))),
                        window_background: theme::Theme::of(cx).window_background_appearance(),
                        ..Default::default()
                    },
                    |_, cx| {
                        let frame = cx.new(|cx| {
                            visuals::VisualFrame::from_script(
                                format!("fixture#{name}").into(),
                                name.into(),
                                script,
                                cx,
                            )
                        });
                        cx.new(|_| Page(frame))
                    },
                )
                .unwrap();
            appearance::reapply_window_background(cx);
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            cx.activate(true);
            let window: gpui::AnyWindowHandle = window.into();
            cx.spawn(async move |cx| {
                let Some(dir) = capture_dir else { return };
                std::fs::create_dir_all(&dir).ok();
                pause(cx, 1000).await;
                let mut captured = capture(window, cx, &dir, name);
                if let Some((x, y)) = hover {
                    // Twice: the first move enters the plot, the second lands.
                    for dx in [0.0, 1.0] {
                        window
                            .update(cx, |_, w, cx| {
                                w.dispatch_event(
                                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                        position: gpui::point(px(x + dx), px(y)),
                                        pressed_button: None,
                                        modifiers: Default::default(),
                                    }),
                                    cx,
                                );
                            })
                            .ok();
                        pause(cx, 300).await;
                    }
                    captured &= capture(window, cx, &dir, &format!("{name}-hover"));
                }
                if !captured {
                    pause(cx, hold).await;
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
