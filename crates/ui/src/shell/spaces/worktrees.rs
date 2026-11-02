//! The By-project sidebar's worktree level: each project group's sessions
//! split by the copy of the code they ran in, under a collapsible header
//! whose `+` starts a session in that checkout.

use super::*;
use crate::pickers::CheckoutPreset;

/// The copy of a project's code a session ran in. Keyed by checkout id, so
/// the same branch on two devices is two worktrees.
struct SidebarWorktree {
    key: String,
    device_id: String,
    /// The project folder's own checkout rather than a linked worktree.
    main: bool,
}

/// `None` outside a git project. Sessions recorded before checkouts were
/// stamped belong to their project folder's checkout.
fn sidebar_worktree(state: &AppState, chat: &zeron_proto::Chat) -> Option<SidebarWorktree> {
    let space = state.space_for_chat(chat)?;
    if !space.git_detected {
        return None;
    }
    let checkout =
        crate::change_requests::conversation_checkout_id(chat).filter(|id| !id.is_empty());
    let main_checkout = space.checkout_id.as_deref().filter(|id| !id.is_empty());
    let (main, key) = match (checkout, main_checkout) {
        (Some(id), Some(main_id)) => (id == main_id, id.to_owned()),
        (None, Some(main_id)) => (true, main_id.to_owned()),
        (None, None) => (true, format!("space:{}", space.id)),
        // The project's own checkout was never recorded: its folder decides.
        (Some(id), None) => {
            let folder = chat
                .source_context
                .as_ref()
                .map(|source| source.repo_root.as_str())
                .or(chat.cwd.as_deref());
            // A project may be a subfolder of the repository.
            if folder.is_some_and(|folder| std::path::Path::new(&space.path).starts_with(folder)) {
                (true, format!("space:{}", space.id))
            } else {
                (false, id.to_owned())
            }
        }
    };
    Some(SidebarWorktree {
        key,
        device_id: chat.device_id.clone(),
        main,
    })
}

/// Row indices per worktree: main checkouts first (this device's leading),
/// then the rest in the order their first row appears. `None` when any row
/// has no worktree, which keeps that group one flat list.
fn group_by_worktree(
    worktrees: impl IntoIterator<Item = Option<SidebarWorktree>>,
    local_device_id: Option<&str>,
) -> Option<Vec<(SidebarWorktree, Vec<usize>)>> {
    let mut groups: Vec<(SidebarWorktree, Vec<usize>)> = Vec::new();
    for (index, worktree) in worktrees.into_iter().enumerate() {
        let worktree = worktree?;
        match groups
            .iter_mut()
            .find(|(group, _)| group.key == worktree.key)
        {
            Some((_, rows)) => rows.push(index),
            None => groups.push((worktree, vec![index])),
        }
    }
    groups.sort_by_key(|(worktree, _)| {
        match (
            worktree.main,
            local_device_id == Some(worktree.device_id.as_str()),
        ) {
            (true, true) => 0,
            (true, false) => 1,
            (false, _) => 2,
        }
    });
    Some(groups)
}

fn take_in_order<T>(rows: Vec<T>, order: impl IntoIterator<Item = usize>) -> Vec<T> {
    let mut slots: Vec<Option<T>> = rows.into_iter().map(Some).collect();
    order
        .into_iter()
        .filter_map(|index| slots.get_mut(index).and_then(Option::take))
        .collect()
}

/// A project group's chats in the order `split_groups_by_worktree` lays its
/// rows out, for keyboard order.
pub(super) fn order_chats_by_worktree(state: &AppState, chats: &mut Vec<zeron_proto::Chat>) {
    let worktrees = chats.iter().map(|chat| sidebar_worktree(state, chat));
    if let Some(order) = group_by_worktree(worktrees, state.local_device_id.as_deref()) {
        *chats = take_in_order(
            std::mem::take(chats),
            order.into_iter().flat_map(|(_, rows)| rows),
        );
    }
}

