//! Native GitHub tabs, composed from Zeron's surface chrome, markdown and Changes.
use crate::{
    changes::Changes, icons, markdown, popover, state::AppState, surface_chrome, theme::Theme,
};

#[cfg(feature = "github-fixture")]
mod fixture;
use gpui::{
    Action, AnyElement, Context, Entity, EventEmitter, FocusHandle, Focusable, KeyDownEvent,
    Render, SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::rc::Rc;
use zeron_proto::{CheckoutDiff, GitHubDiff, GitHubPage, GitHubResource, GitHubTarget};
use zeron_rpc::methods;

#[derive(Clone, PartialEq, Deserialize, Action)]
#[action(namespace = github, no_json)]
pub struct OpenGitHub {
    pub url: String,
    pub source_session: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailTab {
    Summary,
    Discussion,
    Checks,
    Files,
}

pub enum GitHubEvent {
    Changed,
    OpenLink(markdown::render::LinkActivation),
}

pub struct GitHubSurface {
    state: Entity<AppState>,
    source_session: String,
    cwd: String,
    device_id: String,
    pub target: GitHubTarget,
    page: Option<GitHubPage>,
    body: Option<markdown::BlockTree>,
    comments: Vec<markdown::BlockTree>,
    tab: DetailTab,
    back: Vec<GitHubTarget>,
    forward: Vec<GitHubTarget>,
    focus: FocusHandle,
    scroll: gpui::ScrollHandle,
    loading: bool,
    unsupported_host: bool,
    error: Option<SharedString>,
    task: Option<Task<()>>,
    changes: Entity<Changes>,
    diff_loaded: bool,
    diff_loading: bool,
    diff_error: Option<SharedString>,
    diff_task: Option<Task<()>>,
    _observe: Subscription,
}

impl EventEmitter<GitHubEvent> for GitHubSurface {}
impl Focusable for GitHubSurface {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus.clone()
    }
}

impl GitHubSurface {
    pub fn new(
        state: Entity<AppState>,
        source_session: String,
        target: GitHubTarget,
        cx: &mut Context<Self>,
    ) -> Self {
        let chat = state
            .read(cx)
            .chats
            .iter()
            .find(|chat| chat.id == source_session);
        let cwd = chat.and_then(|c| c.cwd.clone()).unwrap_or_default();
        let device_id = chat.map(|c| c.device_id.clone()).unwrap_or_default();
        let observe = cx.observe(&state, |this: &mut Self, _, cx| this.ensure_loaded(cx));
        let changes = cx.new(|cx| Changes::for_published(state.clone(), cx));
        Self {
            state,
            source_session,
            cwd,
            device_id,
            target,
            page: None,
            body: None,
            comments: vec![],
            tab: DetailTab::Summary,
            back: vec![],
            forward: vec![],
            focus: cx.focus_handle(),
            scroll: gpui::ScrollHandle::new(),
            loading: false,
            unsupported_host: false,
            error: None,
            task: None,
            changes,
            diff_loaded: false,
            diff_loading: false,
            diff_error: None,
            diff_task: None,
            _observe: observe,
        }
    }

    pub fn title(&self) -> SharedString {
        if self.target.resolves_checkout() {
            return "GitHub".into();
        }
        match &self.target.resource {
            GitHubResource::PullRequest(n) => format!("PR #{n}").into(),
            GitHubResource::Issue(n) => format!("Issue #{n}").into(),
            GitHubResource::Commit(sha) => format!("Commit {}", &sha[..sha.len().min(7)]).into(),
            GitHubResource::Compare(_) => "Compare".into(),
            GitHubResource::Page(url) => url::Url::parse(url)
                .ok()
                .and_then(|url| {
                    url.path_segments()
                        .and_then(|mut segments| segments.rfind(|s| !s.is_empty()))
                        .map(str::to_string)
                })
                .unwrap_or_else(|| "GitHub".into())
                .into(),
            _ => self.target.repository_name().into(),
        }
    }
    pub fn loading(&self) -> bool {
        self.loading || self.diff_loading
    }
    pub fn source_session(&self) -> &str {
        &self.source_session
    }

    fn params(&self, cx: &gpui::App) -> serde_json::Value {
        let url = if self.target.resolves_checkout() {
            String::new()
        } else {
            self.target.url()
        };
        let mut params = serde_json::json!({"cwd": self.cwd, "url": url});
        if self.state.read(cx).local_device_id.as_deref() != Some(&self.device_id) {
            params["targetDeviceId"] = self.device_id.clone().into();
        }
        params
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        if self.page.is_none()
            && self.error.is_none()
            && !self.loading
            && (!self.unsupported_host || self.host_supports(cx))
        {
            self.refresh(cx);
        }
    }

    fn host_supports(&self, cx: &gpui::App) -> bool {
        self.state
            .read(cx)
            .device_supports(&self.device_id, zeron_proto::capabilities::GITHUB_VIEWER_V1)
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        if !self.host_supports(cx) {
            self.unsupported_host = true;
            self.page = None;
            self.error = None;
            self.diff_task = None;
            self.diff_loading = false;
            cx.emit(GitHubEvent::Changed);
            cx.notify();
            return;
        }
        self.unsupported_host = false;
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if self.cwd.is_empty() {
            self.error = Some("Open GitHub from a session with a workspace folder.".into());
            cx.notify();
            return;
        }
        let params = self.params(cx);
        let target = self.target.clone();
        self.loading = true;
        self.error = None;
        self.diff_loading = false;
        self.diff_task = None;
        self.diff_error = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::GET_GITHUB_PAGE, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    serde_json::from_value::<GitHubPage>(value).map_err(|e| e.to_string())
                });
            this.update(cx, |view, cx| {
                if view.target != target {
                    return;
                }
                view.loading = false;
                match result {
                    Ok(page) if page.target == target || target.resolves_checkout() => {
                        view.target = page.target.clone();
                        view.diff_loaded = false;
                        view.body = Some(markdown::parse_full(&page.body));
                        view.comments = page
                            .comments
                            .iter()
                            .map(|c| markdown::parse_full(&c.body))
                            .collect();
                        view.page = Some(page);
                        if view.tab == DetailTab::Files {
                            view.ensure_diff(cx);
                        }
                    }
                    Ok(_) => {
                        view.error =
                            Some("GitHub returned a different page. Try refreshing.".into())
                    }
                    Err(error) => view.error = Some(friendly_error(&error).into()),
                }
                cx.emit(GitHubEvent::Changed);
                cx.notify();
            })
            .ok();
        }));
        cx.emit(GitHubEvent::Changed);
        cx.notify();
    }

    fn ensure_diff(&mut self, cx: &mut Context<Self>) {
        if self.diff_loaded || self.diff_loading || self.loading {
            return;
        }
        if !self.host_supports(cx) {
            self.refresh(cx);
            return;
        }
        let Some(page) = self.page.clone() else {
            return;
        };
        if !self.target.has_diff() {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let params = self.params(cx);
        let target = self.target.clone();
        self.diff_loading = true;
        self.diff_error = None;
        self.diff_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::GET_GITHUB_DIFF, params)
                .await
                .map_err(|e| e.to_string())
                .and_then(|value| {
                    serde_json::from_value::<GitHubDiff>(value).map_err(|e| e.to_string())
                });
            let diff = result.map(|diff| CheckoutDiff {
                checkout_id: target.url(),
                device_id: String::new(),
                cwd: String::new(),
                checksum: format!("{:x}", Sha256::digest(diff.patch.as_bytes())),
                patch: diff.patch,
                files: vec![],
                additions: page.additions,
                deletions: page.deletions,
                truncated: diff.truncated,
                updated_at: chrono::Utc::now(),
            });
            this.update(cx, |view, cx| {
                if view.target != target {
                    return;
                }
                view.diff_loading = false;
                match diff {
                    Ok(diff) => {
                        view.changes
                            .update(cx, |changes, cx| changes.set_published_diff(diff, cx));
                        view.diff_loaded = true;
                    }
                    Err(error) => view.diff_error = Some(friendly_error(&error).into()),
                }
                cx.emit(GitHubEvent::Changed);
                cx.notify();
            })
            .ok();
        }));
    }

    fn navigate(&mut self, target: GitHubTarget, cx: &mut Context<Self>) {
        if target == self.target {
            return;
        }
        self.back.push(self.target.clone());
        self.forward.clear();
        self.load_target(target, cx);
    }
    fn load_target(&mut self, target: GitHubTarget, cx: &mut Context<Self>) {
        self.task = None;
        self.diff_task = None;
        self.target = target;
        self.page = None;
        self.body = None;
        self.comments.clear();
        self.loading = false;
        self.unsupported_host = false;
        self.error = None;
        self.diff_loading = false;
        self.diff_loaded = false;
        self.diff_error = None;
        self.tab = DetailTab::Summary;
        self.scroll = gpui::ScrollHandle::new();
        self.changes = cx.new(|cx| Changes::for_published(self.state.clone(), cx));
        self.refresh(cx);
        cx.emit(GitHubEvent::Changed);
        cx.notify();
    }
    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(target) = self.back.pop() {
            self.forward.push(self.target.clone());
            self.load_target(target, cx);
        }
    }
    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(target) = self.forward.pop() {
            self.back.push(self.target.clone());
            self.load_target(target, cx);
        }
    }
    fn select_tab(&mut self, tab: DetailTab, cx: &mut Context<Self>) {
        self.tab = tab;
        self.scroll = gpui::ScrollHandle::new();
        if tab == DetailTab::Files {
            self.ensure_diff(cx);
        }
        cx.notify();
    }
    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if (key.modifiers.control || key.modifiers.platform) && key.key == "r" {
            self.refresh(cx);
            cx.stop_propagation();
        } else if key.modifiers.alt && key.key == "left" {
            self.go_back(cx);
            cx.stop_propagation();
        } else if key.modifiers.alt && key.key == "right" {
            self.go_forward(cx);
            cx.stop_propagation();
        } else if !key.modifiers.control && !key.modifiers.platform && !key.modifiers.alt {
            let tabs = self.detail_tabs();
            if let Some(index) = tabs.iter().position(|(tab, _)| *tab == self.tab) {
                let next = match key.key.as_str() {
                    "left" => Some((index + tabs.len() - 1) % tabs.len()),
                    "right" => Some((index + 1) % tabs.len()),
                    _ => None,
                };
                if let Some(next) = next {
                    self.select_tab(tabs[next].0, cx);
                    cx.stop_propagation();
                }
            }
        }
    }

    fn detail_tabs(&self) -> Vec<(DetailTab, &'static str)> {
        match self.target.resource {
            GitHubResource::PullRequest(_) => vec![
                (DetailTab::Summary, "Summary"),
                (DetailTab::Discussion, "Discussion"),
                (DetailTab::Checks, "Checks"),
                (DetailTab::Files, "Files"),
            ],
            GitHubResource::Issue(_) => vec![
                (DetailTab::Summary, "Summary"),
                (DetailTab::Discussion, "Discussion"),
            ],
            GitHubResource::Commit(_) => vec![
                (DetailTab::Summary, "Summary"),
                (DetailTab::Discussion, "Discussion"),
                (DetailTab::Checks, "Checks"),
                (DetailTab::Files, "Files"),
            ],
            GitHubResource::Compare(_) => {
                vec![(DetailTab::Summary, "Summary"), (DetailTab::Files, "Files")]
            }
            GitHubResource::Page(_) => {
                let mut tabs = vec![(DetailTab::Summary, "Summary")];
                if self
                    .page
                    .as_ref()
                    .is_some_and(|page| !page.checks.is_empty())
                {
                    tabs.push((DetailTab::Checks, "Checks"));
                }
                tabs
            }
            _ => vec![],
        }
    }

    fn markdown_options(&self, key: String, cx: &Context<Self>) -> markdown::render::RenderOptions {
        let mut options = markdown::render::RenderOptions::settled(
            format!("github-{}-{key}", cx.entity_id().as_u64()).into(),
        );
        let weak = cx.entity().downgrade();
        options.link = Some(markdown::render::LinkUi {
            source_session: Some(self.source_session.clone()),
            source_local: false,
            file_roots: None,
            handler: Rc::new(move |activation, _, cx| {
                use markdown::render::{LinkAction, LinkOutcome};
                let Ok(url) = &activation.target.navigation else {
                    return LinkOutcome::Rejected;
                };
                if activation.action == LinkAction::External {
                    return LinkOutcome::External(url.clone());
                }
                let Some(view) = weak.upgrade() else {
                    return LinkOutcome::Rejected;
                };
                view.update(cx, |view, cx| {
                    if let Some(target) = GitHubTarget::from_url(url) {
                        view.navigate(target, cx);
                    } else {
                        cx.emit(GitHubEvent::OpenLink(activation.clone()));
                    }
                });
                LinkOutcome::Internal
            }),
        });
        options
    }

    fn toolbar(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let url = self.target.url();
        let can_refresh = !self.loading && self.state.read(cx).engine().is_some();
        let back = crate::files::toolbar_button("github-back", "Back (Alt+Left)")
            .child(
                icons::icon(icons::ARROW_LEFT)
                    .size(px(surface_chrome::ICON_SIZE))
                    .text_color(theme.text_muted),
            )
            .when(self.back.is_empty(), |el| el.opacity(0.35).cursor_default())
            .when(!self.back.is_empty(), |el| {
                el.on_click(cx.listener(|this, _, _, cx| this.go_back(cx)))
            });
        let forward = crate::files::toolbar_button("github-forward", "Forward (Alt+Right)")
            .child(
                icons::icon(icons::ARROW_RIGHT)
                    .size(px(surface_chrome::ICON_SIZE))
                    .text_color(theme.text_muted),
            )
            .when(self.forward.is_empty(), |el| {
                el.opacity(0.35).cursor_default()
            })
            .when(!self.forward.is_empty(), |el| {
                el.on_click(cx.listener(|this, _, _, cx| this.go_forward(cx)))
            });
        surface_chrome::toolbar(theme)
            .child(back)
            .child(forward)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .child(if self.target.resolves_checkout() {
                        "GitHub".to_string()
                    } else {
                        self.target.repository_name()
                    }),
            )
            .child(
                crate::files::toolbar_button("github-refresh", "Refresh (Ctrl/Cmd+R)")
                    .when(!can_refresh, |el| el.opacity(0.35).cursor_default())
                    .when(can_refresh, |el| {
                        el.on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
                    })
                    .child(
                        icons::icon(icons::REFRESH)
                            .size(px(surface_chrome::ICON_SIZE))
                            .text_color(theme.text_muted),
                    ),
            )
            .child(
                popover::btn_ghost(theme, "Open in browser", "github-external")
                    .id("github-external")
                    .role(gpui::Role::Button)
                    .aria_label("Open this GitHub page in your browser")
                    .when(self.target.resolves_checkout(), |el| {
                        el.opacity(0.35).cursor_default()
                    })
                    .when(!self.target.resolves_checkout(), |el| {
                        el.on_click(move |_, _, cx| cx.open_url(&url))
                    }),
            )
            .into_any_element()
    }

    fn render_page(
        &mut self,
        page: &GitHubPage,
        window: &Window,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_detail = !self.detail_tabs().is_empty();
        let heading = div()
            .flex_none()
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(16.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(page.title.clone()),
            )
            .when(!page.state.is_empty() || !page.author.is_empty(), |el| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .when(!page.state.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(state_color(&page.state, theme))
                                    .child(state_label(&page.state, page.draft)),
                            )
                        })
                        .when(!page.author.is_empty(), |el| {
                            el.child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(theme.text_muted)
                                    .child(format!("by {}", page.author)),
                            )
                        }),
                )
            })
            .when(!page.head_ref.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(px(11.0))
                        .font_family(theme.font_mono.clone())
                        .text_color(theme.text_muted)
                        .child(format!("{} → {}", page.head_ref, page.base_ref)),
                )
            });
        let tabs = if is_detail {
            div()
                .flex_none()
                .flex()
                .flex_wrap()
                .items_center()
                .px(px(8.0))
                .border_b_1()
                .border_color(theme.border)
                .children(self.detail_tabs().into_iter().map(|(tab, label)| {
                    let selected = self.tab == tab;
                    let label = match tab {
                        DetailTab::Files => format!("Files ({})", page.changed_files),
                        DetailTab::Checks => format!("Checks ({})", page.checks.len()),
                        DetailTab::Discussion => format!("Discussion ({})", page.comments.len()),
                        _ => label.to_string(),
                    };
                    popover::btn_ghost(theme, &label, format!("github-tab-{tab:?}"))
                        .id(SharedString::from(format!("github-tab-{tab:?}")))
                        .role(gpui::Role::Button)
                        .aria_label(label)
                        .when(selected, |el| el.text_color(theme.text).bg(theme.ink(0.06)))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            window.focus(&this.focus, cx);
                            this.select_tab(tab, cx);
                        }))
                        .into_any_element()
                }))
                .into_any_element()
        } else {
            div()
                .flex_none()
                .flex()
                .px(px(8.0))
                .border_b_1()
                .border_color(theme.border)
                .children(
                    [
                        (GitHubResource::PullRequests, "Pull requests"),
                        (GitHubResource::Issues, "Issues"),
                        (
                            GitHubResource::Page(format!(
                                "https://github.com/{}/commits",
                                self.target.repository_name()
                            )),
                            "Commits",
                        ),
                    ]
                    .into_iter()
                    .map(|(resource, label)| {
                        let selected = self.target.resource == resource
                            || (resource == GitHubResource::PullRequests
                                && self.target.resource == GitHubResource::Repository);
                        let mut target = self.target.clone();
                        target.resource = resource;
                        popover::btn_ghost(theme, label, format!("github-list-{label}"))
                            .id(SharedString::from(format!("github-list-{label}")))
                            .role(gpui::Role::Button)
                            .aria_label(label)
                            .when(selected, |el| el.text_color(theme.text).bg(theme.ink(0.06)))
                            .on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.navigate(target.clone(), cx)
                                }),
                            )
                            .into_any_element()
                    }),
                )
                .into_any_element()
        };
        let content = if self.tab == DetailTab::Files {
            if let Some(error) = self.diff_error.clone() {
                self.notice(&error, "Retry loading files", true, theme, cx)
            } else if !self.diff_loaded {
                skeleton(theme, "Loading published changes…")
            } else {
                let controls = self
                    .changes
                    .update(cx, |changes, cx| changes.render_header_controls(cx));
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(surface_chrome::toolbar(theme).child(controls))
                    .child(div().flex_1().min_h_0().child(self.changes.clone()))
                    .into_any_element()
            }
        } else {
            let mut body = div()
                .id("github-content-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .track_scroll(&self.scroll)
                .p(px(16.0))
                .flex()
                .flex_col()
                .gap(px(16.0));
            match self.tab {
                DetailTab::Summary => {
                    if let Some(tree) = &self.body {
                        if page.body.trim().is_empty() && is_detail {
                            body = body.child(quiet(theme, "No description provided."));
                        } else {
                            body = body.child(markdown::render::render_tree(
                                tree,
                                &self.markdown_options("github-description".into(), cx),
                                theme,
                                window,
                                &|_| None,
                            ));
                        }
                    }
                    if !is_detail {
                        body = body.child(quiet(
                            theme,
                            if self.target.resource == GitHubResource::Issues {
                                "Showing up to 50 issues"
                            } else {
                                "Showing up to 50 pull requests"
                            },
                        ));
                        if page.items.is_empty() {
                            body = body
                                .child(quiet(theme, "There are no items in this repository yet."));
                        }
                        body = body.children(page.items.iter().enumerate().map(|(index, item)| {
                            let target = GitHubTarget::from_url(&item.url);
                            div()
                                .id(("github-item", index))
                                .role(gpui::Role::Button)
                                .w_full()
                                .py(px(10.0))
                                .border_b_1()
                                .border_color(theme.border)
                                .cursor_pointer()
                                .hover(|style| style.bg(theme.ink(0.04)))
                                .aria_label(format!("Open #{} {}", item.number, item.title))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(target) = &target {
                                        this.navigate(target.clone(), cx);
                                    }
                                }))
                                .child(
                                    div()
                                        .text_size(px(13.0))
                                        .text_color(theme.text)
                                        .child(format!("#{}  {}", item.number, item.title)),
                                )
                                .child(
                                    div()
                                        .mt(px(4.0))
                                        .text_size(px(11.0))
                                        .text_color(theme.text_muted)
                                        .child(format!(
                                            "{} · {}",
                                            state_label(&item.state, item.draft),
                                            item.author
                                        )),
                                )
                        }));
                    } else if page.target.has_diff() {
                        body = body.child(
                            div()
                                .text_size(px(12.0))
                                .text_color(theme.text_muted)
                                .child(format!(
                                    "{} files changed · +{} / −{}",
                                    page.changed_files, page.additions, page.deletions
                                )),
                        );
                    }
                }
                DetailTab::Discussion => {
                    if page.comments.is_empty() {
                        body = body.child(quiet(theme, "No comments or reviews yet."));
                    }
                    for (index, comment) in page.comments.iter().enumerate() {
                        let header = format!(
                            "{}{} · {}",
                            comment.author,
                            if comment.state.is_empty() {
                                String::new()
                            } else {
                                format!(" · {}", humanize(&comment.state))
                            },
                            comment.created_at.get(..10).unwrap_or(&comment.created_at)
                        );
                        let mut row = div()
                            .py(px(8.0))
                            .border_b_1()
                            .border_color(theme.border)
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .text_color(theme.text_muted)
                                    .child(header),
                            );
                        if let Some(tree) = self.comments.get(index) {
                            row = row.child(markdown::render::render_tree(
                                tree,
                                &self.markdown_options(format!("github-comment-{index}"), cx),
                                theme,
                                window,
                                &|_| None,
                            ));
                        }
                        body = body.child(row);
                    }
                }
                DetailTab::Checks => {
                    if page.checks.is_empty() {
                        body =
                            body.child(quiet(theme, "No checks reported for this pull request."));
                    }
                    body = body.children(page.checks.iter().enumerate().map(|(index, check)| {
                        let url = check.url.clone();
                        div()
                            .id(("github-check", index))
                            .py(px(10.0))
                            .border_b_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(12.0))
                                    .text_color(theme.text)
                                    .child(check.name.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(check_color(&check.status, theme))
                                    .child(humanize(&check.status)),
                            )
                            .when(!url.is_empty(), |el| {
                                el.child(
                                    popover::btn_ghost(
                                        theme,
                                        "Details",
                                        format!("github-check-{index}"),
                                    )
                                    .id(("github-check-details", index))
                                    .role(gpui::Role::Button)
                                    .aria_label(format!("Open details for {}", check.name))
                                    .on_click(cx.listener(
                                        move |this, _, _, cx| {
                                            cx.emit(GitHubEvent::OpenLink(
                                                markdown::render::LinkActivation {
                                                    target: markdown::render::LinkTarget::new(
                                                        "Check details",
                                                        &url,
                                                    ),
                                                    action: markdown::render::LinkAction::Primary,
                                                    source_session: Some(
                                                        this.source_session.clone(),
                                                    ),
                                                },
                                            ));
                                        },
                                    )),
                                )
                            })
                    }));
                }
                DetailTab::Files => unreachable!(),
            }
            body.into_any_element()
        };
        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .child(heading)
            .child(tabs)
            .child(content)
            .into_any_element()
    }

    fn notice(
        &self,
        message: &str,
        retry: &str,
        diff: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .p(px(16.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .text_size(px(12.0))
            .text_color(theme.text_muted)
            .child(message.to_string())
            .child(
                popover::btn_ghost(theme, retry, "github-retry")
                    .id("github-retry")
                    .role(gpui::Role::Button)
                    .aria_label(retry)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if diff {
                            this.ensure_diff(cx);
                        } else {
                            this.refresh(cx);
                        }
                    })),
            )
            .into_any_element()
    }
}

