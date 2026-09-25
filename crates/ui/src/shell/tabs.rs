//! Session navigation — the horizontal tab strip is gone (wing 2026-08-10):
//! the activity sidebar IS the session list, and the titlebar names the
//! selected session (harness brand icon + title). A `+` new-session button
//! lives in the titlebar's left control cluster while an existing session is
//! selected. `UiSettings.open_tabs` is legacy — no longer read or written.

use super::*;

/// The chat one step from `selected` in the sidebar `order`, wrapping at both
/// ends. Pure.
///
/// With nothing selected — the new-session canvas — cycling enters the list at
/// the end it would have wrapped to: the first row going forward, the last
/// going back. A selection that has since left the list (archived from another
/// device mid-cycle) is treated the same way rather than dead-ending.
pub(super) fn cycle_target(
    order: &[String],
    selected: Option<&str>,
    forward: bool,
) -> Option<String> {
    if order.is_empty() {
        return None;
    }
    let at = selected.and_then(|id| order.iter().position(|c| c == id));
    let next = match (at, forward) {
        (Some(at), true) => (at + 1) % order.len(),
        (Some(at), false) => (at + order.len() - 1) % order.len(),
        (None, true) => 0,
        (None, false) => order.len() - 1,
    };
    Some(order[next].clone())
}

pub(super) fn right_pane_expand_icon(expanded: bool) -> &'static str {
    if expanded {
        icons::COLLAPSE_ARROWS
    } else {
        icons::EXPAND_ARROWS
    }
}

struct PanelTitlebarWidths {
    surface_reveal: f32,
    files_controls: f32,
}

/// The two fixed right-edge anchors: the explorer toggle and the pane toggle
/// (28px each) with the same 4px gap the surface strip keeps between its
/// controls, so the two never render as one fused block.
const PANEL_TOGGLE_GAP: f32 = 4.0;
const PANEL_TOGGLE_SLOTS: f32 = 28.0 * 2.0 + PANEL_TOGGLE_GAP;

fn panel_titlebar_widths(
    surfaces_visible: f32,
    files_visible: f32,
    available: f32,
    right_pad: f32,
) -> PanelTitlebarWidths {
    // Caption controls occupy the far-right panel first. Subtract their
    // clearance once across the combined header, then split it at Files.
    // The explorer and pane toggles keep their slots even when closed.
    let files_controls = (files_visible - right_pad).max(PANEL_TOGGLE_SLOTS);
    let surfaces = surfaces_visible + files_visible - right_pad - files_controls;
    PanelTitlebarWidths {
        surface_reveal: surfaces.min(available - files_controls).max(0.0),
        files_controls,
    }
}