/// Reorders each project group's rows worktree by worktree and nests them,
/// returning the groups' worktree blocks by group key.
pub(super) fn split_groups_by_worktree(
    state: &AppState,
    groups: &mut [(Option<(String, String)>, Vec<ActiveChatRow>)],
) -> std::collections::HashMap<String, Vec<SidebarWorktreeBlock>> {
    let mut worktree_blocks = std::collections::HashMap::new();
    for (group, rows) in groups {
        let Some((key, _)) = group else {
            continue;
        };
        let Some(order) = group_by_worktree(
            rows.iter().map(|row| sidebar_worktree(state, &row.chat)),
            state.local_device_id.as_deref(),
        ) else {
            continue;
        };
        let mut sorted = take_in_order(
            std::mem::take(rows),
            order
                .iter()
                .flat_map(|(_, indices)| indices.iter().copied()),
        )
        .into_iter();
        let mut blocks = Vec::with_capacity(order.len());
        for (worktree, indices) in order {
            let block_rows: Vec<ActiveChatRow> = sorted.by_ref().take(indices.len()).collect();
            blocks.push(SidebarWorktreeBlock::new(
                worktree,
                &block_rows,
                &state.spaces,
            ));
            rows.extend(block_rows.into_iter().map(|mut row| {
                row.branch = None;
                row.change_request = None;
                row.nested = true;
                row
            }));
        }
        worktree_blocks.insert(key.clone(), blocks);
    }
    worktree_blocks
}

/// One worktree's run of rows within a project group.
pub(super) struct SidebarWorktreeBlock {
    worktree: SidebarWorktree,
    len: usize,
    branch: Option<String>,
    change_request: Option<zeron_proto::ChangeRequestSummary>,
    /// The most pressing status among its sessions, shown while collapsed.
    attention: Option<ChatIndicator>,
    new_chat: Option<(String, CheckoutPreset)>,
}

impl SidebarWorktreeBlock {
    /// The branch comes from its most recent session that has one, the pull
    /// request from the most recent session on that branch. The branch shows
    /// even when rows hide theirs: it names the worktree.
    fn new(worktree: SidebarWorktree, rows: &[ActiveChatRow], spaces: &[Space]) -> Self {
        fn branch_of<'a>(row: &'a ActiveChatRow, spaces: &'a [Space]) -> Option<&'a str> {
            crate::change_requests::conversation_branch(&row.chat, spaces).map(str::trim)
        }
        let row_branch = |row| branch_of(row, spaces);
        let branch = rows.iter().find_map(row_branch).map(str::to_owned);
        let change_request = rows
            .iter()
            .filter(|row| row_branch(row) == branch.as_deref())
            .find_map(|row| row.change_request.clone());
        let attention = rows
            .iter()
            .map(|row| row.status)
            .min_by_key(|status| zeron_proto::view::attention_rank(*status))
            .filter(|status| !matches!(status, ChatIndicator::Completed | ChatIndicator::Idle));
        let preset = if worktree.main {
            Some(CheckoutPreset::MainCheckout)
        } else {
            let path = rows.iter().find_map(|row| {
                row.chat
                    .source_context
                    .as_ref()
                    .map(|source| source.repo_root.clone())
                    .filter(|root| !root.is_empty())
            });
            branch
                .clone()
                .zip(path)
                .map(|(branch, path)| CheckoutPreset::Worktree { branch, path })
        };
        let new_chat = rows
            .first()
            .and_then(|row| row.chat.space_id.clone())
            .filter(|_| !zeron_proto::is_cloud_device(&worktree.device_id, ""))
            .zip(preset);
        Self {
            worktree,
            len: rows.len(),
            branch,
            change_request,
            attention,
            new_chat,
        }
    }
}

/// A row's drag group and its index within it: its worktree's when the
/// group is split by worktree, so a drag preview stays inside one worktree.
pub(super) fn sidebar_worktree_slot(
    group: &str,
    blocks: Option<&Vec<SidebarWorktreeBlock>>,
    index: usize,
) -> (String, usize) {
    let mut start = 0;
    for block in blocks.into_iter().flatten() {
        if index < start + block.len {
            return (
                worktree_drag_group(group, &block.worktree.key),
                index - start,
            );
        }
        start += block.len;
    }
    (group.to_owned(), index)
}

fn worktree_drag_group(group: &str, worktree_key: &str) -> String {
    format!("{group}/{worktree_key}")
}

