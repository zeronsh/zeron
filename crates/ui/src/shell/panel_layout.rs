//! Panel layout policy: which columns fit the window, how the open ones
//! share its width, and the motion between arrangements.
use super::*;

/// A panel's content tween for a mask running `from → to`, given the content
/// width laid out now. Opening from closed lays out at the destination and
/// closing keeps the current layout, so the mask never reflows what it
/// uncovers; a reversal between two open widths eases the layout to the
/// destination instead of snapping to it on the last frame.
pub(super) fn panel_content_tween(current: f32, from: f32, to: f32) -> (f32, f32) {
    if from < 0.5 {
        (to, to)
    } else if to < 0.5 {
        let width = current.max(from);
        (width, width)
    } else {
        (current.max(from), to)
    }
}

/// The surface host's half of `available`, the width the tools leave for
/// the conversation and the host. When halves would break a minimum, the
/// host keeps its own and the conversation keeps as much of its own as fits.
pub(super) fn content_split(available: f32) -> f32 {
    (available / 2.0)
        .max(RIGHT_PANE_MIN)
        .min((available - CHAT_PANEL_MIN).max(RIGHT_PANE_MIN))
}

/// What the Files and surface host columns paint, read before an action
/// changes anything so the panels that stay open can ease from there.
#[derive(Debug, Clone, Copy)]
pub(super) struct PaintedPanels {
    pub(super) files: f32,
    pub(super) right: f32,
    /// The width the surface host's content lays out at.
    pub(super) right_content: f32,
    /// The width Files' content lays out at.
    pub(super) files_content: f32,
    /// The width the conversation's content lays out at.
    pub(super) conversation_content: f32,
}

/// The columns beside the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuxiliaryPanel {
    Sidebar,
    Right,
    Files,
}

/// Responsive visibility is separate from each panel's open flag. A hidden
/// panel keeps its tabs and width preference and returns when space permits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct HorizontalPanelFit {
    pub(super) sidebar: bool,
    pub(super) right: bool,
    pub(super) files: bool,
    pub(super) sidebar_limit: f32,
}

/// Everything [`horizontal_panel_fit`] reads, in its argument order.
pub(super) type FitInputs = (f32, f32, (bool, u64), (bool, u64), (bool, u64), bool);

pub(super) fn horizontal_panel_fit(
    viewport: f32,
    sidebar_width: f32,
    sidebar: (bool, u64),
    right: (bool, u64),
    files: (bool, u64),
    expanded: bool,
) -> HorizontalPanelFit {
    let mut visible = [
        (AuxiliaryPanel::Sidebar, sidebar.0, sidebar.1),
        (AuxiliaryPanel::Right, right.0, right.1),
        (AuxiliaryPanel::Files, files.0, files.1),
    ];
    let viewport = viewport.max(0.0);
    // Takeover only spends the conversation's width on the right pane. Keep
    // the ordinary fit for the other columns so expansion cannot resurrect a
    // hidden panel or widen a sidebar whose preferred width was squeezed.
    // Below the chat + right minima, the right pane alone may still take over.
    let narrow_takeover = expanded && right.0 && viewport < CHAT_PANEL_MIN + RIGHT_PANE_MIN;
    if narrow_takeover {
        visible[0].1 = false;
        visible[2].1 = false;
    }
    loop {
        let sidebar_open = visible[0].1;
        let right_open = visible[1].1;
        let files_open = visible[2].1;
        let chat_floor = if right_open && narrow_takeover {
            0.0
        } else {
            CHAT_PANEL_MIN
        };
        let minimum = chat_floor
            + if sidebar_open { SIDEBAR_MIN } else { 0.0 }
            + if right_open { RIGHT_PANE_MIN } else { 0.0 }
            + if files_open { FILES_PANEL_MIN } else { 0.0 };
        if minimum <= viewport {
            return HorizontalPanelFit {
                sidebar: sidebar_open,
                right: right_open,
                files: files_open,
                sidebar_limit: if sidebar_open {
                    (viewport
                        - chat_floor
                        - if right_open { RIGHT_PANE_MIN } else { 0.0 }
                        - if files_open { FILES_PANEL_MIN } else { 0.0 })
                    .min(sidebar_width)
                } else {
                    0.0
                },
            };
        }
        // Takeover is the user's explicit choice to give the surface the
        // conversation's space. Evict the other columns before that surface.
        let victim = visible
            .iter()
            .enumerate()
            .filter(|(_, (_, open, _))| *open)
            .min_by_key(|(_, (panel, _, opened_at))| {
                (expanded && *panel == AuxiliaryPanel::Right, *opened_at)
            })
            .map(|(index, _)| index);
        let Some(victim) = victim else {
            return HorizontalPanelFit {
                sidebar: false,
                right: false,
                files: false,
                sidebar_limit: 0.0,
            };
        };
        visible[victim].1 = false;
    }
}

