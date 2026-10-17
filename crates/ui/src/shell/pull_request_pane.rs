//! The Pull requests route's side pane: opened pull requests as tabs beside
//! the board, so the list stays in view while one is inspected.

use super::*;
use crate::pull_request_detail::PullRequestDetailPage;
use crate::settings::PULL_REQUEST_PANE_DEFAULT;

/// Drag marker for the pull request pane's resize handle.
pub(super) struct PullRequestPaneResize;

/// Width the board keeps beside a hand-sized pane. Below this plus the
/// pane's own minimum the pane covers the board instead of squeezing both.
pub(super) const PULL_REQUEST_BOARD_MIN: f32 = 360.0;
/// Opening more closes the least recently viewed tab.
pub(super) const PULL_REQUEST_TABS_MAX: usize = 8;

pub(super) struct PullRequestTab {
    pub(super) url: String,
    /// The device whose engine reads it; `None` for this device.
    pub(super) device: Option<String>,
    pub(super) page: Entity<PullRequestDetailPage>,
    /// Activation stamp; the smallest is the least recently viewed.
    viewed: u64,
    _subscription: Subscription,
}

/// Drag payload for reordering the pane's tabs.
pub(super) struct PullRequestTabDrag {
    from: usize,
    title: SharedString,
}

#[derive(Clone, Copy)]
pub(super) struct PullRequestTabMenu {
    ix: usize,
    position: Point<Pixels>,
}

/// The tabs a context-menu action closes, as indices into `len` tabs.
fn tabs_to_close(len: usize, index: usize, action: TabCloseAction) -> Vec<usize> {
    (0..len)
        .filter(|&i| match action {
            TabCloseAction::This => i == index,
            TabCloseAction::Others => i != index,
            TabCloseAction::Left => i < index,
            TabCloseAction::Right => i > index,
        })
        .collect()
}

/// Session-local view state; tabs are not persisted across launches.
#[derive(Default)]
pub(super) struct PullRequestPane {
    pub(super) tabs: Vec<PullRequestTab>,
    pub(super) active: Option<String>,
    pub(super) open: bool,
    /// Takeover: the pane covers the board, as the detail did before tabs.
    pub(super) expanded: bool,
    pub(super) tween: Option<WidthTween>,
    pub(super) edge_bounce: Option<motion::ResizeEdgeBounce>,
    pub(super) resize_edge: Option<motion::ResizeEdge>,
    pub(super) tab_scroll: gpui::ScrollHandle,
    /// Live drag-over state while a tab is dragged along the strip.
    drag: Option<RightTabDragState>,
    /// A tab's context menu: Close, Close others, Close to the left/right.
    pub(super) menu: popover::Popup<PullRequestTabMenu>,
    /// The last closed tab's view, kept on screen while the pane animates
    /// shut.
    closing: Option<Entity<PullRequestDetailPage>>,
    views: u64,
}

impl PullRequestPane {
    pub(super) fn active_index(&self) -> Option<usize> {
        let active = self.active.as_deref()?;
        self.tabs.iter().position(|tab| tab.url == active)
    }

    pub(super) fn active_page(&self) -> Option<Entity<PullRequestDetailPage>> {
        self.active_index().map(|ix| self.tabs[ix].page.clone())
    }

    fn activate(&mut self, ix: usize) {
        self.views += 1;
        self.tabs[ix].viewed = self.views;
        self.active = Some(self.tabs[ix].url.clone());
    }

    /// Drop tabs past the cap, least recently viewed first, never the
    /// active one.
    fn trim(&mut self) {
        while self.tabs.len() > PULL_REQUEST_TABS_MAX {
            let active = self.active_index();
            let Some(oldest) = (0..self.tabs.len())
                .filter(|ix| Some(*ix) != active)
                .min_by_key(|ix| self.tabs[*ix].viewed)
            else {
                return;
            };
            self.tabs.remove(oldest);
        }
    }

    /// Remove a tab. Closing the active one activates the tab that takes
    /// its place, else the one before it.
    fn remove(&mut self, ix: usize) {
        if ix >= self.tabs.len() {
            return;
        }
        let was_active = self.active_index() == Some(ix);
        self.tabs.remove(ix);
        if !was_active {
            return;
        }
        self.active = None;
        if !self.tabs.is_empty() {
            self.activate(ix.min(self.tabs.len() - 1));
        }
    }
}