impl Render for GitHubSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.ensure_loaded(cx);
        let theme = Theme::of(cx).clone();
        let mut root = div()
            .id("github-viewer")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .font_family(theme.font_sans_fixed.clone())
            .track_focus(&self.focus)
            .key_context("GitHub")
            .on_key_down(cx.listener(Self::key_down))
            .child(markdown::render::selection_frame_reset_for(
                cx.entity_id().as_u64(),
            ))
            .child(self.toolbar(&theme, cx));
        if self.unsupported_host {
            root = root.child(
                div()
                    .p(px(16.0))
                    .child(quiet(&theme, "This session's device doesn't support the GitHub viewer yet. Use Open in browser, or update Zeron on that device.")),
            );
        } else if let Some(error) = self.error.clone() {
            root = root.child(self.notice(&error, "Retry", false, &theme, cx));
        }
        if let Some(page) = self.page.clone() {
            root = root.child(self.render_page(&page, window, &theme, cx));
        } else if self.loading {
            root = root.child(skeleton(&theme, "Loading GitHub…"));
        }
        root
    }
}

fn quiet(theme: &Theme, text: &str) -> gpui::Div {
    div()
        .text_size(px(12.0))
        .text_color(theme.text_muted)
        .child(text.to_string())
}
fn skeleton(theme: &Theme, label: &str) -> AnyElement {
    div()
        .id("github-loading")
        .flex_1()
        .p(px(16.0))
        .flex()
        .flex_col()
        .gap(px(12.0))
        .aria_label(label)
        .child(quiet(theme, label))
        .children([0.85, 0.6, 0.75].into_iter().map(|width| {
            div()
                .w(gpui::relative(width))
                .h(px(12.0))
                .rounded(px(4.0))
                .bg(theme.ink(0.06))
        }))
        .into_any_element()
}
fn humanize(value: &str) -> String {
    let lower = value.replace('_', " ").to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Unknown".into(),
    }
}
fn state_label(state: &str, draft: bool) -> String {
    if draft && state == "OPEN" {
        "Draft".into()
    } else {
        humanize(state)
    }
}
fn state_color(state: &str, theme: &Theme) -> gpui::Hsla {
    match state {
        "OPEN" => theme.success,
        "MERGED" => theme.code_text,
        "CLOSED" => theme.danger,
        _ => theme.text_muted,
    }
}
fn check_color(status: &str, theme: &Theme) -> gpui::Hsla {
    match status {
        "SUCCESS" => theme.success,
        "FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" => theme.danger,
        "PENDING" | "QUEUED" | "IN_PROGRESS" | "WAITING" => theme.warning,
        _ => theme.text_muted,
    }
}
fn friendly_error(error: &str) -> String {
    if error.contains("unknown method") {
        "Update Zeron on this session's device to use the GitHub viewer.".into()
    } else if error.contains("not installed") {
        "Install GitHub CLI on this session's device, then retry.".into()
    } else if error.contains("not authenticated") {
        "Sign in with gh auth login on this session's device, then retry.".into()
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn older_hosts_keep_links_native_without_sending_unsupported_rpcs(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, mut requests) = tokio::sync::mpsc::channel(16);
        let (_replies, inbound) = tokio::sync::mpsc::channel(16);
        let engine =
            crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound));
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.local_device_id = Some("local".into());
            state.set_test_engine(engine);
            state.chats = vec![
                serde_json::from_value(serde_json::json!({
                    "id":"source", "deviceId":"remote", "cwd":"/remote/workspace",
                    "archived":false, "createdAt":"2026-01-01T00:00:00Z"
                }))
                .unwrap(),
            ];
            state.devices = vec![
                serde_json::from_value(serde_json::json!({
                    "id":"remote", "name":"Remote", "platform":"linux", "lastSeenAt":null
                }))
                .unwrap(),
            ];
            state
        });
        let target = GitHubTarget::from_url("https://github.com/acme/repo/commit/42926c8").unwrap();
        let view =
            cx.new(|cx| GitHubSurface::new(state.clone(), "source".into(), target.clone(), cx));
        view.update(cx, |view, cx| {
            view.ensure_loaded(cx);
            view.ensure_diff(cx);
            assert!(view.unsupported_host);
            assert_eq!(view.target, target);
            assert!(view.error.is_none());
            assert!(!view.loading());
        });
        assert!(requests.try_recv().is_err());
        state.update(cx, |state, _| {
            state.devices[0].capabilities =
                vec![zeron_proto::capabilities::GITHUB_VIEWER_V1.into()];
        });
        view.update(cx, |view, cx| assert!(view.host_supports(cx)));
    }

    #[gpui::test]
    fn github_reads_stay_on_the_source_sessions_host_after_selection_changes(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), directory.path(), cx);
            cx.set_global(Theme::default());
        });
        let state = cx.new(|_| {
            let mut state = AppState::new();
            state.local_device_id = Some("local".into());
            state.chats = vec![serde_json::from_value(serde_json::json!({
                "id":"source", "deviceId":"remote", "cwd":"/remote/workspace", "archived":false, "createdAt":"2026-10-09T00:00:00Z"
            })).unwrap()]; state
        });
        let view = cx.new(|cx| {
            GitHubSurface::new(
                state.clone(),
                "source".into(),
                GitHubTarget::from_url("https://github.com/acme/repo/pull/1").unwrap(),
                cx,
            )
        });
        state.update(cx, |state, _| {
            state.selected_chat = Some("somewhere-else".into())
        });
        view.update(cx, |view, cx| {
            let params = view.params(cx);
            assert_eq!(params["cwd"], "/remote/workspace");
            assert_eq!(params["targetDeviceId"], "remote");
            assert_eq!(params["url"], "https://github.com/acme/repo/pull/1");
        });
    }
}