impl Shell {
    /// Navigation and direct window resizing discard the departing allocation.
    pub(super) fn clear_panel_transitions(&mut self) {
        self.files_tween = None;
        self.files_content_tween = None;
        self.right_tween = None;
        self.right_content_tween = None;
        self.main_takeover_tween = None;
    }

    /// Which columns the window has room for this frame. A closing column
    /// keeps its slot until its mask is gone.
    pub(super) fn horizontal_fit(&self) -> HorizontalPanelFit {
        self.fit(true)
    }

    /// The fit once every running close has landed, and the sidebar's width
    /// in it.
    pub(super) fn settled_fit(&self) -> (HorizontalPanelFit, f32) {
        let fit = self.fit(false);
        let sidebar = if fit.sidebar && !self.settings.sidebar_collapsed {
            fit.sidebar_limit
        } else {
            0.0
        };
        (fit, sidebar)
    }

    /// `closing: false` is the fit once every running close has landed.
    fn fit(&self, closing: bool) -> HorizontalPanelFit {
        if !matches!(self.route, Route::Chat) {
            return HorizontalPanelFit {
                sidebar: !self.settings.sidebar_collapsed,
                right: false,
                files: false,
                sidebar_limit: self.settings.sidebar_width,
            };
        }
        let panels = self.panels.get(&self.active_chat);
        let on_chat = !self.active_chat.is_empty();
        // A closing column keeps its slot while it still paints: one the fit
        // had already hidden closes from zero and claims nothing.
        let present = |open: bool, tween: Option<WidthTween>| {
            open || (closing
                && tween.is_some_and(|tween| self.tween_active(Some(tween)) && tween.from > 0.5))
        };
        let inputs: FitInputs = (
            self.viewport_width,
            self.settings.sidebar_width,
            (
                present(!self.settings.sidebar_collapsed, self.sidebar_tween),
                self.sidebar_opened_at,
            ),
            (
                on_chat && present(panels.changes_open, self.right_tween),
                panels.changes_opened_at,
            ),
            (
                on_chat && present(panels.files_open, self.files_tween),
                panels.files_opened_at,
            ),
            self.right_pane_expanded,
        );
        let memo = &self.fit_memo[usize::from(closing)];
        if let Some((memo, fit)) = memo.get()
            && memo == inputs
        {
            return fit;
        }
        let (viewport, sidebar_width, sidebar, right, files, expanded) = inputs;
        let fit = horizontal_panel_fit(viewport, sidebar_width, sidebar, right, files, expanded);
        memo.set(Some((inputs, fit)));
        fit
    }

