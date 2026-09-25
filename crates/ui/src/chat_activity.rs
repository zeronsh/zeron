//! Main-conversation activity, using the model picker's tabs, scoped search,
//! compact scrolling list and pinned action tray.

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::state::AppState;
use crate::theme::Theme;
use crate::{loaders, popover};
use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, Context, Entity, EntityId, EventEmitter, FocusHandle, Focusable, MouseButton,
    ScrollHandle, SharedString, Subscription, UniformListScrollHandle, Window, div, prelude::*, px,
};
use std::hash::{Hash, Hasher};
use zeron_doc::{MessagePart, SubagentStatus};
use zeron_proto::{Chat, ChatIndicator};

const MENU_WIDTH: f32 = 304.0;
const ROW_HEIGHT: f32 = 32.0;
const LIST_MIN_HEIGHT: f32 = 96.0;
const LIST_MAX_HEIGHT: f32 = 216.0;
// Two 40px header bands, two 32px actions, tray padding and card borders.
const MENU_CHROME_HEIGHT: f32 = 154.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ActivityTab {
    #[default]
    Subagents,
    Chats,
}
impl ActivityTab {
    fn key(self) -> &'static str {
        match self {
            Self::Subagents => "subagents",
            Self::Chats => "chats",
        }
    }
    fn label(self) -> &'static str {
        match self {
            Self::Subagents => "Subagents",
            Self::Chats => "Side chats",
        }
    }
    fn icon(self) -> &'static str {
        match self {
            Self::Subagents => icons::BOT,
            Self::Chats => icons::CHAT_ROUND_LINE,
        }
    }
    fn placeholder(self) -> &'static str {
        match self {
            Self::Subagents => "Search subagents…",
            Self::Chats => "Search side chats…",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ChatActivityEvent {
    OpenSubagent {
        doc_id: String,
        title: String,
        frozen: bool,
    },
    OpenChildChat(String),
    ChildChatContextMenu {
        chat_id: String,
        position: gpui::Point<gpui::Pixels>,
    },
    NewChildChat,
    ForkChat,
}

#[derive(Clone)]
enum ActivityRow {
    Subagent(SubagentRow),
    Chat(ChildChatRow),
}
impl ActivityRow {
    fn title(&self) -> &str {
        match self {
            Self::Subagent(row) => row.title.as_ref(),
            Self::Chat(row) => row.title.as_ref(),
        }
    }
    fn event(&self) -> ChatActivityEvent {
        match self {
            Self::Subagent(row) => ChatActivityEvent::OpenSubagent {
                doc_id: row.doc_id.clone(),
                title: row.title.to_string(),
                frozen: row.frozen(),
            },
            Self::Chat(row) => ChatActivityEvent::OpenChildChat(row.chat_id.clone()),
        }
    }
}

pub(crate) struct ChatActivity {
    state: Entity<AppState>,
    chat_id: String,
    tab: ActivityTab,
    active: usize,
    fingerprint: u64,
    search: Entity<ComposerInput>,
    scroll: UniformListScrollHandle,
    scrollbar: popover::MenuScrollbarState,
    menu: popover::Popup<()>,
    focus: FocusHandle,
    composer_focus: FocusHandle,
    focus_pending: bool,
    child_overlay_open: bool,
    _observe: Subscription,
    _search_events: Subscription,
}
impl EventEmitter<ChatActivityEvent> for ChatActivity {}

impl ChatActivity {
    pub(crate) fn new(
        state: Entity<AppState>,
        composer_focus: FocusHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let chat_id = state.read(cx).selected_chat.clone().unwrap_or_default();
        let search = cx.new(|cx| {
            ComposerInput::with_context(ActivityTab::Subagents.placeholder(), "PaletteSearch", cx)
                .with_accessibility_role(gpui::Role::SearchInput)
        });
        let search_events = cx.subscribe(&search, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Edited => {
                this.reset_list();
                cx.notify();
            }
            ComposerInputEvent::Submitted | ComposerInputEvent::ModifiedSubmitted => {
                this.activate(cx)
            }
            _ => {}
        });
        let observe = cx.observe(&state, |this: &mut Self, state, cx| {
            let chat_id = state.read(cx).selected_chat.clone().unwrap_or_default();
            if this.chat_id != chat_id {
                this.chat_id = chat_id;
                this.menu = popover::Popup::default();
                this.tab = ActivityTab::Subagents;
                this.search.update(cx, |input, cx| input.set_text("", cx));
                this.reset_list();
                this.focus_pending = false;
                this.child_overlay_open = false;
                cx.notify();
            }
            let fingerprint = fingerprint(state.read(cx), &this.chat_id, Utc::now());
            if fingerprint != this.fingerprint {
                this.fingerprint = fingerprint;
                let count = this.rows(cx).len();
                this.active = this.active.min(count + 1);
                cx.notify();
            }
        });
        Self {
            state,
            chat_id,
            tab: ActivityTab::Subagents,
            active: 0,
            fingerprint: 0,
            search,
            scroll: UniformListScrollHandle::new(),
            scrollbar: Default::default(),
            menu: Default::default(),
            focus: cx.focus_handle(),
            composer_focus,
            focus_pending: false,
            child_overlay_open: false,
            _observe: observe,
            _search_events: search_events,
        }
    }

    pub(crate) fn is_open(&self) -> bool {
        self.menu.is_open()
    }
    pub(crate) fn set_child_overlay_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.child_overlay_open != open {
            self.child_overlay_open = open;
            cx.notify();
        }
    }
    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.focus_pending = false;
        if self.menu.begin_close() {
            popover::reap_popup(cx, |activity: &mut Self| &mut activity.menu);
            cx.notify();
        }
    }
    fn scroll_base(&self) -> ScrollHandle {
        self.scroll.0.borrow().base_handle.clone()
    }
    fn reset_list(&mut self) {
        self.active = 0;
        popover::reset_menu_scroll(&self.scroll_base(), &mut self.scrollbar);
    }
    fn set_tab(&mut self, tab: ActivityTab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.reset_list();
        self.search
            .update(cx, |input, cx| input.set_placeholder(tab.placeholder(), cx));
        cx.notify();
    }
    fn open(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let agents = subagent_rows(state, &self.chat_id).len();
        let chats = child_chat_rows(state, &self.chat_id, Utc::now()).len();
        let tab = match (self.tab, agents, chats) {
            (ActivityTab::Subagents, 0, 1..) => ActivityTab::Chats,
            (ActivityTab::Chats, 1.., 0) => ActivityTab::Subagents,
            _ => self.tab,
        };
        self.search.update(cx, |input, cx| input.set_text("", cx));
        self.set_tab(tab, cx);
        self.menu.open(());
        self.focus_pending = true;
        cx.notify();
    }
    fn rows(&self, cx: &gpui::App) -> Vec<ActivityRow> {
        let state = self.state.read(cx);
        let rows: Vec<_> = match self.tab {
            ActivityTab::Subagents => subagent_rows(state, &self.chat_id)
                .into_iter()
                .map(ActivityRow::Subagent)
                .collect(),
            ActivityTab::Chats => child_chat_rows(state, &self.chat_id, Utc::now())
                .into_iter()
                .map(ActivityRow::Chat)
                .collect(),
        };
        let names: Vec<_> = rows.iter().map(ActivityRow::title).collect();
        popover::filter_indices(self.search.read(cx).text().trim(), &names)
            .into_iter()
            .map(|ix| rows[ix].clone())
            .collect()
    }
    fn activate(&mut self, cx: &mut Context<Self>) {
        if !self.is_open() || self.child_overlay_open {
            return;
        }
        let rows = self.rows(cx);
        let event = if let Some(row) = rows.get(self.active) {
            row.event()
        } else if self.active == rows.len() {
            ChatActivityEvent::NewChildChat
        } else {
            ChatActivityEvent::ForkChat
        };
        self.dismiss(cx);
        cx.emit(event);
    }
    fn on_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.child_overlay_open || !self.is_open() {
            return;
        }
        match popover::classify_key(
            &event.keystroke.key,
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        ) {
            popover::MenuKey::Escape => {
                self.dismiss(cx);
                window.focus(&self.composer_focus, cx);
            }
            key @ (popover::MenuKey::Up | popover::MenuKey::Down) => {
                let count = self.rows(cx).len();
                self.active = popover::menu_step(
                    Some(self.active),
                    count + 2,
                    if key == popover::MenuKey::Up { -1 } else { 1 },
                )
                .unwrap_or(0);
                if self.active < count {
                    self.scroll
                        .scroll_to_item(self.active, gpui::ScrollStrategy::Nearest);
                }
                cx.notify();
            }
            popover::MenuKey::Enter => self.activate(cx),
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_menu(
        &mut self,
        viewport_height: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let counts = [
            subagent_rows(state, &self.chat_id).len(),
            child_chat_rows(state, &self.chat_id, Utc::now()).len(),
        ];
        let mut tabs = div()
            .flex_none()
            .h(px(40.0))
            .px(px(popover::CARD_INSET))
            .border_b_1()
            .border_color(crate::theme::hairline(0.08))
            .flex()
            .items_center()
            .gap(px(2.0));
        for (tab, count) in [ActivityTab::Subagents, ActivityTab::Chats]
            .into_iter()
            .zip(counts)
        {
            let selected = self.tab == tab;
            let id = format!("chat-activity-tab-{}", tab.key());
            tabs =
                tabs.child(
                    div()
                        .id(SharedString::from(id.clone()))
                        .debug_selector(move || id.clone())
                        .role(gpui::Role::Button)
                        .aria_label(SharedString::from(format!("{} ({count})", tab.label())))
                        .relative()
                        .flex_1()
                        .min_w_0()
                        .h(px(32.0))
                        .px(px(8.0))
                        .rounded(px(popover::MENU_ITEM_RADIUS))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .cursor_pointer()
                        .text_color(if selected {
                            theme.text
                        } else {
                            theme.text_muted
                        })
                        .when(!selected, |tab| {
                            tab.hover(|s| s.bg(crate::theme::ink(0.06)))
                        })
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.set_tab(tab, cx);
                            window.focus(&this.search.focus_handle(cx), cx);
                        }))
                        .child(icon(tab.icon()).size(px(14.0)).flex_none().text_color(
                            if selected {
                                theme.text
                            } else {
                                theme.text_muted
                            },
                        ))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::ui_rems(12.0))
                                .child(tab.label()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_size(crate::typography::ui_rems(10.0))
                                .text_color(theme.text_muted)
                                .child(count.to_string()),
                        )
                        .when(selected, |tab| {
                            tab.child(popover::tab_indicator(theme.accent))
                        }),
                );
        }
        let search = div()
            .flex_none()
            .h(px(40.0))
            .px(px(10.0))
            .border_b_1()
            .border_color(crate::theme::hairline(0.08))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                icon(icons::MAGNIFER)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::typography::ui_rems(13.0))
                    .child(self.search.clone()),
            );
        let rows = self.rows(cx);
        let list_height = (rows.len() as f32 * ROW_HEIGHT + 2.0 * popover::CARD_INSET)
            .clamp(LIST_MIN_HEIGHT, LIST_MAX_HEIGHT)
            .min((viewport_height - 100.0 - MENU_CHROME_HEIGHT).max(ROW_HEIGHT));
        let row_count = rows.len();
        let body = if rows.is_empty() {
            let copy = if !self.search.read(cx).text().trim().is_empty() {
                "No matches found"
            } else if self.tab == ActivityTab::Subagents {
                "No subagents yet"
            } else {
                "No side chats yet"
            };
            div()
                .size_full()
                .px(px(12.0))
                .py(px(12.0))
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(12.5))
                        .text_color(theme.text_muted)
                        .child(copy),
                )
                .when(self.search.read(cx).text().trim().is_empty(), |el| {
                    el.child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(if self.tab == ActivityTab::Subagents {
                                "Agents created in this conversation appear here."
                            } else {
                                "Start a side chat or fork this conversation below."
                            }),
                    )
                })
                .into_any_element()
        } else {
            let entity = cx.entity();
            let list =
                gpui::uniform_list("chat-activity-list", rows.len(), move |range, _, app| {
                    entity.update(app, |this, cx| {
                        range
                            .filter_map(|ix| rows.get(ix).map(|row| this.render_row(ix, row, cx)))
                            .collect::<Vec<_>>()
                    })
                })
                .size_full()
                .px(px(popover::CARD_INSET))
                .track_scroll(&self.scroll);
            popover::faded_menu_list(&self.scroll_base(), list).into_any_element()
        };
        let rail = popover::rail(self, "chat-activity-scrollbar", theme, cx);
        let list = div()
            .id("chat-activity-list-host")
            .relative()
            .flex_none()
            .h(px(list_height))
            .py(px(popover::CARD_INSET))
            .bg(crate::theme::ink(0.02))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.scrollbar.set_list_hovered(*hovered) {
                    cx.notify();
                }
            }))
            .child(body)
            .children(rail);
        let mut tray = div()
            .flex_none()
            .border_t_1()
            .border_color(crate::theme::hairline(0.08))
            .px(px(popover::CARD_INSET))
            .py(px(4.0))
            .flex()
            .flex_col()
            .gap(px(2.0));
        for (ix, (id, label, glyph, event)) in [
            (
                "chat-activity-new",
                "New side chat",
                icons::PLUS,
                ChatActivityEvent::NewChildChat,
            ),
            (
                "chat-activity-fork",
                "Fork this chat",
                icons::GIT_BRANCH,
                ChatActivityEvent::ForkChat,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            tray = tray.child(
                popover::menu_row_nav(theme, false, self.active == row_count + ix, id)
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .h(px(30.0))
                    .py(px(0.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.dismiss(cx);
                        cx.emit(event.clone());
                    }))
                    .child(icon(glyph).size(px(14.0)).text_color(theme.text_muted))
                    .child(div().flex_1().child(label)),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(tabs)
            .child(search)
            .child(list)
            .child(tray)
            .into_any_element()
    }

    fn render_row(&mut self, ix: usize, row: &ActivityRow, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let (id, status, time, pr) = match row {
            ActivityRow::Subagent(row) => (
                format!("chat-activity-subagent-{}", row.doc_id),
                row.indicator(),
                SharedString::from(zeron_proto::view::format_time_ago(
                    row.spawned_at,
                    Utc::now(),
                )),
                None,
            ),
            ActivityRow::Chat(row) => (
                format!("chat-activity-chat-{}", row.chat_id),
                row.status,
                row.time_ago.clone(),
                row.change_request.clone(),
            ),
        };
        let glyph = status_glyph(id.clone(), status, cx.entity_id(), &theme, cx);
        let event = row.event();
        let query = self.search.read(cx).text().trim().to_owned();
        let label = SharedString::from(row.title().to_owned());
        let mut item = div()
            .id(SharedString::from(id.clone()))
            .debug_selector(move || id.clone())
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!("Open {}", row.title())))
            .h(px(30.0))
            .px(px(8.0))
            .rounded(px(popover::MENU_ITEM_RADIUS))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .when(self.active == ix, |el| el.bg(crate::theme::ink(0.05)))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && this.active != ix {
                    this.active = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.dismiss(cx);
                cx.emit(event.clone());
            }))
            .child(glyph)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(12.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(popover::search_highlight(label, Some(&query), &theme)),
            )
            .children(pr.map(|pr| {
                crate::change_requests::pull_request_badge(
                    format!("chat-activity-pr-{ix}").into(),
                    pr,
                    crate::change_requests::ChangeRequestBadgeSurface::Sidebar,
                    &theme,
                )
            }))
            .child(
                div()
                    .flex_none()
                    .text_size(crate::typography::ui_rems(10.0))
                    .text_color(theme.text_muted)
                    .child(time),
            );
        if let ActivityRow::Chat(row) = row {
            let chat_id = row.chat_id.clone();
            item = item.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |_, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    cx.emit(ChatActivityEvent::ChildChatContextMenu {
                        chat_id: chat_id.clone(),
                        position: event.position,
                    });
                }),
            );
        }
        div()
            .h(px(ROW_HEIGHT))
            .pb(px(2.0))
            .child(item)
            .into_any_element()
    }
}
impl popover::ScrollRailHost for ChatActivity {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        &mut self.scrollbar
    }
    fn rail_scroll(&self) -> Option<ScrollHandle> {
        Some(self.scroll_base())
    }
}