/// The pane's width budget: a hand-sized pane leaves the board its minimum;
/// when even that cannot fit, the pane takes the whole content area.
fn pull_request_pane_width(available: f32, preferred: f32, expanded: bool) -> f32 {
    let max = (available - PULL_REQUEST_BOARD_MIN).max(0.0);
    if expanded || max < RIGHT_PANE_MIN {
        available
    } else {
        preferred.clamp(RIGHT_PANE_MIN, max)
    }
}

impl Shell {
    /// The board's side pane shows: on its route, with at least one tab.
    pub(super) fn pull_request_pane_open(&self) -> bool {
        matches!(self.route, Route::PullRequests)
            && self.pull_request_pane.open
            && !self.pull_request_pane.tabs.is_empty()
    }

    fn pull_request_pane_available(&self) -> f32 {
        (self.viewport_width - self.sidebar_now()).max(0.0)
    }

    /// Whether the pane can be resized by hand: not in takeover, and the
    /// window leaves room for the board beside it.
    pub(super) fn pull_request_pane_resizable(&self) -> bool {
        !self.pull_request_pane.expanded
            && self.pull_request_pane_available() - PULL_REQUEST_BOARD_MIN >= RIGHT_PANE_MIN
    }

    pub(super) fn pull_request_pane_target(&self) -> f32 {
        if !self.pull_request_pane_open() {
            return 0.0;
        }
        pull_request_pane_width(
            self.pull_request_pane_available(),
            self.settings.pull_request_pane_width,
            self.pull_request_pane.expanded,
        )
    }

    pub(super) fn pull_request_pane_visible_width(&self) -> f32 {
        if !matches!(self.route, Route::PullRequests) {
            return 0.0;
        }
        let now = self.eval_tween(
            self.pull_request_pane.tween,
            self.pull_request_pane_target(),
        ) + self.eval_resize_edge_bounce(
            self.pull_request_pane.edge_bounce,
            self.pull_request_pane_open() && self.pull_request_pane_resizable(),
        );
        now.max(0.0).min(self.pull_request_pane_available())
    }

    /// Animate the pane to its current target from wherever it is now.
    fn retarget_pull_request_pane(&mut self, from: f32) {
        self.pull_request_pane.edge_bounce = None;
        self.pull_request_pane.resize_edge = None;
        self.finish_pane_resize(PaneResizeKind::PullRequests);
        self.pull_request_pane.tween = Some(WidthTween::new(from, self.pull_request_pane_target()));
    }

    /// Show or hide the pane, keeping its tabs. A no-op when already there.
    pub(super) fn set_pull_request_pane_open(&mut self, open: bool, cx: &mut Context<Self>) {
        let open = open && !self.pull_request_pane.tabs.is_empty();
        if self.pull_request_pane.open == open {
            return;
        }
        let from = self.pull_request_pane_visible_width();
        self.pull_request_pane.open = open;
        if !open {
            // Reopening at full bleed with the board gone reads as broken.
            self.pull_request_pane.expanded = false;
        }
        self.retarget_pull_request_pane(from);
        self.sync_pull_request_selection(cx);
        cx.notify();
    }

    /// The board highlights the pull request shown beside it.
    pub(super) fn sync_pull_request_selection(&mut self, cx: &mut Context<Self>) {
        if self.pull_request_pane.open
            && let Some(ix) = self.pull_request_pane.active_index()
            && let Some(board) = &self.pull_requests_page
        {
            let tab = &self.pull_request_pane.tabs[ix];
            let board = board.read(cx);
            // Only compare revisions observed by the same device. This is a
            // local lookup, not another board fetch on every tab activation.
            if board.target_device() == tab.device
                && let Some(preview) = board.preview(&tab.url)
            {
                tab.page
                    .update(cx, |page, cx| page.update_board_preview(preview, cx));
            }
        }
        let selected = self
            .pull_request_pane
            .open
            .then(|| self.pull_request_pane.active.clone())
            .flatten();
        if let Some(page) = &self.pull_requests_page {
            page.update(cx, |page, cx| page.select_url(selected, cx));
        }
    }