    /// Follow the window fit from frame to frame. A column the fit newly
    /// hides eases out from the width it last painted, and one it brings
    /// back returns from zero. Closing columns keep their own tween and fit
    /// slot until their mask is gone, so a returning neighbour never jumps
    /// into their space mid-close.
    pub(super) fn track_horizontal_fit(&mut self, cx: &App) {
        if !matches!(self.route, Route::Chat) {
            self.painted_columns = None;
            return;
        }
        let fit = self.horizontal_fit();
        let previous = self
            .painted_columns
            .as_ref()
            .filter(|(chat, ..)| *chat == self.active_chat)
            .map(|(_, fit, columns)| (*fit, *columns));
        // A live window resize or divider drag is direct manipulation: a
        // column the fit hides or brings back, and the sidebar's allowance,
        // follow the window edge or the pointer at once instead of easing
        // behind it.
        let direct = self.viewport_resized || self.pane_resize_dragging.is_some();
        if direct {
            self.fit_exits = [None; 3];
            if self
                .sidebar_tween
                .is_some_and(|tween| tween.from > 0.5 && tween.to > 0.5)
            {
                self.sidebar_tween = None;
            }
        }
        if let Some((previous, painted)) = previous.filter(|_| !direct) {
            // Closing columns count too: the fit may hide one mid-close.
            let panels = self.panels.get(&self.active_chat);
            let open = [
                !self.settings.sidebar_collapsed || self.tween_active(self.sidebar_tween),
                panels.changes_open || self.tween_active(self.right_tween),
                panels.files_open || self.tween_active(self.files_tween),
            ];
            let was = [previous.sidebar, previous.right, previous.files];
            let now = [fit.sidebar, fit.right, fit.files];
            for column in 0..3 {
                if now[column] {
                    self.fit_exits[column] = None;
                } else if was[column] && open[column] && painted[column].0 > 0.5 {
                    let (width, content) = painted[column];
                    self.fit_exits[column] = Some((self.panel_tween(width, 0.0), content));
                }
            }
            // A column the fit shows again returns from what it painted last
            // frame, toward its live target: its own tween, if any, was
            // heading for a target the fit had just zeroed. One the user just
            // opened already runs its own open tween; keep it.
            let opening = [self.sidebar_tween, self.right_tween, self.files_tween].map(|tween| {
                tween.is_some_and(|tween| self.tween_active(Some(tween)) && tween.to > 0.5)
            });
            if fit.sidebar && !previous.sidebar && !self.settings.sidebar_collapsed && !opening[0] {
                self.sidebar_tween = Some(self.panel_tween(painted[0].0, fit.sidebar_limit));
            }
            // A neighbour opening or closing can squeeze or release an open
            // sidebar. Ease it between the two limits alongside that panel.
            let sidebar_closing = self
                .sidebar_tween
                .is_some_and(|tween| self.tween_active(Some(tween)) && tween.to <= 0.0);
            if fit.sidebar
                && previous.sidebar
                && !self.settings.sidebar_collapsed
                && !sidebar_closing
                && (fit.sidebar_limit - previous.sidebar_limit).abs() > 0.5
            {
                // Retarget from the painted width, including mid-flight.
                let from = self
                    .eval_tween(self.sidebar_tween, previous.sidebar_limit)
                    .min(self.sidebar_tween_limit(previous.sidebar_limit));
                self.sidebar_tween = Some(self.panel_tween(from, fit.sidebar_limit));
            }
            if fit.right && !previous.right && panels.changes_open && !opening[1] {
                let tween = self.panel_tween(painted[1].0, self.right_settled_target(cx));
                let (from, to) = panel_content_tween(painted[1].1, tween.from, tween.to);
                self.right_tween = Some(tween);
                self.right_content_tween = Some(WidthTween { from, to, ..tween });
            }
            if fit.files && !previous.files && panels.files_open && !opening[2] {
                let (width, content) = painted[2];
                self.transition_files(width, content, self.files_settled_width(cx));
            }
        }
        // Measuring is not drawing: leave frame scheduling to the columns
        // that actually paint this frame.
        let animating = self.motion_active.get();
        let columns = [
            (
                self.sidebar_now(),
                self.fit_exit_content(0).unwrap_or(fit.sidebar_limit),
            ),
            (
                self.right_visible_width(cx),
                self.right_content_width(self.right_target(cx)),
            ),
            (self.files_visible_width(cx), self.files_content_width(cx)),
        ];
        self.motion_active.set(animating);
        match &mut self.painted_columns {
            Some((chat, painted_fit, painted)) if *chat == self.active_chat => {
                (*painted_fit, *painted) = (fit, columns);
            }
            slot => *slot = Some((self.active_chat.clone(), fit, columns)),
        }
    }

    /// Width of a column the fit hid mid-flight, while its exit runs.
    pub(super) fn fit_exit_width(&self, column: usize) -> Option<f32> {
        self.fit_exits[column]
            .filter(|(exit, _)| self.tween_active(Some(*exit)))
            .map(|(exit, _)| self.eval_tween(Some(exit), 0.0))
    }