/// Indents nested content under a guide line that starts beneath its
/// parent header's icon.
fn sidebar_worktree_nest(theme: &Theme, content: AnyElement) -> AnyElement {
    div()
        .ml(px(14.0))
        .pl(px(4.0))
        .border_l_1()
        .border_color(theme.border.opacity(0.6))
        .child(content)
        .into_any_element()
}

const SIDEBAR_NEW_CHAT_MENU: [(&str, &str, CheckoutPreset); 2] = [
    (
        "New chat in current checkout",
        icons::GIT_BRANCH,
        CheckoutPreset::MainCheckout,
    ),
    ("New worktree", icons::WORKTREE, CheckoutPreset::NewWorktree),
];

impl Shell {
    pub(super) fn sidebar_worktrees_shown(&self) -> bool {
        self.settings.sidebar_organization == SidebarOrganization::ByProject
            && self.settings.sidebar_show_worktrees
    }

    pub(super) fn sidebar_collapsed_worktree(
        &self,
        state: &AppState,
        chat: &zeron_proto::Chat,
    ) -> Option<String> {
        if !self.sidebar_worktrees_shown() {
            return None;
        }
        sidebar_worktree(state, chat)
            .map(|worktree| worktree.key)
            .filter(|key| self.settings.sidebar_collapsed_worktrees.contains(key))
    }

    pub(super) fn open_new_session_in_checkout(
        &mut self,
        space_id: String,
        preset: CheckoutPreset,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_new_chat_menu = None;
        self.open_new_session(Some(NewSessionTarget::Project(space_id.clone())), cx);
        let pickers = self.composer.read(cx).pickers().clone();
        pickers.update(cx, |pickers, cx| {
            pickers.preset_checkout(space_id, preset, cx)
        });
    }

    pub(super) fn open_sidebar_new_chat_menu(
        &mut self,
        space_id: String,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sidebar_new_chat_menu = Some((space_id, position));
        self.sidebar_new_chat_menu_active = None;
        window.focus(&self.sidebar_new_chat_menu_focus, cx);
        cx.notify();
    }

