//! Settings → MCP servers: how agents start their MCP servers and when idle
//! sessions go to sleep. The preferences live on the engine's device, where
//! the agents launch, and are read and written over `GetMcpSettings` /
//! `SetMcpSettings`. The same reply lists the servers the engine has seen,
//! each with the mode the user chose for it.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Entity, SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use zeron_engine::registry::{DiscoveredMcpServer, McpPrefs, McpSettings};
use zeron_harness::mcp_policy::McpClass;
use zeron_proto::McpServerChoice;
use zeron_rpc::methods;

use super::search::{SearchAnchor, SettingId};
use super::widgets;
use crate::popover::{self, Loadable};
use crate::state::AppState;
use crate::theme::Theme;

/// A channel server stays on in every session, so it has no mode to pick.
const CHANNEL_LABEL: &str = "Always on (channel)";

/// What the page says when the connected engine predates lazy MCP.
const ENGINE_TOO_OLD: &str =
    "This engine is too old for these settings. Update Zeron to change them.";

/// Stale numbers (what is running, how much memory it holds) are read again
/// on the next draw after this long.
const REFRESH_AFTER: Duration = Duration::from_secs(10);

const IDLE_MINUTES: [u32; 7] = [1, 2, 5, 10, 15, 30, 60];
/// Zero is "never".
const SLEEP_MINUTES: [u32; 7] = [0, 5, 10, 15, 30, 60, 120];

/// The mode menu: the class default first, then each choice.
const MODE_CHOICES: [Option<McpServerChoice>; 6] = [
    None,
    Some(McpServerChoice::Off),
    Some(McpServerChoice::Eager),
    Some(McpServerChoice::Lazy),
    Some(McpServerChoice::LazyKeepAlive),
    Some(McpServerChoice::MainOnly),
];

fn mode_label(choice: Option<McpServerChoice>) -> &'static str {
    match choice {
        None => "Default",
        Some(McpServerChoice::Off) => "Off",
        Some(McpServerChoice::Eager) => "Eager",
        Some(McpServerChoice::Lazy) => "Lazy",
        Some(McpServerChoice::LazyKeepAlive) => "Lazy, keep alive",
        Some(McpServerChoice::MainOnly) => "Main chats only",
    }
}

fn mode_detail(choice: Option<McpServerChoice>) -> &'static str {
    match choice {
        None => "by kind",
        Some(McpServerChoice::Off) => "never starts",
        Some(McpServerChoice::Eager) => "starts with the session",
        Some(McpServerChoice::Lazy) => "starts on first use",
        Some(McpServerChoice::LazyKeepAlive) => "stays up once started",
        Some(McpServerChoice::MainOnly) => "off in side chats",
    }
}

fn class_label(class: McpClass) -> &'static str {
    match class {
        McpClass::Remote => "Remote",
        McpClass::Channel => "Channel",
        McpClass::HeavyRuntime => "Heavy runtime",
        McpClass::NativeSmall => "Native",
    }
}

/// The key a server's choice is stored under: `<harness>:<name>`.
fn server_key(server: &DiscoveredMcpServer) -> String {
    format!("{}:{}", server.harness.as_str(), server.name)
}

fn choice_for(prefs: &McpPrefs, server: &DiscoveredMcpServer) -> Option<McpServerChoice> {
    prefs.servers.get(&server_key(server)).copied()
}

/// "412 MiB" from kibibytes.
fn mib_label(kib: u64) -> String {
    format!("{} MiB", (kib + 512) / 1024)
}

/// "3 running now · 412 MiB". The memory is what the running copies were
/// last seen to hold; servers the engine never measured add nothing.
fn running_summary(servers: &[DiscoveredMcpServer]) -> String {
    let running: u32 = servers.iter().map(|server| server.running).sum();
    if running == 0 {
        return "Nothing running now".into();
    }
    let kib: u64 = servers
        .iter()
        .map(|server| u64::from(server.running) * server.last_seen_pss_kib.unwrap_or(0))
        .sum();
    if kib == 0 {
        format!("{running} running now")
    } else {
        format!("{running} running now · {}", mib_label(kib))
    }
}