    /// The width a fit-hidden column keeps laying out at while it exits.
    pub(super) fn fit_exit_content(&self, column: usize) -> Option<f32> {
        self.fit_exits[column]
            .filter(|(exit, _)| self.tween_active(Some(*exit)))
            .map(|(_, content)| content)
    }

    /// The width the sidebar lays its content out at: the fit's allowance,
    /// or what it had while the fit slides it out. Below the saved width
    /// when the window squeezes it.
    pub(super) fn sidebar_content_width(&self) -> f32 {
        self.fit_exit_content(0)
            .unwrap_or(self.horizontal_fit().sidebar_limit)
    }

    pub(super) fn painted_panels(&self, cx: &App) -> PaintedPanels {
        let right = self.right_visible_width(cx);
        PaintedPanels {
            files: self.files_visible_width(cx),
            files_content: self.files_content_width(cx),
            right,
            right_content: self.right_content_width(self.right_target(cx)),
            conversation_content: self.main_content_width(self.main_target_width(right, cx), cx),
        }
    }

    /// Split the space the sidebar and Files leave equally between the
    /// conversation and the surface host. The sidebar and Files are tools:
    /// they keep their own widths, like a navigator and an inspector, and the
    /// content columns absorb every change. The share persists with the next
    /// settings save, like a drag. Returns whether the surface host took one.
    fn split_panel_widths(&mut self, cx: &App) -> bool {
        if !matches!(self.route, Route::Chat) || self.active_chat.is_empty() {
            return false;
        }
        // Split for where the columns will rest, not for a slot a closing
        // column still holds this frame.
        let (fit, sidebar) = self.settled_fit();
        // Takeover spends the conversation's width on the surface host.
        let right_open = fit.right && self.right_pane_open(cx) && !self.right_pane_expanded;
        if !right_open {
            return false;
        }
        let files = self.files_width_for(fit, sidebar, cx);
        self.settings.right_pane_width =
            content_split((self.viewport_width - sidebar - files).max(0.0));
        true
    }

    /// Opening or closing a panel re-splits the width (a dragged divider
    /// holds until then). A surface host that stays open eases from `before`
    /// to its new share; `skip` is the panel the caller is opening or
    /// closing, which runs its own tween.
    pub(super) fn resplit_panels(
        &mut self,
        before: PaintedPanels,
        skip: Option<AuxiliaryPanel>,
        cx: &App,
    ) {
        let right_open = self.split_panel_widths(cx);
        let takeover_running = self.tween_active(self.main_takeover_tween);
        // Files keeps its own width, but a window too narrow for it beside the
        // conversation and the surface host squeezes it; ease that change too.
        // A resting column resizes, so its content follows. One already
        // opening, closing or returning keeps its reveal.
        let files_to = self.files_settled_width(cx);
        if self.files_panel_open(cx)
            && skip != Some(AuxiliaryPanel::Files)
            && (files_to - before.files).abs() > 0.5
        {
            let resting = !self.tween_active(self.files_tween);
            self.transition_files(before.files, before.files_content, files_to);
            if resting {
                self.files_content_tween = None;
            }
        }
        let right_to = self.right_settled_target(cx);
        if right_open
            && skip != Some(AuxiliaryPanel::Right)
            && (right_to - before.right).abs() > 0.5
        {
            let resting = !self.tween_active(self.right_tween);
            let tween = self.panel_tween(before.right, right_to);
            self.right_tween = Some(tween);
            self.right_content_tween = (!resting).then(|| {
                let (from, to) = panel_content_tween(before.right_content, before.right, right_to);
                WidthTween { from, to, ..tween }
            });
        }
        // A full-screen entry or exit still running restarts with the pane,
        // from the conversation's current layout, so it lands when they do.
        if takeover_running {
            let mut tween = self.panel_tween(before.conversation_content, 0.0);
            if let Some(pane) = self
                .right_tween
                .filter(|pane| self.tween_active(Some(*pane)))
            {
                tween.duration = pane.duration;
            }
            self.main_takeover_tween = Some(tween);
        }
    }

