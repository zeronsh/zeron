//! Pull requests opened from a chat — a transcript link or a sidebar badge —
//! as tabs in the focused session's right pane, so several can sit beside
//! one conversation: Zeron's own view, or a Browser tab when Settings asks.

use super::*;
use crate::pull_request_detail::PullRequestDetailPage;

pub(super) struct ChatPullRequest {
    pub(super) url: String,
    pub(super) page: Entity<PullRequestDetailPage>,
    _subscription: Subscription,
}

impl Shell {
    /// Whether a pull request opens in the focused session's pane: on the
    /// chat route, with a session (not the new-session canvas) in focus.
    pub(super) fn chat_pull_requests_available(&self) -> bool {
        matches!(self.route, Route::Chat) && !self.active_chat.is_empty()
    }

    /// Open `url` as a tab in the focused session's pane, or activate the
    /// tab that already shows it there. `device` reads it from another
    /// device's engine; `None` reads it here.
    pub(super) fn open_chat_pull_request(
        &mut self,
        url: String,
        device: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.chat_pull_requests_available() {
            return;
        }
        let key = self.panel_key(cx);
        let existing = self.right_tabs.get(&key).and_then(|tabs| {
            tabs.iter().copied().find(|surface| match surface {
                RightSurface::PullRequest(id) => self
                    .chat_pull_requests
                    .get(id)
                    .is_some_and(|tab| tab.url == url),
                _ => false,
            })
        });
        let surface = match existing {
            Some(surface) => surface,
            None => {
                self.chat_pull_request_seq += 1;
                let id = self.chat_pull_request_seq;
                let page = cx.new(|cx| {
                    PullRequestDetailPage::new(
                        self.state.clone(),
                        url.clone(),
                        device,
                        self.pull_request_cache.clone(),
                        None,
                        window,
                        cx,
                    )
                });
                let subscription = cx.observe(&page, |_, _, cx| cx.notify());
                self.chat_pull_requests.insert(
                    id,
                    ChatPullRequest {
                        url,
                        page,
                        _subscription: subscription,
                    },
                );
                let surface = RightSurface::PullRequest(id);
                self.right_tabs.entry(key).or_default().push(surface);
                surface
            }
        };
        self.set_surfaces_open(true, cx);
        self.set_right_active(surface, cx);
        cx.notify();
    }

    /// Open `url` in a new Browser tab in the focused session's pane. Browser
    /// tabs belong to a session, so without one on screen (the board, the
    /// new-session canvas) the pull request opens in the default browser.
    pub(super) fn browse_chat_pull_request(
        &mut self,
        url: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.chat_pull_requests_available() {
            cx.open_url(&url);
            return;
        }
        self.set_surfaces_open(true, cx);
        self.add_browser_surface(Some(url), window, cx);
    }

    /// The device a chat's pull request is read from: the one named, else
    /// the focused session's; `None` when that is this device.
    pub(super) fn chat_pull_request_device(
        &self,
        device: Option<String>,
        cx: &App,
    ) -> Option<String> {
        let state = self.state.read(cx);
        device
            .or_else(|| state.selected_chat_row().map(|chat| chat.device_id.clone()))
            .filter(|device| Some(device.as_str()) != state.local_device_id.as_deref())
    }

    /// The tab's title and its full name for the tooltip.
    pub(super) fn chat_pull_request_title(
        &self,
        id: u64,
        cx: &App,
    ) -> Option<(SharedString, SharedString)> {
        let page = self.chat_pull_requests.get(&id)?.page.read(cx);
        let (number, title) = page.identity();
        let detail = match number {
            Some(number) => format!("#{number} {title}"),
            None => title.clone(),
        };
        Some((title.into(), detail.into()))
    }

    pub(super) fn chat_pull_request_loading(&self, id: u64, cx: &App) -> bool {
        self.chat_pull_requests
            .get(&id)
            .is_some_and(|tab| tab.page.read(cx).is_loading())
    }

    /// The pane's content for a pull request tab: its toolbar row, then the
    /// detail, like the Diffs surface.
    pub(super) fn render_chat_pull_request(
        &mut self,
        id: u64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let page = self.chat_pull_requests.get(&id)?.page.clone();
        let theme = Theme::of(cx).clone();
        let toolbar = page.update(cx, |page, cx| page.toolbar(cx));
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .child(crate::surface_chrome::toolbar(&theme).child(toolbar))
                .child(div().flex_1().min_h_0().child(page))
                .into_any_element(),
        )
    }
}
