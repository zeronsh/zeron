//! Native ticket workspace. Tickets are planning records; linked chats remain
//! execution records owned by the existing chat and child-chat engine.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Entity, EventEmitter, IntoElement, MouseButton, Render, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};
use serde::Deserialize;
use serde_json::{Value, json};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::motion;
use crate::popover;
use crate::state::{AppState, EngineHandle};
use crate::theme::Theme;
use crate::typography::ui_rems;

const STATUSES: [&str; 6] = [
    "backlog",
    "todo",
    "inProgress",
    "inReview",
    "done",
    "canceled",
];
const PRIORITIES: [&str; 5] = ["none", "urgent", "high", "medium", "low"];
const DETAIL_WIDTH: f32 = 488.0;

#[derive(Clone, Copy)]
struct DetailTween {
    from: f32,
    to: f32,
    started: Instant,
}

#[derive(Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TicketSnapshot {
    #[serde(default)]
    boards: Vec<Board>,
    #[serde(default)]
    board_space_links: Vec<BoardSpaceLink>,
    #[serde(default)]
    tickets: Vec<Ticket>,
    #[serde(default)]
    chat_links: Vec<TicketChatLink>,
    #[serde(default)]
    comments: Vec<TicketComment>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Board {
    id: String,
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    archived: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BoardSpaceLink {
    board_id: String,
    space_id: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Ticket {
    id: String,
    board_id: String,
    kind: String,
    parent_ticket_id: Option<String>,
    title: String,
    #[serde(default)]
    description: String,
    status: String,
    priority: String,
    #[serde(default)]
    archived: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TicketChatLink {
    ticket_id: String,
    chat_id: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TicketComment {
    id: String,
    ticket_id: String,
    body: String,
    #[serde(default)]
    author: Option<String>,
}

pub enum TicketsEvent {
    OpenChat(String),
    StartChat {
        ticket_id: String,
        space_id: String,
    },
    StartChildChat {
        ticket_id: String,
        parent_chat_id: String,
    },
}

enum TicketDialog {
    Board {
        id: Option<String>,
        name: Entity<ComposerInput>,
    },
    Ticket {
        id: Option<String>,
        kind: &'static str,
        parent_id: Option<String>,
        title: Entity<ComposerInput>,
        description: Entity<ComposerInput>,
    },
    LinkChat {
        ticket_id: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Layout {
    List,
    Board,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FilterMenu {
    Status,
    Priority,
    Archive,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArchiveFilter {
    Active,
    Archived,
    All,
}

pub struct TicketsView {
    state: Entity<AppState>,
    _state_observation: Subscription,
    engine: Option<EngineHandle>,
    watch: Option<Task<()>>,
    snapshot: TicketSnapshot,
    loaded: bool,
    error: Option<SharedString>,
    selected_board: Option<String>,
    selected_ticket: Option<String>,
    detail_tween: Option<DetailTween>,
    layout: Layout,
    search: Entity<ComposerInput>,
    _search_events: Subscription,
    status_filter: Option<&'static str>,
    priority_filter: Option<&'static str>,
    show_archived: bool,
    archive_filter: ArchiveFilter,
    filter_menu: popover::Popup<FilterMenu>,
    dialog: Option<TicketDialog>,
    comment: Entity<ComposerInput>,
    _comment_events: Subscription,
    scroll: gpui::ScrollHandle,
}

impl EventEmitter<TicketsEvent> for TicketsView {}

impl TicketsView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| ComposerInput::new("Search tickets", cx).with_single_line());
        let search_events = cx.subscribe(&search, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                let _ = this;
                cx.notify();
            }
        });
        let comment = cx.new(|cx| ComposerInput::new("Write a comment…", cx));
        let comment_events = cx.subscribe(&comment, |this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_comment(cx);
            }
        });
        let state_observation = cx.observe(&state, |this, _, cx| {
            this.ensure_watch(cx);
            cx.notify();
        });
        let mut view = Self {
            state,
            _state_observation: state_observation,
            engine: None,
            watch: None,
            snapshot: TicketSnapshot::default(),
            loaded: false,
            error: None,
            selected_board: None,
            selected_ticket: None,
            detail_tween: None,
            layout: Layout::List,
            search,
            _search_events: search_events,
            status_filter: None,
            priority_filter: None,
            show_archived: false,
            archive_filter: ArchiveFilter::Active,
            filter_menu: popover::Popup::default(),
            dialog: None,
            comment,
            _comment_events: comment_events,
            scroll: gpui::ScrollHandle::new(),
        };
        view.ensure_watch(cx);
        view
    }

    fn ensure_watch(&mut self, cx: &mut Context<Self>) {
        let next = self.state.read(cx).engine().cloned();
        if self
            .engine
            .as_ref()
            .zip(next.as_ref())
            .is_some_and(|(a, b)| a.same_connection(b))
        {
            return;
        }
        self.watch = None;
        self.engine = next.clone();
        self.loaded = false;
        let Some(engine) = next else {
            return;
        };
        self.watch = Some(cx.spawn(async move |this, cx| {
            loop {
                let mut stream = match engine
                    .client()
                    .subscribe(methods::WATCH_TICKETS, json!({}))
                    .await
                {
                    Ok(stream) => stream,
                    Err(err) => {
                        let alive = this.update(cx, |view, cx| {
                            view.error = Some(format!("Could not load tickets: {err}").into());
                            cx.notify();
                        });
                        if alive.is_err() {
                            return;
                        }
                        cx.background_executor().timer(Duration::from_secs(2)).await;
                        continue;
                    }
                };
                while let Some(frame) = stream.recv().await {
                    match serde_json::from_value::<TicketSnapshot>(frame) {
                        Ok(snapshot) => {
                            if this
                                .update(cx, |view, cx| {
                                    view.apply_snapshot(snapshot, cx);
                                })
                                .is_err()
                            {
                                return;
                            }
                        }
                        Err(err) => tracing::warn!(error = %err, "invalid ticket snapshot"),
                    }
                }
                if this
                    .update(cx, |view, cx| {
                        view.error = Some("Ticket connection lost. Reconnecting…".into());
                        cx.notify();
                    })
                    .is_err()
                {
                    return;
                }
                cx.background_executor().timer(Duration::from_secs(2)).await;
            }
        }));
    }

    fn apply_snapshot(&mut self, snapshot: TicketSnapshot, cx: &mut Context<Self>) {
        let selected_space = self.state.read(cx).selected_space.clone();
        let keep_board = self.selected_board.as_ref().is_some_and(|id| {
            snapshot
                .boards
                .iter()
                .any(|board| (self.show_archived || !board.archived) && &board.id == id)
        });
        if !keep_board {
            self.selected_board = selected_space
                .as_ref()
                .and_then(|space| {
                    snapshot
                        .board_space_links
                        .iter()
                        .find(|link| &link.space_id == space)
                        .map(|link| link.board_id.clone())
                })
                .filter(|id| {
                    snapshot
                        .boards
                        .iter()
                        .any(|board| (self.show_archived || !board.archived) && &board.id == id)
                })
                .or_else(|| {
                    snapshot
                        .boards
                        .iter()
                        .find(|board| self.show_archived || !board.archived)
                        .map(|board| board.id.clone())
                });
        }
        if self.selected_ticket.as_ref().is_some_and(|id| {
            !snapshot
                .tickets
                .iter()
                .any(|ticket| &ticket.id == id && (self.show_archived || !ticket.archived))
        }) {
            self.selected_ticket = None;
            self.detail_tween = None;
        }
        self.snapshot = snapshot;
        self.loaded = true;
        self.error = None;
        cx.notify();
    }

    fn mutate(&mut self, operation: Value, cx: &mut Context<Self>) {
        let Some(engine) = self.engine.clone() else {
            self.error = Some("Ticket engine is not connected".into());
            cx.notify();
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::MUTATE_TICKET, operation)
                .await;
            let _ = this.update(cx, |view, cx| {
                if let Err(err) = result {
                    view.error = Some(format!("Could not save ticket: {err}").into());
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn link_chat(&mut self, ticket_id: String, chat_id: String, cx: &mut Context<Self>) {
        self.mutate(
            json!({"op":"linkTicketChat","ticketId":ticket_id,"chatId":chat_id}),
            cx,
        );
    }

    fn create_board(&mut self, cx: &mut Context<Self>) {
        let input = cx.new(|cx| ComposerInput::new("Board name", cx).with_single_line());
        self.dialog = Some(TicketDialog::Board {
            id: None,
            name: input,
        });
        cx.notify();
    }

    fn edit_board(&mut self, board: Board, cx: &mut Context<Self>) {
        let input = cx.new(|cx| ComposerInput::new("Board name", cx).with_single_line());
        input.update(cx, |input, cx| input.set_text(board.name, cx));
        self.dialog = Some(TicketDialog::Board {
            id: Some(board.id),
            name: input,
        });
        cx.notify();
    }

    fn open_ticket_form(
        &mut self,
        ticket: Option<Ticket>,
        kind: &'static str,
        parent_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let title = cx.new(|cx| ComposerInput::new("Ticket title", cx).with_single_line());
        let description = cx.new(|cx| ComposerInput::new("Add a description…", cx));
        if let Some(ticket) = &ticket {
            title.update(cx, |input, cx| input.set_text(ticket.title.clone(), cx));
            description.update(cx, |input, cx| {
                input.set_text(ticket.description.clone(), cx)
            });
        }
        self.dialog = Some(TicketDialog::Ticket {
            id: ticket.map(|ticket| ticket.id),
            kind,
            parent_id,
            title,
            description,
        });
        cx.notify();
    }

    fn submit_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        match dialog {
            TicketDialog::Board { id, name } => {
                let name = name.read(cx).text().trim().to_owned();
                if name.is_empty() {
                    self.error = Some("Board name is required".into());
                    return;
                }
                if let Some(board_id) = id {
                    self.mutate(
                        json!({"op":"updateBoard","boardId":board_id,"name":name}),
                        cx,
                    );
                    return;
                }
                let board_id = uuid::Uuid::new_v4().to_string();
                let space_id = self.state.read(cx).selected_space.clone();
                let Some(engine) = self.engine.clone() else {
                    return;
                };
                self.selected_board = Some(board_id.clone());
                cx.spawn(async move |this, cx| {
                    let created = engine.client().call(methods::MUTATE_TICKET,
                        json!({"op":"createBoard","boardId":board_id,"name":name})).await;
                    let result = match created {
                        Ok(_) => {
                            if let Some(space_id) = space_id {
                                engine.client().call(methods::MUTATE_TICKET,
                                    json!({"op":"linkBoardSpace","boardId":board_id,"spaceId":space_id})).await
                            } else { Ok(json!({"ok":true})) }
                        }
                        Err(err) => Err(err),
                    };
                    let _ = this.update(cx, |view, cx| {
                        if let Err(err) = result { view.error = Some(format!("Could not create board: {err}").into()); }
                        cx.notify();
                    });
                }).detach();
            }
            TicketDialog::Ticket {
                id,
                kind,
                parent_id,
                title,
                description,
            } => {
                let title = title.read(cx).text().trim().to_owned();
                if title.is_empty() {
                    self.error = Some("Ticket title is required".into());
                    return;
                }
                let description = description.read(cx).text().trim().to_owned();
                if let Some(ticket_id) = id {
                    self.mutate(json!({"op":"updateTicket","ticketId":ticket_id,"title":title,"description":description}), cx);
                } else if let Some(board_id) = self.selected_board.clone() {
                    let ticket_id = uuid::Uuid::new_v4().to_string();
                    self.open_ticket(ticket_id.clone(), cx);
                    self.mutate(json!({"op":"createTicket","ticketId":ticket_id,"boardId":board_id,
                        "kind":kind,"parentTicketId":parent_id,"title":title,"description":description,
                        "status":"backlog","priority":"none"}), cx);
                }
            }
            TicketDialog::LinkChat { .. } => {}
        }
        cx.notify();
    }

    fn submit_comment(&mut self, cx: &mut Context<Self>) {
        let Some(ticket_id) = self.selected_ticket.clone() else {
            return;
        };
        let body = self.comment.read(cx).text().trim().to_owned();
        if body.is_empty() {
            return;
        }
        let comment_id = uuid::Uuid::new_v4().to_string();
        self.mutate(json!({"op":"createComment","commentId":comment_id,"ticketId":ticket_id,"body":body,"author":"You"}), cx);
        self.comment.update(cx, |input, cx| input.set_text("", cx));
    }

    fn cycle_status(&mut self, ticket: &Ticket, cx: &mut Context<Self>) {
        let index = STATUSES
            .iter()
            .position(|status| *status == ticket.status)
            .unwrap_or(0);
        self.mutate(json!({"op":"updateTicket","ticketId":ticket.id,"status":STATUSES[(index+1)%STATUSES.len()]}), cx);
    }

    fn cycle_priority(&mut self, ticket: &Ticket, cx: &mut Context<Self>) {
        let choices = &PRIORITIES;
        let index = choices
            .iter()
            .position(|priority| *priority == ticket.priority)
            .unwrap_or(0);
        self.mutate(json!({"op":"updateTicket","ticketId":ticket.id,"priority":choices[(index+1)%choices.len()]}), cx);
    }

    fn detail_width(&self, reduced_motion: bool) -> f32 {
        let Some(tween) = self.detail_tween else {
            return if self.selected_ticket.is_some() {
                DETAIL_WIDTH
            } else {
                0.0
            };
        };
        if reduced_motion {
            return tween.to;
        }
        let duration = motion::RESIZE.total().mul_f32(motion::speed_scale());
        let raw = tween.started.elapsed().as_secs_f32() / duration.as_secs_f32();
        motion::lerp(tween.from, tween.to, motion::RESIZE.progress(raw.min(1.0)))
    }

    fn open_ticket(&mut self, id: String, cx: &mut Context<Self>) {
        let reduced = motion::reduced_motion(cx);
        let from = self.detail_width(reduced);
        self.selected_ticket = Some(id);
        self.detail_tween = if reduced || from >= DETAIL_WIDTH {
            None
        } else {
            Some(DetailTween {
                from,
                to: DETAIL_WIDTH,
                started: Instant::now(),
            })
        };
        cx.notify();
    }

    fn close_ticket(&mut self, cx: &mut Context<Self>) {
        if self.selected_ticket.is_none() {
            return;
        }
        if motion::reduced_motion(cx) {
            self.selected_ticket = None;
            self.detail_tween = None;
        } else {
            self.detail_tween = Some(DetailTween {
                from: self.detail_width(false),
                to: 0.0,
                started: Instant::now(),
            });
        }
        cx.notify();
    }

    fn visible_tickets(&self, cx: &mut Context<Self>) -> Vec<Ticket> {
        let Some(board_id) = self.selected_board.as_deref() else {
            return Vec::new();
        };
        let query = self.search.read(cx).text().trim().to_lowercase();
        self.snapshot
            .tickets
            .iter()
            .filter(|ticket| {
                ticket.board_id == board_id
                    && match self.archive_filter {
                        ArchiveFilter::Active => !ticket.archived,
                        ArchiveFilter::Archived => ticket.archived,
                        ArchiveFilter::All => true,
                    }
            })
            .filter(|ticket| {
                self.status_filter
                    .is_none_or(|status| ticket.status == status)
            })
            .filter(|ticket| {
                self.priority_filter
                    .is_none_or(|priority| ticket.priority == priority)
            })
            .filter(|ticket| {
                query.is_empty()
                    || ticket.title.to_lowercase().contains(&query)
                    || ticket.description.to_lowercase().contains(&query)
                    || ticket.id.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    fn render_ticket_row(
        &mut self,
        ticket: Ticket,
        depth: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let selected = self.selected_ticket.as_deref() == Some(ticket.id.as_str());
        let row_id = ticket.id.clone();
        let status_ticket = ticket.clone();
        let is_epic = ticket.kind == "epic";
        let is_subissue = !is_epic
            && ticket.parent_ticket_id.as_ref().is_some_and(|parent_id| {
                self.snapshot
                    .tickets
                    .iter()
                    .find(|parent| &parent.id == parent_id)
                    .is_none_or(|parent| parent.kind != "epic")
            });
        let tone = status_tone(&ticket.status, theme);
        let marker = if is_epic {
            div()
                .size(px(25.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(Theme::CONTROL_RADIUS))
                .bg(theme.accent.opacity(0.12))
                .child(icon(icons::WIDGET).size(px(15.0)).text_color(theme.accent))
                .into_any_element()
        } else if is_subissue {
            div()
                .size(px(25.0))
                .flex_none()
                .flex()
                .items_start()
                .justify_center()
                .child(
                    div()
                        .w(px(12.0))
                        .h(px(15.0))
                        .border_l_1()
                        .border_b_1()
                        .border_color(theme.text_faint.opacity(0.7)),
                )
                .into_any_element()
        } else {
            div()
                .size(px(25.0))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    icon(icons::CHECKLIST)
                        .size(px(15.0))
                        .text_color(theme.text_muted),
                )
                .into_any_element()
        };
        div()
            .id(SharedString::from(format!("ticket-row-{}", ticket.id)))
            .debug_selector(|| "ticket-row".into())
            .h(px(if is_epic { 57.0 } else { 44.0 }))
            .pr(px(15.0))
            .pl(px(12.0 + (depth.min(4) as f32 * 22.0)))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.0))
            .border_b_1()
            .border_color(theme.border.opacity(if is_epic { 0.8 } else { 0.4 }))
            .bg(if selected {
                theme.element_active
            } else if is_epic {
                theme.accent.opacity(0.055)
            } else {
                gpui::transparent_black()
            })
            .hover(|style| style.bg(theme.element_hover))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.open_ticket(row_id.clone(), cx);
            }))
            .child(marker)
            .child(
                div()
                    .w(px(66.0))
                    .flex_none()
                    .text_size(ui_rems(9.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(if is_epic {
                        theme.accent
                    } else {
                        theme.text_faint
                    })
                    .child(if is_epic {
                        "EPIC"
                    } else if is_subissue {
                        "SUBISSUE"
                    } else {
                        "ISSUE"
                    }),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(ui_rems(if is_epic { 13.0 } else { 12.0 }))
                    .font_weight(if is_epic {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(theme.text)
                    .child(ticket.title.clone()),
            )
            .child(
                div()
                    .id(SharedString::from(format!("ticket-status-{}", ticket.id)))
                    .h(px(22.0))
                    .px(px(8.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .bg(tone.opacity(0.075))
                    .text_color(tone)
                    .text_size(ui_rems(10.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(tone.opacity(0.18)))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.cycle_status(&status_ticket, cx)),
                    )
                    .child(status_label(&ticket.status)),
            )
            .into_any_element()
    }

    fn render_list(
        &mut self,
        tickets: &[Ticket],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (epics, issues) = ordered_ticket_tree(tickets);
        let mut content = div().flex().flex_col();
        if !epics.is_empty() {
            let epic_count = epics
                .iter()
                .filter(|(ticket, _)| ticket.kind == "epic")
                .count();
            content = content.child(section_label("EPICS", epic_count, theme));
            let mut group = div().flex().flex_col();
            let mut has_group = false;
            for (ticket, depth) in epics {
                if depth == 0 && has_group {
                    content = content.child(
                        group
                            .mx(px(13.0))
                            .mt(px(10.0))
                            .rounded(px(Theme::PANEL_RADIUS))
                            .border_1()
                            .border_color(theme.border.opacity(0.75))
                            .bg(theme.surface.opacity(0.18)),
                    );
                    group = div().flex().flex_col();
                }
                has_group = true;
                group = group.child(self.render_ticket_row(ticket, depth, theme, cx));
            }
            if has_group {
                content = content.child(
                    group
                        .mx(px(13.0))
                        .mt(px(10.0))
                        .mb(px(10.0))
                        .rounded(px(Theme::PANEL_RADIUS))
                        .border_1()
                        .border_color(theme.border.opacity(0.75))
                        .bg(theme.surface.opacity(0.18)),
                );
            }
        }
        if !issues.is_empty() {
            let issue_count = issues.iter().filter(|(_, depth)| *depth == 0).count();
            content = content.child(section_label("ISSUES", issue_count, theme));
        }
        for (ticket, depth) in issues {
            content = content.child(self.render_ticket_row(ticket, depth, theme, cx));
        }
        content.into_any_element()
    }

    fn render_board(
        &mut self,
        tickets: &[Ticket],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut columns = div()
            .id("ticket-board-columns")
            .size_full()
            .min_w_0()
            .overflow_x_scroll()
            .flex()
            .gap(px(12.0))
            .p(px(16.0));
        for status in STATUSES {
            let cards: Vec<_> = tickets
                .iter()
                .filter(|ticket| ticket.status == status)
                .cloned()
                .collect();
            let mut column = div()
                .w(px(245.0))
                .h_full()
                .flex_none()
                .flex()
                .flex_col()
                .rounded(px(Theme::PANEL_RADIUS))
                .border_1()
                .border_color(theme.border)
                .bg(theme.surface.opacity(0.34))
                .child(
                    div()
                        .h(px(43.0))
                        .flex_none()
                        .px(px(12.0))
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .size(px(7.0))
                                .rounded_full()
                                .bg(status_tone(status, theme)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .text_size(ui_rems(12.0))
                                .child(status_label(status)),
                        )
                        .child(
                            div()
                                .text_color(theme.text_faint)
                                .text_size(ui_rems(11.0))
                                .child(cards.len().to_string()),
                        ),
                );
            let mut body = div()
                .id(SharedString::from(format!("ticket-board-column-{status}")))
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .p(px(8.0))
                .flex()
                .flex_col()
                .gap(px(7.0));
            for ticket in cards {
                let id = ticket.id.clone();
                let parent = ticket.parent_ticket_id.as_ref().and_then(|parent_id| {
                    self.snapshot
                        .tickets
                        .iter()
                        .find(|row| &row.id == parent_id)
                });
                let is_epic = ticket.kind == "epic";
                let is_subissue = parent.is_some_and(|parent| parent.kind == "issue");
                let role = if is_epic {
                    "EPIC"
                } else if is_subissue {
                    "SUBISSUE"
                } else {
                    "ISSUE"
                };
                let parent_title = parent.map(|parent| parent.title.clone());
                let selected = self.selected_ticket.as_deref() == Some(id.as_str());
                body = body.child(
                    div()
                        .id(SharedString::from(format!("ticket-card-{id}")))
                        .debug_selector(|| "ticket-card".into())
                        .p(px(11.0))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .border_1()
                        .border_color(if selected {
                            theme.accent.opacity(0.55)
                        } else if is_epic {
                            theme.accent.opacity(0.28)
                        } else {
                            theme.border
                        })
                        .bg(if selected {
                            theme.element_active
                        } else if is_epic {
                            theme.accent.opacity(0.055)
                        } else {
                            theme.surface_card
                        })
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_ticket(id.clone(), cx);
                        }))
                        .child(
                            div()
                                .mb(px(8.0))
                                .text_size(ui_rems(9.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(if is_epic {
                                    theme.accent
                                } else {
                                    theme.text_faint
                                })
                                .child(role),
                        )
                        .child(
                            div()
                                .text_size(ui_rems(12.0))
                                .text_color(theme.text)
                                .font_weight(if is_epic {
                                    gpui::FontWeight::SEMIBOLD
                                } else {
                                    gpui::FontWeight::MEDIUM
                                })
                                .child(ticket.title.clone()),
                        )
                        .children(parent_title.map(|title| {
                            div()
                                .mt(px(9.0))
                                .truncate()
                                .text_size(ui_rems(10.0))
                                .text_color(theme.text_faint)
                                .child(format!("In {title}"))
                        })),
                );
            }
            column = column.child(body);
            columns = columns.child(column);
        }
        columns.into_any_element()
    }

    fn render_board_tabs(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let boards: Vec<_> = self
            .snapshot
            .boards
            .iter()
            .filter(|board| self.show_archived || !board.archived)
            .cloned()
            .collect();
        let mut tabs = div()
            .id("ticket-board-tabs")
            .min_w_0()
            .overflow_x_scroll()
            .flex()
            .items_center()
            .gap(px(4.0));
        for board in boards {
            let active = self.selected_board.as_deref() == Some(board.id.as_str());
            let id = board.id.clone();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("board-tab-{id}")))
                    .h(px(28.0))
                    .px(px(10.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .bg(if active {
                        theme.element_active
                    } else {
                        gpui::transparent_black()
                    })
                    .text_color(if active { theme.text } else { theme.text_muted })
                    .text_size(ui_rems(11.0))
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.element_hover).text_color(theme.text))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected_board = Some(id.clone());
                        this.selected_ticket = None;
                        this.detail_tween = None;
                        cx.notify();
                    }))
                    .child(board.name),
            );
        }
        tabs.child(
            div()
                .id("ticket-new-board")
                .h(px(28.0))
                .px(px(8.0))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(4.0))
                .rounded(px(Theme::CONTROL_RADIUS))
                .text_color(theme.text_muted)
                .text_size(ui_rems(11.0))
                .cursor_pointer()
                .hover(|style| style.bg(theme.element_hover).text_color(theme.text))
                .on_click(cx.listener(|this, _, _, cx| this.create_board(cx)))
                .child(icon(icons::ADD_CIRCLE).size(px(13.0)))
                .child("New board"),
        )
        .into_any_element()
    }

    fn close_filter_menu(&mut self, cx: &mut Context<Self>) {
        if self.filter_menu.begin_close() {
            popover::reap_popup(cx, |tickets: &mut Self| &mut tickets.filter_menu);
            cx.notify();
        }
    }

    pub(crate) fn handle_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.filter_menu.is_open() {
            self.close_filter_menu(cx);
            return true;
        }
        self.filter_menu.get().is_some()
    }

    fn set_archive_filter(&mut self, filter: ArchiveFilter, cx: &mut Context<Self>) {
        if self.archive_filter == filter {
            return;
        }
        self.archive_filter = filter;
        self.show_archived = filter != ArchiveFilter::Active;
        self.selected_ticket = None;
        self.detail_tween = None;
        if !self.selected_board.as_ref().is_some_and(|id| {
            self.snapshot
                .boards
                .iter()
                .any(|board| &board.id == id && (self.show_archived || !board.archived))
        }) {
            self.selected_board = self
                .snapshot
                .boards
                .iter()
                .find(|board| self.show_archived || !board.archived)
                .map(|board| board.id.clone());
        }
        cx.notify();
    }

    fn render_filter_menu(
        &mut self,
        kind: FilterMenu,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let popup_theme = theme.for_popup();
        let (labels, selected): (Vec<&'static str>, usize) = match kind {
            FilterMenu::Status => (
                std::iter::once("All status")
                    .chain(STATUSES.into_iter().map(status_label))
                    .collect(),
                self.status_filter
                    .and_then(|status| STATUSES.iter().position(|choice| *choice == status))
                    .map_or(0, |index| index + 1),
            ),
            FilterMenu::Priority => (
                std::iter::once("All priority")
                    .chain(PRIORITIES.into_iter().map(priority_label))
                    .collect(),
                self.priority_filter
                    .and_then(|priority| PRIORITIES.iter().position(|choice| *choice == priority))
                    .map_or(0, |index| index + 1),
            ),
            FilterMenu::Archive => (
                vec!["Active tickets", "Archived tickets", "All tickets"],
                match self.archive_filter {
                    ArchiveFilter::Active => 0,
                    ArchiveFilter::Archived => 1,
                    ArchiveFilter::All => 2,
                },
            ),
        };
        popover::popover_card(&popup_theme)
            .w(px(176.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_filter_menu(cx)))
            .child(div().flex().flex_col().gap(px(2.0)).children(
                labels.into_iter().enumerate().map(|(index, label)| {
                    popover::menu_row(
                        &popup_theme,
                        index == selected,
                        format!("ticket-filter-{kind:?}-{index}"),
                    )
                    .id(SharedString::from(format!(
                        "ticket-filter-option-{kind:?}-{index}"
                    )))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        match kind {
                            FilterMenu::Status => {
                                this.status_filter =
                                    index.checked_sub(1).map(|index| STATUSES[index]);
                            }
                            FilterMenu::Priority => {
                                this.priority_filter =
                                    index.checked_sub(1).map(|index| PRIORITIES[index]);
                            }
                            FilterMenu::Archive => {
                                this.set_archive_filter(
                                    [
                                        ArchiveFilter::Active,
                                        ArchiveFilter::Archived,
                                        ArchiveFilter::All,
                                    ][index],
                                    cx,
                                );
                            }
                        }
                        this.close_filter_menu(cx);
                        cx.notify();
                    }))
                    .child(div().flex_1().child(label))
                    .when(index == selected, |row| {
                        row.child(
                            icon(icons::CHECK)
                                .size(px(13.0))
                                .text_color(popup_theme.accent),
                        )
                    })
                }),
            ))
            .into_any_element()
    }

    fn render_filter_control(
        &mut self,
        kind: FilterMenu,
        label: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = match kind {
            FilterMenu::Status => "ticket-status-filter",
            FilterMenu::Priority => "ticket-priority-filter",
            FilterMenu::Archive => "ticket-archive-filter",
        };
        let mut trigger = div()
            .id(id)
            .relative()
            .h(px(27.0))
            .px(px(9.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(6.0))
            .rounded(px(Theme::CONTROL_RADIUS))
            .border_1()
            .border_color(theme.border)
            .text_size(ui_rems(11.0))
            .text_color(theme.text_muted)
            .cursor_pointer()
            .hover(|style| style.bg(theme.element_hover).text_color(theme.text))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, _| {
                    window.prevent_default();
                    this.filter_menu
                        .note_trigger_press_matching(|open| *open == kind);
                }),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                if this.filter_menu.take_press_was_open() {
                    this.close_filter_menu(cx);
                } else {
                    this.filter_menu.open(kind);
                }
                cx.notify();
            }))
            .child(label)
            .child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(px(11.0))
                    .text_color(theme.text_faint),
            );
        if self.filter_menu.get() == Some(&kind) {
            let closing = self.filter_menu.closing_since();
            let menu = self.render_filter_menu(kind, theme, cx);
            trigger = trigger.child(popover::anchored_menu_below_gap(
                format!("ticket-filter-menu-{id}"),
                menu,
                closing,
                5.0,
            ));
        }
        trigger.into_any_element()
    }

    fn render_toolbar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let status = self.status_filter.map(status_label).unwrap_or("All status");
        let priority = self
            .priority_filter
            .map(priority_label)
            .unwrap_or("All priority");
        let has_board = self.selected_board.as_ref().is_some_and(|id| {
            self.snapshot
                .boards
                .iter()
                .any(|board| &board.id == id && !board.archived)
        });
        div()
            .h(px(49.0))
            .flex_none()
            .px(px(16.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .max_w(px(220.0))
                    .h(px(29.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_card)
                    .child(
                        icon(icons::MAGNIFER)
                            .size(px(14.0))
                            .text_color(theme.text_faint),
                    )
                    .child(
                        div()
                            .ml(px(6.0))
                            .flex_1()
                            .min_w_0()
                            .child(self.search.clone()),
                    ),
            )
            .child(self.render_filter_control(FilterMenu::Status, status, theme, cx))
            .child(self.render_filter_control(FilterMenu::Priority, priority, theme, cx))
            .child(self.render_filter_control(
                FilterMenu::Archive,
                match self.archive_filter {
                    ArchiveFilter::Active => "Active tickets",
                    ArchiveFilter::Archived => "Archived tickets",
                    ArchiveFilter::All => "All tickets",
                },
                theme,
                cx,
            ))
            .child(div().flex_1())
            .child(
                toolbar_chip(
                    "ticket-list-view",
                    "List",
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.layout = Layout::List;
                        cx.notify();
                    }),
                )
                .bg(if self.layout == Layout::List {
                    theme.element_active
                } else {
                    gpui::transparent_black()
                }),
            )
            .child(
                toolbar_chip(
                    "ticket-board-view",
                    "Board",
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.layout = Layout::Board;
                        cx.notify();
                    }),
                )
                .bg(if self.layout == Layout::Board {
                    theme.element_active
                } else {
                    gpui::transparent_black()
                }),
            )
            .when(has_board, |el| {
                el.child(primary_button(
                    "New issue",
                    "ticket-new-issue",
                    theme,
                    cx.listener(|this, _, _, cx| this.open_ticket_form(None, "issue", None, cx)),
                ))
            })
            .into_any_element()
    }

    fn render_chat_row(
        &mut self,
        ticket_id: String,
        chat_id: String,
        linked: bool,
        depth: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chat = self
            .state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == chat_id)
            .cloned();
        let title = chat
            .as_ref()
            .and_then(|chat| chat.title.clone())
            .unwrap_or_else(|| "Conversation unavailable".into());
        let is_child = chat
            .as_ref()
            .is_some_and(|chat| chat.parent_chat_id.is_some());
        let parent_id = chat.as_ref().map(|chat| chat.id.clone());
        let target_id = chat_id.clone();
        let mut row = div()
            .id(SharedString::from(format!("ticket-chat-{chat_id}")))
            .h(px(36.0))
            .px(px(9.0))
            .pl(px(9.0 + depth.min(4) as f32 * 13.0))
            .flex()
            .items_center()
            .gap(px(7.0))
            .rounded(px(Theme::CONTROL_RADIUS))
            .border_1()
            .border_color(theme.border)
            .bg(theme.surface_card)
            .cursor_pointer()
            .hover(|style| style.bg(theme.element_hover))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.emit(TicketsEvent::OpenChat(target_id.clone()));
                this.dialog = None;
            }))
            .child(
                icon(if is_child {
                    icons::BOT
                } else {
                    icons::CHAT_ROUND_LINE
                })
                .size(px(14.0))
                .text_color(theme.accent),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(ui_rems(11.0))
                    .text_color(theme.text)
                    .child(title),
            )
            .child(
                div()
                    .text_size(ui_rems(10.0))
                    .text_color(theme.text_faint)
                    .child(if is_child {
                        "Child thread"
                    } else if linked {
                        "Linked"
                    } else {
                        "Thread"
                    }),
            );
        if let Some(parent_id) = parent_id {
            let ticket_id = ticket_id.clone();
            row = row.child(
                div()
                    .id(SharedString::from(format!("ticket-chat-child-{chat_id}")))
                    .h(px(22.0))
                    .px(px(6.0))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .flex()
                    .items_center()
                    .text_size(ui_rems(10.0))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.element_hover).text_color(theme.text))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(TicketsEvent::StartChildChat {
                            ticket_id: ticket_id.clone(),
                            parent_chat_id: parent_id.clone(),
                        });
                    }))
                    .child("＋ Child"),
            );
        }
        row.into_any_element()
    }

    fn render_detail(
        &mut self,
        ticket: Ticket,
        full_width: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ticket_id = ticket.id.clone();
        let board_id = ticket.board_id.clone();
        let parent = ticket
            .parent_ticket_id
            .as_deref()
            .and_then(|id| {
                self.snapshot
                    .tickets
                    .iter()
                    .find(|candidate| candidate.id == id)
            })
            .cloned();
        let role = if ticket.kind == "epic" {
            "EPIC"
        } else if parent.as_ref().is_some_and(|parent| parent.kind == "issue") {
            "SUBISSUE"
        } else {
            "ISSUE"
        };
        let mut children: Vec<_> = self
            .snapshot
            .tickets
            .iter()
            .filter(|child| {
                !child.archived && child.parent_ticket_id.as_deref() == Some(ticket_id.as_str())
            })
            .cloned()
            .collect();
        children.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        let links: Vec<_> = self
            .snapshot
            .chat_links
            .iter()
            .filter(|link| link.ticket_id == ticket_id)
            .map(|link| link.chat_id.clone())
            .collect();
        let mut linked_chats: Vec<(String, usize)> =
            links.iter().cloned().map(|id| (id, 0)).collect();
        // A linked parent makes all of its agent descendants visible even when
        // each child was not separately attached to the ticket.
        let chats = self.state.read(cx).chats.clone();
        let mut seen: HashSet<String> = links.iter().cloned().collect();
        let mut cursor = 0;
        while cursor < linked_chats.len() {
            let (parent, depth) = linked_chats[cursor].clone();
            for chat in &chats {
                if chat.parent_chat_id.as_deref() == Some(parent.as_str())
                    && seen.insert(chat.id.clone())
                {
                    linked_chats.push((chat.id.clone(), depth + 1));
                }
            }
            cursor += 1;
        }
        let comments: Vec<_> = self
            .snapshot
            .comments
            .iter()
            .filter(|comment| comment.ticket_id == ticket_id)
            .cloned()
            .collect();
        let available_space = self.state.read(cx).selected_space.clone();
        let start_space = {
            let state = self.state.read(cx);
            state
                .selected_space
                .as_ref()
                .filter(|space| {
                    self.snapshot
                        .board_space_links
                        .iter()
                        .any(|link| link.board_id == board_id && &link.space_id == *space)
                })
                .cloned()
                .or_else(|| {
                    self.snapshot
                        .board_space_links
                        .iter()
                        .find(|link| {
                            link.board_id == board_id
                                && state.spaces.iter().any(|space| space.id == link.space_id)
                        })
                        .map(|link| link.space_id.clone())
                })
        };
        let edit_ticket = ticket.clone();
        let status_ticket = ticket.clone();
        let priority_ticket = ticket.clone();
        let close_ticket = ticket.clone();
        let link_ticket_id = ticket_id.clone();
        let new_chat_ticket_id = ticket_id.clone();
        let child_parent = ticket_id.clone();
        let mut body = div()
            .id("ticket-detail-scroll")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_y_scroll()
            .p(px(23.0))
            .flex()
            .flex_col()
            .gap(px(20.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(full_width, |row| {
                        row.child(toolbar_chip(
                            "ticket-detail-back",
                            "All tickets",
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.close_ticket(cx);
                            }),
                        ))
                    })
                    .child(
                        icon(if ticket.kind == "epic" {
                            icons::WIDGET
                        } else {
                            icons::CHECKLIST
                        })
                        .size(px(16.0))
                        .text_color(theme.accent),
                    )
                    .child(
                        div()
                            .text_size(ui_rems(9.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if ticket.kind == "epic" {
                                theme.accent
                            } else {
                                theme.text_muted
                            })
                            .child(role),
                    )
                    .child(
                        div()
                            .font_family(theme.font_mono.clone())
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(short_id(&ticket.id)),
                    )
                    .child(div().flex_1())
                    .child(toolbar_chip(
                        "ticket-detail-edit",
                        "Edit",
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            let kind = if edit_ticket.kind == "epic" {
                                "epic"
                            } else {
                                "issue"
                            };
                            this.open_ticket_form(Some(edit_ticket.clone()), kind, None, cx);
                        }),
                    ))
                    .when(!full_width, |row| {
                        row.child(toolbar_chip(
                            "ticket-detail-close",
                            "Close",
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.close_ticket(cx);
                            }),
                        ))
                    }),
            )
            .child(
                div()
                    .text_size(ui_rems(23.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(ticket.title.clone()),
            );
        if let Some(parent) = parent.as_ref() {
            let parent_id = parent.id.clone();
            body = body.child(
                div()
                    .id("ticket-parent-link")
                    .flex()
                    .gap(px(7.0))
                    .text_size(ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_ticket(parent_id.clone(), cx);
                    }))
                    .child(icon(icons::ALT_ARROW_LEFT).size(px(12.0)))
                    .child(format!("In {}", parent.title)),
            );
        }
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap(px(7.0))
                .child(detail_label("DESCRIPTION", theme))
                .child(
                    div()
                        .text_size(ui_rems(12.0))
                        .text_color(if ticket.description.is_empty() {
                            theme.text_faint
                        } else {
                            theme.text_muted
                        })
                        .child(if ticket.description.is_empty() {
                            "Add a description to give the agent context.".to_owned()
                        } else {
                            ticket.description.clone()
                        }),
                ),
        );
        let mut children_section = div().flex().flex_col().gap(px(7.0)).child(
            div()
                .flex()
                .items_center()
                .child(detail_label(
                    if ticket.kind == "epic" {
                        "ISSUES IN EPIC"
                    } else {
                        "SUBISSUES"
                    },
                    theme,
                ))
                .child(div().flex_1())
                .child(toolbar_chip(
                    "ticket-new-subissue",
                    "＋ Add",
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.open_ticket_form(None, "issue", Some(child_parent.clone()), cx)
                    }),
                )),
        );
        if children.is_empty() {
            children_section = children_section.child(empty_line("No child issues yet", theme));
        } else {
            for child in children {
                let child_id = child.id.clone();
                children_section = children_section.child(
                    div()
                        .id(SharedString::from(format!("ticket-child-{child_id}")))
                        .h(px(34.0))
                        .px(px(9.0))
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .rounded(px(Theme::CONTROL_RADIUS))
                        .border_1()
                        .border_color(theme.border)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.element_hover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_ticket(child_id.clone(), cx);
                        }))
                        .child(
                            div()
                                .size(px(7.0))
                                .rounded_full()
                                .bg(status_tone(&child.status, theme)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_size(ui_rems(11.0))
                                .text_color(theme.text)
                                .child(child.title.clone()),
                        )
                        .child(
                            div()
                                .font_family(theme.font_mono.clone())
                                .text_size(ui_rems(10.0))
                                .text_color(theme.text_faint)
                                .child(short_id(&child.id)),
                        ),
                );
            }
        }
        body = body.child(children_section);
        let mut chats_section = div().flex().flex_col().gap(px(7.0)).child(
            div()
                .flex()
                .items_center()
                .child(detail_label("AGENT THREADS", theme))
                .child(div().flex_1())
                .child(toolbar_chip(
                    "ticket-link-chat",
                    "＋ Link",
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        this.dialog = Some(TicketDialog::LinkChat {
                            ticket_id: link_ticket_id.clone(),
                        });
                        cx.notify();
                    }),
                )),
        );
        if let Some(space_id) = start_space {
            chats_section = chats_section.child(primary_button(
                "Start chat for this ticket",
                "ticket-start-chat",
                theme,
                cx.listener(move |_, _, _, cx| {
                    cx.emit(TicketsEvent::StartChat {
                        ticket_id: new_chat_ticket_id.clone(),
                        space_id: space_id.clone(),
                    })
                }),
            ));
        } else if let Some(space_id) = available_space {
            let board_id = board_id.clone();
            chats_section = chats_section.child(toolbar_chip(
                "ticket-link-project",
                "Link current project to start a chat",
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.mutate(
                        json!({"op":"linkBoardSpace","boardId":board_id,"spaceId":space_id}),
                        cx,
                    );
                }),
            ));
        }
        if linked_chats.is_empty() {
            chats_section = chats_section.child(empty_line(
                "Link a conversation to track execution here.",
                theme,
            ));
        } else {
            for (chat_id, depth) in linked_chats {
                let linked = links.contains(&chat_id);
                chats_section = chats_section.child(self.render_chat_row(
                    ticket_id.clone(),
                    chat_id.clone(),
                    linked,
                    depth,
                    theme,
                    cx,
                ));
                if linked {
                    let unlink_id = ticket_id.clone();
                    let unlink_chat = chat_id.clone();
                    chats_section = chats_section.child(div()
                        .id(SharedString::from(format!("ticket-unlink-{chat_id}")))
                        .ml(px(9.0)).text_size(ui_rems(10.0)).text_color(theme.text_faint)
                        .cursor_pointer().hover(|style| style.text_color(theme.danger))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.mutate(json!({"op":"unlinkTicketChat","ticketId":unlink_id,"chatId":unlink_chat}), cx);
                        }))
                        .child("Unlink from ticket"));
                }
            }
        }
        body = body.child(chats_section);
        let mut activity = div()
            .flex()
            .flex_col()
            .gap(px(9.0))
            .child(detail_label("ACTIVITY", theme));
        for comment in comments {
            let label = comment.author.clone().unwrap_or_else(|| "Comment".into());
            let comment_id = comment.id.clone();
            activity = activity.child(
                div()
                    .p(px(10.0))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_card)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(ui_rems(10.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text_muted)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!(
                                        "ticket-comment-delete-{comment_id}"
                                    )))
                                    .text_size(ui_rems(10.0))
                                    .text_color(theme.text_faint)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(theme.danger))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.mutate(
                                            json!({"op":"deleteComment","commentId":comment_id}),
                                            cx,
                                        );
                                    }))
                                    .child("Remove"),
                            ),
                    )
                    .child(
                        div()
                            .mt(px(6.0))
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text)
                            .child(comment.body),
                    ),
            );
        }
        activity = activity
            .child(
                div()
                    .min_h(px(54.0))
                    .p(px(7.0))
                    .rounded(px(Theme::CONTROL_RADIUS))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface_card)
                    .child(self.comment.clone()),
            )
            .child(primary_button(
                "Post comment",
                "ticket-post-comment",
                theme,
                cx.listener(|this, _, _, cx| this.submit_comment(cx)),
            ));
        body = body.child(activity);

        let status_tone = status_tone(&ticket.status, theme);
        let priority_tone = priority_tone(&ticket.priority, theme);
        let rail = div()
            .w(px(166.0))
            .h_full()
            .flex_none()
            .p(px(13.0))
            .flex()
            .flex_col()
            .gap(px(15.0))
            .border_l_1()
            .border_color(theme.border)
            .child(detail_label("PROPERTIES", theme))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(detail_label("Status", theme))
                    .child(property_button(
                        "ticket-detail-status",
                        status_label(&ticket.status),
                        status_tone,
                        theme,
                        cx.listener(move |this, _, _, cx| this.cycle_status(&status_ticket, cx)),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(detail_label("Priority", theme))
                    .child(property_button(
                        "ticket-detail-priority",
                        priority_label(&ticket.priority),
                        priority_tone,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            this.cycle_priority(&priority_ticket, cx)
                        }),
                    )),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(detail_label("Type", theme))
                    .child(
                        div()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(if ticket.kind == "epic" {
                                "Epic"
                            } else if parent.as_ref().is_some_and(|parent| parent.kind == "issue") {
                                "Subissue"
                            } else {
                                "Issue"
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(detail_label("Board", theme))
                    .child(
                        div()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(
                                self.snapshot
                                    .boards
                                    .iter()
                                    .find(|board| board.id == board_id)
                                    .map(|board| board.name.clone())
                                    .unwrap_or_default(),
                            ),
                    ),
            )
            .child(div().flex_1())
            .child(toolbar_chip(
                "ticket-close",
                if ticket.archived { "Restore ticket" } else { "Archive ticket" },
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.mutate(
                        json!({"op":"updateTicket","ticketId":close_ticket.id,"archived":!close_ticket.archived}),
                        cx,
                    );
                    if !this.show_archived { this.selected_ticket = None; }
                    cx.notify();
                }),
            ));
        div()
            .w(px(DETAIL_WIDTH))
            .when(full_width, |pane| pane.w_full().flex_1().min_w_0())
            .h_full()
            .flex_none()
            .flex()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.bg)
            .child(body)
            .child(rail)
            .into_any_element()
    }

    fn render_dialog(
        &mut self,
        window: &Window,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.dialog.as_ref()?;
        let theme = theme.for_popup();
        let (title, fields, save): (&str, AnyElement, &str) = match dialog {
            TicketDialog::Board { id, name } => (
                if id.is_some() {
                    "Rename board"
                } else {
                    "New board"
                },
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(detail_label("NAME", &theme))
                    .child(popover::dialog_field(name.clone().into_any_element()))
                    .child(
                        div()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child("Boards keep tickets independent of a device or checkout."),
                    )
                    .into_any_element(),
                if id.is_some() {
                    "Save board"
                } else {
                    "Create board"
                },
            ),
            TicketDialog::Ticket {
                id,
                kind,
                parent_id,
                title,
                description,
            } => (
                if id.is_some() {
                    "Edit ticket"
                } else if *kind == "epic" {
                    "New epic"
                } else if parent_id.is_some() {
                    "New subissue"
                } else {
                    "New issue"
                },
                div()
                    .flex()
                    .flex_col()
                    .gap(px(9.0))
                    .child(detail_label("TITLE", &theme))
                    .child(popover::dialog_field(title.clone().into_any_element()))
                    .child(detail_label("DESCRIPTION", &theme))
                    .child(div().min_h(px(100.0)).child(popover::dialog_field(
                        description.clone().into_any_element(),
                    )))
                    .into_any_element(),
                if id.is_some() {
                    "Save changes"
                } else {
                    "Create ticket"
                },
            ),
            TicketDialog::LinkChat { ticket_id } => {
                let already: HashSet<_> = self
                    .snapshot
                    .chat_links
                    .iter()
                    .filter(|link| &link.ticket_id == ticket_id)
                    .map(|link| link.chat_id.as_str())
                    .collect();
                let chats: Vec<_> = self
                    .state
                    .read(cx)
                    .chats
                    .iter()
                    .filter(|chat| !chat.archived && !already.contains(chat.id.as_str()))
                    .take(80)
                    .cloned()
                    .collect();
                let mut list = div()
                    .id("ticket-chat-picker-list")
                    .max_h(px(360.0))
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .gap(px(4.0));
                for chat in chats {
                    let id = chat.id.clone();
                    let target = ticket_id.clone();
                    let title = chat.title.clone().unwrap_or_else(|| "New session".into());
                    list = list.child(
                        div()
                            .id(SharedString::from(format!("ticket-link-choice-{id}")))
                            .h(px(34.0))
                            .px(px(9.0))
                            .flex()
                            .items_center()
                            .gap(px(7.0))
                            .rounded(px(Theme::CONTROL_RADIUS))
                            .cursor_pointer()
                            .hover(|style| style.bg(theme.element_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.mutate(
                                    json!({"op":"linkTicketChat","ticketId":target,"chatId":id}),
                                    cx,
                                );
                                this.dialog = None;
                                cx.notify();
                            }))
                            .child(
                                icon(if chat.parent_chat_id.is_some() {
                                    icons::BOT
                                } else {
                                    icons::CHAT_ROUND_LINE
                                })
                                .size(px(14.0))
                                .text_color(theme.text_muted),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(ui_rems(11.0))
                                    .text_color(theme.text)
                                    .child(title),
                            ),
                    );
                }
                ("Link conversation", list.into_any_element(), "")
            }
        };
        let linking = matches!(dialog, TicketDialog::LinkChat { .. });
        let card = popover::dialog_card(&theme)
            .w(px(if linking { 480.0 } else { 520.0 }))
            .child(popover::dialog_title(&theme, title))
            .child(div().mt(px(15.0)).child(fields))
            .child(
                div()
                    .mt(px(16.0))
                    .flex()
                    .justify_end()
                    .gap(px(8.0))
                    .child(
                        popover::btn_ghost(&theme, "Cancel", "ticket-dialog-cancel")
                            .id("ticket-dialog-cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.dialog = None;
                                cx.notify();
                            })),
                    )
                    .when(!linking, |row| {
                        row.child(
                            popover::btn_primary(&theme, save)
                                .id("ticket-dialog-save")
                                .on_click(cx.listener(|this, _, _, cx| this.submit_dialog(cx))),
                        )
                    }),
            )
            .into_any_element();
        Some(popover::modal(
            "ticket-dialog",
            window.viewport_size(),
            card,
        ))
    }
}