    /// Double-clicking a seam restores the default layout, easing from what
    /// is on screen: the sidebar's or Files' seam brings that tool back to
    /// its default width, and the content columns split equally again.
    pub(super) fn reset_panel_widths(&mut self, seam: PaneResizeKind, cx: &mut Context<Self>) {
        let before = self.painted_panels(cx);
        match seam {
            PaneResizeKind::Sidebar => {
                let from = self.sidebar_now();
                self.settings.sidebar_width = SIDEBAR_DEFAULT;
                self.sidebar_edge_bounce = None;
                self.sidebar_tween = Some(self.panel_tween(from, self.sidebar_target()));
            }
            PaneResizeKind::Files => {
                self.settings.files_panel_width = settings::FILES_PANEL_DEFAULT;
                let to = self.files_settled_width(cx);
                self.transition_files(before.files, before.files_content, to);
            }
            PaneResizeKind::Right | PaneResizeKind::Terminal => {}
        }
        self.right_edge_bounce = None;
        self.resplit_panels(before, None, cx);
    }

    /// A panel the window fit hides is open but off screen. Asking for it
    /// again ranks it most recent; when that brings it back, the columns
    /// re-split and the next frame returns it from what it paints (see
    /// [`Self::track_horizontal_fit`]). True when it came back.
    pub(super) fn promote_hidden_panel(
        &mut self,
        panel: AuxiliaryPanel,
        cx: &mut Context<Self>,
    ) -> bool {
        let shown = |fit: HorizontalPanelFit| match panel {
            AuxiliaryPanel::Sidebar => fit.sidebar,
            AuxiliaryPanel::Right => fit.right,
            AuxiliaryPanel::Files => fit.files,
        };
        if shown(self.horizontal_fit()) {
            return false;
        }
        let before = self.painted_panels(cx);
        let key = self.panel_key(cx);
        let rank = (self.panel_open_sequence, self.panel_opened_at(panel, &key));
        self.record_panel_open(panel, &key);
        if !shown(self.horizontal_fit()) {
            // Still hidden: the caller closes it instead, so keep its rank.
            self.panel_open_sequence = rank.0;
            self.set_panel_opened_at(panel, &key, rank.1);
            return false;
        }
        self.resplit_panels(before, Some(panel), cx);
        cx.notify();
        true
    }

    pub(super) fn record_panel_open(&mut self, panel: AuxiliaryPanel, key: &str) {
        self.panel_open_sequence += 1;
        self.set_panel_opened_at(panel, key, self.panel_open_sequence);
    }

    fn panel_opened_at(&self, panel: AuxiliaryPanel, key: &str) -> u64 {
        let panels = self.panels.get(key);
        match panel {
            AuxiliaryPanel::Sidebar => self.sidebar_opened_at,
            AuxiliaryPanel::Right => panels.changes_opened_at,
            AuxiliaryPanel::Files => panels.files_opened_at,
        }
    }

    fn set_panel_opened_at(&mut self, panel: AuxiliaryPanel, key: &str, opened_at: u64) {
        match panel {
            AuxiliaryPanel::Sidebar => self.sidebar_opened_at = opened_at,
            AuxiliaryPanel::Right => self
                .panels
                .update(key, |panels| panels.changes_opened_at = opened_at),
            AuxiliaryPanel::Files => self
                .panels
                .update(key, |panels| panels.files_opened_at = opened_at),
        }
    }

    /// A running sidebar tween may start above a limit that just dropped; it
    /// eases down to that limit rather than being cut on its first frame.
    pub(super) fn sidebar_tween_limit(&self, limit: f32) -> f32 {
        self.sidebar_tween
            .filter(|tween| self.tween_active(Some(*tween)))
            .map_or(limit, |tween| {
                limit.max(tween.from.min(self.settings.sidebar_width))
            })
    }

    pub(super) fn main_target_width(&self, right_width: f32, cx: &App) -> f32 {
        // The flex column follows the visible sidebar, not its destination.
        // Sizing the composer from sidebar_target() made it overflow that
        // column while the sidebar closed beside an open right pane.
        conversation_width(
            self.viewport_width - self.files_reserved_width(cx),
            self.sidebar_now(),
            right_width,
        )
    }