impl Render for ChatActivity {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.chat_id.is_empty() {
            return div().into_any_element();
        }
        let theme = Theme::of(cx).clone();
        let state = self.state.read(cx);
        let agents = subagent_rows(state, &self.chat_id).len();
        let chats = child_chat_rows(state, &self.chat_id, Utc::now()).len();
        let count = agents + chats;
        if count == 0 {
            self.menu = popover::Popup::default();
            self.focus_pending = false;
            if self.focus.contains_focused(window, cx) {
                window.focus(&self.composer_focus, cx);
            }
            return gpui::Empty.into_any_element();
        }
        if std::mem::take(&mut self.focus_pending) {
            window.focus(&self.search.focus_handle(cx), cx);
        }
        let label = format!("Subagents and side chats: {agents} subagents, {chats} side chats");
        let mut trigger = div()
            .id("chat-activity-trigger")
            .debug_selector(|| "chat-activity-trigger".into())
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(label))
            .relative()
            .flex_none()
            .h(px(24.0))
            .px(px(5.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .cursor_pointer()
            .text_color(if self.is_open() {
                theme.text
            } else {
                theme.text_muted
            })
            .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| {
                    this.menu.note_trigger_press();
                }),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                if this.menu.take_press_was_open() || this.is_open() {
                    this.dismiss(cx);
                    window.focus(&this.composer_focus, cx);
                } else {
                    this.open(cx);
                }
            }))
            .child(
                icon(icons::BOT)
                    .size(px(15.0))
                    .text_color(if self.is_open() {
                        theme.text
                    } else {
                        theme.text_muted
                    }),
            )
            .child(
                div()
                    .id("chat-activity-count")
                    .min_w(px(16.0))
                    .h(px(16.0))
                    .px(px(4.0))
                    .rounded_full()
                    .bg(theme.glass_hover())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(crate::typography::ui_rems(10.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child(count.to_string()),
            );
        if self.menu.get().is_some() {
            // Capture the painted overlay state: its outside-click handler
            // may close it before this menu handles the same mouse press.
            let child_overlay_open = self.child_overlay_open;
            let theme = theme.for_popup();
            let viewport = window.viewport_size();
            let card = popover::popover_card_flush(&theme)
                // macOS leaves the backdrop visible without a fill; Linux
                // uses the shared popover tint and translucency unchanged.
                .when(cfg!(target_os = "macos"), |card| {
                    card.bg(gpui::transparent_black())
                })
                .w(px((f32::from(viewport.width) - 24.0).clamp(0.0, MENU_WIDTH)))
                .id("chat-activity-menu")
                .debug_selector(|| "chat-activity-menu".into())
                .role(gpui::Role::Group)
                .aria_label("Subagents and side chats")
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::on_key_down))
                .on_mouse_down_out(cx.listener(move |this, _, window, cx| {
                    if child_overlay_open {
                        return;
                    }
                    this.dismiss(cx);
                    if this.focus.contains_focused(window, cx) {
                        window.blur();
                    }
                }))
                .child(self.render_menu(f32::from(viewport.height), &theme, cx));
            trigger = trigger.child(popover::anchored_menu_above_end(
                "chat-activity-popover",
                card.into_any_element(),
                self.menu.closing_since(),
            ));
        }
        trigger.into_any_element()
    }
}