impl Shell {
    /// Navigation requests focus once the destination composer renders.
    pub(super) fn focus_composer(&mut self, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| {
            composer.focus_pending = true;
            cx.notify();
        });
    }

    /// Ctrl+Tab / Ctrl+Shift+Tab: step through the sidebar's Sessions list in
    /// the order it is drawn. Selection is immediate (no MRU overlay held open
    /// on the modifier) — one press, one session.
    ///
    /// Chat-scoped chrome, like the panel toggles: gpui dispatches a matched
    /// binding before any `on_key_down`, so an unscoped cycle would fire
    /// underneath Settings (yanking the user off the page mid-record, since
    /// these are the very keys the shortcuts table invites them to press) or
    /// underneath the add-space palette, stranding the overlay over a session
    /// they never picked.
    pub(super) fn cycle_session(&mut self, forward: bool, cx: &mut Context<Self>) {
        if !matches!(self.route, Route::Chat) || self.overlay_owns_keyboard(cx) {
            return;
        }
        // The same list `render_active_rows` draws and the jump shortcuts
        // count — one function, so neither can drift from the screen.
        let order = self.sidebar_visible_order(cx);
        let selected = self.state.read(cx).selected_chat.clone();
        if let Some(target) = cycle_target(&order, selected.as_deref(), forward) {
            self.open_chat(target, cx);
        }
    }

    /// Boot landing: the most recently active visible chat once the first
    /// chats frame has synced (manual selection wins; no chats → the
    /// new-session canvas shows).
    pub(super) fn boot_select_chat(&mut self, cx: &mut Context<Self>) {
        let first = {
            let state = self.state.read(cx);
            if !state.chats_synced || state.selected_chat.is_some() || state.auto_selected {
                return;
            }
            state
                .overview_chats(Utc::now())
                .first()
                .map(|(_, c)| c.id.clone())
        };
        if let Some(first) = first {
            self.focus_composer(cx);
            self.state
                .update(cx, |s, cx| s.select_chat(Some(first), cx));
        }
    }

    /// Open a session from the sidebar: select it, the main area follows.
    pub(crate) fn open_chat(&mut self, chat_id: String, cx: &mut Context<Self>) {
        self.pending_ticket_chat_link = None;
        self.thread_header_menu = None;
        // Opening a child from search, a link, or the header reveals its row.
        let ancestors = {
            let state = self.state.read(cx);
            let mut ancestors = Vec::new();
            let mut seen = std::collections::HashSet::new();
            let mut current = Some(chat_id.as_str());
            while let Some(id) = current.filter(|id| seen.insert(*id)) {
                current = state
                    .chats
                    .iter()
                    .find(|chat| chat.id == id)
                    .and_then(|chat| chat.parent_chat_id.as_deref());
                if let Some(parent) = current {
                    ancestors.push(parent.to_owned());
                }
            }
            ancestors
        };
        for ancestor in ancestors {
            self.sidebar_collapsed_threads.remove(&ancestor);
        }
        self.command_palette = None;
        self.route = Route::Chat;
        self.focus_composer(cx);
        self.state
            .update(cx, |s, cx| s.select_chat(Some(chat_id), cx));
        cx.notify();
    }

    /// `+` in the titlebar: open the new-session canvas. A set sidebar filter
    /// re-homes the canvas onto that project; under "All" the current pick
    /// (the last selected project, restored from composer defaults) stands.
    pub(super) fn open_new_session(&mut self, cx: &mut Context<Self>) {
        self.pending_ticket_chat_link = None;
        self.thread_header_menu = None;
        self.command_palette = None;
        self.route = Route::Chat;
        self.focus_composer(cx);
        let target = {
            let state = self.state.read(cx);
            self.settings
                .space_filter
                .clone()
                .filter(|id| state.space_row(id).is_some())
        };
        let defaults = crate::settings::composer::ComposerDefaults::load(&self.data_dir);
        self.state.update(cx, |s, cx| {
            if target.is_some() {
                s.select_space(target, cx);
            } else if defaults.no_project {
                // Opening an existing project session (including boot's last
                // session) must not erase the saved new-session opt-out.
                s.select_space(None, cx);
                if let Some(device) = defaults.device {
                    s.select_device(device, cx);
                }
            }
            s.select_chat(None, cx);
        });
        cx.notify();
    }

    /// Open a child canvas from the selected session. The relationship stays
    /// pending until its first send creates the chat on the parent's host.
    pub(super) fn open_child_session(&mut self, parent_id: String, cx: &mut Context<Self>) {
        self.pending_ticket_chat_link = None;
        if !self
            .state
            .read(cx)
            .chats
            .iter()
            .any(|chat| chat.id == parent_id && !chat.archived)
        {
            return;
        }
        self.sidebar_collapsed_threads.remove(&parent_id);
        self.thread_header_menu = None;
        self.command_palette = None;
        self.route = Route::Chat;
        self.focus_composer(cx);
        self.state.update(cx, |s, cx| {
            s.begin_child_chat(&parent_id, cx);
        });
        cx.notify();
    }

    /// Parent navigation and the direct-child dropdown stay with the thread
    /// title, so a sidebar filter or a folded tree cannot strand a child.
    fn render_thread_relation_controls(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some((selected_id, parent, children, can_create_child)) = ({
            let state = self.state.read(cx);
            state.selected_chat_row().map(|selected| {
                let parent = selected.parent_chat_id.as_deref().and_then(|id| {
                    state.chats.iter().find(|chat| chat.id == id).map(|chat| {
                        (
                            chat.id.clone(),
                            transcript::single_line(chat.title.as_deref().unwrap_or("New session")),
                        )
                    })
                });
                let mut children: Vec<_> = state
                    .chats
                    .iter()
                    .filter(|chat| {
                        !chat.archived
                            && chat.parent_chat_id.as_deref() == Some(selected.id.as_str())
                    })
                    .collect();
                children.sort_by(|left, right| {
                    spaces::compare_sidebar_chats(self.settings.sidebar_sort, left, right)
                });
                let children = children
                    .into_iter()
                    .map(|chat| {
                        (
                            chat.id.clone(),
                            transcript::single_line(chat.title.as_deref().unwrap_or("New session")),
                            state.display_status_for(chat, Utc::now()),
                        )
                    })
                    .collect::<Vec<_>>();
                (selected.id.clone(), parent, children, !selected.archived)
            })
        }) else {
            return Vec::new();
        };

        let mut controls = Vec::new();
        if let Some((parent_id, parent_title)) = parent {
            controls.push(
                div()
                    .id("thread-parent-link")
                    .role(gpui::Role::Button)
                    .aria_label(format!("Open parent thread: {parent_title}"))
                    .h(px(26.0))
                    .max_w(px(190.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(theme.border.opacity(0.7))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.glass_hover()).text_color(theme.text))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.open_chat(parent_id.clone(), cx)),
                    )
                    .child(icon(icons::ALT_ARROW_LEFT).size(px(12.0)))
                    .child(div().min_w_0().truncate().child(parent_title))
                    .into_any_element(),
            );
        }

        if !children.is_empty() {
            let count = children.len();
            let needs_input = children
                .iter()
                .any(|(_, _, status)| *status == zeron_proto::ChatIndicator::AwaitingInput);
            let menu_open = self.thread_header_menu.as_deref() == Some(selected_id.as_str());
            let toggle_id = selected_id.clone();
            let mut trigger = div()
                .id("thread-children-trigger")
                .relative()
                .role(gpui::Role::Button)
                .aria_label(format!("Show {count} child threads"))
                .h(px(26.0))
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(theme.border.opacity(0.7))
                .text_size(crate::typography::ui_rems(11.0))
                .text_color(if needs_input {
                    theme.accent
                } else {
                    theme.text_muted
                })
                .cursor_pointer()
                .hover(|style| style.bg(theme.glass_hover()).text_color(theme.text))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.thread_header_menu =
                        if this.thread_header_menu.as_deref() == Some(toggle_id.as_str()) {
                            None
                        } else {
                            Some(toggle_id.clone())
                        };
                    cx.notify();
                }))
                .child(
                    icon(icons::ALT_ARROW_RIGHT)
                        .size(px(12.0))
                        .with_transformation(gpui::Transformation::rotate(gpui::percentage(0.25))),
                )
                .child(SharedString::from(if needs_input {
                    "Needs you".to_string()
                } else {
                    format!("{count} {}", if count == 1 { "child" } else { "children" })
                }));
            if menu_open {
                let popup_theme = theme.for_popup();
                let mut menu = popover::popover_card(&popup_theme)
                    .w(px(320.0))
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.thread_header_menu = None;
                        cx.notify();
                    }))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .px(px(8.0))
                            .py(px(6.0))
                            .text_size(crate::typography::ui_rems(11.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(popup_theme.text_muted)
                            .child(SharedString::from(format!("Children ({count})"))),
                    );
                for (id, title, status) in children {
                    let color = spaces::status_dot_color(status, &popup_theme);
                    menu = menu.child(
                        popover::menu_row(&popup_theme, false, format!("thread-child-{id}"))
                            .id(SharedString::from(format!("thread-child-{id}")))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.open_chat(id.clone(), cx)),
                            )
                            .child(div().size(px(7.0)).rounded_full().bg(color))
                            .child(div().flex_1().min_w_0().truncate().child(title))
                            .child(SharedString::from(match status {
                                zeron_proto::ChatIndicator::AwaitingInput => "Needs input",
                                zeron_proto::ChatIndicator::Working => "Working",
                                zeron_proto::ChatIndicator::Completed => "Done",
                                zeron_proto::ChatIndicator::Errored => "Failed",
                                zeron_proto::ChatIndicator::Idle => "",
                            })),
                    );
                }
                if can_create_child {
                    let new_child_id = selected_id.clone();
                    menu = menu.child(popover::menu_separator()).child(
                        popover::menu_row(&popup_theme, false, "thread-child-new")
                            .id("thread-child-new")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.open_child_session(new_child_id.clone(), cx)
                            }))
                            .child(
                                icon(icons::PLUS)
                                    .size(px(14.0))
                                    .text_color(popup_theme.text_muted),
                            )
                            .child(SharedString::from("New child thread")),
                    );
                }
                trigger = trigger.child(popover::anchored_menu_below(
                    "thread-children-menu",
                    menu.into_any_element(),
                    None,
                ));
            }
            controls.push(trigger.into_any_element());
        }

        if can_create_child {
            let new_child_id = selected_id;
            controls.push(
                div()
                    .id("new-child-thread")
                    .role(gpui::Role::Button)
                    .aria_label("New child thread")
                    .h(px(26.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(4.0))
                    .px(px(8.0))
                    .rounded(px(7.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.glass_hover()).text_color(theme.text))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_child_session(new_child_id.clone(), cx)
                    }))
                    .child(icon(icons::PLUS).size(px(12.0)))
                    .child("Child")
                    .into_any_element(),
            );
        }
        controls
    }

    /// The unified titlebar in chat mode:
    /// `[new-session +] [harness icon + session title] … [toggle-changes]`.
    /// Replaces the tab strip; inherits its titlebar duties (drag region,
    /// animated left inset, the toggle-changes button on git projects).
    pub(super) fn render_session_title_bar(
        &mut self,
        viewport_height: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        // The ordinary new-session canvas has no title; a child canvas names
        // its parent so the pending relationship stays visible before Send.
        // The bar keeps its height, drag region, and buttons. A session appends its target as a muted
        // "project @ device" tag right of the title (the composer footer no
        // longer carries it).
        let (title, target, harness, on_canvas): (
            SharedString,
            Option<SharedString>,
            Option<zeron_proto::HarnessId>,
            bool,
        ) = {
            let state = self.state.read(cx);
            match state.selected_chat_row() {
                Some(chat) => {
                    let folder = chat
                        .space_id
                        .as_deref()
                        .and_then(|id| state.space_row(id))
                        .map(|s| s.display_name().to_string())
                        .unwrap_or_else(|| "~".to_string());
                    let device = state
                        .device_name(&chat.device_id)
                        .unwrap_or("Unknown device");
                    (
                        SharedString::from(transcript::single_line(
                            &chat.title.clone().unwrap_or_else(|| "New session".into()),
                        )),
                        Some(SharedString::from(format!("{folder} @ {device}"))),
                        chat.config.as_ref().map(|c| c.harness),
                        false,
                    )
                }
                None => {
                    let child_of = state.pending_child_chat.as_ref().and_then(|pending| {
                        state
                            .chats
                            .iter()
                            .find(|chat| chat.id == pending.parent_chat_id)
                    });
                    let title = child_of.map_or_else(String::new, |parent| {
                        format!(
                            "Child of {}",
                            transcript::single_line(
                                parent.title.as_deref().unwrap_or("New session")
                            )
                        )
                    });
                    (SharedString::from(title), None, None, true)
                }
            }
        };

        // The new-session `+` renders in the WINDOW-CONTROL CLUSTER whenever a
        // session is selected (`render_titlebar_cluster`) — this row budgets
        // one button slot so the title never sits under it.
        let sidebar_now = self.sidebar_now();
        let plus_inset = TITLEBAR_ACTION_SLOT_WIDTH * self.titlebar_plus_alpha(cx);

        // Same glide as the old strip: content starts at the inset card's
        // left edge while the sidebar is open, and slides toward the control
        // cluster as it collapses.
        let content_left =
            (sidebar_now + Theme::SPACE_LG).max(self.title_bar_content_start() + plus_inset);

        // Trailing titlebar section. With the changes pane open this is the
        // PANE'S HEADER — a strip exactly as wide as the pane carrying its
        // controls (scope dropdown, ref selector, fold-all from the Changes
        // entity; expand + close shell-side). It lives up here because the
        // titlebar overlay owns this band's hit-testing: controls mounted in
        // the pane itself would sit under the drag region and never see a
        // click. Closed, it is just the stable open/close toggle. Hidden on
        // the new-session canvas (user request) — nothing to diff yet.
        let right_pane_open = !on_canvas && self.right_pane_open(cx);
        let takeover = right_pane_open && self.right_pane_expanded;
        // In takeover the title hides and the strip owns the whole band, so
        // the row's left inset pulls back to the sidebar seam — the title
        // inset would push the scope dropdown off the pane's own left gutter
        // (user report: misaligned dead space). With the sidebar COLLAPSED
        // the seam is the window edge, where the traffic lights + nav
        // cluster overlay lives — the strip must still clear it, but only
        // just: `title_bar_content_start` carries the identity-group margin the
        // strip doesn't want (it brings its own 8px pad), and doubling up
        // read as a hole after the `+` (user report).
        let row_left = if takeover {
            // The surface tabs must LEFT-ALIGN with the pane's own rows (the
            // diff options and stats strip carry an 8px box gutter off the
            // seam — user report: rows started at different insets). The
            // strip's width is capped to `avail`, which subtracts the row's
            // 8px child gap — pulling row_left 8 LEFT of the seam cancels
            // that, so the uncapped strip starts exactly at the seam and its
            // own 8px pad lands the first chip on the pane gutter. The
            // window-control cluster still wins while the sidebar is
            // collapsed (the chips clear it instead of underlapping).
            let cluster_end =
                self.title_bar_content_start() - TITLEBAR_IDENTITY_GAP + plus_inset - 14.0;
            (sidebar_now - 8.0).max(cluster_end)
        } else {
            content_left
        };
        let row_gap = 8.0;
        let files_width = self.files_visible_width(cx);
        let right_pad = self.titlebar_right_pad(TITLEBAR_ACTION_EDGE_INSET);
        // The title row's gaps are outside the fixed-width panel controls.
        let gap_budget = if takeover { row_gap } else { row_gap * 3.0 };
        let right_visible = self.right_visible_width(cx);
        let widths = panel_titlebar_widths(
            right_visible,
            files_width,
            self.viewport_width - row_left - right_pad - gap_budget,
            right_pad,
        );
        // The trailing strip always carries the explorer slot with its two
        // toggles; the surface tabs reveal to their left only while the surface
        // host is open.
        let trailing_width = if on_canvas {
            0.0
        } else {
            let surface = if right_pane_open {
                widths.surface_reveal
            } else {
                0.0
            };
            surface + widths.files_controls
        };
        let available_titlebar_width =
            (self.viewport_width - row_left - right_pad - trailing_width - row_gap * 3.0).max(0.0);

        let trailing: Option<gpui::AnyElement> = if on_canvas {
            None
        } else {
            let mut controls = div()
                .id("right-titlebar-controls")
                .flex_none()
                .h_full()
                .flex()
                .flex_row()
                .items_center();
            if right_pane_open {
                // The right pane's SURFACE TABS (t3 RightPanelTabs) — the diff
                // options that used to live here moved into the pane's own
                // second row; expand stays in this band (user request).
                let tabs = self.render_right_tab_strip(cx);
                // The toggle is the fixed right-edge anchor, like the left
                // sidebar control. Only the tabs + expand section reveals to
                // its left; including the toggle in this animated width
                // compressed both icons into the same clipped box at open.
                controls = controls.child(
                    div()
                        .w(px(widths.surface_reveal))
                        .h_full()
                        .flex_none()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(4.0))
                        .overflow_hidden()
                        // 8 + the trigger's own 8px pad = the pane's 16px
                        // text gutter. The 4px right padding is the stable
                        // gap before the fixed toggle.
                        .pl(px(8.0))
                        .pr(px(4.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .overflow_hidden()
                                .child(tabs),
                        )
                        .child(header_icon_button(
                            "expand-changes",
                            right_pane_expand_icon(self.right_pane_expanded),
                            &theme,
                            cx.listener(|this, _, _, cx| this.toggle_right_pane_expand(cx)),
                        )),
                );
            }
            // The explorer slot sits over the explorer column and carries the
            // two fixed right-edge anchors — the explorer toggle and,
            // outermost, the pane toggle — which stay mounted at one position
            // while the surface tabs reveal to their left. The explorer's own
            // search and visibility controls live in its secondary header.
            Some(
                controls
                    .child(
                        div()
                            .w(px(widths.files_controls))
                            .h_full()
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap(px(PANEL_TOGGLE_GAP))
                            // No hairline here: the explorer column below is
                            // padded down by the titlebar height and its left
                            // border already runs through this band, so a
                            // second one on the slot stacked on the same
                            // pixels and read lighter than the seam beneath
                            // it (user report).
                            .child(
                                header_icon_button(
                                    "toggle-files-panel",
                                    icons::FILE_TREE,
                                    &theme,
                                    cx.listener(|this, _, window, cx| {
                                        this.toggle_files_panel(window, cx)
                                    }),
                                )
                                .role(gpui::Role::Button)
                                .aria_label(if self.files_panel_open(cx) {
                                    "Hide files panel"
                                } else {
                                    "Show files panel"
                                })
                                .when(self.files_panel_open(cx), |button| {
                                    button.bg(crate::theme::wash(0.09))
                                }),
                            )
                            .child(header_icon_button(
                                "toggle-changes",
                                icons::SIDEBAR_MINIMALISTIC,
                                &theme,
                                cx.listener(|this, _, _, cx| this.toggle_right_pane(cx)),
                            )),
                    )
                    .into_any_element(),
            )
        };

        let actions = (!takeover && !on_canvas)
            .then(|| {
                self.render_project_actions_control(available_titlebar_width, viewport_height, cx)
            })
            .flatten();
        let thread_relations = if !takeover && !on_canvas {
            self.render_thread_relation_controls(&theme, cx)
        } else {
            Vec::new()
        };
        let inner = div()
            .size_full()
            .flex()
            .items_center()
            .pt(px(Theme::TITLEBAR_TOP_PAD))
            .gap(px(row_gap))
            .pl(px(row_left))
            .pr(px(right_pad))
            // In panel takeover the header strip spans the whole band — the
            // title would sit UNDER it (both flex_none, the row overflows and
            // paint order stacks them), so it hides for the duration.
            .when(!takeover, |el| {
                el.child(
                    div()
                        .min_w_0()
                        .overflow_hidden()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .when_some(
                            harness.map(crate::pickers::harness_brand_icon),
                            |el, (path, tint)| {
                                el.child(
                                    icon(path)
                                        .size(px(14.0))
                                        .flex_none()
                                        .text_color(tint.unwrap_or(theme.text_muted)),
                                )
                            },
                        )
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::ui_rems(12.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(if on_canvas {
                                    theme.text_muted.opacity(0.7)
                                } else {
                                    theme.text.opacity(0.85)
                                })
                                .child(title),
                        )
                        .when_some(target, |el, target| {
                            el.child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .text_color(theme.text_muted.opacity(0.5))
                                    .child(target),
                            )
                        }),
                )
            })
            .children(thread_relations)
            .child(div().flex_1())
            .children(actions)
            .children(trailing);

        // The unified window titlebar: full-width on the glass shell, ABOVE
        // the inset card. No bottom border — the card's own hairline is the
        // separation; the glass gutter shows between.
        let bar = div().h(px(Theme::TITLEBAR_HEIGHT)).flex_none().child(inner);
        self.titlebar_drag_region("chat-titlebar", bar, cx)
            .into_any_element()
    }
}