    /// The history entry for the route as it stands: the pull request
    /// shown in the pane, else the board alone.
    pub(super) fn pull_request_pane_entry(&self) -> NavEntry {
        match self.pull_request_pane.active_index() {
            Some(ix) if self.pull_request_pane.open => {
                let tab = &self.pull_request_pane.tabs[ix];
                NavEntry::PullRequest {
                    url: tab.url.clone(),
                    device: tab.device.clone(),
                }
            }
            _ => NavEntry::PullRequests,
        }
    }

    /// The active tab's detail view, if the pane has one.
    pub(super) fn active_pull_request_page(&self) -> Option<Entity<PullRequestDetailPage>> {
        self.pull_request_pane.active_page()
    }

    /// Open `url` as a tab beside the board (or activate its tab) without
    /// recording a navigation. Re-showing an open pull request keeps its
    /// view state.
    pub(super) fn show_pull_request_detail(
        &mut self,
        url: String,
        target: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.pending_pull_request = None;
        if self.pull_requests_page.is_none() {
            self.pull_requests_page =
                Some(cx.new(|cx| PullRequestsPage::new(self.state.clone(), cx)));
        }
        let existing = self
            .pull_request_pane
            .tabs
            .iter()
            .position(|tab| tab.url == url);
        let ix = match existing {
            Some(ix) => ix,
            None => {
                let preview = self
                    .pull_requests_page
                    .as_ref()
                    .and_then(|page| page.read(cx).preview(&url));
                let page = cx.new(|cx| {
                    PullRequestDetailPage::new(
                        self.state.clone(),
                        url.clone(),
                        target.clone(),
                        self.pull_request_cache.clone(),
                        preview,
                        window,
                        cx,
                    )
                });
                let subscription = cx.observe(&page, |_, _, cx| cx.notify());
                let pane = &mut self.pull_request_pane;
                pane.tabs.push(PullRequestTab {
                    url: url.clone(),
                    device: target,
                    page,
                    viewed: 0,
                    _subscription: subscription,
                });
                pane.tabs.len() - 1
            }
        };
        self.pull_request_pane.closing = None;
        self.pull_request_pane.activate(ix);
        self.pull_request_pane.trim();
        self.set_pull_request_pane_open(true, cx);
        self.sync_pull_request_selection(cx);
        cx.notify();
    }

    /// Show a tab, recording it in history so Back returns to the one
    /// viewed before.
    pub(super) fn activate_pull_request_tab(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.pull_request_pane.tabs.get(ix) else {
            return;
        };
        let entry = NavEntry::PullRequest {
            url: tab.url.clone(),
            device: tab.device.clone(),
        };
        if self.nav.current() != &entry {
            self.nav.push(entry);
        }
        // The old page's focused content is about to unmount. Keep keyboard
        // navigation in the pane, as activating a chat surface does.
        self.navigation_focus.right_was_focused = true;
        window.focus(&self.navigation_focus.right, cx);
        self.pull_request_pane.activate(ix);
        self.set_pull_request_pane_open(true, cx);
        self.sync_pull_request_selection(cx);
        cx.notify();
    }

    /// The next or previous tab, wrapping (Ctrl+Tab while the pane has
    /// focus).
    pub(super) fn cycle_pull_request_tabs(
        &mut self,
        forward: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let count = self.pull_request_pane.tabs.len();
        if count <= 1 {
            return;
        }
        let next = match (self.pull_request_pane.active_index(), forward) {
            (Some(at), true) => (at + 1) % count,
            (Some(at), false) => (at + count - 1) % count,
            (None, true) => 0,
            (None, false) => count - 1,
        };
        self.activate_pull_request_tab(next, window, cx);
    }

    /// The ⌘W cascade on this route: close the active tab while the pane
    /// shows. Returns true when the close was consumed.
    pub(super) fn close_active_pull_request_tab(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.pull_request_pane_open() {
            return false;
        }
        let Some(ix) = self.pull_request_pane.active_index() else {
            return false;
        };
        self.close_pull_request_tab(ix, cx);
        true
    }