fn minutes_label(minutes: u32) -> String {
    match minutes {
        0 => "Never".into(),
        1 => "1 minute".into(),
        60 => "1 hour".into(),
        120 => "2 hours".into(),
        minutes => format!("{minutes} minutes"),
    }
}

/// `base` with `current` among it, in order: a hand-edited value still shows.
fn minute_options(base: &[u32], current: u32) -> Vec<u32> {
    let mut options = base.to_vec();
    if !options.contains(&current) {
        options.push(current);
        options.sort_unstable();
    }
    options
}

pub struct McpSettingsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    settings: Loadable<McpSettings>,
    loaded_at: Option<Instant>,
    error: Option<SharedString>,
    load_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    idle_select: widgets::SelectState,
    sleep_select: widgets::SelectState,
    mode_selects: Vec<widgets::SelectState>,
    _observe: Subscription,
}

impl McpSettingsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // Settings can open before the engine connects: load once it does.
        let observe = cx.observe(&state, |this: &mut Self, _, cx| {
            if matches!(this.settings, Loadable::Idle) {
                this.request(cx);
            }
            cx.notify();
        });
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            settings: Loadable::Idle,
            loaded_at: None,
            error: None,
            load_task: None,
            save_task: None,
            idle_select: widgets::SelectState::default(),
            sleep_select: widgets::SelectState::default(),
            mode_selects: Vec::new(),
            _observe: observe,
        };
        page.request(cx);
        page
    }

    /// The connected engine, when it speaks lazy MCP.
    fn engine(&self, cx: &gpui::App) -> Option<crate::state::EngineHandle> {
        self.state
            .read(cx)
            .engine()
            .filter(|engine| {
                engine
                    .engine_info()
                    .supports(zeron_proto::capabilities::MCP_LAZY_V1)
            })
            .cloned()
    }

    /// An engine is connected and it predates lazy MCP.
    fn engine_too_old(&self, cx: &gpui::App) -> bool {
        self.state.read(cx).engine().is_some() && self.engine(cx).is_none()
    }

    fn editable(&self, cx: &gpui::App) -> bool {
        self.engine(cx).is_some() && self.settings.ready().is_some()
    }

    /// `GetMcpSettings`.
    fn request(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        if self.settings.ready().is_none() {
            self.settings = Loadable::Loading;
        }
        self.loaded_at = Some(Instant::now());
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = call(&engine, methods::GET_MCP_SETTINGS, serde_json::json!({})).await;
            this.update(cx, |page, cx| {
                page.load_task = None;
                // A save in flight answers with newer numbers than this read.
                if page.save_task.is_none() {
                    page.settle(result, cx);
                }
            })
            .ok();
        }));
    }

    fn settle(&mut self, result: Result<McpSettings, String>, cx: &mut Context<Self>) {
        match result {
            Ok(settings) => {
                self.settings = Loadable::Ready(settings);
                self.error = None;
            }
            Err(error) if self.settings.ready().is_some() => self.error = Some(error.into()),
            Err(error) => self.settings = Loadable::Error(error),
        }
        cx.notify();
    }

    /// Change the preferences at once and send them. The engine replaces its
    /// whole block, so the reply is what it kept, servers included.
    fn edit(&mut self, change: impl FnOnce(&mut McpPrefs), cx: &mut Context<Self>) {
        if !self.editable(cx) {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let Loadable::Ready(settings) = &mut self.settings else {
            return;
        };
        change(&mut settings.prefs);
        let params = serde_json::to_value(&settings.prefs).unwrap_or_default();
        self.error = None;
        // A newer edit supersedes the reply to an older one.
        self.save_task = Some(cx.spawn(async move |this, cx| {
            let result = call(&engine, methods::SET_MCP_SETTINGS, params).await;
            this.update(cx, |page, cx| {
                page.save_task = None;
                match result {
                    Ok(_) => page.settle(result, cx),
                    // Show the engine's preferences again, not the refused edit.
                    Err(error) => {
                        page.error = Some(error.into());
                        page.request(cx);
                    }
                }
            })
            .ok();
        }));
        cx.notify();
    }

    fn set_lazy(&mut self, lazy: bool, cx: &mut Context<Self>) {
        self.edit(|prefs| prefs.lazy = lazy, cx);
    }

    fn set_idle_minutes(&mut self, minutes: u32, cx: &mut Context<Self>) {
        self.edit(|prefs| prefs.idle_minutes = minutes, cx);
    }

    fn set_session_idle_minutes(&mut self, minutes: u32, cx: &mut Context<Self>) {
        self.edit(|prefs| prefs.session_idle_minutes = minutes, cx);
    }

    fn set_prewarm(&mut self, prewarm: bool, cx: &mut Context<Self>) {
        self.edit(|prefs| prefs.prewarm = prewarm, cx);
    }

    /// Pick a mode for the server in row `ix` (`None` clears the choice).
    fn set_mode(&mut self, ix: usize, choice: Option<McpServerChoice>, cx: &mut Context<Self>) {
        let Some(server) = self
            .settings
            .ready()
            .and_then(|settings| settings.servers.get(ix))
        else {
            return;
        };
        // A channel server's mode is not the user's to set.
        if server.class == McpClass::Channel {
            return;
        }
        let key = server_key(server);
        self.edit(
            |prefs| match choice {
                Some(choice) => {
                    prefs.servers.insert(key, choice);
                }
                None => {
                    prefs.servers.remove(&key);
                }
            },
            cx,
        );
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    /// A switch row: `title`, a line under it, and the switch.
    #[allow(clippy::too_many_arguments)]
    fn switch_row(
        &self,
        theme: &Theme,
        first: bool,
        id: &'static str,
        title: &'static str,
        line: &'static str,
        on: bool,
        enabled: bool,
        anchor: SettingId,
        toggle: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let toggle = std::rc::Rc::new(toggle);
        let on_key = {
            let toggle = toggle.clone();
            cx.listener(move |page, event: &gpui::KeyDownEvent, _, cx| {
                if !event.is_held && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                    toggle(page, cx);
                    cx.stop_propagation();
                }
            })
        };
        widgets::card_row(theme, first)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(theme, title))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().child(line).into_any_element()],
                    )),
            )
            .child(
                widgets::toggle_switch(theme, on, id)
                    .id(SharedString::from(format!("{id}-toggle")))
                    .role(gpui::Role::Switch)
                    .aria_label(title)
                    .aria_toggled(if on {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .when(enabled, |switch| {
                        switch
                            .cursor_pointer()
                            .tab_index(0)
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(move |page, _, _, cx| toggle(page, cx)))
                            .on_key_down(on_key)
                    })
                    .when(!enabled, |switch| switch.opacity(0.5)),
            )
            .search_anchor(anchor)
            .into_any_element()
    }

    /// A minutes row: `title`, a line under it, and a select of `options`.
    #[allow(clippy::too_many_arguments)]
    fn minutes_row(
        &self,
        theme: &Theme,
        id: &'static str,
        title: &'static str,
        line: &'static str,
        options: Vec<u32>,
        current: u32,
        enabled: bool,
        anchor: SettingId,
        reach: fn(&mut Self) -> &mut widgets::SelectState,
        state: &widgets::SelectState,
        pick: fn(&mut Self, u32, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let control = if enabled {
            let picked = options.clone();
            widgets::select(id, title, theme, reach)
                .options(
                    options
                        .iter()
                        .map(|minutes| widgets::SelectOption::new(minutes_label(*minutes))),
                    options
                        .iter()
                        .position(|minutes| *minutes == current)
                        .unwrap_or_default(),
                )
                .width(140.0)
                .on_select(move |page, ix, _, cx| pick(page, picked[ix], cx))
                .render(state, cx)
                .into_any_element()
        } else {
            inert_select(theme, id, title, minutes_label(current)).into_any_element()
        };
        widgets::card_row(theme, false)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(theme, title))
                    .child(widgets::meta_line(
                        theme,
                        vec![div().child(line).into_any_element()],
                    )),
            )
            .child(control)
            .search_anchor(anchor)
            .into_any_element()
    }

    fn server_row(
        &mut self,
        theme: &Theme,
        ix: usize,
        server: &DiscoveredMcpServer,
        prefs: &McpPrefs,
        enabled: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut meta: Vec<AnyElement> = vec![
            div()
                .child(zeron_proto::providers::provider_name(server.harness))
                .into_any_element(),
            div().child(class_label(server.class)).into_any_element(),
        ];
        if let Some(scope) = server.scope.as_deref().filter(|scope| !scope.is_empty()) {
            meta.push(div().child(scope.to_owned()).into_any_element());
        }
        meta.push(
            div()
                .child(if server.running == 0 {
                    "Not running".to_owned()
                } else {
                    format!("{} running", server.running)
                })
                .into_any_element(),
        );
        if let Some(kib) = server.last_seen_pss_kib {
            meta.push(div().child(mib_label(kib)).into_any_element());
        }
        let control: AnyElement = if server.class == McpClass::Channel {
            div()
                .flex_none()
                .text_size(crate::typography::ui_rems(12.5))
                .text_color(theme.text_muted)
                .child(CHANNEL_LABEL)
                .into_any_element()
        } else {
            let chosen = choice_for(prefs, server);
            let selected = MODE_CHOICES
                .iter()
                .position(|choice| *choice == chosen)
                .unwrap_or_default();
            let id = format!("mcp-server-mode-{ix}");
            let label = format!("{}: mode", server.name);
            if enabled {
                widgets::select(id, label, theme, move |page: &mut Self| {
                    &mut page.mode_selects[ix]
                })
                .options(
                    MODE_CHOICES.iter().map(|choice| {
                        widgets::SelectOption::new(mode_label(*choice)).detail(mode_detail(*choice))
                    }),
                    selected,
                )
                .width(168.0)
                .on_select(move |page, picked, _, cx| page.set_mode(ix, MODE_CHOICES[picked], cx))
                .render(&self.mode_selects[ix], cx)
                .into_any_element()
            } else {
                inert_select(theme, id, label, mode_label(chosen).to_owned()).into_any_element()
            }
        };
        widgets::card_row(theme, ix == 0)
            .child(
                div()
                    .flex_1()
                    .min_w(px(160.0))
                    .child(widgets::row_title(theme, server.name.clone()))
                    .child(widgets::meta_line(theme, meta)),
            )
            .child(control)
            .into_any_element()
    }
}