#[cfg(test)]
mod panel_titlebar_tests {
    use super::*;

    #[test]
    fn tabs_align_with_the_panel_for_each_caption_layout_and_files_width() {
        let viewport = 1400.0;
        for right_pad in [6.0, 40.0, 92.0, 114.0] {
            for files in [0.0, 10.0, 28.0, 100.0, 220.0, 286.0, 440.0] {
                let widths = panel_titlebar_widths(520.0, files, 1100.0, right_pad);
                let controls_left =
                    viewport - right_pad - widths.files_controls - widths.surface_reveal;
                assert_eq!(
                    controls_left,
                    viewport - files - 520.0,
                    "caption clearance {right_pad}, Files width {files}"
                );
                assert!(widths.files_controls >= PANEL_TOGGLE_SLOTS);
                if files >= right_pad + PANEL_TOGGLE_SLOTS {
                    assert_eq!(
                        viewport - right_pad - widths.files_controls,
                        viewport - files
                    );
                }
            }
        }
    }

    #[test]
    fn expanded_tabs_clear_the_left_controls_without_reserving_captions_twice() {
        for right_pad in [6.0, 92.0, 114.0] {
            let viewport = 1400.0;
            let files = 286.0;
            // With the sidebar open, the header starts exactly at its seam.
            // With it collapsed, leave room for the window/nav controls.
            for (sidebar, row_left) in [(256.0, 248.0), (0.0, 180.0)] {
                let widths = panel_titlebar_widths(
                    viewport - sidebar - files,
                    files,
                    viewport - row_left - right_pad - 8.0,
                    right_pad,
                );
                let controls_left =
                    viewport - right_pad - widths.files_controls - widths.surface_reveal;
                assert_eq!(controls_left, sidebar.max(row_left + 8.0));
            }
        }
    }