    pub(super) fn main_takeover_active(&self) -> bool {
        (self.right_pane_expanded && self.horizontal_fit().right)
            || self.tween_active(self.main_takeover_tween)
    }

    /// Takeover clips the conversation; it must not reflow the composer behind
    /// the mask, including after expansion settles and during reversals.
    pub(super) fn main_content_width(&self, target: f32, cx: &App) -> f32 {
        if self.main_takeover_active() {
            let available =
                (self.viewport_width - self.files_reserved_width(cx) - self.sidebar_now()).max(0.0);
            let retained = self
                .main_takeover_tween
                .map(|tween| {
                    if self.right_pane_expanded {
                        tween.from
                    } else {
                        // Return from the captured width to where the
                        // conversation will rest, which a panel opening or
                        // closing meanwhile may have moved.
                        let settled = conversation_width(
                            self.viewport_width - self.files_settled_width(cx),
                            self.sidebar_target(),
                            self.right_settled_target(cx),
                        );
                        self.ease_toward(Some(tween), settled)
                    }
                })
                .unwrap_or_else(|| {
                    // Returning to an expanded chat or resizing clears the tween.
                    // Reconstruct its ordinary conversation geometry, never zero.
                    let right = self
                        .settings
                        .right_pane_width
                        .min((available - CHAT_PANEL_MIN).max(0.0));
                    (available - right).max(0.0)
                });
            retained.max(target).min(available)
        } else {
            target
        }
    }

    pub(super) fn right_content_width(&self, target: f32) -> f32 {
        self.column_content_width(
            (1, self.horizontal_fit().right),
            self.right_tween,
            self.right_content_tween,
            target,
        )
    }

    /// The width a column lays its content out at while its mask eases
    /// toward `target`. `column` is its fit slot and whether the fit shows
    /// it. Shared by the surface host and Files, so a reversal behaves the
    /// same in both.
    pub(super) fn column_content_width(
        &self,
        (column, shown): (usize, bool),
        mask: Option<WidthTween>,
        content: Option<WidthTween>,
        target: f32,
    ) -> f32 {
        if !shown && let Some(width) = self.fit_exit_content(column) {
            return width;
        }
        let content =
            content.filter(|content| mask.is_some_and(|mask| mask.started == content.started));
        // A resize between two open widths (a re-split) has no content tween:
        // the content follows the column instead of relaying out twice.
        if content.is_none()
            && let Some((from, to)) = self.active_tween_endpoints(mask)
            && from > 0.5
            && to > 0.5
        {
            return self.ease_toward(mask, target);
        }
        // Content bound for the column's destination heads for where the
        // column is going now, like the column itself (see `right_now`).
        let moving = content
            .filter(|content| self.tween_active(Some(*content)))
            .map(|content| {
                if mask.is_some_and(|mask| mask.to == content.to) {
                    self.ease_toward(Some(content), target)
                } else {
                    self.eval_tween(Some(content), target)
                }
            });
        right_panel_content_width(target, self.active_tween_endpoints(mask), moving)
    }

    /// Record this frame's window width. A change is direct manipulation:
    /// it drops width tweens and keeps the open columns equal, applied
    /// directly so nothing lags the window edge.
    pub(super) fn observe_viewport_width(&mut self, viewport: f32, cx: &App) {
        self.viewport_resized = (self.viewport_width - viewport).abs() > 1.0;
        if self.viewport_resized {
            self.clear_panel_transitions();
        }
        self.viewport_width = viewport;
        // Re-split directly whenever the window changed since the last
        // split, sub-pixel steps included. Only a shown surface host outside
        // takeover takes a share, so a chat switch, a visit to Settings or a
        // relaunch keeps a dragged divider; panel toggles re-split on their
        // own (see `Self::resplit_panels`).
        let last = *self.split_viewport.get_or_insert(viewport);
        let (fit, _) = self.settled_fit();
        if matches!(self.route, Route::Chat)
            && fit.right
            && !self.right_pane_expanded
            && (viewport - last).abs() > 0.01
        {
            self.split_panel_widths(cx);
            self.split_viewport = Some(viewport);
        }
    }
}