/// A spawn chip of the active transcript, as the menu lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubagentRow {
    pub doc_id: String,
    pub title: SharedString,
    pub status: Option<SubagentStatus>,
    /// When the spawning turn was written — the closest thing a subagent
    /// has to a start time.
    pub spawned_at: DateTime<Utc>,
}

impl SubagentRow {
    /// A settled subagent is frozen: the tab reads its blob first.
    pub fn frozen(&self) -> bool {
        matches!(
            self.status,
            Some(SubagentStatus::Done) | Some(SubagentStatus::Failed)
        )
    }

    fn indicator(&self) -> ChatIndicator {
        match self.status {
            Some(SubagentStatus::Running) => ChatIndicator::Working,
            Some(SubagentStatus::Done) => ChatIndicator::Completed,
            Some(SubagentStatus::Failed) => ChatIndicator::Errored,
            None => ChatIndicator::Idle,
        }
    }
}

/// The active chat's subagents, in spawn order, one row per subagent doc.
/// Only genuine spawn chips with a stamped doc ref qualify — the chip IS the
/// index (there is no listing endpoint), and a stray ref on a non-Agent tool
/// must not surface as a phantom subagent.
pub(crate) fn subagent_rows(state: &AppState, chat_id: &str) -> Vec<SubagentRow> {
    if state.selected_chat.as_deref() != Some(chat_id) {
        return Vec::new();
    }
    let mut rows: Vec<SubagentRow> = Vec::new();
    for entry in &state.transcript {
        let spawned_at =
            DateTime::<Utc>::from_timestamp_millis(entry.created_at).unwrap_or_else(Utc::now);
        for part in &entry.parts {
            let MessagePart::Tool {
                call,
                subagent_ref: Some(doc_id),
                subagent_status,
                ..
            } = part
            else {
                continue;
            };
            if !call.is_subagent_spawn() {
                continue;
            }
            let row = SubagentRow {
                doc_id: doc_id.clone(),
                title: crate::transcript::subagent_tab_title(call),
                status: *subagent_status,
                spawned_at,
            };
            match rows.iter_mut().find(|r| r.doc_id == row.doc_id) {
                // A reopened (steered) subagent updates its row in place.
                Some(existing) => *existing = row,
                None => rows.push(row),
            }
        }
    }
    rows
}