    #[test]
    fn narrow_panels_and_tight_headers_keep_nonnegative_reveal_widths() {
        // A narrow surface and Files share a 520px header.
        let widths = panel_titlebar_widths(234.0, 286.0, 600.0, 92.0);
        assert_eq!(
            1000.0 - 92.0 - widths.files_controls - widths.surface_reveal,
            480.0
        );
        for available in [-20.0, 0.0, 28.0, 56.0, 100.0] {
            let widths = panel_titlebar_widths(0.0, 0.0, available, 114.0);
            assert_eq!(widths.surface_reveal, 0.0);
            assert_eq!(widths.files_controls, PANEL_TOGGLE_SLOTS);
        }
    }
}

#[cfg(test)]
mod cycle_tests {
    use super::*;

    fn order(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    #[test]
    fn steps_forward_and_back_through_the_list() {
        let list = order(&["a", "b", "c"]);
        assert_eq!(cycle_target(&list, Some("a"), true).as_deref(), Some("b"));
        assert_eq!(cycle_target(&list, Some("b"), true).as_deref(), Some("c"));
        assert_eq!(cycle_target(&list, Some("c"), false).as_deref(), Some("b"));
        assert_eq!(cycle_target(&list, Some("b"), false).as_deref(), Some("a"));
    }

    #[test]
    fn wraps_at_both_ends() {
        let list = order(&["a", "b", "c"]);
        assert_eq!(cycle_target(&list, Some("c"), true).as_deref(), Some("a"));
        assert_eq!(cycle_target(&list, Some("a"), false).as_deref(), Some("c"));
    }

    #[test]
    fn a_single_session_cycles_to_itself() {
        // Not a no-op by accident: with one row both directions must resolve,
        // so the shortcut never looks broken by dead-ending on `None`.
        let list = order(&["only"]);
        assert_eq!(
            cycle_target(&list, Some("only"), true).as_deref(),
            Some("only")
        );
        assert_eq!(
            cycle_target(&list, Some("only"), false).as_deref(),
            Some("only")
        );
    }

    #[test]
    fn no_selection_enters_the_list_from_the_matching_end() {
        let list = order(&["a", "b", "c"]);
        assert_eq!(cycle_target(&list, None, true).as_deref(), Some("a"));
        assert_eq!(cycle_target(&list, None, false).as_deref(), Some("c"));
        assert_eq!(
            cycle_target(&list, Some("gone"), true).as_deref(),
            Some("a")
        );
        assert_eq!(
            cycle_target(&list, Some("gone"), false).as_deref(),
            Some("c")
        );
    }

    #[test]
    fn an_empty_list_has_nothing_to_select() {
        assert_eq!(cycle_target(&[], None, true), None);
        assert_eq!(cycle_target(&[], Some("a"), true), None);
    }

    // Cycling walks the rows the sidebar is drawing, not every chat: that
    // guarantee is structural now — `cycle_session` reads the same
    // `AppState::sidebar_chats` the sidebar and the jump shortcuts read, and
    // `jump_slots_count_the_rows_the_sidebar_draws` (state.rs) covers the
    // space-filter behaviour for all of them.
}