    /// Move a dragged tab to the slot it was dropped on. The active tab is
    /// tracked by URL, so it stays active wherever it lands.
    fn reorder_pull_request_tabs(&mut self, from: usize, to: usize, cx: &mut Context<Self>) {
        let tabs = &mut self.pull_request_pane.tabs;
        if from < tabs.len() && to < tabs.len() && from != to {
            let tab = tabs.remove(from);
            tabs.insert(to, tab);
            cx.notify();
        }
    }

    /// Track the hovered drop slot mid-drag; epoch bumps restart the slide.
    fn update_pull_request_tab_drag_over(
        &mut self,
        from: usize,
        over: usize,
        cx: &mut Context<Self>,
    ) {
        match &mut self.pull_request_pane.drag {
            Some(drag) if drag.over != over => {
                drag.prev_over = drag.over;
                drag.over = over;
                drag.epoch += 1;
                cx.notify();
            }
            Some(_) => {}
            None => {
                self.pull_request_pane.drag = Some(RightTabDragState {
                    from,
                    over,
                    epoch: 0,
                    prev_over: from,
                });
                cx.notify();
            }
        }
    }

    /// Finish a drag on the slot it was last over.
    fn drop_pull_request_tab(&mut self, payload: &PullRequestTabDrag, cx: &mut Context<Self>) {
        let to = self
            .pull_request_pane
            .drag
            .take()
            .map_or(payload.from, |drag| drag.over);
        self.reorder_pull_request_tabs(payload.from, to, cx);
        cx.notify();
    }

    /// Close several tabs, keeping the active one where it survives.
    fn close_pull_request_tabs(&mut self, mut tabs: Vec<usize>, cx: &mut Context<Self>) {
        // Highest first, so earlier indices stay valid.
        tabs.sort_unstable_by(|a, b| b.cmp(a));
        for ix in tabs {
            self.close_pull_request_tab(ix, cx);
        }
    }