/// A select that can't be opened: the engine can't take the change.
fn inert_select(
    theme: &Theme,
    id: impl Into<SharedString>,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> impl IntoElement {
    let value: SharedString = value.into();
    widgets::select_trigger(theme, id, false)
        .opacity(0.5)
        .cursor_default()
        .aria_label(format!("{}: {value}", label.into()))
        .child(widgets::select_value(value))
        .child(widgets::select_chevron(theme, false))
}

/// One engine call whose reply is the device's MCP settings.
async fn call(
    engine: &crate::state::EngineHandle,
    method: &'static str,
    params: serde_json::Value,
) -> Result<McpSettings, String> {
    engine
        .client()
        .call(method, params)
        .await
        .map_err(|error| error.to_string())
        .and_then(|value| {
            serde_json::from_value::<McpSettings>(value).map_err(|error| error.to_string())
        })
}

impl popover::ScrollRailHost for McpSettingsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for McpSettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        crate::toast::take_error(&mut self.error, cx);
        if self.load_task.is_none()
            && self.save_task.is_none()
            && self
                .loaded_at
                .is_some_and(|at| at.elapsed() > REFRESH_AFTER)
        {
            self.request(cx);
        }
        let theme = Theme::of(cx).for_settings_surface();
        let enabled = self.editable(cx);
        let too_old = self.engine_too_old(cx);
        let (prefs, servers) = self
            .settings
            .ready()
            .map(|settings| (settings.prefs.clone(), settings.servers.clone()))
            .unwrap_or_default();
        if self.mode_selects.len() < servers.len() {
            self.mode_selects
                .resize_with(servers.len(), widgets::SelectState::default);
        }

        let mut behavior = widgets::section_card(&theme).child(self.switch_row(
            &theme,
            true,
            "mcp-lazy",
            "Lazy MCP servers",
            "Start a server when an agent first calls it, and stop it after it sits idle.",
            prefs.lazy,
            enabled,
            SettingId::McpLazy,
            |page, cx| {
                let lazy = page.settings.ready().is_some_and(|s| !s.prefs.lazy);
                page.set_lazy(lazy, cx)
            },
            cx,
        ));
        if prefs.lazy {
            behavior = behavior.child(self.minutes_row(
                &theme,
                "mcp-idle-minutes",
                "Stop idle servers after",
                "How long a started server may go unused.",
                minute_options(&IDLE_MINUTES, prefs.idle_minutes),
                prefs.idle_minutes,
                enabled,
                SettingId::McpIdleMinutes,
                |page| &mut page.idle_select,
                &self.idle_select,
                Self::set_idle_minutes,
                cx,
            ));
        }
        behavior = behavior
            .child(self.minutes_row(
                &theme,
                "mcp-sleep-minutes",
                "Put idle sessions to sleep after",
                "A sleeping session holds no memory and resumes on your next message.",
                minute_options(&SLEEP_MINUTES, prefs.session_idle_minutes),
                prefs.session_idle_minutes,
                enabled,
                SettingId::SessionSleep,
                |page| &mut page.sleep_select,
                &self.sleep_select,
                Self::set_session_idle_minutes,
                cx,
            ))
            .child(self.switch_row(
                &theme,
                false,
                "mcp-prewarm",
                "Wake sessions early",
                "Resume a sleeping session as you open its composer or start typing.",
                prefs.prewarm,
                enabled,
                SettingId::Prewarm,
                |page, cx| {
                    let prewarm = page.settings.ready().is_some_and(|s| !s.prefs.prewarm);
                    page.set_prewarm(prewarm, cx)
                },
                cx,
            ));

        let server_rows: Vec<AnyElement> = servers
            .iter()
            .enumerate()
            .map(|(ix, server)| self.server_row(&theme, ix, server, &prefs, enabled, cx))
            .collect();
        let servers_card = widgets::section_card(&theme).mt(px(0.0)).children(
            if server_rows.is_empty() {
                vec![
                    widgets::card_row(&theme, true)
                        .child(
                            div()
                                .text_size(crate::typography::ui_rems(13.0))
                                .text_color(theme.text_muted)
                                .child(match &self.settings {
                                    _ if too_old => {
                                        SharedString::from("Not available on this engine.")
                                    }
                                    Loadable::Error(error) => SharedString::from(error.clone()),
                                    Loadable::Ready(_) => SharedString::from(
                                        "No servers yet. They appear after an agent starts a session.",
                                    ),
                                    _ => SharedString::from("Loading…"),
                                }),
                        )
                        .into_any_element(),
                ]
            } else {
                server_rows
            },
        );
        let servers_block = div()
            .mt(px(32.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_baseline()
                    .justify_between()
                    .child(widgets::section_label(&theme, "Servers"))
                    .child(
                        div()
                            .px(px(8.0))
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child(running_summary(&servers)),
                    ),
            )
            .child(servers_card);

        let scrollbar = popover::rail(self, "mcp-settings-page-scrollbar", &theme, cx);
        div()
            .id("mcp-settings-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("mcp-settings-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(&theme, "MCP servers", None))
                                .child(widgets::page_subtitle(
                                    &theme,
                                    "Agents start most MCP servers only when they use them, and \
                                     idle sessions sleep, which frees memory.",
                                ))
                                .when(too_old, |column| {
                                    column.child(
                                        widgets::warning_strip(&theme, ENGINE_TOO_OLD)
                                            .id("mcp-engine-too-old")
                                            .debug_selector(|| "mcp-engine-too-old".into()),
                                    )
                                })
                                .child(behavior)
                                .child(servers_block),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .child(widgets::page_reveal(&self.scroll.scroll))
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
    use crate::state::EngineHandle;
    use std::sync::{Arc, Mutex};
    use zeron_proto::HarnessId;

    fn server(name: &str, class: McpClass, running: u32, kib: Option<u64>) -> DiscoveredMcpServer {
        DiscoveredMcpServer {
            harness: HarnessId::ClaudeCode,
            name: name.into(),
            class,
            scope: Some("user".into()),
            last_seen_pss_kib: kib,
            running,
        }
    }

    fn fixture_settings() -> McpSettings {
        McpSettings {
            prefs: McpPrefs::default(),
            servers: vec![
                server("playwright", McpClass::HeavyRuntime, 2, Some(200 * 1024)),
                server("telegram", McpClass::Channel, 1, Some(30 * 1024)),
                server("notes", McpClass::NativeSmall, 0, None),
            ],
        }
    }

    type Calls = Arc<Mutex<Vec<(String, serde_json::Value)>>>;

    /// An engine that serves `fixture_settings` and keeps what is sent to it.
    fn scripted() -> (Arc<ScriptedRpc>, Calls) {
        let calls: Calls = Default::default();
        let log = calls.clone();
        let current = Arc::new(Mutex::new(fixture_settings()));
        let rpc = ScriptedRpc::new(move |method, params| {
            let (log, current) = (log.clone(), current.clone());
            async move {
                log.lock().unwrap().push((method.clone(), params.clone()));
                if method == methods::SET_MCP_SETTINGS {
                    current.lock().unwrap().prefs = serde_json::from_value(params).unwrap();
                }
                zeron_rpc::RpcReply::value(&*current.lock().unwrap())
            }
        });
        (rpc, calls)
    }

    fn open_page(
        cx: &mut gpui::TestAppContext,
        engine: Option<EngineHandle>,
    ) -> (
        Entity<McpSettingsPage>,
        &mut gpui::VisualTestContext,
        tempfile::TempDir,
    ) {
        let dir = fixture::settings(cx, Default::default());
        cx.update(|cx| gpui_base::init(cx));
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = fixture::state(cx, None);
            if let Some(engine) = engine {
                state.update(cx, |state, _| state.set_test_engine(engine));
            }
            McpSettingsPage::new(state, cx)
        });
        (page, cx, dir)
    }

    fn calls_to(calls: &Calls, method: &str) -> Vec<serde_json::Value> {
        calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(called, _)| called == method)
            .map(|(_, params)| params.clone())
            .collect()
    }

    #[test]
    fn running_summary_counts_copies_and_memory() {
        assert_eq!(running_summary(&[]), "Nothing running now");
        assert_eq!(
            running_summary(&fixture_settings().servers),
            "3 running now · 430 MiB"
        );
        // Unmeasured servers add copies but no memory.
        assert_eq!(
            running_summary(&[server("a", McpClass::Remote, 2, None)]),
            "2 running now"
        );
    }

    #[test]
    fn minute_options_keep_a_hand_edited_value() {
        assert_eq!(minute_options(&[5, 10], 7), [5, 7, 10]);
        assert_eq!(minute_options(&[5, 10], 10), [5, 10]);
        assert_eq!(minutes_label(0), "Never");
        assert_eq!(minutes_label(60), "1 hour");
    }

    #[gpui::test]
    fn the_pane_lists_a_row_per_server_and_the_running_summary(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let (rpc, calls) = scripted();
        let (page, cx, _dir) = open_page(cx, Some(EngineHandle::from_test_client(rpc.client())));
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| page.settings.ready().is_some())
        });
        assert_eq!(calls_to(&calls, methods::GET_MCP_SETTINGS).len(), 1);
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        let drawn = crate::settings::widgets::take_row_titles();
        for expected in ["playwright", "telegram", "notes"] {
            assert!(
                drawn.iter().any(|title| title == expected),
                "{expected} missing from {drawn:?}"
            );
        }
        for id in ["mcp-server-mode-0", "mcp-server-mode-2"] {
            assert!(cx.debug_bounds(id).is_some(), "{id} has a mode menu");
        }
        assert!(
            cx.debug_bounds("mcp-server-mode-1").is_none(),
            "a channel server's mode is not editable"
        );
        assert!(cx.debug_bounds("mcp-engine-too-old").is_none());
        page.read_with(cx, |page, _| {
            let settings = page.settings.ready().unwrap();
            assert_eq!(
                running_summary(&settings.servers),
                "3 running now · 430 MiB"
            );
            assert_eq!(CHANNEL_LABEL, "Always on (channel)");
        });
    }

    #[gpui::test]
    fn changing_a_mode_sends_the_whole_prefs_through_set_mcp_settings(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let (rpc, calls) = scripted();
        let (page, cx, _dir) = open_page(cx, Some(EngineHandle::from_test_client(rpc.client())));
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| page.settings.ready().is_some())
        });
        page.update(cx, |page, cx| {
            page.set_mode(0, Some(McpServerChoice::LazyKeepAlive), cx);
            // The row moves before the engine answers.
            let prefs = &page.settings.ready().unwrap().prefs;
            assert_eq!(
                prefs.servers.get("claude-code:playwright"),
                Some(&McpServerChoice::LazyKeepAlive)
            );
        });
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| page.save_task.is_none())
        });
        let sent = calls_to(&calls, methods::SET_MCP_SETTINGS);
        assert_eq!(sent.len(), 1);
        assert_eq!(
            sent[0]["servers"]["claude-code:playwright"],
            "lazy-keep-alive"
        );
        // The rest of the prefs travel with it: the engine replaces the block.
        assert_eq!(sent[0]["lazy"], true);
        assert_eq!(sent[0]["idleMinutes"], 10);
        assert_eq!(sent[0]["sessionIdleMinutes"], 10);
        assert_eq!(sent[0]["prewarm"], true);

        // Default clears the choice; the channel server takes none.
        page.update(cx, |page, cx| {
            page.set_mode(1, Some(McpServerChoice::Off), cx);
            page.set_mode(0, None, cx);
        });
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| page.save_task.is_none())
        });
        let sent = calls_to(&calls, methods::SET_MCP_SETTINGS);
        assert_eq!(sent.len(), 2);
        assert!(sent[1]["servers"].as_object().is_none_or(|s| s.is_empty()));
    }

    #[gpui::test]
    fn the_global_controls_write_their_own_fields(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let (rpc, calls) = scripted();
        let (page, cx, _dir) = open_page(cx, Some(EngineHandle::from_test_client(rpc.client())));
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| page.settings.ready().is_some())
        });
        for step in 0..4 {
            page.update(cx, |page, cx| match step {
                0 => page.set_lazy(false, cx),
                1 => page.set_idle_minutes(30, cx),
                2 => page.set_session_idle_minutes(0, cx),
                _ => page.set_prewarm(false, cx),
            });
            rpc.settle(cx, &runtime, |cx| {
                page.read_with(cx, |page, _| page.save_task.is_none())
            });
        }
        let sent = calls_to(&calls, methods::SET_MCP_SETTINGS);
        let last = sent.last().unwrap();
        assert_eq!(sent.len(), 4);
        assert_eq!(last["lazy"], false);
        assert_eq!(last["idleMinutes"], 30);
        assert_eq!(last["sessionIdleMinutes"], 0);
        assert_eq!(last["prewarm"], false);
    }

    #[gpui::test]
    fn an_engine_without_lazy_mcp_gets_a_notice_and_disabled_controls(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let (rpc, calls) = scripted();
        let old = EngineHandle::from_test_client(rpc.client()).with_test_capabilities(&[]);
        let (page, cx, _dir) = open_page(cx, Some(old));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("mcp-engine-too-old").is_some());
        page.update(cx, |page, cx| {
            assert!(!page.editable(cx));
            page.set_lazy(false, cx);
            page.set_mode(0, Some(McpServerChoice::Off), cx);
        });
        cx.run_until_parked();
        assert!(
            calls.lock().unwrap().is_empty(),
            "an engine that predates the settings is never asked for them: {:?}",
            calls.lock().unwrap()
        );
    }
}
