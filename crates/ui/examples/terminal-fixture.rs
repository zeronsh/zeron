//! Offscreen terminal render: a canned ANSI stream (box drawing, blocks,
//! braille, Powerline, ligatures, emoji clusters, combining marks, CJK, RTL)
//! through the real terminal panel, no engine or PTY. Writes
//! `terminal-dark.png` and `terminal-light.png` to the output directory.
use gpui::{
    AppContext, AsyncApp, Bounds, Context, Entity, IntoElement, Render, Window, WindowBounds,
    WindowOptions, point, px, size,
};
use std::{path::PathBuf, time::Duration};
use zeron_ui::*;

const CHAT: &str = "fixture";

/// The window's root: the panel is drawn as a child, so the capture can
/// update the window while prepaint updates the panel.
struct Fixture(Entity<terminal::panel::TerminalPanel>);

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.0.clone()
    }
}

fn sample() -> String {
    let lines = [
        "\x1b[1mBox drawing\x1b[0m  ╭──────┬──────╮ ┏━━━━┳━━━━┓ ╔════╦════╗ ┌─┐",
        "             │ thin │ ╌╌╌╌ │ ┃ hv ┃ ━━ ┃ ║ db ║ ══ ║ └─┘",
        "             ├──────┼──────┤ ┣━━━━╋━━━━┫ ╠════╬════╣ ╱╲╳",
        "             ╰──────┴──────╯ ┗━━━━┻━━━━┛ ╚════╩════╝ ╲╱",
        "\x1b[1mBlocks\x1b[0m       █▓▒░ ▀▄▌▐ ▁▂▃▄▅▆▇█ ▏▎▍▌▋▊▉ ▖▗▘▙▚▛▜▝▞▟ 🬀🬁🬂🬃 🭨🭩🭪🭫",
        "\x1b[1mBraille\x1b[0m      ⠁⠃⠇⡇⣇⣧⣷⣿ ⣀⣤⣶⣿⠿⠛⠉ ⢀⡠⠔⠊⠑⠢⢄⡀  \x1b[1mShapes\x1b[0m ◢◣◤◥ ●○◐◑ ■□▪▫",
        "\x1b[1mPowerline\x1b[0m    \x1b[30;44m \u{e0a0} main \x1b[34;42m\u{e0b0}\x1b[30m ~/zeron \x1b[32;49m\u{e0b0}\x1b[0m \x1b[35m\u{e0b6}\x1b[30;45m 3.12 \x1b[0;35m\u{e0b4}\x1b[0m \x1b[36m\u{e0b2}\x1b[30;46m ok \x1b[0;36m\u{e0b0} \u{e0b1} \u{e0b3} \u{e0b8} \u{e0bc}\x1b[0m",
        "\x1b[1mLigatures\x1b[0m    -> => != === !== <= >= |> <| :: ++ -- // /* */ www ..= ~>",
        "\x1b[1mEmoji\x1b[0m        🇺🇦 🇯🇵 👍🏽 👩🏾‍💻 👨‍👩‍👧 1️⃣ #️⃣ ❤️ ✅ 🚀|after",
        "\x1b[1mCombining\x1b[0m    e\u{301} n\u{303} a\u{308} o\u{302}\u{323} Z\u{335}a\u{336}|after",
        "\x1b[1mCJK\x1b[0m          漢字テスト한국어|after  \x1b[1mRTL\x1b[0m مرحبا بالعالم · שלום עולם|after",
        "\x1b[1mStyles\x1b[0m       \x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[4munder\x1b[0m \x1b[2mdim\x1b[0m \x1b[7minverse\x1b[0m \x1b[9mstrike\x1b[0m \x1b[38;5;208m208\x1b[0m \x1b[38;2;120;200;255mrgb\x1b[0m \x1b[48;5;236m bg236 \x1b[0m",
        "\x1b[1mANSI\x1b[0m         \x1b[30m██\x1b[31m██\x1b[32m██\x1b[33m██\x1b[34m██\x1b[35m██\x1b[36m██\x1b[37m██\x1b[90m██\x1b[91m██\x1b[92m██\x1b[93m██\x1b[94m██\x1b[95m██\x1b[96m██\x1b[97m██\x1b[0m",
        "\x1b[1mNerd icons\x1b[0m   \u{f07c} \u{e7a8} \u{f418} \u{f02a2} \u{e606} \u{f121} \u{e702}|after",
        "$ codex --yolo && echo done",
    ];
    lines.join("\r\n")
}

async fn pause(cx: &mut AsyncApp) {
    cx.background_executor()
        .timer(Duration::from_millis(300))
        .await;
}

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let temp = tempfile::tempdir()?;
    let data = temp.path().to_path_buf();
    gpui_platform::application()
        .with_assets(icons::Assets)
        .run(move |cx| {
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
            terminal::panel::init(cx);
            let state = cx.new(|_| {
                let mut state = state::AppState::new();
                state.selected_chat = Some(CHAT.into());
                state
            });
            let mut key = 0;
            let window = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                            point(px(40.0), px(40.0)),
                            size(px(980.0), px(380.0)),
                        ))),
                        ..Default::default()
                    },
                    |window, cx| {
                        let panel = cx.new(|cx| {
                            let mut panel = terminal::panel::TerminalPanel::new_embedded(state, cx);
                            key = panel.reserve_tab_for_chat(CHAT.into(), "Fixture", cx);
                            panel.set_open(true, cx);
                            panel
                        });
                        window.focus(&panel.read(cx).focus_handle(), cx);
                        cx.new(|_| Fixture(panel))
                    },
                )
                .unwrap();
            cx.activate(true);
            cx.spawn(async move |cx| {
                let result: anyhow::Result<()> = async {
                    // The first frame sizes the grid; feed after it so nothing reflows.
                    pause(cx).await;
                    window.update(cx, |fixture, _, cx| {
                        fixture.0.update(cx, |panel, cx| {
                            panel.feed_tab(CHAT, key, sample().as_bytes(), cx)
                        })
                    })?;
                    for (mode, name) in [
                        (appearance::AppearanceMode::Dark, "terminal-dark.png"),
                        (appearance::AppearanceMode::Light, "terminal-light.png"),
                    ] {
                        window.update(cx, |_, _, cx| appearance::set_mode(mode, cx))?;
                        pause(cx).await;
                        // Untyped, so the root view is not leased while it draws.
                        let any: gpui::AnyWindowHandle = window.into();
                        any.update(cx, |_, window, cx| -> anyhow::Result<()> {
                            window.draw(cx).clear();
                            window.render_to_image()?.save(output.join(name))?;
                            Ok(())
                        })??;
                    }
                    Ok(())
                }
                .await;
                if let Err(error) = result {
                    eprintln!("terminal fixture failed: {error:#}");
                    std::process::exit(1);
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    Ok(())
}