    pub(super) fn close_pull_request_tab_menu(&mut self, cx: &mut Context<Self>) {
        if self.pull_request_pane.menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.pull_request_pane.menu);
            cx.notify();
        }
    }

    /// A tab's context menu, at the pointer.
    pub(super) fn render_pull_request_tab_menu(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let PullRequestTabMenu { ix, position } = *self.pull_request_pane.menu.get()?;
        let closing = self.pull_request_pane.menu.closing_since();
        let theme = Theme::of(cx).clone();
        let len = self.pull_request_pane.tabs.len();
        let mut rows = div().flex().flex_col().gap(px(2.0));
        for (action, label) in [
            (TabCloseAction::This, "Close tab"),
            (TabCloseAction::Others, "Close other tabs"),
            (TabCloseAction::Left, "Close tabs to the left"),
            (TabCloseAction::Right, "Close tabs to the right"),
        ] {
            let enabled = ix < len && !tabs_to_close(len, ix, action).is_empty();
            rows = rows.child(
                popover::menu_row(&theme, false, label)
                    .id(SharedString::from(format!("pull-request-tab-menu-{label}")))
                    .debug_selector(move || format!("pull-request-tab-menu-{label}"))
                    .when(!enabled, |el| el.opacity(0.4).cursor_default())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !enabled {
                            return;
                        }
                        this.close_pull_request_tab_menu(cx);
                        let len = this.pull_request_pane.tabs.len();
                        if ix < len {
                            this.close_pull_request_tabs(tabs_to_close(len, ix, action), cx);
                        }
                    }))
                    .child(label),
            );
        }
        let menu = popover::popover_card(&theme)
            .w(px(216.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_pull_request_tab_menu(cx)))
            .child(rows);
        Some(popover::menu_at(
            "pull-request-tab-menu",
            position,
            menu.into_any_element(),
            closing,
        ))
    }

    /// The titlebar toggle: hide or reveal the pane, keeping its tabs.
    pub(super) fn toggle_pull_request_pane(&mut self, cx: &mut Context<Self>) {
        self.set_pull_request_pane_open(!self.pull_request_pane.open, cx);
    }

    /// Takeover: the pane covers the board, and back to its own width.
    pub(super) fn toggle_pull_request_pane_expand(&mut self, cx: &mut Context<Self>) {
        if !self.pull_request_pane_open() {
            return;
        }
        let from = self.pull_request_pane_visible_width();
        self.pull_request_pane.expanded = !self.pull_request_pane.expanded;
        self.retarget_pull_request_pane(from);
        cx.notify();
    }

    /// Close one tab; closing the last collapses the pane.
    pub(super) fn close_pull_request_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix >= self.pull_request_pane.tabs.len() {
            return;
        }
        if self.pull_request_pane.tabs.len() == 1 {
            self.pull_request_pane.closing = Some(self.pull_request_pane.tabs[ix].page.clone());
            self.set_pull_request_pane_open(false, cx);
        }
        self.pull_request_pane.remove(ix);
        self.sync_pull_request_selection(cx);
        cx.notify();
    }

    /// Return to the board alone without recording a navigation. Tabs stay
    /// for the next time the pane opens.
    pub(super) fn dismiss_pull_request_detail(&mut self, cx: &mut Context<Self>) {
        self.pending_pull_request = None;
        self.set_pull_request_pane_open(false, cx);
        self.sync_pull_request_selection(cx);
        cx.notify();
    }

    /// Forget every tab, e.g. when the browser profile changes.
    pub(super) fn reset_pull_request_pane(&mut self) {
        self.pending_pull_request = None;
        self.pull_request_pane = PullRequestPane::default();
    }

    pub(super) fn on_pull_request_pane_drag(
        &mut self,
        event: &gpui::DragMoveEvent<PullRequestPaneResize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let viewport = f32::from(window.viewport_size().width);
        let width = viewport - f32::from(event.event.position.x);
        let max = (self.pull_request_pane_available() - PULL_REQUEST_BOARD_MIN).max(0.0);
        if max < RIGHT_PANE_MIN {
            return;
        }
        let sample = motion::resize_drag_sample(
            width,
            RIGHT_PANE_MIN,
            max,
            self.pull_request_pane.resize_edge,
            self.reduced_motion,
        );
        self.settings.pull_request_pane_width = sample.width;
        self.pane_resize_dragging = Some(PaneResizeKind::PullRequests);
        if sample.starts_bounce {
            self.pull_request_pane.edge_bounce = sample.edge.map(motion::ResizeEdgeBounce::new);
        } else if sample.edge.is_none() {
            self.pull_request_pane.edge_bounce = None;
        }
        self.pane_resize_active = sample
            .edge
            .is_none()
            .then_some(PaneResizeKind::PullRequests);
        self.pull_request_pane.resize_edge = sample.edge;
        self.pull_request_pane.tween = None;
        self.schedule_save(cx);
        cx.notify();
    }

    /// The seam's resize handle, when the pane can be sized by hand.
    pub(super) fn pull_request_pane_handle(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Stateful<gpui::Div>> {
        (self.pull_request_pane_open()
            && self.pull_request_pane_resizable()
            && !self.tween_active(self.pull_request_pane.tween))
        .then(|| {
            self.resize_handle(
                "pull-request-pane-resize",
                PaneResizeKind::PullRequests,
                || PullRequestPaneResize,
                |shell, _| {
                    shell.settings.pull_request_pane_width = PULL_REQUEST_PANE_DEFAULT;
                    shell.pull_request_pane.edge_bounce = None;
                },
                cx,
            )
            .left(px(-PANE_RESIZE_HITBOX_HALF_WIDTH))
        })
    }

    /// The pane: the active tab's detail view in the right pane's flush
    /// panel, clipped to the animated width.
    pub(super) fn render_pull_request_pane(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let visible = self.pull_request_pane_visible_width();
        if !self.tween_active(self.pull_request_pane.tween) {
            self.pull_request_pane.closing = None;
            if visible <= 0.0 {
                return Empty.into_any_element();
            }
        }
        let theme = Theme::of(cx).clone();
        let target = self.pull_request_pane_target();
        let covers_board = target >= self.pull_request_pane_available() - 0.5;
        let corner = Self::window_corner_radius(window);
        let content = match self
            .active_pull_request_page()
            .or_else(|| self.pull_request_pane.closing.clone())
        {
            // The pull request's title and actions sit in the pane's own
            // toolbar row; its tabs live in the titlebar band above.
            Some(page) => {
                let toolbar = page.update(cx, |page, cx| page.toolbar(cx));
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .child(crate::surface_chrome::toolbar(&theme).child(toolbar))
                    .child(div().flex_1().min_h_0().child(page))
                    .into_any_element()
            }
            None => Empty.into_any_element(),
        };
        let panel = div()
            .id("pull-request-pane")
            .debug_selector(|| "pull-request-pane".into())
            // The chat pane is unmounted on this route; this pane takes its
            // focus boundary, so Ctrl+Tab cycles these tabs.
            .track_focus(&self.navigation_focus.right)
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.capture_navigation_focus(true, false, window, cx);
            }))
            .size_full()
            .flex()
            .flex_col()
            // Covering the board, the pane's left edge is the sidebar seam,
            // which already carries a hairline.
            .when(!covers_board, |el| {
                el.border_l_1().border_color(theme.border)
            })
            .when(corner > 0.0, |el| {
                el.rounded_tr(px(corner)).rounded_br(px(corner))
            })
            .bg(theme.panel_bg())
            .overflow_hidden()
            // The titlebar is a glass overlay over the full-height content
            // row; the pane's own chrome starts below it.
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .child(content);
        let transition = self.active_tween_endpoints(self.pull_request_pane.tween);
        let content_width = stable_panel_content_width(target, transition).max(visible);
        div()
            .h_full()
            .flex_none()
            .relative()
            .overflow_hidden()
            .w(px(visible))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .h_full()
                    .w(px(content_width))
                    .child(panel),
            )
            .into_any_element()
    }
}