impl Render for TicketsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let reduced_motion = motion::reduced_motion(cx);
        if let Some(tween) = self.detail_tween {
            let duration = motion::RESIZE.total().mul_f32(motion::speed_scale());
            if reduced_motion || tween.started.elapsed() >= duration {
                self.detail_tween = None;
                if tween.to <= 0.0 {
                    self.selected_ticket = None;
                }
            } else {
                window.request_animation_frame();
            }
        }
        let detail_width = self.detail_width(reduced_motion);
        let board = self
            .selected_board
            .as_deref()
            .and_then(|id| self.snapshot.boards.iter().find(|board| board.id == id))
            .cloned();
        let visible = self.visible_tickets(cx);
        let all_count = board.as_ref().map_or(0, |board| {
            self.snapshot
                .tickets
                .iter()
                .filter(|ticket| {
                    ticket.board_id == board.id
                        && match self.archive_filter {
                            ArchiveFilter::Active => !ticket.archived,
                            ArchiveFilter::Archived => ticket.archived,
                            ArchiveFilter::All => true,
                        }
                })
                .count()
        });
        let selected = self
            .selected_ticket
            .as_deref()
            .and_then(|id| {
                self.snapshot
                    .tickets
                    .iter()
                    .find(|ticket| ticket.id == id && (self.show_archived || !ticket.archived))
            })
            .cloned();
        let error = self.error.clone();
        let dialog = self.render_dialog(window, &theme, cx);
        let board_tabs = self.render_board_tabs(&theme, cx);
        let mut root = div()
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(theme.bg.opacity(0.72))
            .text_color(theme.text)
            .child(
                div()
                    .h(px(55.0))
                    .px(px(18.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        icon(icons::CHECKLIST)
                            .size(px(17.0))
                            .text_color(theme.accent),
                    )
                    .child(
                        div()
                            .text_size(ui_rems(17.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("Tasks"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(if self.loaded {
                                format!("{all_count} tickets")
                            } else {
                                "Connecting…".into()
                            }),
                    ),
            )
            .child(
                div()
                    .h(px(42.0))
                    .flex_none()
                    .px(px(11.0))
                    .flex()
                    .items_center()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(board_tabs),
            );
        if let Some(error) = error {
            root = root.child(
                div()
                    .flex_none()
                    .px(px(16.0))
                    .py(px(7.0))
                    .bg(theme.danger.opacity(0.10))
                    .text_color(theme.danger)
                    .text_size(ui_rems(11.0))
                    .child(error),
            );
        }
        if !self.loaded {
            root = root.child(centered_empty(
                "Loading tickets",
                "Connecting to the ticket store…",
                &theme,
            ));
        } else if board.is_none() {
            root = root.child(div().flex_1().flex().items_center().justify_center()
                .child(div().w(px(420.0)).p(px(28.0)).flex().flex_col().items_start().gap(px(13.0))
                    .rounded(px(Theme::PANEL_RADIUS)).border_1().border_color(theme.border)
                    .bg(theme.surface_card)
                    .child(icon(icons::WIDGET).size(px(24.0)).text_color(theme.accent))
                    .child(div().text_size(ui_rems(18.0)).font_weight(gpui::FontWeight::SEMIBOLD)
                        .child("Plan work in one place"))
                    .child(div().text_size(ui_rems(12.0)).text_color(theme.text_muted)
                        .child("Create a board for epics, issues, subissues and linked agent threads."))
                    .child(primary_button("Create a board", "ticket-empty-create-board", &theme,
                        cx.listener(|this, _, _, cx| this.create_board(cx))))));
        } else {
            let board = board.unwrap();
            let board_archived = board.archived;
            let edit_board = board.clone();
            let archive_board = board.clone();
            let epic_count = self
                .snapshot
                .tickets
                .iter()
                .filter(|ticket| {
                    !ticket.archived && ticket.board_id == board.id && ticket.kind == "epic"
                })
                .count();
            let done_count = self
                .snapshot
                .tickets
                .iter()
                .filter(|ticket| {
                    !ticket.archived && ticket.board_id == board.id && ticket.status == "done"
                })
                .count();
            root = root
                .child(
                    div()
                        .h(px(69.0))
                        .flex_none()
                        .px(px(18.0))
                        .flex()
                        .items_center()
                        .gap(px(14.0))
                        .border_b_1()
                        .border_color(theme.border)
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .child(
                                    div()
                                        .truncate()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .text_size(ui_rems(16.0))
                                        .child(board.name),
                                )
                                .child(
                                    div()
                                        .text_size(ui_rems(11.0))
                                        .text_color(theme.text_faint)
                                        .child(if board.description.is_empty() {
                                            format!("{epic_count} epics · {done_count} complete")
                                        } else {
                                            board.description
                                        }),
                                ),
                        )
                        .child(toolbar_chip(
                            "ticket-edit-board",
                            "Rename",
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.edit_board(edit_board.clone(), cx)
                            }),
                        ))
                        .child(toolbar_chip(
                            "ticket-archive-board",
                            if board_archived {
                                "Restore board"
                            } else {
                                "Archive board"
                            },
                            &theme,
                            cx.listener(move |this, _, _, cx| {
                                this.mutate(
                                    json!({"op":"updateBoard","boardId":archive_board.id,
                                    "archived":!archive_board.archived}),
                                    cx,
                                );
                                if !this.show_archived {
                                    this.selected_ticket = None;
                                }
                                cx.notify();
                            }),
                        ))
                        .when(!board_archived, |header| {
                            header.child(toolbar_chip(
                                "ticket-new-epic",
                                "＋ Epic",
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.open_ticket_form(None, "epic", None, cx)
                                }),
                            ))
                        }),
                )
                .child(self.render_toolbar(&theme, cx));
            let content = if visible.is_empty() {
                centered_empty(
                    "No matching tickets",
                    "Create an issue or change the search and filters.",
                    &theme,
                )
            } else if self.layout == Layout::Board {
                self.render_board(&visible, &theme, cx)
            } else {
                div()
                    .id("ticket-list-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(self.render_list(&visible, &theme, cx))
                    .into_any_element()
            };
            let narrow = f32::from(window.viewport_size().width) < 1220.0;
            let detail = selected.map(|ticket| self.render_detail(ticket, narrow, &theme, cx));
            if narrow {
                root = root.child(
                    div()
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .child(div().size_full().child(content))
                        .children(detail.map(|pane| {
                            div().absolute().inset_0().overflow_hidden().child(
                                div()
                                    .absolute()
                                    .top_0()
                                    .bottom_0()
                                    .w_full()
                                    .left(gpui::relative(1.0 - detail_width / DETAIL_WIDTH))
                                    .child(pane),
                            )
                        })),
                );
            } else {
                root = root.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .child(div().flex_1().min_w_0().child(content))
                        .children(detail.map(|pane| {
                            div()
                                .w(px(detail_width))
                                .h_full()
                                .flex_none()
                                .relative()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .h_full()
                                        .w(px(DETAIL_WIDTH))
                                        .child(pane),
                                )
                        })),
                );
            }
        }
        root.children(dialog).into_any_element()
    }
}