/// A side chat of the active chat, as the menu lists it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChildChatRow {
    pub chat_id: String,
    pub title: SharedString,
    pub status: ChatIndicator,
    pub time_ago: SharedString,
    /// The chat's linked pull request, drawn as the sidebar's badge.
    pub change_request: Option<zeron_proto::ChangeRequestSummary>,
    activity: DateTime<Utc>,
}

/// The live (unarchived) children of `chat_id`, most recent activity first —
/// the same order the sidebar's Sessions list keeps.
pub(crate) fn child_chat_rows(
    state: &AppState,
    chat_id: &str,
    now: DateTime<Utc>,
) -> Vec<ChildChatRow> {
    let mut rows: Vec<ChildChatRow> = state
        .chats
        .iter()
        .filter(|chat| !chat.archived && chat.parent_chat_id.as_deref() == Some(chat_id))
        .map(|chat| {
            let activity = chat.last_message_at.unwrap_or(chat.created_at);
            ChildChatRow {
                chat_id: chat.id.clone(),
                title: child_chat_title(chat).into(),
                status: state.display_status_for(chat, now),
                time_ago: zeron_proto::view::format_time_ago(activity, now).into(),
                change_request: state.change_request_for_chat(chat).cloned(),
                activity,
            }
        })
        .collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.activity));
    rows
}

