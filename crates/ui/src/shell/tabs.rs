//! Session navigation — the horizontal tab strip is gone (wing 2026-08-10):
//! the activity sidebar IS the session list, and the titlebar names the
//! selected session (harness brand icon + title). A `+` new-session button
//! lives in the titlebar's left control cluster while an existing session is
//! selected. `UiSettings.open_tabs` is legacy — no longer read or written.

use super::*;

const RIGHT_PANE_HEADER_CONTROLS_MIN_WIDTH: f32 = 8.0 + 4.0 + 4.0 + 28.0;

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

/// The session header's "+" and fork buttons (28px each, 2px gap) plus the
/// row gap they cost the project-actions control.
const SESSION_CONTROLS_WIDTH: f32 = 28.0 * 2.0 + 2.0 + 8.0;

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
    ///
    /// A new chat always starts with the terminal hidden: when the drawer is
    /// open it just hides (detach, not close — the source chat's tabs and
    /// PTYs survive for the return trip).
    pub(super) fn open_new_session(&mut self, cx: &mut Context<Self>) {
        self.command_palette = None;
        self.route = Route::Chat;
        self.focus_composer(cx);
        // Pre-hide before the selection flips so the state change can't
        // auto-create a canvas tab (the panel's observer runs on the same
        // update). The source chat's flag stays set — returning restores it.
        let was_open = self.terminal_open(cx)
            || self
                .terminal
                .as_ref()
                .is_some_and(|panel| panel.read(cx).is_open());
        if was_open {
            self.terminal_tween = None;
            if let Some(panel) = self.terminal.clone() {
                panel.update(cx, |panel, cx| panel.set_open(false, cx));
            }
        }
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
        // The canvas never restores a drawer: a previously opened canvas
        // terminal must not pop open on a fresh new chat.
        let key = self.panel_key(cx);
        if self.panels.get(&key).terminal_open {
            self.panels.update(&key, |panels| {
                panels.terminal_open = false;
            });
        }
        self.terminal_tween = None;
        if let Some(panel) = self.terminal.clone()
            && panel.read(cx).is_open()
        {
            panel.update(cx, |panel, cx| panel.set_open(false, cx));
        }
        cx.notify();
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
        // The canvas titles as NOTHING (user request — a "New session"
        // header over the empty canvas was noise); the bar keeps its height,
        // drag region, and buttons. A session appends its target as a muted
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
                None => (SharedString::from(""), None, None, true),
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
        let fit = self.horizontal_fit();
        let right_pane_visible = !on_canvas
            && fit.right
            && (self.right_pane_open(cx) || self.tween_active(self.right_tween));
        let takeover = right_pane_visible
            && (self.right_pane_expanded || self.tween_active(self.main_takeover_tween));
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
        // Match the surface's visible width, including its resize bounce,
        // so the header follows the pane seam as the sidebar moves.
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
            let surface = if right_pane_visible {
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
                .debug_selector(|| "right-titlebar-controls".into())
                .flex_none()
                .h_full()
                .flex()
                .flex_row()
                .items_center()
                // The transcript extends under this band (it fades beneath
                // the tab strip); a wheel over the tabs must scroll the
                // strip, never the surface behind it.
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
            if right_pane_visible {
                // The right pane's SURFACE TABS (t3 RightPanelTabs) — the diff
                // options that used to live here moved into the pane's own
                // second row; expand stays in this band (user request).
                let show_pane_controls =
                    widths.surface_reveal >= RIGHT_PANE_HEADER_CONTROLS_MIN_WIDTH;
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
                        .when(show_pane_controls, |el| {
                            el.pl(px(8.0))
                                .pr(px(4.0))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .h_full()
                                        .overflow_hidden()
                                        .child(self.render_right_tab_strip(cx)),
                                )
                                .child(header_icon_button(
                                    "expand-changes",
                                    right_pane_expand_icon(self.right_pane_expanded),
                                    &theme,
                                    cx.listener(|this, _, _, cx| this.toggle_right_pane_expand(cx)),
                                ))
                        }),
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
                                .aria_label(if self.files_panel_open(cx) && fit.files {
                                    "Hide files panel"
                                } else {
                                    "Show files panel"
                                })
                                .when(self.files_panel_open(cx) && fit.files, |button| {
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

        // The session's own side-chat controls: "+" mints a fresh side chat
        // under this session, fork copies its history into one. Same pair
        // the side-chat header carries, so a family reads the same from
        // either end.
        let show_actions = !takeover
            && !on_canvas
            && available_titlebar_width >= actions_ui::PROJECT_ACTION_CONTROL_MIN_WIDTH;
        let show_session_controls = !takeover
            && !on_canvas
            && available_titlebar_width
                >= SESSION_CONTROLS_WIDTH
                    + if show_actions {
                        actions_ui::PROJECT_ACTION_CONTROL_MIN_WIDTH
                    } else {
                        0.0
                    };
        let session_controls = show_session_controls.then(|| {
            let busy = self.side_chat_creating;
            div()
                .flex_none()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(2.0))
                .child(
                    header_icon_button(
                        "session-new-side-chat",
                        icons::PLUS,
                        &theme,
                        cx.listener(|this, _, _, cx| this.create_child_chat(None, cx)),
                    )
                    .role(gpui::Role::Button)
                    .aria_label("New side chat")
                    .when(busy, |el| el.opacity(0.4)),
                )
                .child(
                    header_icon_button(
                        "session-fork",
                        icons::GIT_BRANCH,
                        &theme,
                        cx.listener(|this, _, _, cx| this.create_side_chat(cx)),
                    )
                    .role(gpui::Role::Button)
                    .aria_label("Fork this session")
                    .when(busy, |el| el.opacity(0.4)),
                )
        });
        let available_titlebar_width = if session_controls.is_some() {
            (available_titlebar_width - SESSION_CONTROLS_WIDTH).max(0.0)
        } else {
            available_titlebar_width
        };
        let actions = show_actions
            .then(|| {
                self.render_project_actions_control(available_titlebar_width, viewport_height, cx)
            })
            .flatten();
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
            .child(div().flex_1())
            .children(session_controls)
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

#[cfg(test)]
mod titlebar_geometry_tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    struct TitlebarHost {
        shell: Entity<Shell>,
        _data_dir: tempfile::TempDir,
    }

    impl Render for TitlebarHost {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.shell.update(cx, |shell, cx| {
                shell.viewport_width = f32::from(window.viewport_size().width);
                div()
                    .w(px(shell.viewport_width))
                    .h(px(80.0))
                    .relative()
                    .child(shell.render_session_title_bar(window.viewport_size().height, cx))
                    .child(shell.render_titlebar_cluster(cx))
            })
        }
    }

    #[gpui::test]
    fn panel_titlebar_controls_keep_their_edges_during_sidebar_motion(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            settings::init(settings::UiSettings::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            let shell = cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                );
                shell.active_chat = "session".into();
                shell.state.update(cx, |state, _| {
                    state.chats.push(serde_json::from_value(serde_json::json!({
                        "id": "session", "title": "A deliberately long session title for geometry testing",
                        "deviceId": "local", "archived": false, "createdAt": Utc::now(),
                        "spaceId": "project",
                    })).unwrap());
                    state.spaces.push(serde_json::from_value(serde_json::json!({
                        "id": "project", "deviceId": "local", "path": "/project",
                        "createdAt": Utc::now(),
                    })).unwrap());
                    state.selected_chat = Some("session".into());
                });
                let key = crate::project_actions::ProjectActionsKey {
                    device_id: "local".into(),
                    space_id: "project".into(),
                };
                shell.project_actions.active = Some(key.clone());
                shell.project_actions.cache.insert(
                    key,
                    crate::project_actions::ProjectActionsStatus::Ready(
                        zeron_proto::ProjectActionsSnapshot {
                            space_id: "project".into(),
                            actions: vec![zeron_proto::ProjectAction {
                                id: "dev".into(),
                                name: "Development web server".into(),
                                command: "dev".into(),
                                icon: zeron_proto::ProjectActionIcon::Play,
                                run_on_worktree_create: false,
                            }],
                            importable_actions: Vec::new(),
                            project_file_issue: None,
                        },
                    ),
                );
                let key = shell.panel_key(cx);
                shell.panels.toggle_changes(&key);
                shell
            });
            TitlebarHost { shell, _data_dir: dir }
        });
        let shell = host.read_with(cx, |host, _| host.shell.clone());
        let duration = RESIZE.total().mul_f32(motion::speed_scale());
        let started = std::time::Instant::now();
        let mut sidebar_button_left = None;
        for width in [1400.0, 1024.0, 800.0, 634.0, 633.0, 630.0, 629.0, 600.0] {
            cx.simulate_resize(gpui::size(px(width), px(600.0)));
            for (collapsed, from, to) in [(false, 0.0, 256.0), (true, 256.0, 0.0)] {
                shell.update(cx, |shell, _| {
                    shell.settings.sidebar_collapsed = collapsed;
                    shell.sidebar_tween = Some(WidthTween { from, to, started });
                });
                for progress in [0.0, 0.5, 1.0] {
                    shell.update(cx, |shell, _| {
                        shell.render_time = Some(started + duration.mul_f32(progress));
                    });
                    host.update(cx, |_, cx| cx.notify());
                    cx.update(|window, cx| window.draw(cx).clear());
                    let left = cx.debug_bounds("toggle-sidebar").unwrap();
                    let right = cx.debug_bounds("toggle-changes").unwrap();
                    let expand = cx.debug_bounds("expand-changes");
                    let row = cx.debug_bounds("right-titlebar-controls").unwrap();
                    let action = cx.debug_bounds("project-actions-control");
                    let (pane_width, right_pad, left_clearance) =
                        shell.read_with(cx, |shell, cx| {
                            let content_left = (shell.sidebar_now() + Theme::SPACE_LG).max(
                                shell.title_bar_content_start()
                                    + TITLEBAR_ACTION_SLOT_WIDTH * shell.titlebar_plus_alpha(cx),
                            );
                            (
                                shell.right_visible_width(cx),
                                shell.titlebar_right_pad(TITLEBAR_ACTION_EDGE_INSET),
                                content_left + 8.0 * 3.0,
                            )
                        });
                    let left_x = f32::from(left.left());
                    if let Some(initial) = sidebar_button_left {
                        assert_eq!(left_x, initial, "sidebar trigger shifted");
                    } else {
                        sidebar_button_left = Some(left_x);
                    }
                    assert!(
                        (f32::from(right.left()) - (width - right_pad - 28.0)).abs() < 0.5,
                        "right trigger at {}, expected {} for {width}px, {collapsed:?}, {progress}; row {}, action right {:?}, pane width {}",
                        f32::from(right.left()),
                        width - right_pad - 28.0,
                        f32::from(row.left()),
                        action.as_ref().map(|action| f32::from(action.right())),
                        pane_width,
                    );
                    // At tight widths, the fixed toggles can be wider than
                    // the pane, or the window-control cluster can constrain
                    // the reveal. Otherwise the strip follows the seam.
                    let expected_left = (width - pane_width)
                        .min(width - right_pad - PANEL_TOGGLE_SLOTS)
                        .max(left_clearance);
                    assert!(
                        (f32::from(row.left()) - expected_left).abs() < 0.5,
                        "pane header at {}, expected {} at {width}px, {collapsed:?}, {progress}",
                        f32::from(row.left()),
                        expected_left,
                    );
                    if let Some(action) = action {
                        assert!(action.right() + px(8.0) <= row.left());
                    }
                    assert_eq!(
                        expand.is_some(),
                        f32::from(row.size.width)
                            >= RIGHT_PANE_HEADER_CONTROLS_MIN_WIDTH + PANEL_TOGGLE_SLOTS
                    );
                    if let Some(expand) = expand {
                        assert!(expand.right() + px(4.0) <= right.left());
                    }
                }
            }
        }

        // The expanded pane intentionally leaves room for the fixed left
        // cluster while the sidebar is collapsed; its buttons still cannot
        // collide with one another or change their right-edge position.
        cx.simulate_resize(gpui::size(px(1024.0), px(600.0)));
        shell.update(cx, |shell, _| {
            shell.right_pane_expanded = true;
            shell.settings.sidebar_collapsed = true;
            shell.sidebar_tween = None;
            shell.render_time = None;
        });
        host.update(cx, |_, cx| cx.notify());
        cx.update(|window, cx| window.draw(cx).clear());
        let right = cx.debug_bounds("toggle-changes").unwrap();
        let expand = cx.debug_bounds("expand-changes").unwrap();
        let row = cx.debug_bounds("right-titlebar-controls").unwrap();
        assert!(expand.right() + px(4.0) <= right.left());
        assert!(row.left() > px(0.0));

        // A recently resized right pane can still be bouncing when the
        // sidebar changes. The titlebar follows the painted width, including
        // the responsive cap that preserves the conversation minimum.
        let bounce_started = std::time::Instant::now();
        cx.simulate_resize(gpui::size(px(800.0), px(600.0)));
        shell.update(cx, |shell, _| {
            shell.right_pane_expanded = false;
            shell.settings.sidebar_collapsed = false;
            shell.sidebar_tween = Some(WidthTween {
                from: 0.0,
                to: 256.0,
                started: bounce_started,
            });
            shell.right_tween = None;
            shell.right_edge_bounce = Some(motion::ResizeEdgeBounce {
                edge: motion::ResizeEdge::Max,
                started: bounce_started,
            });
            shell.render_time = Some(bounce_started + std::time::Duration::from_millis(80));
        });
        host.update(cx, |_, cx| cx.notify());
        cx.update(|window, cx| window.draw(cx).clear());
        let row = cx.debug_bounds("right-titlebar-controls").unwrap();
        let pane_width = shell.read_with(cx, |shell, cx| {
            let bounce = shell.right_now(cx) - shell.right_target(cx);
            assert!(bounce.abs() > 0.5);
            shell.right_visible_width(cx)
        });
        assert!((f32::from(row.left()) - (800.0 - pane_width)).abs() < 0.5);

        // Closing the surface must keep its header mounted behind the same
        // shrinking mask as the pane; Files and the fixed toggles remain in
        // their own slots while both widths move.
        cx.simulate_resize(gpui::size(px(1400.0), px(600.0)));
        for files_open in [false, true] {
            for progress in [0.0, 0.1, 0.25, 0.5, 0.75] {
                shell.update(cx, |shell, _| {
                    shell.settings.sidebar_collapsed = true;
                    shell.sidebar_tween = None;
                    shell.right_edge_bounce = None;
                    shell.panels.update("session", |panels| {
                        panels.changes_open = false;
                        panels.files_open = files_open;
                    });
                    shell.files_tween = None;
                    shell.right_tween = Some(WidthTween {
                        from: 520.0,
                        to: 0.0,
                        started,
                    });
                    shell.render_time = Some(started + duration.mul_f32(progress));
                });
                host.update(cx, |_, cx| cx.notify());
                cx.update(|window, cx| window.draw(cx).clear());
                let toggle = cx.debug_bounds("toggle-changes").unwrap();
                let files_toggle = cx.debug_bounds("toggle-files-panel").unwrap();
                let expand = cx.debug_bounds("expand-changes");
                let strip = cx.debug_bounds("right-titlebar-controls").unwrap();
                let (right, files, pad) = shell.read_with(cx, |shell, cx| {
                    (
                        shell.right_visible_width(cx),
                        shell.files_visible_width(cx),
                        shell.titlebar_right_pad(TITLEBAR_ACTION_EDGE_INSET),
                    )
                });
                assert!((f32::from(toggle.left()) - (1400.0 - pad - 28.0)).abs() < 0.5);
                assert!(files_toggle.right() + px(PANEL_TOGGLE_GAP) <= toggle.left());
                if right + files >= pad + PANEL_TOGGLE_SLOTS {
                    assert!(
                        (f32::from(strip.left()) - (1400.0 - right - files)).abs() < 0.5,
                        "header drift with Right {right}px and Files {files}px"
                    );
                }
                if right > 160.0 {
                    assert!(
                        expand.is_some(),
                        "surface controls vanished early at {right}px"
                    );
                }
                if let Some(expand) = expand {
                    assert!(expand.right() + px(4.0) <= files_toggle.left());
                }
            }
        }
    }
}