fn status_label(status: &str) -> &'static str {
    match status {
        "backlog" => "Backlog",
        "todo" => "Todo",
        "inProgress" => "In progress",
        "inReview" => "In review",
        "done" => "Done",
        "canceled" => "Canceled",
        _ => "Unknown",
    }
}

fn priority_label(priority: &str) -> &'static str {
    match priority {
        "urgent" => "Urgent",
        "high" => "High",
        "medium" => "Medium",
        "low" => "Low",
        _ => "No priority",
    }
}

fn status_tone(status: &str, theme: &Theme) -> gpui::Hsla {
    match status {
        "done" => theme.success,
        "inProgress" | "inReview" => theme.accent,
        "canceled" => theme.danger,
        _ => theme.text_muted,
    }
}

fn priority_tone(priority: &str, theme: &Theme) -> gpui::Hsla {
    match priority {
        "urgent" => theme.danger,
        "high" => theme.warning,
        "medium" => theme.accent,
        _ => theme.text_muted,
    }
}

fn short_id(id: &str) -> String {
    format!("#{}", id.chars().take(7).collect::<String>().to_uppercase())
}

fn ordered_ticket_tree(tickets: &[Ticket]) -> (Vec<(Ticket, usize)>, Vec<(Ticket, usize)>) {
    let ids: HashSet<_> = tickets.iter().map(|ticket| ticket.id.as_str()).collect();
    let mut children: HashMap<String, Vec<Ticket>> = HashMap::new();
    for ticket in tickets {
        if let Some(parent) = ticket.parent_ticket_id.as_ref()
            && ids.contains(parent.as_str())
            && parent != &ticket.id
        {
            children
                .entry(parent.clone())
                .or_default()
                .push(ticket.clone());
        }
    }
    for siblings in children.values_mut() {
        siblings.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    }
    fn append(
        ticket: Ticket,
        depth: usize,
        children: &HashMap<String, Vec<Ticket>>,
        seen: &mut HashSet<String>,
        output: &mut Vec<(Ticket, usize)>,
    ) {
        if !seen.insert(ticket.id.clone()) {
            return;
        }
        let id = ticket.id.clone();
        output.push((ticket, depth));
        for child in children.get(&id).into_iter().flatten() {
            append(child.clone(), depth + 1, children, seen, output);
        }
    }
    let mut seen = HashSet::new();
    let mut epics = Vec::new();
    let mut epic_roots: Vec<_> = tickets
        .iter()
        .filter(|ticket| ticket.kind == "epic")
        .cloned()
        .collect();
    epic_roots.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    for epic in epic_roots {
        append(epic, 0, &children, &mut seen, &mut epics);
    }
    let mut issues = Vec::new();
    let mut roots: Vec<_> = tickets
        .iter()
        .filter(|ticket| {
            !seen.contains(&ticket.id)
                && ticket
                    .parent_ticket_id
                    .as_ref()
                    .is_none_or(|parent| !ids.contains(parent.as_str()))
        })
        .cloned()
        .collect();
    roots.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    for root in roots {
        append(root, 0, &children, &mut seen, &mut issues);
    }
    // A malformed cycle has no root. Show each remaining ticket once.
    let mut remaining: Vec<_> = tickets
        .iter()
        .filter(|ticket| !seen.contains(&ticket.id))
        .cloned()
        .collect();
    remaining.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    for ticket in remaining {
        append(ticket, 0, &children, &mut seen, &mut issues);
    }
    (epics, issues)
}