/// A side chat titles itself on its first turn; until then the preview or a
/// placeholder stands in.
pub(crate) fn child_chat_title(chat: &Chat) -> String {
    chat.title
        .clone()
        .or_else(|| chat.last_message_preview.clone())
        .unwrap_or_else(|| "New side chat".into())
}

/// What the menu would draw for `chat_id`, hashed. Cheap enough to run on
/// every state notification.
pub(crate) fn fingerprint(state: &AppState, chat_id: &str, now: DateTime<Utc>) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for row in subagent_rows(state, chat_id) {
        row.doc_id.hash(&mut hasher);
        row.title.as_ref().hash(&mut hasher);
        (row.status.map(|s| s as u8)).hash(&mut hasher);
    }
    0xC0FFEEu64.hash(&mut hasher);
    for row in child_chat_rows(state, chat_id, now) {
        row.chat_id.hash(&mut hasher);
        row.title.as_ref().hash(&mut hasher);
        (row.status as u8).hash(&mut hasher);
        row.time_ago.as_ref().hash(&mut hasher);
        row.change_request
            .as_ref()
            .map(|pr| (pr.number, pr.state as u8))
            .hash(&mut hasher);
    }
    hasher.finish()
}

fn status_glyph(
    key: String,
    status: ChatIndicator,
    view: EntityId,
    theme: &Theme,
    cx: &mut gpui::App,
) -> AnyElement {
    let color = crate::shell::spaces::status_dot_color(status, theme);
    let glyph: AnyElement = match status {
        ChatIndicator::Completed => icon(icons::CHECK)
            .size(px(11.0))
            .flex_none()
            .text_color(color)
            .into_any_element(),
        ChatIndicator::Working => {
            loaders::mini_glyph_spinner(format!("{key}-working"), 2.0, theme.glyph, view, cx)
                .into_any_element()
        }
        _ => div()
            .size(px(6.0))
            .flex_none()
            .rounded_full()
            .bg(color)
            .into_any_element(),
    };
    div()
        .flex_none()
        .size(px(13.0))
        .flex()
        .items_center()
        .justify_center()
        .child(glyph)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::{MessageRole, MessageStatus, SessionMessageEntry};
    use zeron_proto::ToolCall;

    #[gpui::test]
    fn activity_menu_toggles_navigates_and_resets_with_the_conversation(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Host {
            activity: Entity<ChatActivity>,
            events: Vec<ChatActivityEvent>,
            _events: Subscription,
        }
        impl Render for Host {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .items_end()
                    .p(px(20.0))
                    .child(div().track_focus(&self.activity.read(cx).composer_focus))
                    .child(self.activity.clone())
            }
        }
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            crate::composer::init(cx, Default::default());
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.selected_chat = Some("main".into());
                state.chats = vec![chat("main", None, 60), chat("child", Some("main"), 1)];
                state.transcript = vec![entry(vec![spawn(
                    "t1",
                    "Agent: verify",
                    Some("sub-1"),
                    Some(SubagentStatus::Done),
                )])];
                state
            });
            let focus = cx.focus_handle();
            let activity = cx.new(|cx| ChatActivity::new(state, focus, cx));
            let events = cx.subscribe(
                &activity,
                |this: &mut Host, _, event: &ChatActivityEvent, _| {
                    this.events.push(event.clone());
                },
            );
            Host {
                activity,
                events: Vec::new(),
                _events: events,
            }
        });
        let activity = host.read_with(cx, |host, _| host.activity.clone());
        let click = |cx: &mut gpui::VisualTestContext, selector| {
            let position = cx.debug_bounds(selector).expect(selector).center();
            cx.simulate_mouse_down(position, MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_up(position, MouseButton::Left, gpui::Modifiers::default());
        };

        click(cx, "chat-activity-trigger");
        assert!(activity.read_with(cx, |activity, _| activity.is_open()));
        let trigger = cx.debug_bounds("chat-activity-trigger").unwrap();
        let menu = cx.debug_bounds("chat-activity-menu").unwrap();
        assert!(
            menu.bottom() <= trigger.top(),
            "the menu opens above the footer"
        );
        assert!(cx.debug_bounds("chat-activity-subagent-sub-1").is_some());
        assert!(cx.debug_bounds("chat-activity-chat-child").is_none());
        assert_eq!(f32::from(menu.size.width), MENU_WIDTH);
        cx.simulate_input("verify");
        assert!(cx.debug_bounds("chat-activity-subagent-sub-1").is_some());
        click(cx, "chat-activity-tab-chats");
        assert!(cx.debug_bounds("chat-activity-chat-child").is_none());
        assert!(cx.debug_bounds("chat-activity-new").is_some());
        assert!(cx.debug_bounds("chat-activity-fork").is_some());
        click(cx, "chat-activity-tab-subagents");
        assert!(cx.debug_bounds("chat-activity-subagent-sub-1").is_some());
        click(cx, "chat-activity-tab-chats");
        cx.simulate_keystrokes("backspace backspace backspace backspace backspace backspace");
        assert!(cx.debug_bounds("chat-activity-chat-child").is_some());

        click(cx, "chat-activity-trigger");
        assert!(
            !activity.read_with(cx, |activity, _| activity.is_open()),
            "a second click closes instead of reopening"
        );
        activity.update(cx, |activity, cx| {
            activity.menu = popover::Popup::default();
            cx.notify();
        });
        click(cx, "chat-activity-trigger");
        cx.simulate_keystrokes("escape");
        assert!(!activity.read_with(cx, |activity, _| activity.is_open()));
        cx.update(|window, cx| assert!(activity.read(cx).composer_focus.is_focused(window)));

        activity.update(cx, |activity, cx| {
            activity.menu = popover::Popup::default();
            cx.notify();
        });
        click(cx, "chat-activity-trigger");
        click(cx, "chat-activity-chat-child");
        assert!(!activity.read_with(cx, |activity, _| activity.is_open()));
        host.read_with(cx, |host, _| {
            assert!(matches!(host.events.last(), Some(ChatActivityEvent::OpenChildChat(id)) if id == "child"));
        });

        activity.update(cx, |activity, cx| {
            activity.menu = popover::Popup::default();
            cx.notify();
        });
        click(cx, "chat-activity-trigger");
        click(cx, "chat-activity-tab-subagents");
        cx.simulate_input("verify");
        cx.simulate_keystrokes("down up enter");
        assert!(!activity.read_with(cx, |activity, _| activity.is_open()));
        host.read_with(cx, |host, _| {
            assert!(matches!(host.events.last(), Some(ChatActivityEvent::OpenSubagent { doc_id, .. }) if doc_id == "sub-1"));
        });

        activity.update(cx, |activity, cx| {
            activity.menu.open(());

            activity.state.update(cx, |state, cx| {
                state.selected_chat = Some("other".into());
                state.transcript.clear();
                cx.notify();
            });
        });
        activity.read_with(cx, |activity, cx| {
            assert!(!activity.is_open());
            assert_eq!(activity.chat_id, "other");
            assert_eq!(activity.tab, ActivityTab::Subagents);
            assert!(activity.search.read(cx).text().is_empty());
        });

        assert!(cx.debug_bounds("chat-activity-trigger").is_none());
        assert!(cx.debug_bounds("chat-activity-chat-child").is_none());
        let state = activity.read_with(cx, |activity, _| activity.state.clone());
        state.update(cx, |state, cx| {
            state.chats.push(chat("other-child", Some("other"), 1));
            cx.notify();
        });
        click(cx, "chat-activity-trigger");
        assert!(cx.debug_bounds("chat-activity-chat-other-child").is_some());
        let outside = gpui::point(px(5.0), px(5.0));
        cx.simulate_mouse_down(outside, MouseButton::Left, gpui::Modifiers::default());
        assert!(!activity.read_with(cx, |activity, _| activity.is_open()));

        activity.update(cx, |activity, cx| {
            activity.menu = popover::Popup::default();
            cx.notify();
        });
        click(cx, "chat-activity-trigger");
        state.update(cx, |state, cx| {
            state.chats.last_mut().unwrap().archived = true;
            cx.notify();
        });
        assert!(cx.debug_bounds("chat-activity-trigger").is_none());
        assert!(cx.debug_bounds("chat-activity-menu").is_none());
        assert!(!activity.read_with(cx, |activity, _| activity.is_open()));

        state.update(cx, |state, cx| {
            state.transcript = vec![entry(vec![spawn(
                "t2",
                "Agent: verify",
                Some("sub-2"),
                Some(SubagentStatus::Running),
            )])];
            cx.notify();
        });
        click(cx, "chat-activity-trigger");
        assert!(cx.debug_bounds("chat-activity-subagent-sub-2").is_some());
    }

    fn chat(id: &str, parent: Option<&str>, minutes_ago: i64) -> Chat {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "deviceId": "dev",
            "archived": false,
            "createdAt": Utc::now() - chrono::Duration::minutes(minutes_ago),
            "parentChatId": parent,
        }))
        .unwrap()
    }

    fn spawn(
        id: &str,
        name: &str,
        doc: Option<&str>,
        status: Option<SubagentStatus>,
    ) -> MessagePart {
        MessagePart::Tool {
            id: id.into(),
            call: ToolCall::Unknown {
                name: name.into(),
                input: Some(serde_json::json!({ "description": "verify the marker pipeline" })),
            },
            is_error: false,
            resolved: true,
            output: None,
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: doc.map(str::to_owned),
            subagent_status: status,
            subagent_tail: None,
        }
    }

    fn entry(parts: Vec<MessagePart>) -> SessionMessageEntry {
        SessionMessageEntry {
            duration_ms: None,
            id: "e1".into(),
            role: MessageRole::Assistant,
            parts,
            created_at: (Utc::now() - chrono::Duration::minutes(3)).timestamp_millis(),
            device_id: "dev".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        }
    }

    #[test]
    fn subagent_rows_list_only_stamped_spawn_chips_of_the_selected_chat() {
        let mut state = AppState::new();
        state.selected_chat = Some("main".into());
        state.transcript = vec![entry(vec![
            spawn(
                "t1",
                "Agent: verify",
                Some("main--sub--t1"),
                Some(SubagentStatus::Running),
            ),
            // No doc ref yet: the engine stamps it asynchronously.
            spawn("t2", "Agent: later", None, None),
            // A stray ref on a non-spawn tool never surfaces.
            spawn(
                "t3",
                "Read",
                Some("main--sub--t3"),
                Some(SubagentStatus::Done),
            ),
            spawn(
                "t4",
                "Agent: done",
                Some("main--sub--t4"),
                Some(SubagentStatus::Done),
            ),
        ])];
        let rows = subagent_rows(&state, "main");
        assert_eq!(
            rows.iter().map(|r| r.doc_id.as_str()).collect::<Vec<_>>(),
            ["main--sub--t1", "main--sub--t4"]
        );
        // The bare task, genus stripped — the same title the tab wears.
        assert_eq!(rows[0].title.as_ref(), "verify");
        assert!(!rows[0].frozen());
        assert!(rows[1].frozen());
        // Spawn time comes from the turn that carried the chip.
        assert!((Utc::now() - rows[0].spawned_at).num_minutes() >= 2);
        // Another chat's activity menu sees nothing of this transcript.
        assert!(subagent_rows(&state, "other").is_empty());
    }

    #[test]
    fn child_chat_rows_are_live_children_newest_first() {
        let mut state = AppState::new();
        let mut archived = chat("old", Some("main"), 1);
        archived.archived = true;
        let mut titled = chat("b", Some("main"), 30);
        titled.title = Some("Investigate caching".into());
        state.apply_chats(vec![
            chat("main", None, 60),
            chat("a", Some("main"), 5),
            titled,
            chat("unrelated", Some("elsewhere"), 2),
            archived,
        ]);
        let rows = child_chat_rows(&state, "main", Utc::now());
        assert_eq!(
            rows.iter().map(|r| r.chat_id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(rows[0].title.as_ref(), "New side chat");
        assert_eq!(rows[1].title.as_ref(), "Investigate caching");
        assert_eq!(rows[0].status, ChatIndicator::Idle);
    }

    #[test]
    fn fingerprint_tracks_membership_and_status() {
        let mut state = AppState::new();
        let now = Utc::now();
        state.apply_chats(vec![chat("main", None, 60)]);
        let empty = fingerprint(&state, "main", now);
        state.apply_chats(vec![chat("main", None, 60), chat("a", Some("main"), 5)]);
        let one = fingerprint(&state, "main", now);
        assert_ne!(empty, one);
        assert_eq!(one, fingerprint(&state, "main", now));
    }
}
