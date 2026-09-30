//! Device file transfers on the desktop (docs/file-transfer.md): the
//! "Send to device…" picker (file tree context menu and command palette), the
//! titlebar transfers indicator with its panel, and the incoming-transfer
//! toast. Presentation strings and state mapping live in
//! [`crate::file_transfers`]; this file owns rendering and the RPCs.
//!
//! The desktop's own engine feed arrives through `AppState::file_transfers`.
//! When the user sends from a project hosted on another device, that host is
//! the sender, so its feed is watched here too (forwarded with
//! `targetDeviceId`) for as long as this window lives.

use super::*;
use crate::file_transfers::{
    self as ft, IncomingNotice, IncomingNotices, RowAction, SendTarget, TransferRow,
};
use zeron_proto::{FileTransfer, FileTransferDirection, FileTransferState};

const PANEL_WIDTH: f32 = 380.0;
const PANEL_LIST_MAX_HEIGHT: f32 = 420.0;
const SEND_MENU_WIDTH: f32 = 264.0;
const TOAST_WIDTH: f32 = 400.0;
const PILL_HEIGHT: f32 = 24.0;
const TOAST_SCOPE: &str = "transfer-toast";

#[derive(Default)]
pub(super) struct FileTransfersUi {
    /// Remote sending hosts' feeds, keyed by device id.
    remote: std::collections::BTreeMap<String, RemoteFeed>,
    panel: popover::Popup<()>,
    send_menu: popover::Popup<SendMenu>,
    notices: IncomingNotices,
    /// The transfer the toast follows.
    toast: Option<String>,
    toast_timer: Option<(String, Task<()>)>,
    /// Transfer ids with an action RPC in flight (buttons go inert).
    busy: std::collections::HashSet<String>,
    panel_scroll: gpui::ScrollHandle,
}

#[derive(Default)]
struct RemoteFeed {
    rows: Vec<FileTransfer>,
    watch: Option<Task<()>>,
}

#[derive(Clone)]
pub(super) struct SendMenu {
    /// The sending engine: the device hosting the files.
    host: String,
    /// Absolute paths on `host`.
    paths: Vec<String>,
    label: String,
    position: Point<Pixels>,
}

/// Why the picker can't list targets, if it can't.
fn send_blocker(state: &AppState, host: &str) -> Option<String> {
    if state.workspace_scope == Some(WorkspaceScope::Local) {
        return Some("Sign in to send files between your devices.".into());
    }
    if !state.device_supports(host, zeron_proto::capabilities::FILE_TRANSFER_V1) {
        let name = state.device_name(host).unwrap_or("This device");
        return Some(format!(
            "{name} can't send files yet. Update Zeron on it first."
        ));
    }
    None
}

/// A compact pill button in the agent-update card's style.
fn pill(theme: &Theme, primary: bool) -> gpui::Div {
    div()
        .h(px(PILL_HEIGHT))
        .px(px(9.0))
        .flex_none()
        .rounded_full()
        .border_1()
        .border_color(gpui::transparent_black())
        .flex()
        .items_center()
        .justify_center()
        .text_size(crate::typography::ui_rems(11.5))
        .font_weight(gpui::FontWeight::MEDIUM)
        .bg(if primary {
            theme.text
        } else {
            theme.element_hover
        })
        .text_color(if primary {
            theme.on_solid
        } else {
            theme.text_muted
        })
        .cursor_pointer()
        .hover(|el| el.opacity(0.85))
}