impl Shell {
    /// The pane's tabs, drawn in the titlebar band above it like the chat
    /// pane's surface tabs.
    pub(super) fn render_pull_request_tab_strip(&mut self, cx: &mut Context<Self>) -> AnyElement {
        use crate::surface_chrome::{TAB_CHIP_HEIGHT, TAB_CHIP_SLOT, TAB_CHIP_WIDTH};
        let theme = Theme::of(cx).clone();
        // Heal drag state if the pointer was released outside the strip.
        if self.pull_request_pane.drag.is_some() && !cx.has_active_drag() {
            self.pull_request_pane.drag = None;
        }
        let drag = self
            .pull_request_pane
            .drag
            .as_ref()
            .map(|d| (d.from, d.over, d.epoch, d.prev_over));
        let count = self.pull_request_pane.tabs.len();
        let scroll = self.pull_request_pane.tab_scroll.clone();
        let active = self.pull_request_pane.active_index();
        let rows: Vec<_> = self
            .pull_request_pane
            .tabs
            .iter()
            .map(|tab| {
                let page = tab.page.read(cx);
                let (number, title) = page.identity();
                (number, title, page.is_loading())
            })
            .collect();
        let mut strip = div()
            .id("pull-request-tab-strip")
            .track_focus(&self.navigation_focus.tabs)
            .capture_any_mouse_down(cx.listener(|this, _, window, cx| {
                this.capture_navigation_focus(true, true, window, cx);
            }))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(crate::surface_chrome::CONTROL_GAP))
            .min_w_0()
            .overflow_x_scroll()
            .track_scroll(&self.pull_request_pane.tab_scroll)
            // Windows caption hit-testing includes the scroll-only hitboxes
            // behind each chip. Stop at the scroller so the titlebar cannot
            // claim tab clicks, while wheel events still reach this scroller.
            .when(cfg!(target_os = "windows"), |strip| strip.occlude())
            // Drop math runs in content coordinates: viewport-relative x
            // plus the scrolled-off width.
            .on_drag_move::<PullRequestTabDrag>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<PullRequestTabDrag>, _, cx| {
                    let from = event.drag(cx).from;
                    let x = f32::from(event.event.position.x)
                        - f32::from(event.bounds.left())
                        - f32::from(scroll.offset().x);
                    let over = crate::terminal::panel::drop_index(x, TAB_CHIP_SLOT, count);
                    this.update_pull_request_tab_drag_over(from, over, cx);
                },
            ))
            .on_drop::<PullRequestTabDrag>(
                cx.listener(|this, payload, _, cx| this.drop_pull_request_tab(payload, cx)),
            );
        for (ix, (number, title, loading)) in rows.into_iter().enumerate() {
            let is_active = active == Some(ix);
            let label: SharedString = match number {
                Some(number) => format!("#{number} {title}"),
                None => title.clone(),
            }
            .into();
            let leading = if loading {
                loaders::mini_glyph_spinner(
                    format!("pull-request-tab-{ix}"),
                    2.0,
                    theme.glyph,
                    cx.entity_id(),
                    cx,
                )
                .into_any_element()
            } else {
                crate::surface_chrome::tab_chip_icon(icons::PULL_REQUEST, is_active, &theme)
            };
            let tooltip = label.clone();
            let ghost_title: SharedString = title.clone().into();
            let chip = crate::surface_chrome::tab_chip(
                crate::surface_chrome::TabChip {
                    id: "pull-request-tab",
                    close_id: "pull-request-tab-close",
                    ix,
                    active: is_active,
                    title: title.into(),
                    dirty: false,
                    leading,
                },
                &theme,
                cx.listener(move |this, _, _, cx| this.close_pull_request_tab(ix, cx)),
            )
            .aria_label(label)
            .tooltip(move |_, cx| {
                cx.new(|_| SurfaceTabTooltip {
                    text: tooltip.clone(),
                })
                .into()
            })
            .tooltip_show_delay(Duration::from_millis(350))
            .on_click(cx.listener(move |this, _, window, cx| {
                cx.stop_propagation();
                this.activate_pull_request_tab(ix, window, cx);
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.pull_request_pane.menu.open(PullRequestTabMenu {
                        ix,
                        position: event.position,
                    });
                    cx.notify();
                }),
            )
            // Middle-click closes, like every tab strip.
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _, _, cx| this.close_pull_request_tab(ix, cx)),
            )
            .on_drag(
                PullRequestTabDrag {
                    from: ix,
                    title: ghost_title,
                },
                |payload, _point, _, cx| {
                    let title = payload.title.clone();
                    cx.stop_propagation();
                    cx.new(|_| SurfaceTabGhost { title })
                },
            )
            // The chip's hitbox cuts the strip out of the hover stack, so the
            // chip receives the drop too (the chat strip's carve-out).
            .on_drop::<PullRequestTabDrag>(
                cx.listener(|this, payload, _, cx| this.drop_pull_request_tab(payload, cx)),
            );
            // Siblings slide aside while a tab drags over them; the dragged
            // tab leaves a spacer, the ghost carries it.
            let wrapped: AnyElement = match drag {
                Some((from, over, epoch, prev_over)) if ix != from => {
                    let target =
                        crate::terminal::panel::slide_offset(ix, from, over) * TAB_CHIP_SLOT;
                    let start =
                        crate::terminal::panel::slide_offset(ix, from, prev_over) * TAB_CHIP_SLOT;
                    div()
                        .relative()
                        .child(chip.with_animation(
                            (
                                "pull-request-tab-slide",
                                (ix as u64) | ((epoch as u64) << 32),
                            ),
                            TAB_SLIDE.animation(),
                            move |el, t| el.left(px(motion::lerp(start, target, t))),
                        ))
                        .into_any_element()
                }
                Some((from, ..)) if ix == from => div()
                    .w(px(TAB_CHIP_WIDTH))
                    .h(px(TAB_CHIP_HEIGHT))
                    .flex_none()
                    .into_any_element(),
                _ => chip.into_any_element(),
            };
            strip = strip.child(wrapped);
        }
        crate::surface_chrome::tab_strip_region(strip, &self.pull_request_pane.tab_scroll, &theme)
    }

    /// The route's titlebar: the pane's tabs and controls over the pane,
    /// and its toggle while it is hidden.
    pub(super) fn render_pull_request_title_bar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        /// The pane toggle's fixed right-edge slot.
        const TOGGLE_SLOT: f32 = 28.0;
        let theme = Theme::of(cx).clone();
        let plus_inset = TITLEBAR_ACTION_SLOT_WIDTH * self.titlebar_plus_alpha(cx);
        let sidebar_now = self.sidebar_now();
        let right_pad = self.titlebar_right_pad(TITLEBAR_ACTION_EDGE_INSET);
        let open = self.pull_request_pane_open();
        let pane_width = self.pull_request_pane_visible_width();
        let covers_board = open && pane_width >= self.pull_request_pane_available() - 0.5;
        // Covering the board, the tabs start at the pane's own gutter off the
        // sidebar seam, clear of the window-control cluster (the chat pane's
        // takeover inset).
        let row_left = if covers_board {
            let cluster_end =
                self.title_bar_content_start() - TITLEBAR_IDENTITY_GAP + plus_inset - 14.0;
            (sidebar_now - 8.0).max(cluster_end)
        } else {
            (sidebar_now + Theme::SPACE_LG).max(self.title_bar_content_start() + plus_inset)
        };
        let row_gap = 8.0;
        let reveal = if open {
            (pane_width - right_pad - TOGGLE_SLOT)
                .min(self.viewport_width - row_left - right_pad - TOGGLE_SLOT - row_gap)
                .max(0.0)
        } else {
            0.0
        };
        let has_tabs = !self.pull_request_pane.tabs.is_empty();
        let expanded = self.pull_request_pane.expanded;
        let can_expand = expanded || self.pull_request_pane_resizable();
        let mut controls = div()
            .id("pull-request-titlebar-controls")
            .flex_none()
            .h_full()
            .flex()
            .flex_row()
            .items_center()
            // A wheel over the tabs must scroll the strip, never the board.
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());
        if open {
            let tabs = self.render_pull_request_tab_strip(cx);
            controls = controls.child(
                div()
                    .w(px(reveal))
                    .h_full()
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.0))
                    .overflow_hidden()
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
                    .when(can_expand, |el| {
                        el.child(
                            header_icon_button(
                                "expand-pull-request-pane",
                                super::tabs::right_pane_expand_icon(expanded),
                                if expanded {
                                    "Collapse panel"
                                } else {
                                    "Expand panel"
                                },
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_pull_request_pane_expand(cx)
                                }),
                            )
                            .debug_selector(|| "expand-pull-request-pane".into()),
                        )
                    }),
            );
        }
        if has_tabs {
            controls = controls.child(
                header_icon_button_with(
                    "toggle-pull-request-pane",
                    icons::sidebar_glyph(
                        motion::state_t(
                            "toggle-pull-request-pane",
                            open,
                            motion::GLYPH_STATE,
                            self.reduced_motion,
                        ),
                        true,
                        16.0,
                        theme.text_muted,
                    ),
                    ShortcutId::ToggleChanges.label(),
                    cx.listener(|this, _, _, cx| this.toggle_pull_request_pane(cx)),
                )
                .debug_selector(|| "toggle-pull-request-pane".into()),
            );
        }
        let inner = div()
            .size_full()
            .flex()
            .items_center()
            .pt(px(Theme::TITLEBAR_TOP_PAD))
            .pb(px(super::titlebar_bottom_pad()))
            .gap(px(row_gap))
            .pl(px(row_left))
            .pr(px(right_pad))
            .child(div().flex_1())
            .child(controls);
        let bar = div().h(px(Theme::TITLEBAR_HEIGHT)).flex_none().child(inner);
        self.titlebar_drag_region("pull-requests-titlebar", bar, cx)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_menu_actions_pick_their_tabs() {
        assert_eq!(tabs_to_close(4, 1, TabCloseAction::This), [1]);
        assert_eq!(tabs_to_close(4, 1, TabCloseAction::Others), [0, 2, 3]);
        assert_eq!(tabs_to_close(4, 1, TabCloseAction::Left), [0]);
        assert_eq!(tabs_to_close(4, 1, TabCloseAction::Right), [2, 3]);
        assert!(tabs_to_close(1, 0, TabCloseAction::Others).is_empty());
    }

    #[test]
    fn pane_leaves_the_board_its_minimum_or_covers_it() {
        // Room for both: the preferred width, kept clear of the board.
        assert_eq!(pull_request_pane_width(1400.0, 640.0, false), 640.0);
        assert_eq!(
            pull_request_pane_width(1000.0, 900.0, false),
            1000.0 - PULL_REQUEST_BOARD_MIN
        );
        assert_eq!(
            pull_request_pane_width(1400.0, 100.0, false),
            RIGHT_PANE_MIN
        );
        // Too narrow for both: the pane covers the board.
        assert_eq!(pull_request_pane_width(600.0, 640.0, false), 600.0);
        // Takeover always covers it.
        assert_eq!(pull_request_pane_width(1400.0, 640.0, true), 1400.0);
    }
}