fn section_label(label: &'static str, count: usize, theme: &Theme) -> AnyElement {
    div()
        .h(px(34.0))
        .flex_none()
        .px(px(13.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .bg(theme.surface.opacity(0.25))
        .text_size(ui_rems(10.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_muted)
        .child(label)
        .child(count.to_string())
        .into_any_element()
}

fn detail_label(label: &'static str, theme: &Theme) -> AnyElement {
    div()
        .text_size(ui_rems(10.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(theme.text_faint)
        .child(label)
        .into_any_element()
}

fn empty_line(label: &'static str, theme: &Theme) -> AnyElement {
    div()
        .px(px(9.0))
        .py(px(10.0))
        .rounded(px(Theme::CONTROL_RADIUS))
        .border_1()
        .border_color(theme.border)
        .text_size(ui_rems(11.0))
        .text_color(theme.text_faint)
        .child(label)
        .into_any_element()
}

fn centered_empty(title: &'static str, subtitle: &'static str, theme: &Theme) -> AnyElement {
    div()
        .flex_1()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(7.0))
                .child(
                    icon(icons::CHECKLIST)
                        .size(px(25.0))
                        .text_color(theme.text_faint),
                )
                .child(
                    div()
                        .text_size(ui_rems(15.0))
                        .text_color(theme.text)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(ui_rems(11.0))
                        .text_color(theme.text_faint)
                        .child(subtitle),
                ),
        )
        .into_any_element()
}

fn toolbar_chip(
    id: &'static str,
    label: &'static str,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(27.0))
        .px(px(9.0))
        .flex_none()
        .flex()
        .items_center()
        .rounded(px(Theme::CONTROL_RADIUS))
        .border_1()
        .border_color(theme.border)
        .text_size(ui_rems(11.0))
        .text_color(theme.text_muted)
        .cursor_pointer()
        .hover(|style| style.bg(theme.element_hover).text_color(theme.text))
        .on_click(on_click)
        .child(label)
}

fn primary_button(
    label: &'static str,
    id: &'static str,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(29.0))
        .px(px(12.0))
        .flex_none()
        .flex()
        .items_center()
        .rounded(px(Theme::CONTROL_RADIUS))
        .bg(theme.solid)
        .text_color(theme.on_solid)
        .text_size(ui_rems(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|style| style.opacity(0.83))
        .on_click(on_click)
        .child(label)
}

fn property_button(
    id: &'static str,
    label: &'static str,
    color: gpui::Hsla,
    theme: &Theme,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(29.0))
        .px(px(8.0))
        .flex()
        .items_center()
        .gap(px(7.0))
        .rounded(px(Theme::CONTROL_RADIUS))
        .border_1()
        .border_color(theme.border)
        .cursor_pointer()
        .hover(|style| style.bg(theme.element_hover))
        .on_click(on_click)
        .child(div().size(px(7.0)).rounded_full().bg(color))
        .child(
            div()
                .text_size(ui_rems(11.0))
                .text_color(theme.text)
                .child(label),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(id: &str, parent: Option<&str>, kind: &str) -> Ticket {
        Ticket {
            id: id.into(),
            board_id: "board".into(),
            kind: kind.into(),
            parent_ticket_id: parent.map(str::to_owned),
            title: id.into(),
            description: String::new(),
            status: "todo".into(),
            priority: "none".into(),
            archived: false,
        }
    }

    #[test]
    fn ticket_tree_keeps_subissues_beside_parents_and_survives_cycles() {
        let rows = [
            ticket("Epic", None, "epic"),
            ticket("Issue", Some("Epic"), "issue"),
            ticket("Child", Some("Issue"), "issue"),
            ticket("Free", None, "issue"),
            ticket("CycleA", Some("CycleB"), "issue"),
            ticket("CycleB", Some("CycleA"), "issue"),
        ];
        let (epics, issues) = ordered_ticket_tree(&rows);
        assert_eq!(
            epics
                .iter()
                .map(|(ticket, depth)| (ticket.id.as_str(), *depth))
                .collect::<Vec<_>>(),
            [("Epic", 0), ("Issue", 1), ("Child", 2)]
        );
        assert_eq!(
            issues
                .iter()
                .map(|(ticket, _)| ticket.id.as_str())
                .collect::<Vec<_>>(),
            ["Free", "CycleA", "CycleB"]
        );
    }
}