/// A 22px square icon button (panel row trailing controls).
fn icon_button(theme: &Theme, glyph: &'static str) -> gpui::Div {
    let hover = theme.element_hover;
    div()
        .size(px(22.0))
        .flex_none()
        .rounded(px(6.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |el| el.bg(hover))
        .child(icon(glyph).size(px(13.0)).text_color(theme.text_muted))
}

/// Thin progress track, filled from the left.
fn progress_bar(fraction: f32, tint: gpui::Hsla) -> gpui::Div {
    div()
        .h(px(3.0))
        .w_full()
        .rounded_full()
        .bg(crate::theme::wash(0.1))
        .overflow_hidden()
        .child(
            div()
                .h_full()
                .w(gpui::relative(fraction.clamp(0.0, 1.0)))
                .rounded_full()
                .bg(tint),
        )
}

/// The direction/outcome tile leading a panel row.
fn row_tile(theme: &Theme, transfer: &FileTransfer) -> gpui::Div {
    let (glyph, tint) = match transfer.state {
        FileTransferState::Completed => (icons::CHECK, theme.success),
        FileTransferState::Failed => (icons::DANGER_TRIANGLE, theme.danger),
        FileTransferState::Cancelled | FileTransferState::Declined => {
            (icons::CLOSE_CIRCLE, theme.text_faint)
        }
        _ => (
            match transfer.direction {
                FileTransferDirection::Incoming => icons::ARROW_DOWN,
                FileTransferDirection::Outgoing => icons::ARROW_UP,
            },
            theme.accent,
        ),
    };
    div()
        .size(px(28.0))
        .flex_none()
        .rounded(px(8.0))
        .bg(tint.opacity(0.12))
        .flex()
        .items_center()
        .justify_center()
        .child(icon(glyph).size(px(14.0)).text_color(tint))
}

impl Shell {
    /// Local feed plus the remote hosts' feeds, newest first.
    pub(super) fn file_transfer_rows(&self, cx: &App) -> Vec<TransferRow> {
        let remote: Vec<(String, Vec<FileTransfer>)> = self
            .file_transfers
            .remote
            .iter()
            .map(|(host, feed)| (host.clone(), feed.rows.clone()))
            .collect();
        ft::merge_feeds(&self.state.read(cx).file_transfers, &remote)
    }

    /// State-observer hook: drop remote watches with the runtime, and turn
    /// new incoming transfers into a toast and (in the background) a banner.
    pub(super) fn sync_file_transfer_notices(&mut self, cx: &mut Context<Self>) {
        let (ready, local) = {
            let state = self.state.read(cx);
            (
                matches!(state.connection, ConnectionStatus::Ready),
                state.file_transfers.clone(),
            )
        };
        if !ready {
            self.file_transfers.remote.clear();
            self.file_transfers.notices.reset();
            self.file_transfers.toast = None;
            return;
        }
        let notices = self.file_transfers.notices.observe(&local);
        let Some(last) = notices.last() else {
            return;
        };
        let (IncomingNotice::Arrived(id) | IncomingNotice::NeedsAcceptance(id)) = last;
        // An open panel already shows everything; the toast would duplicate it.
        if !self.file_transfers.panel.is_open() {
            self.file_transfers.toast = Some(id.clone());
        }
        let app_focused = cx.active_window().is_some();
        if self.settings.notifications_enabled
            && !(self.settings.notifications_background_only && app_focused)
        {
            for notice in &notices {
                let (IncomingNotice::Arrived(id) | IncomingNotice::NeedsAcceptance(id)) = notice;
                let Some(transfer) = local.iter().find(|t| &t.id == id) else {
                    continue;
                };
                let body = match notice {
                    IncomingNotice::NeedsAcceptance(_) => "Open Zeron to accept or decline".into(),
                    IncomingNotice::Arrived(_) => ft::status_line(transfer),
                };
                crate::notify::post(&ft::headline(transfer), &body, None);
            }
        }
        cx.notify();
    }

    /// Watch a remote sending host's feed (idempotent).
    fn ensure_remote_transfer_feed(&mut self, host: String, cx: &mut Context<Self>) {
        if self.state.read(cx).local_device_id.as_deref() == Some(host.as_str()) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let feed = self.file_transfers.remote.entry(host.clone()).or_default();
        if feed.watch.is_some() {
            return;
        }
        feed.watch = Some(cx.spawn(async move |this, cx| {
            let mut retry = 1;
            loop {
                if let Ok(mut stream) = engine
                    .client()
                    .subscribe_checked(
                        methods::WATCH_FILE_TRANSFERS,
                        serde_json::json!({ "targetDeviceId": host }),
                    )
                    .await
                {
                    while let Some(value) = stream.recv().await {
                        let Ok(rows) = serde_json::from_value::<Vec<FileTransfer>>(value) else {
                            continue;
                        };
                        let alive = this.update(cx, |shell, cx| {
                            if let Some(feed) = shell.file_transfers.remote.get_mut(&host) {
                                feed.rows = rows;
                                cx.notify();
                            }
                        });
                        if alive.is_err() {
                            return;
                        }
                        retry = 1;
                    }
                }
                if this.update(cx, |_, _| {}).is_err() {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_secs(retry))
                    .await;
                retry = (retry * 2).min(30);
            }
        }));
    }

    // ---- Send to device ----

    /// Open the device picker for a workspace-relative tree path of `chat_id`.
    pub(super) fn open_send_menu_for_path(
        &mut self,
        chat_id: &str,
        relative: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let resolved = {
            let state = self.state.read(cx);
            state
                .chats
                .iter()
                .find(|chat| chat.id == chat_id)
                .map(|chat| {
                    let local = state.local_device_id.as_deref() == Some(chat.device_id.as_str());
                    let root = chat.cwd.clone().unwrap_or_else(|| "~".into());
                    (chat.device_id.clone(), local, root)
                })
        };
        let Some((host, local, root)) = resolved else {
            return;
        };
        // A projectless chat's `~` can only be expanded here when this
        // machine is its host.
        let root = match root.strip_prefix('~') {
            Some(rest) if local => std::env::home_dir()
                .map(|home| format!("{}{rest}", home.display()))
                .unwrap_or(root),
            _ => root,
        };
        let Some(path) = ft::host_path(&root, relative) else {
            self.sidebar_notice = Some("Can't send this item: its folder isn't known".into());
            cx.notify();
            return;
        };
        let label = relative
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or("this folder")
            .to_string();
        self.file_transfers.send_menu.open(SendMenu {
            host,
            paths: vec![path],
            label,
            position,
        });
        cx.notify();
    }

    /// The command palette's "Send current file to device…": the file of
    /// the focused editor tab, picker centered under the titlebar.
    pub(super) fn open_send_menu_for_current_file(&mut self, cx: &mut Context<Self>) {
        let Some(relative) = self.current_file_path(cx) else {
            return;
        };
        let chat_id = self.active_chat.clone();
        let x = ((self.viewport_width - SEND_MENU_WIDTH) * 0.5).max(8.0);
        let position = gpui::point(px(x), px(Theme::TITLEBAR_HEIGHT + 48.0));
        self.open_send_menu_for_path(&chat_id, &relative, position, cx);
    }

    /// The focused editor tab's workspace-relative path, if any.
    pub(super) fn current_file_path(&self, cx: &App) -> Option<String> {
        if self.active_chat.is_empty() || !self.right_pane_open(cx) {
            return None;
        }
        let RightSurface::File(id) = self.resolved_right_active(cx) else {
            return None;
        };
        self.file_surface_paths.get(&id).cloned()
    }

    pub(super) fn close_send_menu(&mut self, cx: &mut Context<Self>) {
        if self.file_transfers.send_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.file_transfers.send_menu);
        }
        cx.notify();
    }

    fn send_files(&mut self, menu: SendMenu, to: SendTarget, cx: &mut Context<Self>) {
        self.close_send_menu(cx);
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let remote_host = (self.state.read(cx).local_device_id.as_deref()
            != Some(menu.host.as_str()))
        .then(|| menu.host.clone());
        let mut params = serde_json::json!({
            "toDeviceId": to.id,
            "paths": menu.paths,
        });
        if let Some(host) = &remote_host {
            params["targetDeviceId"] = serde_json::json!(host);
        }
        if let Some(host) = remote_host.clone() {
            self.ensure_remote_transfer_feed(host, cx);
        }
        cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::SEND_FILES, params).await;
            this.update(cx, |shell, cx| {
                match result {
                    Ok(_) => shell.open_file_transfers_panel(cx),
                    Err(error) => {
                        shell.sidebar_notice =
                            Some(format!("Couldn't send to {}: {error}", to.name).into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ---- Panel ----

    pub(super) fn open_file_transfers_panel(&mut self, cx: &mut Context<Self>) {
        self.file_transfers.toast = None;
        self.file_transfers.panel.open(());
        cx.notify();
    }

    pub(super) fn close_file_transfers_panel(&mut self, cx: &mut Context<Self>) {
        if self.file_transfers.panel.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.file_transfers.panel);
        }
        cx.notify();
    }

    /// Escape closes the transfers popovers first. Returns whether it did.
    pub(super) fn dismiss_file_transfer_popovers(&mut self, cx: &mut Context<Self>) -> bool {
        if self.file_transfers.send_menu.is_open() {
            self.close_send_menu(cx);
            return true;
        }
        if self.file_transfers.panel.is_open() {
            self.close_file_transfers_panel(cx);
            return true;
        }
        false
    }

    fn run_file_transfer_action(
        &mut self,
        row: &TransferRow,
        action: RowAction,
        cx: &mut Context<Self>,
    ) {
        let transfer = &row.transfer;
        let method = match action {
            RowAction::Accept => methods::ACCEPT_FILE_TRANSFER,
            RowAction::Decline => methods::DECLINE_FILE_TRANSFER,
            RowAction::Cancel => methods::CANCEL_FILE_TRANSFER,
            RowAction::Dismiss => methods::CLEAR_FILE_TRANSFERS,
            RowAction::Open => {
                if let Some(path) = ft::open_target(transfer) {
                    cx.open_with_system(std::path::Path::new(&path));
                }
                return;
            }
            RowAction::ShowInFolder => {
                if let Some(path) = ft::reveal_target(transfer) {
                    cx.reveal_path(std::path::Path::new(&path));
                }
                return;
            }
        };
        if !self.file_transfers.busy.insert(transfer.id.clone()) {
            return;
        }
        let mut params = serde_json::json!({ "transferId": transfer.id });
        if let Some(host) = &row.host {
            params["targetDeviceId"] = serde_json::json!(host);
        }
        self.call_file_transfer_rpc(method, params, Some(transfer.id.clone()), cx);
    }

    fn clear_finished_file_transfers(&mut self, cx: &mut Context<Self>) {
        for host in ft::hosts_with_finished(&self.file_transfer_rows(cx)) {
            let mut params = serde_json::json!({});
            if let Some(host) = host {
                params["targetDeviceId"] = serde_json::json!(host);
            }
            self.call_file_transfer_rpc(methods::CLEAR_FILE_TRANSFERS, params, None, cx);
        }
    }

    fn call_file_transfer_rpc(
        &mut self,
        method: &'static str,
        params: serde_json::Value,
        busy: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            if let Some(id) = busy {
                self.file_transfers.busy.remove(&id);
            }
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = engine.client().call(method, params).await;
            this.update(cx, |shell, cx| {
                if let Some(id) = &busy {
                    shell.file_transfers.busy.remove(id);
                }
                if let Err(error) = result {
                    shell.sidebar_notice = Some(format!("File transfer: {error}").into());
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Trailing controls of a panel row (or the toast).
    fn render_transfer_actions(
        &mut self,
        row: &TransferRow,
        scope: &'static str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let busy = self.file_transfers.busy.contains(&row.transfer.id);
        let id = row.transfer.id.clone();
        let mut out = div().flex_none().flex().items_center().gap(px(4.0));
        // The toast closes itself; its rows' history stays.
        let toast = scope == TOAST_SCOPE;
        for action in ft::actions(row)
            .into_iter()
            .filter(|action| !(toast && *action == RowAction::Dismiss))
        {
            let label = ft::action_label(action);
            let element_id = SharedString::from(format!("{scope}-{action:?}-{id}"));
            let row = row.clone();
            let control = match action {
                RowAction::ShowInFolder => icon_button(theme, icons::FOLDER),
                RowAction::Dismiss => icon_button(theme, icons::CLOSE),
                RowAction::Accept => pill(theme, true).child(label),
                _ => pill(theme, false).child(label),
            };
            let tooltip_label: SharedString = label.into();
            out = out.child(
                control
                    .id(element_id)
                    .role(gpui::Role::Button)
                    .aria_label(format!("{label} · {}", ft::title(&row.transfer)))
                    .when(busy, |el| el.opacity(0.45).cursor_default())
                    .when(
                        matches!(action, RowAction::ShowInFolder | RowAction::Dismiss),
                        |el| {
                            el.tooltip(move |_, cx| {
                                cx.new(|_| SurfaceTabTooltip {
                                    text: tooltip_label.clone(),
                                })
                                .into()
                            })
                        },
                    )
                    .when(!busy, |el| {
                        el.on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.run_file_transfer_action(&row, action, cx);
                        }))
                    }),
            );
        }
        out
    }

    fn render_transfer_row(
        &mut self,
        row: &TransferRow,
        first: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let transfer = &row.transfer;
        let failed = transfer.state == FileTransferState::Failed;
        let status = ft::status_line(transfer);
        let actions = self.render_transfer_actions(row, "transfer-row", theme, cx);
        let host_note = row.host.as_ref().map(|host| {
            let name = self
                .state
                .read(cx)
                .device_name(host)
                .unwrap_or(host)
                .to_string();
            format!("on {name}")
        });
        let mut peer = ft::peer_line(transfer);
        if let Some(note) = host_note {
            peer = format!("{peer} · {note}");
        }
        div()
            .id(SharedString::from(format!("transfer-row-{}", transfer.id)))
            .relative()
            .px(px(10.0))
            .py(px(9.0))
            .flex()
            .items_center()
            .gap(px(10.0))
            .when(!first, |el| {
                el.child(
                    div()
                        .absolute()
                        .top_0()
                        .left(px(48.0))
                        .right(px(10.0))
                        .h(px(1.0))
                        .bg(settings::widgets::row_divider(theme)),
                )
            })
            .child(row_tile(theme, transfer))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(12.5))
                            .line_height(crate::typography::ui_rems(16.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .truncate()
                            .child(ft::title(transfer)),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .line_height(crate::typography::ui_rems(14.0))
                            .text_color(theme.text_muted)
                            .truncate()
                            .child(peer),
                    )
                    .when(ft::shows_progress(transfer), |el| {
                        el.child(
                            div()
                                .py(px(3.0))
                                .child(progress_bar(transfer.fraction(), theme.accent)),
                        )
                    })
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .line_height(crate::typography::ui_rems(14.0))
                            .text_color(if failed {
                                theme.danger
                            } else {
                                theme.text_faint
                            })
                            .truncate()
                            .child(status.clone()),
                    ),
            )
            .child(actions)
            // A failure's full message rarely fits one line.
            .when(failed, |el| {
                let text: SharedString = status.into();
                el.tooltip(move |_, cx| cx.new(|_| SurfaceTabTooltip { text: text.clone() }).into())
            })
            .into_any_element()
    }

    fn render_file_transfers_panel(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.file_transfers.panel.get()?;
        let closing = self.file_transfers.panel.closing_since();
        let theme = Theme::of(cx).for_popup();
        let rows = self.file_transfer_rows(cx);
        let has_finished = rows.iter().any(|row| !row.transfer.is_live());
        let list: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(ix, row)| self.render_transfer_row(row, ix == 0, &theme, cx))
            .collect();
        let body = if list.is_empty() {
            div()
                .px(px(10.0))
                .py(px(18.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_faint)
                .child(
                    "Nothing yet. Right-click a file or folder in a project and choose \
                     Send to device…",
                )
                .into_any_element()
        } else {
            crate::edge_fade::edge_faded(
                16.0,
                true,
                true,
                div()
                    .id("file-transfers-list")
                    .max_h(px(PANEL_LIST_MAX_HEIGHT))
                    .overflow_y_scroll()
                    .track_scroll(&self.file_transfers.panel_scroll)
                    .flex()
                    .flex_col()
                    .children(list),
            )
            .fade_overflow_y(&self.file_transfers.panel_scroll)
            .into_any_element()
        };
        let header = div()
            .h(px(32.0))
            .pl(px(10.0))
            .pr(px(4.0))
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_size(crate::typography::ui_rems(12.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .child("Transfers"),
            )
            .when(has_finished, |el| {
                el.child(
                    settings::widgets::text_action(
                        &theme,
                        settings::widgets::ActionTone::Quiet,
                        "Clear finished",
                    )
                    .min_h(px(24.0))
                    .py(px(2.0))
                    .text_size(crate::typography::ui_rems(11.5))
                    .id("file-transfers-clear")
                    .role(gpui::Role::Button)
                    .on_click(cx.listener(|this, _, _, cx| this.clear_finished_file_transfers(cx))),
                )
            });
        let card = popover::popover_card(&theme)
            .id("file-transfers-panel")
            .w(px(PANEL_WIDTH))
            .flex()
            .flex_col()
            .role(gpui::Role::Dialog)
            .aria_label("File transfers")
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_file_transfers_panel(cx)))
            .child(header)
            .child(popover::menu_separator())
            .child(body)
            .into_any_element();
        Some(popover::anchored_menu_below(
            "file-transfers-panel-layer",
            card,
            closing,
        ))
    }

    /// The incoming-transfer toast, hanging under the indicator.
    fn render_file_transfer_toast(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.file_transfers.panel.get().is_some() {
            return None;
        }
        let id = self.file_transfers.toast.clone()?;
        let row = self
            .file_transfer_rows(cx)
            .into_iter()
            .find(|row| row.host.is_none() && row.transfer.id == id);
        let Some(row) = row else {
            self.file_transfers.toast = None;
            return None;
        };
        let transfer = &row.transfer;
        if !transfer.is_live() {
            let finished = transfer.finished_at.unwrap_or(transfer.updated_at);
            let left = ft::TOAST_LINGER_MS - (Utc::now().timestamp_millis() - finished);
            if left <= 0 {
                self.file_transfers.toast = None;
                return None;
            }
            if self
                .file_transfers
                .toast_timer
                .as_ref()
                .is_none_or(|(timer_id, _)| *timer_id != id)
            {
                let timer_id = id.clone();
                let task = cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(left as u64 + 50))
                        .await;
                    this.update(cx, |shell, cx| {
                        if shell.file_transfers.toast.as_deref() == Some(timer_id.as_str()) {
                            shell.file_transfers.toast = None;
                        }
                        shell.file_transfers.toast_timer = None;
                        cx.notify();
                    })
                    .ok();
                });
                self.file_transfers.toast_timer = Some((id.clone(), task));
            }
        }
        let theme = Theme::of(cx).for_settings_surface();
        let glyph = ft::platform_icon(
            self.state
                .read(cx)
                .devices
                .iter()
                .find(|d| d.id == transfer.peer_device_id)
                .map_or("", |d| d.platform.as_str()),
        );
        let failed = transfer.state == FileTransferState::Failed;
        let actions = self.render_transfer_actions(&row, TOAST_SCOPE, &theme, cx);
        let awaiting = transfer.state == FileTransferState::AwaitingAcceptance;
        let card = div()
            .id("file-transfer-toast")
            .w(px(TOAST_WIDTH))
            .rounded(px(16.0))
            .overflow_hidden()
            .border_1()
            .border_color(theme.border.opacity(0.7))
            .text_color(theme.text)
            .bg(popover::surface_bg(&theme))
            .when(!theme.is_frost(), |el| el.shadow_lg())
            .role(gpui::Role::Alert)
            .aria_label(ft::headline(transfer))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.open_file_transfers_panel(cx)))
            .child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        icon(glyph)
                            .size(px(18.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .line_height(crate::typography::ui_rems(16.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .truncate()
                                    .child(ft::headline(transfer)),
                            )
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .line_height(crate::typography::ui_rems(14.0))
                                    .text_color(if failed {
                                        theme.danger
                                    } else {
                                        theme.text_muted
                                    })
                                    .truncate()
                                    .child(ft::toast_detail(transfer)),
                            ),
                    )
                    .child(actions)
                    .when(!awaiting, |el| {
                        el.child(
                            icon_button(&theme, icons::CLOSE)
                                .id("file-transfer-toast-close")
                                .role(gpui::Role::Button)
                                .aria_label("Dismiss")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.file_transfers.toast = None;
                                    cx.notify();
                                })),
                        )
                    }),
            )
            .when(ft::shows_progress(transfer), |el| {
                el.child(
                    div()
                        .px(px(12.0))
                        .pb(px(10.0))
                        .child(progress_bar(transfer.fraction(), theme.accent)),
                )
            });
        let card = crate::frost::frosted(16.0, 16.0, card).into_any_element();
        Some(popover::anchored_menu_below(
            "file-transfer-toast-layer",
            card,
            None,
        ))
    }

    /// The titlebar indicator: shown while transfers are live or recently
    /// finished (or its panel is open); anchors the panel and the toast.
    pub(super) fn render_file_transfers_indicator(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let rows = self.file_transfer_rows(cx);
        let summary = ft::indicator(&rows, Utc::now().timestamp_millis());
        let panel_mounted = self.file_transfers.panel.get().is_some();
        if summary.is_none() && !panel_mounted && self.file_transfers.toast.is_none() {
            return None;
        }
        let theme = Theme::of(cx).clone();
        let label = summary
            .as_ref()
            .map(ft::indicator_label)
            .unwrap_or_else(|| "File transfers".into());
        let tint = match summary {
            Some(s) if s.awaiting > 0 || s.live > 0 => theme.accent,
            Some(s) if s.failed => theme.danger,
            _ => theme.text_muted,
        };
        let badge = summary.is_some_and(|s| s.awaiting > 0);
        let fraction = summary.and_then(|s| s.fraction);
        let fade_key = "window-control-file-transfers";
        let tooltip: SharedString = label.clone().into();
        let panel = self.render_file_transfers_panel(cx);
        let toast = self.render_file_transfer_toast(cx);
        Some(
            div()
                .flex_none()
                .ml(px(TITLEBAR_GROUP_GAP))
                .relative()
                .child(
                    div()
                        .id("titlebar-file-transfers")
                        .relative()
                        .size(px(24.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.0))
                        .cursor_pointer()
                        .bg(motion::hover_blend(
                            fade_key,
                            theme.glass_hover().opacity(0.0),
                            theme.glass_hover(),
                        ))
                        .when(panel_mounted, |el| el.bg(theme.glass_hover()))
                        .on_hover(motion::hover_listener(SharedString::from(fade_key)))
                        .occlude()
                        .role(gpui::Role::Button)
                        .aria_label(label)
                        .tooltip(move |_, cx| {
                            cx.new(|_| SurfaceTabTooltip {
                                text: tooltip.clone(),
                            })
                            .into()
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, window, _| {
                                window.prevent_default();
                                this.file_transfers.panel.note_trigger_press();
                            }),
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            if this.file_transfers.panel.take_press_was_open() {
                                return;
                            }
                            this.open_file_transfers_panel(cx);
                        }))
                        .child(icon(icons::SORT_VERTICAL).size(px(16.0)).text_color(tint))
                        .when_some(fraction, |el, fraction| {
                            el.child(
                                div()
                                    .absolute()
                                    .left(px(5.0))
                                    .right(px(5.0))
                                    .bottom(px(2.0))
                                    .child(progress_bar(fraction, theme.accent).h(px(2.0))),
                            )
                        })
                        .when(badge, |el| {
                            el.child(
                                div()
                                    .absolute()
                                    .top(px(3.0))
                                    .right(px(3.0))
                                    .size(px(6.0))
                                    .rounded_full()
                                    .bg(theme.accent),
                            )
                        }),
                )
                .children(panel)
                .children(toast)
                .into_any_element(),
        )
    }

    /// The "Send to device…" picker (context-menu overlay).
    pub(super) fn render_send_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.file_transfers.send_menu.get()?.clone();
        let closing = self.file_transfers.send_menu.closing_since();
        let theme = Theme::of(cx).for_popup();
        let (blocker, targets) = {
            let state = self.state.read(cx);
            let now = Utc::now();
            (
                send_blocker(state, &menu.host),
                ft::send_targets(&state.devices, &menu.host, |device| {
                    state.device_online(&device.id, now)
                }),
            )
        };
        let heading = div()
            .px(px(8.0))
            .pt(px(6.0))
            .pb(px(4.0))
            .text_size(crate::typography::ui_rems(11.5))
            .text_color(theme.text_muted)
            .truncate()
            .child(format!("Send \u{201c}{}\u{201d} to", menu.label));
        let note = |text: String| {
            div()
                .px(px(8.0))
                .py(px(6.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_faint)
                .child(text)
                .into_any_element()
        };
        let body: Vec<AnyElement> = if let Some(blocker) = blocker {
            vec![note(blocker)]
        } else if targets.is_empty() {
            vec![note(
                "No other devices yet. Sign in to Zeron on another device to send files to it."
                    .into(),
            )]
        } else {
            targets
                .into_iter()
                .enumerate()
                .map(|(ix, target)| {
                    let selectable = target.selectable();
                    let menu = menu.clone();
                    let pick = target.clone();
                    let trailing: Option<AnyElement> = if !target.supported {
                        Some(
                            div()
                                .flex_none()
                                .text_size(crate::typography::ui_rems(10.5))
                                .text_color(theme.text_faint)
                                .child("Needs update")
                                .into_any_element(),
                        )
                    } else if !target.online {
                        Some(
                            icon(icons::WIFI_OFF)
                                .size(px(12.0))
                                .flex_none()
                                .text_color(theme.warning.opacity(0.8))
                                .into_any_element(),
                        )
                    } else {
                        None
                    };
                    popover::menu_row(&theme, false, format!("send-target-{}", target.id))
                        .id(("send-target", ix))
                        .role(gpui::Role::MenuItem)
                        .aria_label(format!("Send to {}", target.name))
                        .when(!selectable, |el| el.opacity(0.45).cursor_default())
                        .when(selectable, |el| {
                            el.on_click(cx.listener(move |this, _, _, cx| {
                                this.send_files(menu.clone(), pick.clone(), cx)
                            }))
                        })
                        .child(
                            icon(ft::platform_icon(&target.platform))
                                .size(px(16.0))
                                .flex_none()
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(target.name))
                        .children(trailing)
                        .into_any_element()
                })
                .collect()
        };
        let card = popover::popover_card(&theme)
            .id("send-to-device-menu")
            .w(px(SEND_MENU_WIDTH))
            .flex()
            .flex_col()
            .role(gpui::Role::Menu)
            .aria_label("Send to device")
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_send_menu(cx)))
            .child(heading)
            .children(body)
            .into_any_element();
        Some(popover::menu_at(
            "send-to-device-menu-layer",
            menu.position,
            card,
            closing,
        ))
    }
}