    pub(in crate::shell) fn render_sidebar_new_chat_menu(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (space_id, position) = self.sidebar_new_chat_menu.clone()?;
        if self.state.read(cx).space_row(&space_id).is_none() {
            self.sidebar_new_chat_menu = None;
            return None;
        }
        let theme = Theme::of(cx).for_popup();
        let mut card = popover::popover_card(&theme)
            .w(px(240.0))
            .flex()
            .flex_col()
            .track_focus(&self.sidebar_new_chat_menu_focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                match event.keystroke.key.as_str() {
                    "escape" => this.sidebar_new_chat_menu = None,
                    "up" | "down" => {
                        this.sidebar_new_chat_menu_active = popover::menu_step(
                            this.sidebar_new_chat_menu_active,
                            SIDEBAR_NEW_CHAT_MENU.len(),
                            if event.keystroke.key == "up" { -1 } else { 1 },
                        )
                    }
                    "enter" => {
                        if let (Some(action), Some((space_id, _))) = (
                            this.sidebar_new_chat_menu_active,
                            this.sidebar_new_chat_menu.clone(),
                        ) {
                            let preset = SIDEBAR_NEW_CHAT_MENU[action].2.clone();
                            this.open_new_session_in_checkout(space_id, preset, cx);
                        }
                    }
                    _ => return,
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.sidebar_new_chat_menu = None;
                cx.notify();
            }));
        for (action, (label, glyph, preset)) in SIDEBAR_NEW_CHAT_MENU.into_iter().enumerate() {
            let space_id = space_id.clone();
            card = card.child(
                popover::menu_row_nav(
                    &theme,
                    false,
                    self.sidebar_new_chat_menu_active == Some(action),
                    format!("sidebar-new-chat-action-{action}"),
                )
                .id(("sidebar-new-chat-action", action))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.open_new_session_in_checkout(space_id.clone(), preset.clone(), cx);
                }))
                .child(icon(glyph).size(px(16.0)).text_color(theme.text_muted))
                .child(label),
            );
        }
        Some(popover::menu_at(
            "sidebar-new-chat-menu",
            position,
            card.into_any_element(),
            None,
        ))
    }

    /// A project group's body split by worktree: each worktree's header
    /// with its sessions nested beneath, collapsible on its own.
    pub(super) fn render_sidebar_worktrees(
        &mut self,
        drag_group: &str,
        blocks: Vec<SidebarWorktreeBlock>,
        rows: Vec<(String, f32, AnyElement)>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> (AnyElement, f32) {
        let mut rows = rows.into_iter();
        let mut children = Vec::with_capacity(blocks.len());
        let mut height = SIDEBAR_LIST_GAP * blocks.len().saturating_sub(1) as f32;
        for block in blocks {
            let key = block.worktree.key.clone();
            let motion_key = format!("worktree:{key}");
            let collapsed = self.settings.sidebar_collapsed_worktrees.contains(&key);
            let (list, list_height) = sidebar_row_list(
                rows.by_ref().take(block.len).collect(),
                self.sidebar_transfer_extra_gap(&worktree_drag_group(drag_group, &key)),
            );
            let body_height = SIDEBAR_LIST_GAP + list_height;
            self.begin_queued_sidebar_reveal(&motion_key, !collapsed, body_height);
            let header = self.render_sidebar_worktree_header(
                block,
                &motion_key,
                collapsed,
                body_height,
                theme,
                cx,
            );
            let body = self.render_sidebar_disclosure_body(
                &motion_key,
                !collapsed,
                body_height,
                div()
                    .pt(px(SIDEBAR_LIST_GAP))
                    .child(sidebar_worktree_nest(theme, list))
                    .into_any_element(),
            );
            height += SIDEBAR_DISCLOSURE_HEADER_HEIGHT + if collapsed { 0.0 } else { body_height };
            children.push(
                div()
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(body)
                    .into_any_element(),
            );
        }
        let content = div()
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_LIST_GAP))
            .children(children)
            .into_any_element();
        (sidebar_worktree_nest(theme, content), height)
    }

    fn render_sidebar_worktree_header(
        &self,
        block: SidebarWorktreeBlock,
        motion_key: &str,
        collapsed: bool,
        body_height: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = block.worktree.key;
        let device_id = block.worktree.device_id;
        let device = {
            let state = self.state.read(cx);
            (state.local_device_id.as_deref() != Some(device_id.as_str())).then(|| {
                if zeron_proto::is_cloud_device(&device_id, "") {
                    (icons::CLOUD, SharedString::from("Cloud"))
                } else {
                    (
                        icons::REMOTE_SERVER,
                        SharedString::from(
                            state
                                .device_name(&device_id)
                                .unwrap_or("Unknown device")
                                .to_string(),
                        ),
                    )
                }
            })
        };
        let label: SharedString = match (block.branch, block.worktree.main) {
            (Some(branch), _) => branch.into(),
            (None, true) => "Main checkout".into(),
            (None, false) => "Worktree".into(),
        };
        let muted = theme.text_muted;
        let attention = block.attention.filter(|_| collapsed).map(|status| {
            super::super::session_lanes::status_mark(
                format!("sidebar-worktree-{key}"),
                status,
                self.sidebar_pane.entity_id(),
                theme,
                cx,
            )
        });
        let toggle_key = key.clone();
        let toggle_motion_key = motion_key.to_owned();
        let group_name = SharedString::from(format!("sidebar-worktree-hover-{key}"));
        div()
            .group(group_name.clone())
            .id(SharedString::from(format!("sidebar-worktree-{key}")))
            .debug_selector({
                let key = key.clone();
                move || format!("sidebar-worktree-{key}")
            })
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .h(px(SIDEBAR_DISCLOSURE_HEADER_HEIGHT))
            .px(px(Theme::SPACE_SM))
            .rounded(px(6.0))
            .cursor_pointer()
            .hover(|el| el.bg(theme.glass_hover()))
            .child(
                icon(if block.worktree.main {
                    icons::GIT_BRANCH
                } else {
                    icons::WORKTREE
                })
                .size(px(13.0))
                .flex_none()
                .text_color(muted.opacity(0.7)),
            )
            .child(super::super::sidebar_faded_label(
                format!("sidebar-worktree-label-{key}").into(),
                false,
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(muted)
                    .child(label),
            ))
            .children(block.change_request.map(|summary| {
                crate::change_requests::pull_request_badge(
                    format!("sidebar-worktree-pr-{key}").into(),
                    summary,
                    crate::change_requests::ChangeRequestBadgeSurface::Sidebar,
                    Some(device_id.clone()),
                    theme,
                )
            }))
            .children(device.map(|(glyph, name)| {
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(muted.opacity(0.7))
                    .child(icon(glyph).size(px(11.0)))
                    .child(name)
            }))
            .child(div().flex_1())
            .children(attention)
            .children(collapsed.then(|| sidebar_count_badge(theme, block.len)))
            .children(block.new_chat.map(|(space_id, preset)| {
                sidebar_new_chat_button(
                    format!("sidebar-worktree-new-chat-{key}"),
                    match preset {
                        CheckoutPreset::MainCheckout => "New chat in current checkout".into(),
                        _ => "New chat in worktree".into(),
                    },
                    group_name.clone(),
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.open_new_session_in_checkout(space_id.clone(), preset.clone(), cx);
                    }),
                )
            }))
            .child(self.sidebar_disclosure_chevron(motion_key, !collapsed, theme))
            .on_click(cx.listener(move |this, _, _, cx| {
                let was_open = !this
                    .settings
                    .sidebar_collapsed_worktrees
                    .contains(&toggle_key);
                this.begin_sidebar_disclosure_motion(
                    &toggle_motion_key,
                    if was_open { body_height } else { 0.0 },
                    if was_open { 0.0 } else { body_height },
                );
                if was_open {
                    this.settings
                        .sidebar_collapsed_worktrees
                        .insert(toggle_key.clone());
                } else {
                    this.settings
                        .sidebar_collapsed_worktrees
                        .remove(&toggle_key);
                }
                this.schedule_save(cx);
                cx.notify();
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::super::pinned_session_tests::{ids, pin_test_chat, pin_test_shell};
    use super::*;

    /// A git project at `/repo` on this device, grouped By project, with
    /// `chats` newest first as `(id, (checkout id, folder))`.
    fn repo_with_chats(
        shell: &mut Shell,
        project_checkout: Option<&str>,
        chats: &[(&str, Option<(&str, &str)>)],
        cx: &mut Context<Shell>,
    ) {
        shell.settings.sidebar_organization = SidebarOrganization::ByProject;
        shell.settings.sidebar_show_worktrees = true;
        shell.state.update(cx, |state, _| {
            state.workspace_scope = Some(WorkspaceScope::Local);
            state.local_device_id = Some("local".into());
            state.spaces = vec![
                serde_json::from_value(serde_json::json!({
                    "id": "repo", "deviceId": "local", "path": "/repo", "gitDetected": true,
                    "checkoutId": project_checkout, "createdAt": Utc::now()
                }))
                .unwrap(),
            ];
            state.chats = chats
                .iter()
                .enumerate()
                .map(|(ix, (id, source))| {
                    let mut chat = pin_test_chat(id);
                    chat.space_id = Some("repo".into());
                    chat.created_at = Utc::now() - chrono::Duration::minutes(ix as i64);
                    chat.source_context =
                        source.map(|(checkout, root)| zeron_proto::ConversationSourceContext {
                            checkout_id: checkout.into(),
                            repo_root: root.into(),
                            cwd: root.into(),
                            branch: checkout.into(),
                            head_sha: None,
                            observed_at: Utc::now(),
                        });
                    chat
                })
                .collect();
        });
    }

    #[gpui::test]
    fn sidebar_worktrees_order_main_first_and_reveal_collapsed(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                // A worktree, main, the other worktree, the first worktree
                // again, then a chat from before checkouts were recorded.
                repo_with_chats(
                    shell,
                    Some("main-checkout"),
                    &[
                        ("wt-a-new", Some(("wt-a", "/repo"))),
                        ("main-new", Some(("main-checkout", "/repo"))),
                        ("wt-b", Some(("wt-b", "/repo"))),
                        ("wt-a-old", Some(("wt-a", "/repo"))),
                        ("legacy", None),
                    ],
                    cx,
                );
                assert_eq!(
                    shell.sidebar_visible_order(cx),
                    ids(&["main-new", "legacy", "wt-a-new", "wt-a-old", "wt-b"])
                );

                shell
                    .settings
                    .sidebar_collapsed_worktrees
                    .insert("wt-a".into());
                assert!(!shell.sidebar_hover_card_row_visible("wt-a-old", cx));
                assert!(shell.reveal_sidebar_chat("wt-a-old", cx));
                assert!(shell.settings.sidebar_collapsed_worktrees.is_empty());

                shell.settings.sidebar_show_worktrees = false;
                assert_eq!(
                    shell.sidebar_visible_order(cx),
                    ids(&["wt-a-new", "main-new", "wt-b", "wt-a-old", "legacy"])
                );
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn sidebar_worktrees_find_main_by_folder_when_the_project_checkout_is_unknown(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                repo_with_chats(
                    shell,
                    None,
                    &[
                        ("wt", Some(("wt-checkout", "/repo/.worktrees/wt"))),
                        ("main-folder", Some(("main-checkout", "/repo/"))),
                        ("legacy", None),
                    ],
                    cx,
                );
                assert_eq!(
                    shell.sidebar_visible_order(cx),
                    ids(&["main-folder", "legacy", "wt"])
                );
                // A project in a subfolder of the repository.
                shell
                    .state
                    .update(cx, |state, _| state.spaces[0].path = "/repo/pkg".into());
                assert_eq!(
                    shell.sidebar_visible_order(cx),
                    ids(&["main-folder", "legacy", "wt"])
                );
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn sidebar_worktrees_leave_non_git_projects_flat(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.settings.sidebar_organization = SidebarOrganization::ByProject;
                shell.settings.sidebar_show_worktrees = true;
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state.spaces = vec![serde_json::from_value(serde_json::json!({
                        "id": "notes", "deviceId": "local", "path": "/notes", "createdAt": Utc::now()
                    }))
                    .unwrap()];
                    // Interleaved checkouts: grouping them would pull "old"
                    // up next to "new".
                    state.chats = [("new", "x"), ("mid", "y"), ("old", "x")]
                        .into_iter()
                        .enumerate()
                        .map(|(ix, (id, checkout))| {
                            let mut chat = pin_test_chat(id);
                            chat.space_id = Some("notes".into());
                            chat.created_at = Utc::now() - chrono::Duration::minutes(ix as i64);
                            chat.checkout_id = Some(checkout.into());
                            chat
                        })
                        .collect();
                });
                assert_eq!(shell.sidebar_visible_order(cx), ids(&["new", "mid", "old"]));
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn sidebar_worktree_plus_opens_a_new_session_in_that_worktree(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, cx| {
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state.local_device_id = Some("local".into());
                    state.spaces = ["repo", "other"]
                        .into_iter()
                        .map(|id| {
                            serde_json::from_value(serde_json::json!({
                                "id": id, "deviceId": "local", "path": format!("/{id}"),
                                "gitDetected": true, "createdAt": Utc::now()
                            }))
                            .unwrap()
                        })
                        .collect();
                    let mut chat = pin_test_chat("in-other");
                    chat.space_id = Some("other".into());
                    state.chats = vec![chat];
                    state.select_chat(Some("in-other".into()), cx);
                });
                shell.open_new_session_in_checkout(
                    "repo".into(),
                    CheckoutPreset::Worktree {
                        branch: "feature".into(),
                        path: "/repo/.worktrees/feature".into(),
                    },
                    cx,
                );
            })
            .unwrap();
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let state = shell.state.read(cx);
                assert_eq!(state.selected_chat, None);
                assert_eq!(state.selected_space.as_deref(), Some("repo"));
                assert_eq!(
                    shell.composer.read(cx).pickers().read(cx).checkout_plan(),
                    crate::pickers::CheckoutPlan::ReuseWorktree {
                        path: "/repo/.worktrees/feature".into(),
                        branch: "feature".into(),
                    }
                );
            })
            .unwrap();
    }
}
