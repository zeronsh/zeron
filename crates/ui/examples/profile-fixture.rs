//! Run with `--features appshots-fixture --example profile-fixture -- <output>`.
//! Uses isolated settings and synthetic accounts; never starts an agent.
//! Where native capture is unavailable, capture each scene named in
//! `<output>/capture-ready` to `<output>/<scene>.png` with the compositor.
use std::{path::PathBuf, time::Duration};

use gpui::{
    AppContext, Bounds, Context, Entity, IntoElement, Render, Window, WindowBounds, WindowOptions,
    div, prelude::*, px, size,
};
use zeron_ui::*;

struct Fixture {
    profile: Entity<settings::profile::ProfilePage>,
    accounts: Entity<settings::accounts::AccountsPage>,
    shell: Entity<shell::Shell>,
    page: u8,
    width: f32,
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(theme::Theme::of(cx).bg)
            .text_color(theme::Theme::of(cx).text)
            .child(
                div()
                    .w(px(self.width))
                    .max_w_full()
                    .h_full()
                    .mx_auto()
                    .child(match self.page {
                        0 => self.profile.clone().into_any_element(),
                        1 => self.accounts.clone().into_any_element(),
                        _ => self.shell.clone().into_any_element(),
                    }),
            )
    }
}

fn main() -> anyhow::Result<()> {
    let output = PathBuf::from(std::env::args().nth(1).expect("output directory"));
    std::fs::create_dir_all(&output)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let data = tempfile::tempdir()?;
    gpui_platform::application().with_assets(icons::Assets).run(move |cx| {
        gpui_tokio::init_from_handle(cx, runtime.handle().clone());
        gpui_base::init(cx);
        let mut settings = settings::UiSettings::default();
        settings.profile = settings::profile::Profile {
            github: Some(settings::profile::GitHubProfile { login: "octocat".into(), name: Some("Mona Octocat".into()), avatar_path: None }),
            ..Default::default()
        };
        settings::init(settings.clone(), data.path(), cx);
        let fonts = typography::register_fonts(cx);
        typography::init(settings.ui_font_family.clone(), settings.ui_font_size, settings.terminal_font_family.clone(), settings.terminal_font_size, settings.code_font_family.clone(), settings.code_font_size, fonts, cx);
        theme_library::init(data.path(), cx);
        appearance::init(appearance::AppearanceMode::Dark, settings.theme_selection, settings.accent, settings.surface, cx);
        history::init(settings.git_history_columns, settings.git_history_column_widths, settings.git_history_column_order, settings.git_history_author_display, cx);
        composer::init(cx, settings.composer_send_behavior);
        terminal::panel::init(cx);
        app_menus::init(cx);
        if std::env::var_os("ZERON_PROFILE_LIVE_GITHUB").is_some() { settings::profile::refresh_github(cx); }
        let state = cx.new(|_| {
            let mut state = state::AppState::new();
            state.connection = zeron_proto::view::ConnectionStatus::Ready;
            state.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
            state.local_device_id = Some("fixture".into());
            state.auto_selected = true;
            state.chats_synced = true;
            state.spaces_synced = true;
            state
        });
        let snapshot = serde_json::from_value(serde_json::json!({"accounts":[
            {"id":"personal", "harness":"claude-code", "email":"mona.personal@example.com", "active":true, "planLabel":"Pro", "switchable":true},
            {"id":"work", "harness":"codex", "email":"mona.work@example.com", "active":true, "planLabel":"Plus", "switchable":true}
        ],"warnings":[]})).unwrap();
        let boot = EngineBootConfig { data_dir: data.path().into(), ipc_port: 0, edge_url: String::new(), edge_token: None, org_id: None, workos_client_id: None, default_harness: HarnessId::Mock };
        let window = cx.open_window(WindowOptions {
            app_id: Some("zeron-profile-fixture".into()),
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(gpui::point(px(0.0), px(0.0)), size(px(1100.0), px(850.0))))),
            ..Default::default()
        }, |_, cx| cx.new(|cx| Fixture {
            profile: cx.new(settings::profile::ProfilePage::new),
            accounts: cx.new(|cx| settings::accounts::AccountsPage::fixture(state.clone(), snapshot, cx)),
            shell: cx.new(|cx| shell::Shell::new(state.clone(), boot, cx)),
            page: 0, width: 768.0,
        })).unwrap();
        state.update(cx, |_, cx| cx.notify());
        cx.activate(true);
        let capture_window: gpui::AnyWindowHandle = window.into();
        cx.spawn(async move |cx| {
            let _keep_data = data;
            let _keep_runtime = runtime;
            cx.background_executor().timer(Duration::from_secs(5)).await;
            for (name, page, width, font, hidden) in [
                ("profile-dark", 0, 768.0, 16, false),
                ("profile-blurred-14", 0, 768.0, 14, true),
                ("profile-zoom-blurred", 0, 500.0, 20, true),
                ("profile-blurred-light", 0, 768.0, 16, true),
                ("accounts-visible", 1, 768.0, 16, false),
                ("accounts-blurred", 1, 768.0, 16, true),
                ("accounts-zoom-blurred", 1, 650.0, 20, true),
                ("sidebar-profile", 2, 1100.0, 20, true),
            ] {
                let destination = output.join(format!("{name}.png"));
                let _ = std::fs::remove_file(&destination);
                window.update(cx, |fixture, window, cx| {
                    appearance::set_mode(if name.ends_with("-light") { appearance::AppearanceMode::Light } else { appearance::AppearanceMode::Dark }, cx);
                    fixture.page = page;
                    fixture.width = width;
                    typography::set_font_size(typography::UiFontSize::ALL.into_iter().find(|size| size.pixels() == font as f32).unwrap(), window, cx);
                    privacy::set_emails_hidden(hidden, cx);
                    cx.notify();
                }).unwrap();
                cx.background_executor().timer(Duration::from_millis(800)).await;
                if page == 2 {
                    capture_window.update(cx, |_, window, cx| {
                        let position = gpui::point(((window.viewport_size().width - px(width)) / 2.0).max(px(0.0)) + px(120.0), window.viewport_size().height - px(38.0));
                        window.dispatch_event(gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                            button: gpui::MouseButton::Left, position, modifiers: Default::default(), click_count: 1, first_mouse: false,
                        }), cx);
                        window.dispatch_event(gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                            button: gpui::MouseButton::Left, position, modifiers: Default::default(), click_count: 1,
                        }), cx);
                    }).unwrap();
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                }
                let image = capture_window.update(cx, |_, window, cx| {
                    window.draw(cx).clear();
                    window.render_to_image()
                }).unwrap();
                match image {
                    Ok(image) => image.save(&destination).unwrap(),
                    Err(_) => {
                        std::fs::write(output.join("capture-ready"), name).unwrap();
                        let deadline = std::time::Instant::now() + Duration::from_secs(45);
                        while !destination.exists() {
                            assert!(std::time::Instant::now() < deadline, "Capture {name} with your compositor to {}", destination.display());
                            cx.background_executor().timer(Duration::from_millis(100)).await;
                        }
                        let _ = std::fs::remove_file(output.join("capture-ready"));
                    }
                }
            }
            cx.update(|cx| cx.quit());
        }).detach();
    });
    Ok(())
}