/// Review-fixture hooks (`examples/file-transfer-fixture.rs`).
#[doc(hidden)]
impl Shell {
    pub fn fixture_file_transfers_panel(&mut self, cx: &mut Context<Self>) {
        self.open_file_transfers_panel(cx);
    }

    pub fn fixture_file_transfer_toast(&mut self, id: &str, cx: &mut Context<Self>) {
        self.file_transfers.toast = Some(id.to_string());
        cx.notify();
    }

    pub fn fixture_send_menu(
        &mut self,
        chat_id: &str,
        path: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.open_send_menu_for_path(chat_id, path, position, cx);
    }

    pub fn fixture_devices_settings(
        &mut self,
        settings: zeron_proto::FileTransferSettings,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(SettingsSection::Devices, cx);
        if self.devices_page.is_none() {
            let state = self.state.clone();
            self.devices_page = Some(cx.new(|cx| DevicesPage::new(state, cx)));
        }
        if let Some(page) = &self.devices_page {
            page.update(cx, |page, cx| page.set_transfer_settings(settings, cx));
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_profiles_and_old_hosts_explain_why_they_cannot_send() {
        let mut state = AppState::new();
        state.workspace_scope = Some(WorkspaceScope::Local);
        assert!(send_blocker(&state, "desk").unwrap().contains("Sign in"));
        state.workspace_scope = Some(WorkspaceScope::Synced);
        state.devices = vec![
            serde_json::from_value(serde_json::json!({
                "id": "desk", "name": "Desk", "platform": "linux", "lastSeenAt": null,
                "capabilities": ["harness-updates-v1"],
            }))
            .unwrap(),
        ];
        assert_eq!(
            send_blocker(&state, "desk").as_deref(),
            Some("Desk can't send files yet. Update Zeron on it first.")
        );
        state.devices[0]
            .capabilities
            .push(zeron_proto::capabilities::FILE_TRANSFER_V1.into());
        assert_eq!(send_blocker(&state, "desk"), None);
    }
}
