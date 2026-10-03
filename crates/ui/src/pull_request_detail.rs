//! Native PR inspection and explicit comment submission.
#[path = "pull_request_code.rs"]
mod code;
#[path = "pull_request_handoff.rs"]
mod handoff;
#[path = "pull_request_interactions.rs"]
mod interactions;
use crate::{
    settings::{self, PullRequestDestination, widgets},
    state::AppState,
    theme::Theme,
};
use gpui::{
    Action, AnyElement, App, Context, Entity, Focusable, IntoElement, Render, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};
#[cfg(test)]
use std::time::Duration;
use std::{cell::RefCell, rc::Rc, sync::Arc, time::Instant};
use zeron_proto::{ChangeRequestDetail, ChangeRequestListItem};
use zeron_rpc::methods;

#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct OpenPullRequest(pub String, pub Option<String>);

#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct ClosePullRequest;

#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct OpenPrImage(pub String);

/// Open a new session with this prompt staged in the composer (not sent).
#[derive(Clone, PartialEq, Action)]
#[action(namespace = shell, no_json)]
pub struct StartPullRequestSession(pub String);

/// What went wrong and what to do, in words. Error codes and transport
/// details never reach the reader.
fn failure_reason(error: &zeron_rpc::RpcError) -> &'static str {
    use zeron_rpc::{RpcError, capability_errors as codes};
    match error {
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_CLI_UNAVAILABLE => {
            "Install GitHub CLI (gh) on this device and sign in."
        }
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_AUTHENTICATION => {
            "Run gh auth login on this device, then refresh."
        }
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_RATE_LIMITED => {
            "GitHub’s rate limit was reached. Try again in 15 minutes."
        }
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_TIMEOUT => {
            "GitHub didn’t respond in time. Try again."
        }
        RpcError::Capability(code) if code == codes::PULL_REQUESTS_DECODE => {
            "GitHub’s response was too large or unreadable. Open it on GitHub instead."
        }
        RpcError::UnknownMethod(_) => "Update Zeron on the selected device.",
        _ => "Check the link and your connection, then try again.",
    }
}

pub fn open(url: &str, window: &mut Window, cx: &mut App) {
    open_on_device(url, None, window, cx);
}

pub fn open_on_device(url: &str, device: Option<String>, window: &mut Window, cx: &mut App) {
    // The native view reads github.com through `gh`; pull requests on other
    // hosts open in the browser.
    let on_github = url::Url::parse(url).is_ok_and(|url| url.host_str() == Some("github.com"));
    if !on_github
        || settings::current(cx).pull_request_destination == PullRequestDestination::External
    {
        cx.open_url(url);
    } else {
        window.dispatch_action(Box::new(OpenPullRequest(url.to_owned(), device)), cx);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Summary,
    Code,
    Activity,
}

/// Navigation segments in display order: (tab, label, element id, glyph).
const TABS: [(Tab, &str, &str, &str); 3] = [
    (
        Tab::Summary,
        "Summary",
        "pr-summary",
        crate::icons::DOCUMENT,
    ),
    (Tab::Code, "Code", "pr-code", crate::icons::FILE_CODE),
    (
        Tab::Activity,
        "Activity",
        "pr-activity",
        crate::icons::CHAT_ROUND_LINE,
    ),
];

/// Slot of `tab` in the navigation pill, in segments from the leading edge.
fn tab_slot(tab: Tab) -> f32 {
    TABS.iter()
        .position(|(candidate, ..)| *candidate == tab)
        .unwrap_or(0) as f32
}

const NAV_PADDING: f32 = 4.0;
const NAV_SEGMENT_HEIGHT: f32 = 36.0;
/// Room above the window edge for the floating section navigation: its 16px
/// inset, its height, and 16px of air. The diff and the comment dock end here.
const NAV_CLEARANCE: f32 = 16.0 + NAV_SEGMENT_HEIGHT + 2.0 * NAV_PADDING + 16.0;

/// What a patch row stands for in the review stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowRole {
    File,
    Notice,
    Hunk,
    Line,
}

#[derive(Clone)]
struct CodeRow {
    role: RowRole,
    text: SharedString,
    old: String,
    new: String,
    kind: crate::changes::LineKind,
    spans: Vec<zeron_syntax::HighlightSpan>,
}

fn code_rows(patch: &str) -> (Vec<CodeRow>, Vec<(String, usize)>) {
    use crate::changes::LineKind;
    let mut rows = Vec::new();
    let mut files = Vec::new();
    let mut parsed = crate::changes::parse_patch(patch);
    // Directory order, so each folder appears once in the tree and the
    // stream reads in the same order as the tree.
    parsed.sort_by(|a, b| {
        let key = |path: &str| match path.rsplit_once('/') {
            Some((directory, name)) => (1, directory.to_owned(), name.to_owned()),
            None => (0, String::new(), path.to_owned()),
        };
        key(&a.path).cmp(&key(&b.path))
    });
    for file in parsed {
        let highlights = zeron_syntax::language_for_path(&file.path)
            .and_then(|language| crate::changes::excerpt_highlights(&file, language));
        files.push((file.path.clone(), rows.len()));
        rows.push(CodeRow {
            role: RowRole::File,
            text: file.path.clone().into(),
            old: String::new(),
            new: String::new(),
            kind: LineKind::Meta,
            spans: Vec::new(),
        });
        for notice in crate::changes::file_notices(&file) {
            rows.push(CodeRow {
                role: RowRole::Notice,
                text: notice.into(),
                old: String::new(),
                new: String::new(),
                kind: LineKind::Meta,
                spans: Vec::new(),
            });
        }
        for hunk in file.hunks {
            rows.push(CodeRow {
                role: RowRole::Hunk,
                text: hunk.header.into(),
                old: String::new(),
                new: String::new(),
                kind: LineKind::Meta,
                spans: Vec::new(),
            });
            rows.extend(hunk.lines.into_iter().map(|line| {
                CodeRow {
                    role: RowRole::Line,
                    spans: highlights
                        .as_ref()
                        .map(|h| h.spans(&line).to_vec())
                        .unwrap_or_default(),
                    text: line.text.into(),
                    old: line.old_no.map(|n| n.to_string()).unwrap_or_default(),
                    new: line.new_no.map(|n| n.to_string()).unwrap_or_default(),
                    kind: line.kind,
                }
            }));
        }
    }
    (rows, files)
}

#[derive(Clone)]
struct ParsedDiff {
    patch: Arc<String>,
    rows: Arc<Vec<CodeRow>>,
    files: Arc<Vec<(String, usize)>>,
    pairs: Arc<Vec<Vec<crate::changes::LinePair>>>,
    /// Per-file (additions, deletions), counted from the patch itself.
    stats: Arc<Vec<(u64, u64)>>,
    /// Widest code line, in display columns, across the whole patch.
    columns: usize,
    gutter: f32,
}

impl ParsedDiff {
    fn new(patch: String) -> Self {
        use crate::changes::LineKind;
        let (rows, files) = code_rows(&patch);
        let stats = files
            .iter()
            .enumerate()
            .map(|(file, (_, start))| {
                let end = files.get(file + 1).map_or(rows.len(), |(_, end)| *end);
                rows[*start..end]
                    .iter()
                    .fold((0, 0), |(add, del), row| match row.kind {
                        LineKind::Add if row.role == RowRole::Line => (add + 1, del),
                        LineKind::Del if row.role == RowRole::Line => (add, del + 1),
                        _ => (add, del),
                    })
            })
            .collect();
        let columns = rows
            .iter()
            .filter(|row| row.role == RowRole::Line)
            .map(|row| crate::changes::visual_columns(&row.text))
            .max()
            .unwrap_or(0);
        Self {
            pairs: Arc::new(code::split_files(&rows, &files)),
            gutter: code::code_gutter(&rows),
            patch: Arc::new(patch),
            rows: Arc::new(rows),
            files: Arc::new(files),
            stats: Arc::new(stats),
            columns,
        }
    }
}

#[derive(Clone)]
struct DetailSnapshot {
    detail: ChangeRequestDetail,
    body: crate::markdown::BlockTree,
    activity: Vec<crate::markdown::BlockTree>,
    fetched: Instant,
    diff: Option<ParsedDiff>,
}

/// Window/profile scoped, bounded cache. Device remains part of the identity.
#[derive(Default)]
pub(crate) struct PullRequestCache {
    entries: Vec<((Option<String>, String), DetailSnapshot)>,
    /// Unsent comments, with the reason the last send failed.
    drafts: std::collections::HashMap<(Option<String>, String), (String, Option<String>)>,
}

impl PullRequestCache {
    fn get(&mut self, target: &Option<String>, url: &str) -> Option<DetailSnapshot> {
        let index = self
            .entries
            .iter()
            .position(|((device, entry), _)| device == target && entry == url)?;
        let entry = self.entries.remove(index);
        let snapshot = entry.1.clone();
        self.entries.push(entry);
        Some(snapshot)
    }

    fn put(&mut self, target: Option<String>, url: String, snapshot: DetailSnapshot) {
        self.entries
            .retain(|((device, entry), _)| device != &target || entry != &url);
        self.entries.push(((target, url), snapshot));
        if self.entries.len() > 12 {
            self.entries.remove(0);
        }
    }

    fn evict(&mut self, target: &Option<String>, url: &str) {
        self.entries
            .retain(|((device, entry), _)| device != target || entry != url);
    }

    fn set_draft(&mut self, target: &Option<String>, url: &str, text: &str, error: Option<String>) {
        let key = (target.clone(), url.to_owned());
        if text.is_empty() {
            self.drafts.remove(&key);
        } else {
            self.drafts.insert(key, (text.to_owned(), error));
        }
    }
}

pub struct PullRequestDetailPage {
    state: Entity<AppState>,
    pub url: String,
    target: Option<String>,
    detail: Option<ChangeRequestDetail>,
    body: Option<crate::markdown::BlockTree>,
    activity_bodies: Vec<crate::markdown::BlockTree>,
    cache: Rc<RefCell<PullRequestCache>>,
    preview: Option<ChangeRequestListItem>,
    error: Option<String>,
    loading: bool,
    fetched: Option<Instant>,
    task: Option<Task<()>>,
    diff_task: Option<Task<()>>,
    copy_reset: Option<Task<()>>,
    copied_link: bool,
    diff: Option<Arc<String>>,
    code_rows: Arc<Vec<CodeRow>>,
    code_pairs: Arc<Vec<Vec<crate::changes::LinePair>>>,
    code_split: bool,
    code_files: Arc<Vec<(String, usize)>>,
    code_stats: Arc<Vec<(u64, u64)>>,
    code_columns: usize,
    code_gutter: f32,
    code_horizontal: gpui::ScrollHandle,
    /// The review stream: every file's header and (unless folded) body.
    code_list: gpui::ListState,
    code_stream: Rc<Vec<code::StreamRow>>,
    code_ranges: Vec<std::ops::Range<usize>>,
    collapsed_files: std::collections::HashSet<usize>,
    /// A file picked from the tree stays selected until the reader scrolls;
    /// the last files can be too short to reach the top of the viewport.
    jumped_file: Option<usize>,
    file_tree_scroll: gpui::ScrollHandle,
    diff_error: Option<String>,
    /// Refresh ran away from the Code tab: its next diff load must bypass
    /// the engine cache so it matches the refreshed details.
    diff_refresh_owed: bool,
    tab: Tab,
    checks_expanded: bool,
    tab_slide: crate::motion::IndicatorSlide,
    tab_focus: [gpui::FocusHandle; 3],
    /// The file tree is one Tab stop; arrow keys move between files.
    tree_focus: gpui::FocusHandle,
    /// The narrow layout's file picker trigger, refocused when it closes.
    files_focus: gpui::FocusHandle,
    /// Focus to restore on the next frame, from handlers without a window.
    return_focus: Option<gpui::FocusHandle>,
    file_search: Entity<crate::composer::ComposerInput>,
    file_search_subscription: Option<Subscription>,
    file_query: String,
    files_expanded: bool,
    code_pane_width: Option<f32>,
    scroll: widgets::PageScroll,
    comment_input: Entity<crate::composer::ComposerInput>,
    comment_subscription: Option<Subscription>,
    /// A comment is being posted.
    sending: bool,
    comment_error: Option<String>,
    mention_token: Option<crate::composer::MentionToken>,
    mention_choices: Vec<String>,
    mention_index: usize,
    image_preview: Option<crate::attachments::PreviewImage>,
    image_focus: gpui::FocusHandle,
    image_task: Option<Task<()>>,
    image_cached: Option<(String, crate::attachments::PreviewImage)>,
    image_previous_focus: Option<gpui::FocusHandle>,
    /// The image that failed to load, offered in the browser instead.
    image_failed: Option<String>,
}

impl PullRequestDetailPage {
    pub(crate) fn new(
        state: Entity<AppState>,
        url: String,
        target: Option<String>,
        cache: Rc<RefCell<PullRequestCache>>,
        preview: Option<ChangeRequestListItem>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut page = Self {
            state,
            url,
            target,
            detail: None,
            body: None,
            activity_bodies: Vec::new(),
            cache,
            preview,
            error: None,
            loading: false,
            fetched: None,
            task: None,
            diff_task: None,
            copy_reset: None,
            copied_link: false,
            diff: None,
            code_rows: Default::default(),
            code_pairs: Default::default(),
            code_split: crate::settings::current(cx).diff_split,
            code_files: Default::default(),
            code_stats: Default::default(),
            code_columns: 0,
            code_gutter: crate::changes::GUTTER_WIDTH,
            code_horizontal: gpui::ScrollHandle::new(),
            code_list: gpui::ListState::new(0, gpui::ListAlignment::Top, px(1024.0)),
            code_stream: Default::default(),
            code_ranges: Vec::new(),
            collapsed_files: Default::default(),
            jumped_file: None,
            file_tree_scroll: gpui::ScrollHandle::new(),
            diff_error: None,
            diff_refresh_owed: false,
            tab: Tab::Summary,
            checks_expanded: false,
            tab_slide: crate::motion::IndicatorSlide::at(tab_slot(Tab::Summary), Instant::now()),
            tab_focus: [cx.focus_handle(), cx.focus_handle(), cx.focus_handle()],
            tree_focus: cx.focus_handle().tab_stop(true),
            files_focus: cx.focus_handle().tab_stop(true),
            return_focus: None,
            file_search: cx.new(|cx| {
                crate::composer::ComposerInput::with_context(
                    "Find a changed file…",
                    "PaletteSearch",
                    cx,
                )
                .with_single_line()
                .with_text_metrics(12.0, 16.0)
                .with_tab_stop()
            }),
            file_search_subscription: None,
            file_query: String::new(),
            files_expanded: false,
            code_pane_width: None,
            scroll: widgets::PageScroll::default(),
            comment_input: cx.new(|cx| {
                crate::composer::ComposerInput::with_context(
                    "Write a comment… Type @ to mention someone",
                    crate::composer::MESSAGE_COMPOSER_CONTEXT,
                    cx,
                )
                .with_viewport_height(120.0)
                .with_tab_stop()
            }),
            comment_subscription: None,
            sending: false,
            comment_error: None,
            mention_token: None,
            mention_choices: Vec::new(),
            mention_index: 0,
            image_preview: None,
            image_focus: cx.focus_handle(),
            image_task: None,
            image_cached: None,
            image_previous_focus: None,
            image_failed: None,
        };
        page.file_search_subscription = Some(cx.subscribe(
            &page.file_search,
            |page: &mut Self, input, event, cx| {
                if matches!(event, crate::composer::ComposerInputEvent::Edited) {
                    page.file_query = input.read(cx).text().to_lowercase();
                    cx.notify();
                } else if matches!(event, crate::composer::ComposerInputEvent::Submitted) {
                    if let Some(index) = page
                        .code_files
                        .iter()
                        .position(|(path, _)| path.to_lowercase().contains(&page.file_query))
                    {
                        page.select_code_file(index, cx);
                        if page.files_expanded {
                            page.toggle_files(cx);
                            page.return_focus = Some(page.files_focus.clone());
                        }
                    }
                }
            },
        ));
        let page_handle = cx.weak_entity();
        page.code_list.set_scroll_handler(move |_, _, cx| {
            // Scrolling hands the tree selection back to the scroll position
            // and re-pins the sticky header.
            let _ = page_handle.update(cx, |page, cx| {
                page.jumped_file = None;
                cx.notify();
            });
        });
        page.comment_subscription =
            Some(cx.subscribe(&page.comment_input, |page, _, event, cx| {
                page.comment_event(event, cx)
            }));
        let cached = page.cache.borrow_mut().get(&page.target, &page.url);
        if let Some(snapshot) = cached {
            page.fetched = Some(snapshot.fetched);
            page.detail = Some(snapshot.detail);
            page.body = Some(snapshot.body);
            page.activity_bodies = snapshot.activity;
            if let Some(diff) = snapshot.diff {
                page.install_diff(diff, cx);
            }
        }
        let draft = page
            .cache
            .borrow()
            .drafts
            .get(&(page.target.clone(), page.url.clone()))
            .cloned();
        if let Some((text, error)) = draft {
            page.comment_input
                .update(cx, |input, cx| input.set_text(text, cx));
            page.comment_error = error;
        }
        if page.detail.is_none() {
            page.load(false, cx);
        }
        page
    }

    pub(crate) fn titlebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let url = self.url.clone();
        let copy_url = self.url.clone();
        let identity = self
            .detail
            .as_ref()
            .map(|detail| (detail.number, detail.title.clone()))
            .or_else(|| {
                self.preview
                    .as_ref()
                    .map(|item| (item.number, item.title.clone()))
            });
        let label = identity.as_ref().map_or_else(
            || "Pull request".into(),
            |(number, title)| format!("pull request {number}, {title}"),
        );
        let identity = identity.map_or_else(
            || "Pull request".into(),
            |(number, title)| format!("{number} · {title}"),
        );
        div()
            .w_full()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                action("pr-back", "Back to pull requests", &theme)
                    .flex_1()
                    .min_w_0()
                    .w_auto()
                    .justify_start()
                    .px(px(4.0))
                    .aria_label(format!("Back to pull requests from {label}"))
                    .gap(px(6.0))
                    .child(
                        crate::icons::icon(crate::icons::PULL_REQUEST)
                            .size(px(13.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .id("pr-back-title")
                            .debug_selector(|| "pr-back-title".into())
                            .min_w_0()
                            .truncate()
                            .text_size(px(12.0))
                            .child(identity),
                    )
                    .on_click(|_, window, cx| {
                        cx.stop_propagation();
                        window.dispatch_action(Box::new(ClosePullRequest), cx)
                    }),
            )
            .child(
                action("pr-detail-refresh", "Refresh pull request", &theme)
                    .when(self.loading, |el| el.opacity(0.4))
                    .on_click(cx.listener(|page, _, _, cx| {
                        cx.stop_propagation();
                        page.refresh(cx);
                    })),
            )
            .child(
                action(
                    "pr-copy-url",
                    if self.copied_link {
                        "Link copied"
                    } else {
                        "Copy link"
                    },
                    &theme,
                )
                .on_click(cx.listener(move |page, _, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_url.clone()));
                    page.copied_link = true;
                    page.copy_reset = Some(cx.spawn(async move |page, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        let _ = page.update(cx, |page, cx| {
                            page.copied_link = false;
                            cx.notify();
                        });
                    }));
                    cx.notify();
                })),
            )
            .child(
                action("pr-external", "Open in default browser", &theme).on_click(
                    move |_, _, cx| {
                        cx.stop_propagation();
                        cx.open_url(&url);
                    },
                ),
            )
            .into_any_element()
    }

    /// Reload the details, and the diff now or on the next Code visit.
    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.diff = None;
        let cached = self.cache.borrow_mut().get(&self.target, &self.url);
        if let Some(mut snapshot) = cached {
            snapshot.diff = None;
            snapshot.fetched = Instant::now();
            self.cache
                .borrow_mut()
                .put(self.target.clone(), self.url.clone(), snapshot);
        }
        self.load(true, cx);
        if self.tab == Tab::Code {
            self.load_diff(true, cx);
        } else {
            self.diff_task = None;
            self.diff_refresh_owed = true;
        }
    }

    fn params(&self, refresh: bool) -> serde_json::Value {
        let mut params = serde_json::json!({"url": self.url, "refresh": refresh});
        if let Some(target) = &self.target {
            params["targetDeviceId"] = target.clone().into();
        }
        params
    }

    fn load(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.sending {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.error = Some("Connect to your device to load this pull request.".into());
            return;
        };
        self.loading = true;
        self.error = None;
        let params = self.params(refresh);
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::GET_CHANGE_REQUEST, params).await
                .map_err(|error| format!("Couldn’t load this pull request. {}", failure_reason(&error)))
                .and_then(|value| serde_json::from_value::<ChangeRequestDetail>(value).map_err(|_| "The device sent a pull request this version can’t read. Update Zeron on the selected device.".to_owned()));
            // Large descriptions and review threads must not stall the UI thread.
            let result = cx.background_executor().spawn(async move {
                result.map(|detail| DetailSnapshot {
                    body: super::pull_request_media::parse_description(&detail.body),
                    activity: detail.comments.iter().chain(&detail.reviews)
                        .map(|comment| super::pull_request_media::parse_description(&comment.body)).collect(),
                    detail, fetched: Instant::now(), diff: None,
                })
            }).await;
            let _ = this.update(cx, |page, cx| {
                page.loading = false;
                match result {
                    Ok(mut snapshot) => {
                        page.fetched = Some(snapshot.fetched);
                        snapshot.diff = page.diff_snapshot();
                        page.body = Some(snapshot.body.clone());
                        page.activity_bodies = snapshot.activity.clone();
                        page.detail = Some(snapshot.detail.clone());
                        page.cache.borrow_mut().put(page.target.clone(), page.url.clone(), snapshot);
                    }
                    Err(error) => page.error = Some(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn load_diff(&mut self, refresh: bool, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.diff_error = Some("Connect to your device to load the diff.".into());
            return;
        };
        self.diff_error = None;
        let refresh = refresh || std::mem::take(&mut self.diff_refresh_owed);
        let params = self.params(refresh);
        self.diff_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::GET_CHANGE_REQUEST_DIFF, params)
                .await
                .map_err(|error| format!("Couldn’t load the diff. {}", failure_reason(&error)))
                .and_then(|value| {
                    serde_json::from_value::<String>(value)
                        .map_err(|_| "The device returned an invalid diff.".to_owned())
                });
            let result = cx
                .background_executor()
                .spawn(async move { result.map(ParsedDiff::new) })
                .await;
            let _ = this.update(cx, |page, cx| {
                page.diff_task = None;
                match result {
                    Ok(diff) => {
                        let cached = page.cache.borrow_mut().get(&page.target, &page.url);
                        if let Some(mut snapshot) = cached {
                            snapshot.diff = Some(diff.clone());
                            page.cache.borrow_mut().put(
                                page.target.clone(),
                                page.url.clone(),
                                snapshot,
                            );
                        }
                        page.install_diff(diff, cx);
                    }
                    Err(error) => page.diff_error = Some(error),
                }
                cx.notify();
            });
        }));
    }

    fn diff_snapshot(&self) -> Option<ParsedDiff> {
        Some(ParsedDiff {
            pairs: self.code_pairs.clone(),
            patch: self.diff.clone()?,
            rows: self.code_rows.clone(),
            files: self.code_files.clone(),
            stats: self.code_stats.clone(),
            columns: self.code_columns,
            gutter: self.code_gutter,
        })
    }

    fn install_diff(&mut self, diff: ParsedDiff, cx: &mut Context<Self>) {
        self.code_pairs = diff.pairs;
        self.diff = Some(diff.patch);
        self.code_rows = diff.rows;
        self.code_files = diff.files;
        self.code_stats = diff.stats;
        self.code_columns = diff.columns;
        self.code_gutter = diff.gutter;
        self.collapsed_files.clear();
        self.jumped_file = None;
        self.rebuild_stream(cx);
    }

    /// Floating pill nav. One raised thumb glides between equal-width segments
    /// (positioned as a fraction of the track, so it stays aligned at any label
    /// length or text scale) while each segment's icon and label fade with the
    /// thumb's coverage. The pill sizes to its content and only shrinks when
    /// the window is narrower than that.
    fn navigation(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let slots = TABS.len() as f32;
        let position = self.tab_slide.value_at(Instant::now());
        let thumb = div()
            .debug_selector(|| "pr-detail-nav-thumb".into())
            .absolute()
            .top_0()
            .bottom_0()
            .left(gpui::relative(position / slots))
            .w(gpui::relative(1.0 / slots))
            .rounded_full()
            .bg(theme.glass_hover())
            .shadow(crate::theme::card_selected_shadows());
        let counts = self.detail.as_ref().map(|detail| {
            [
                None,
                Some(detail.files.len()),
                Some(detail.comments.len() + detail.reviews.len()),
            ]
        });
        let segments = TABS
            .into_iter()
            .enumerate()
            .map(|(slot, (tab, label, id, glyph))| {
                // 1 while the thumb sits on this slot, 0 once it is a slot away.
                let covered = (1.0 - (position - slot as f32).abs()).clamp(0.0, 1.0);
                let hover_key = format!("pr-detail-{}-{id}", cx.entity_id());
                let hover = crate::motion::hover_t(&hover_key);
                let emphasis = covered.max(hover);
                let selected = tab == self.tab;
                let count = counts
                    .and_then(|counts| counts[slot])
                    .filter(|count| *count > 0);
                div()
                    .id(id)
                    .debug_selector(move || id.into())
                    .role(gpui::Role::Tab)
                    .aria_label(match count {
                        Some(count) => format!("{label}, {count}"),
                        None => label.to_owned(),
                    })
                    .aria_selected(selected)
                    .track_focus(&self.tab_focus[slot].clone().tab_stop(selected))
                    .tab_index(0)
                    .min_w_0()
                    .h(px(NAV_SEGMENT_HEIGHT))
                    .px(px(14.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(6.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(crate::motion::mix(theme.text_muted, theme.text, emphasis))
                    .bg(crate::motion::mix(
                        theme.glass_hover().opacity(0.0),
                        theme.glass_hover(),
                        hover * (1.0 - covered) * 0.6,
                    ))
                    .focus_visible(|style| style.border_2().border_color(theme.accent))
                    .cursor_pointer()
                    .on_hover(crate::motion::hover_listener(hover_key))
                    .child(
                        crate::icons::icon(glyph)
                            .size(px(14.0))
                            .flex_none()
                            .text_color(crate::motion::mix(theme.text_muted, theme.text, emphasis)),
                    )
                    .child(div().min_w_0().truncate().child(label))
                    .children(count.map(|count| {
                        div()
                            .flex_none()
                            .text_size(crate::typography::ui_rems(11.0))
                            .font_weight(gpui::FontWeight::NORMAL)
                            .text_color(theme.text_muted)
                            .child(count.to_string())
                    }))
                    .on_click(cx.listener(move |page, _, _, cx| page.select_tab(tab, cx)))
                    .on_key_down(cx.listener(
                        move |page, event: &gpui::KeyDownEvent, window, cx| {
                            let next = match event.keystroke.key.as_str() {
                                "left" => (slot + TABS.len() - 1) % TABS.len(),
                                "right" => (slot + 1) % TABS.len(),
                                "home" => 0,
                                "end" => TABS.len() - 1,
                                _ => return,
                            };
                            cx.stop_propagation();
                            page.select_tab(TABS[next].0, cx);
                            window.focus(&page.tab_focus[next], cx);
                        },
                    ))
            });
        let tabs = div()
            .id("pr-detail-nav")
            .debug_selector(|| "pr-detail-nav".into())
            .role(gpui::Role::TabList)
            .aria_label("Pull request sections")
            .occlude()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .p(px(NAV_PADDING))
            .rounded_full()
            .bg(theme.input_glass_bg())
            .shadow_lg()
            .flex_none()
            .child(
                div()
                    .relative()
                    .grid()
                    .grid_cols(TABS.len() as u16)
                    .child(thumb)
                    .children(segments),
            );
        // Concentric: the pill radius is the segment radius plus the padding.
        let radius = NAV_SEGMENT_HEIGHT / 2.0 + NAV_PADDING;
        div()
            .absolute()
            .bottom(px(16.0))
            .left_0()
            .right_0()
            .px(px(16.0))
            .flex()
            .justify_center()
            .child(div().max_w_full().child(crate::frost::frosted(
                radius,
                crate::frost::MENU_BLUR,
                tabs,
            )))
            .into_any_element()
    }

    fn toggle_files(&mut self, cx: &mut Context<Self>) {
        self.files_expanded = !self.files_expanded;
        cx.notify();
    }

    fn select_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.tab == tab {
            return;
        }
        self.tab_slide.retarget(
            tab_slot(tab),
            crate::motion::reduced_motion(cx),
            Instant::now(),
        );
        self.tab = tab;
        self.scroll.scroll.set_offset(gpui::Point::default());
        if tab == Tab::Code && self.diff.is_none() && self.diff_task.is_none() {
            self.load_diff(false, cx);
        }
        cx.notify();
    }
}

#[cfg(feature = "pull-request-fixture")]
impl PullRequestDetailPage {
    /// 0 Summary, 1 Code, 2 Activity.
    pub fn fixture_select_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        self.select_tab(TABS[index.min(TABS.len() - 1)].0, cx);
    }

    pub fn fixture_select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        self.select_code_file(index, cx);
    }

    /// Scroll the review stream so a file's header is pinned.
    pub fn fixture_scroll_code(&mut self, distance: f32, cx: &mut Context<Self>) {
        self.code_list.scroll_by(px(distance));
        self.jumped_file = None;
        cx.notify();
    }
}

fn rich_text(
    body: &crate::markdown::BlockTree,
    key: String,
    url: &str,
    theme: &Theme,
    window: &mut Window,
    owner: gpui::WeakEntity<PullRequestDetailPage>,
) -> AnyElement {
    let options = crate::markdown::render::RenderOptions {
        tasks: None,
        media: Some(super::pull_request_media::media(
            url,
            Rc::new(move |source, window, cx| {
                let _ = owner.update(cx, |page, cx| page.open_image(source, window, cx));
            }),
        )),
        row_key: key.into(),
        veil: None,
        cache: None,
        now: Instant::now(),
        link: None,
        workspace_root: None,
        code: None,
        copy: Some(crate::markdown::render::CopyUi {
            handler: Rc::new(|_, code, _, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(code.to_string()))
            }),
            copied_ix: None,
        }),
    };
    div()
        .debug_selector(|| "pr-rich-text".into())
        .child(super::pull_request_media::render_description(
            body, &options, theme, window,
        ))
        .into_any_element()
}

fn activity_time(raw: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|time| {
            crate::pull_requests::relative_time(
                time.with_timezone(&chrono::Utc),
                chrono::Utc::now(),
            )
        })
        .unwrap_or_else(|_| raw.to_owned())
}

fn action(id: &'static str, label: &'static str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    let icon_only = matches!(
        id,
        "pr-back"
            | "pr-external"
            | "pr-copy-url"
            | "pr-copy-patch"
            | "pr-copy-path"
            | "pr-copy-checkout"
            | "pr-files"
            | "pr-fold-all"
            | "pr-detail-refresh"
            | "pr-previous-file"
            | "pr-next-file"
            | "pr-split"
    );
    let glyph = match id {
        "pr-split" => Some(crate::icons::SPLIT_COLUMNS),
        "pr-previous-file" => Some(crate::icons::ALT_ARROW_UP),
        "pr-next-file" => Some(crate::icons::ALT_ARROW_DOWN),
        "pr-fold-all" => Some(crate::icons::FOLD_VERTICAL),
        "pr-back" => Some(crate::icons::ALT_ARROW_LEFT),
        "pr-copy-url" if label == "Link copied" => Some(crate::icons::CHECK),
        "pr-copy-url" | "pr-copy-patch" | "pr-copy-path" | "pr-copy-checkout" => {
            Some(crate::icons::COPY)
        }
        "pr-detail-refresh" | "pr-retry" | "pr-retry-diff" => Some(crate::icons::REFRESH),
        "pr-external" => Some(crate::icons::ARROW_UP_RIGHT),
        "pr-files" => Some(crate::icons::FILE_TREE),
        "pr-summary" => Some(crate::icons::DOCUMENT),
        "pr-code" => Some(crate::icons::FILE_CODE),
        "pr-activity" => Some(crate::icons::CHAT_ROUND_LINE),
        _ => None,
    };
    widgets::ghost_action(theme)
        .id(id)
        .debug_selector(move || id.to_owned())
        .role(gpui::Role::Button)
        .aria_label(label)
        .tab_index(0)
        .border_1()
        .border_color(gpui::transparent_black())
        .focus_visible(|style| style.border_2().border_color(theme.accent))
        .cursor_pointer()
        .when(icon_only, |el| {
            el.size(px(24.0))
                .flex_none()
                .rounded(px(6.0))
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                    window.prevent_default()
                })
                .px_0()
                .py_0()
                .justify_center()
                .tooltip(widgets::text_tooltip(label))
        })
        .children(glyph.map(|glyph| {
            crate::icons::icon(glyph)
                .size(px(14.0))
                .text_color(theme.text_muted)
        }))
        .when(!icon_only, |el| el.child(label))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusTone {
    Neutral,
    Positive,
    Warning,
    Negative,
    Merged,
}

fn ci_status(checks: &[zeron_proto::ChangeRequestCheck]) -> &'static str {
    if checks.is_empty() {
        return "No checks reported";
    }
    let statuses: Vec<_> = checks
        .iter()
        .map(|check| {
            [&check.conclusion, &check.state, &check.status]
                .into_iter()
                .find(|value| !value.is_empty())
                .map(|value| value.to_ascii_uppercase())
                .unwrap_or_default()
        })
        .collect();
    if statuses.iter().any(|value| {
        matches!(
            value.as_str(),
            "FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" | "CANCELLED" | "STARTUP_FAILURE"
        )
    }) {
        "FAILURE"
    } else if statuses
        .iter()
        .any(|value| !matches!(value.as_str(), "SUCCESS" | "NEUTRAL" | "SKIPPED"))
    {
        "PENDING"
    } else if statuses
        .iter()
        .all(|value| matches!(value.as_str(), "NEUTRAL" | "SKIPPED"))
    {
        "SKIPPED"
    } else {
        "SUCCESS"
    }
}

fn status_style(raw: &str) -> (String, &'static str, StatusTone) {
    use crate::icons;
    match raw.to_ascii_uppercase().replace(' ', "_").as_str() {
        "OPEN" => ("Open".into(), icons::PULL_REQUEST, StatusTone::Positive),
        "MERGED" => ("Merged".into(), icons::PULL_REQUEST, StatusTone::Merged),
        "CLOSED" => ("Closed".into(), icons::CLOSE_CIRCLE, StatusTone::Negative),
        "DRAFT" => ("Draft".into(), icons::DOCUMENT, StatusTone::Neutral),
        "APPROVED" => ("Approved".into(), icons::CHECK, StatusTone::Positive),
        "SUCCESS" => ("Passed".into(), icons::CHECK, StatusTone::Positive),
        "FAILURE" | "ERROR" | "TIMED_OUT" | "ACTION_REQUIRED" => {
            (humanize(raw), icons::CLOSE_CIRCLE, StatusTone::Negative)
        }
        "CHANGES_REQUESTED" => (
            "Changes requested".into(),
            icons::DANGER_TRIANGLE,
            StatusTone::Warning,
        ),
        "REVIEW_REQUIRED" => (
            "Awaiting review".into(),
            icons::CLOCK_CIRCLE,
            StatusTone::Warning,
        ),
        "IN_PROGRESS" | "PENDING" | "QUEUED" | "WAITING" => {
            (humanize(raw), icons::CLOCK_CIRCLE, StatusTone::Warning)
        }
        _ => (humanize(raw), icons::DOCUMENT, StatusTone::Neutral),
    }
}

fn humanize(raw: &str) -> String {
    let text = raw.replace('_', " ").to_lowercase();
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_else(|| "Not reported".into())
}

fn status_chip(raw: &str, theme: &Theme) -> AnyElement {
    let (label, glyph, tone) = status_style(raw);
    let color = match tone {
        StatusTone::Neutral => theme.text_muted,
        StatusTone::Positive => theme.success,
        StatusTone::Warning => theme.warning,
        StatusTone::Negative => theme.danger,
        StatusTone::Merged => theme.code_text,
    };
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(6.0))
        .bg(color.opacity(0.08))
        .text_color(theme.text)
        .text_size(crate::typography::ui_rems(12.0))
        .child(crate::icons::icon(glyph).size(px(14.0)).text_color(color))
        .child(label)
        .into_any_element()
}

/// Space between the Summary's stacked cards; the header sits 2× further.
const CARD_GAP: f32 = 12.0;

/// Title and metadata line; shared by the loaded page and its placeholder.
fn detail_header(
    title: &str,
    author: &str,
    repository: String,
    number: u64,
    fetched: Option<Instant>,
    theme: &Theme,
) -> gpui::Div {
    div()
        .min_w_0()
        .flex()
        .flex_col()
        .child(
            // `page_header` keeps one line; a PR title wraps instead.
            div()
                .id("pr-detail-title")
                .debug_selector(|| "pr-detail-title".into())
                .px(px(8.0))
                .min_w_0()
                .text_size(crate::typography::ui_rems(20.0))
                .line_height(crate::typography::ui_rems(26.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text)
                .child(title.to_owned()),
        )
        .child(
            div()
                .id("pr-detail-meta")
                .debug_selector(|| "pr-detail-meta".into())
                .flex_none()
                .mt(px(12.0))
                .px(px(8.0))
                .text_size(crate::typography::ui_rems(12.0))
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .min_w_0()
                        .text_color(theme.text_muted)
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .child(super::pull_request_media::avatar(
                            author,
                            "pr-author".into(),
                            20.0,
                            theme,
                        ))
                        .child(if author.is_empty() {
                            "Deleted account".to_owned()
                        } else {
                            author.to_owned()
                        }),
                )
                .child(meta_item(
                    crate::icons::FOLDER_WITH_FILES,
                    repository,
                    theme,
                ))
                .child(meta_item(
                    crate::icons::PULL_REQUEST,
                    number.to_string(),
                    theme,
                ))
                .when_some(fetched, |el, fetched| {
                    let age = if fetched.elapsed().as_secs() < 60 {
                        "just now".into()
                    } else {
                        format!("{}m ago", fetched.elapsed().as_secs() / 60)
                    };
                    el.child(meta_item(
                        crate::icons::CLOCK_CIRCLE,
                        format!("Loaded {age}"),
                        theme,
                    ))
                }),
        )
}

/// One muted icon-and-text item of the header's metadata line.
fn meta_item(glyph: &'static str, text: String, theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(4.0))
        .text_color(theme.text_muted)
        .child(
            crate::icons::icon(glyph)
                .size(px(13.0))
                .flex_none()
                .text_color(theme.text_muted),
        )
        .child(text)
}

fn field(label: &str, value: String, first: bool, theme: &Theme) -> AnyElement {
    let content = if matches!(label, "Status" | "Review" | "CI") {
        status_chip(&value, theme)
    } else if label == "Changes" {
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.0))
            .children(value.split_whitespace().map(|word| {
                div()
                    .text_color(if word.starts_with('+') {
                        theme.success_muted
                    } else if word.starts_with('−') {
                        theme.danger_muted
                    } else {
                        theme.text_muted
                    })
                    .child(word.to_owned())
            }))
            .into_any_element()
    } else {
        // Wraps rather than truncates: long branch names would hide the base.
        div()
            .min_w_0()
            .font_family(theme.font_mono.clone())
            .text_size(crate::typography::ui_rems(12.0))
            .child(value)
            .into_any_element()
    };
    let id = format!("pr-field-{label}");
    widgets::card_row(theme, first)
        .id(SharedString::from(id.clone()))
        .debug_selector(move || id.clone())
        .min_h(px(44.0))
        .py(px(8.0))
        .child(
            div()
                .w(px(80.0))
                .flex_none()
                .flex()
                .items_center()
                .child(widgets::row_title(theme, label)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(120.0))
                .flex()
                .items_center()
                .child(content),
        )
        .into_any_element()
}

impl crate::popover::ScrollRailHost for PullRequestDetailPage {
    fn rail_bar(&mut self) -> &mut crate::popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }
    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for PullRequestDetailPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(focus) = self.return_focus.take() {
            window.focus(&focus, cx);
        }
        let theme = Theme::of(cx).clone();
        let content = {
            let mut column = widgets::page_column()
                .id("pr-content-column")
                .debug_selector(|| "pr-content-column".into())
                .max_w(px(760.0))
                .when(self.tab == Tab::Code, |el| {
                    el.max_w_full().px(px(24.0)).h_full().min_h_0()
                })
                .pt(px(24.0))
                .pb(px(match self.tab {
                    // The comment dock sits below the scroll, not over it.
                    Tab::Activity => 32.0,
                    // The workspace reserves its own room for the navigation.
                    Tab::Code => 0.0,
                    Tab::Summary => 76.0,
                }))
                .text_size(crate::typography::ui_rems(13.0))
                .text_color(theme.text);
            if let Some(error) = &self.error {
                column = column
                    .child(widgets::error_strip(&theme, error.clone()))
                    .child(
                        div().mt(px(8.0)).flex().child(
                            action("pr-retry", "Try again", &theme)
                                .on_click(cx.listener(|page, _, _, cx| page.refresh(cx))),
                        ),
                    );
            }
            if let Some(detail) = &self.detail {
                let repository = self
                    .url
                    .split('/')
                    .skip(3)
                    .take(2)
                    .collect::<Vec<_>>()
                    .join("/");
                column = column
                    .when(self.tab != Tab::Code, |column| {
                        column.child(detail_header(
                            &detail.title,
                            &detail.author.login,
                            repository,
                            detail.number,
                            self.fetched,
                            &theme,
                        ))
                    })
                    .when(self.tab == Tab::Summary, |el| {
                        el.child(
                            widgets::section_card(&theme)
                                .id("pr-overview")
                                .debug_selector(|| "pr-overview".into())
                                .mt(px(24.0))
                                .child(field(
                                    "Status",
                                    if detail.is_draft {
                                        "Draft".into()
                                    } else {
                                        detail.state.clone()
                                    },
                                    true,
                                    &theme,
                                ))
                                .child(field(
                                    "Review",
                                    if detail.review_decision.is_empty() {
                                        "No review decision".into()
                                    } else {
                                        detail.review_decision.clone()
                                    },
                                    false,
                                    &theme,
                                ))
                                .child(field(
                                    "Branch",
                                    format!("{} → {}", detail.head_ref_name, detail.base_ref_name),
                                    false,
                                    &theme,
                                ))
                                .child(field(
                                    "Changes",
                                    format!(
                                        "{} {} · +{} −{}",
                                        detail.files.len(),
                                        if detail.files.len() == 1 {
                                            "file"
                                        } else {
                                            "files"
                                        },
                                        detail.additions,
                                        detail.deletions
                                    ),
                                    false,
                                    &theme,
                                )),
                        )
                    });
                match self.tab {
                    Tab::Summary => {
                        let mut checks = widgets::section_card(&theme)
                            .mt_0()
                            .id("pr-checks-card")
                            .debug_selector(|| "pr-checks-card".into());
                        checks = checks.child(
                            div()
                                .id("pr-checks-toggle")
                                .debug_selector(|| "pr-checks-toggle".into())
                                .role(gpui::Role::Button)
                                .tab_index(0)
                                .aria_label(format!(
                                    "Checks, {}, {}",
                                    detail.status_check_rollup.len(),
                                    status_style(ci_status(&detail.status_check_rollup)).0
                                ))
                                .aria_expanded(self.checks_expanded)
                                .rounded_t(px(12.0))
                                .when(!self.checks_expanded, |el| el.rounded_b(px(12.0)))
                                .px(px(16.0))
                                .py(px(10.0))
                                .flex()
                                .flex_wrap()
                                .items_center()
                                .gap(px(8.0))
                                .cursor_pointer()
                                .hover(|style| style.bg(theme.glass_hover()))
                                .focus_visible(|style| style.border_2().border_color(theme.accent))
                                .child(
                                    crate::icons::icon(if self.checks_expanded {
                                        crate::icons::ALT_ARROW_DOWN
                                    } else {
                                        crate::icons::ALT_ARROW_RIGHT
                                    })
                                    .size(px(14.0))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                                )
                                .child(div().flex_1().child(format!(
                                    "Checks · {}",
                                    detail.status_check_rollup.len()
                                )))
                                .child(status_chip(ci_status(&detail.status_check_rollup), &theme))
                                .on_click(cx.listener(|page, _, _, cx| {
                                    page.checks_expanded = !page.checks_expanded;
                                    cx.notify();
                                })),
                        );
                        if self.checks_expanded && detail.status_check_rollup.is_empty() {
                            checks = checks.child(
                                div()
                                    .p(px(16.0))
                                    .text_color(theme.text_muted)
                                    .child("No checks reported."),
                            );
                        }
                        for (index, check) in detail
                            .status_check_rollup
                            .iter()
                            .enumerate()
                            .filter(|_| self.checks_expanded)
                        {
                            let name = if check.name.is_empty() {
                                &check.context
                            } else {
                                &check.name
                            };
                            let status = [&check.conclusion, &check.state, &check.status]
                                .into_iter()
                                .find(|v| !v.is_empty())
                                .map(|v| v.replace('_', " ").to_lowercase())
                                .unwrap_or_else(|| "Pending".into());
                            let link = if check.details_url.is_empty() {
                                &check.target_url
                            } else {
                                &check.details_url
                            }
                            .clone();
                            checks = checks.child(
                                widgets::card_row(&theme, false)
                                    .mx_0()
                                    .px(px(16.0))
                                    .min_h(px(36.0))
                                    .py(px(4.0))
                                    .flex_nowrap()
                                    .when(index + 1 == detail.status_check_rollup.len(), |el| {
                                        el.rounded_b(px(12.0))
                                    })
                                    .id(SharedString::from(format!("pr-check-{index}")))
                                    .debug_selector(move || format!("pr-check-{index}"))
                                    .child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "pr-check-name-{index}"
                                            )))
                                            .debug_selector(move || {
                                                format!("pr-check-name-{index}")
                                            })
                                            .flex_1()
                                            .min_w_0()
                                            .child(name.clone()),
                                    )
                                    .child(div().flex_none().child(status_chip(&status, &theme)))
                                    .when(!link.is_empty(), |el| {
                                        el.cursor_pointer()
                                            .role(gpui::Role::Link)
                                            .tab_index(0)
                                            .aria_label(format!("Open {name}"))
                                            .focus_visible(|style| {
                                                style.border_2().border_color(theme.accent)
                                            })
                                            .hover(|style| style.bg(theme.glass_hover()))
                                            .on_click(move |_, _, cx| cx.open_url(&link))
                                    }),
                            );
                        }
                        column = column
                            .child(checks.mt(px(CARD_GAP)))
                            .child(self.handoff_card(detail, &theme, cx));
                        let description = div()
                            .p(px(16.0))
                            .min_h(px(60.0))
                            .when(detail.body.is_empty(), |el| {
                                el.text_color(theme.text_muted)
                                    .child("No description provided.")
                            })
                            .when_some(
                                self.body.as_ref().filter(|_| !detail.body.is_empty()),
                                |el, body| {
                                    el.child(rich_text(
                                        body,
                                        "pr-description".into(),
                                        &self.url,
                                        &theme,
                                        window,
                                        cx.weak_entity(),
                                    ))
                                },
                            );
                        column = column.child(
                            widgets::section_card(&theme)
                                .id("pr-description")
                                .debug_selector(|| "pr-description".into())
                                .mt(px(CARD_GAP))
                                .child(description),
                        );
                    }
                    Tab::Code => {
                        if let Some(error) = &self.diff_error {
                            column = column
                                .child(widgets::error_strip(&theme, error.clone()))
                                .child(action("pr-retry-diff", "Retry diff", &theme).on_click(
                                    cx.listener(|page, _, _, cx| page.load_diff(true, cx)),
                                ));
                        } else {
                            // Loading renders the same workspace with placeholders.
                            column = column.child(self.code_workspace(&theme, window, cx));
                        }
                    }
                    Tab::Activity => {
                        let mut activity: Vec<_> = detail
                            .comments
                            .iter()
                            .chain(detail.reviews.iter())
                            .enumerate()
                            .collect();
                        activity.sort_by_key(|(_, comment)| {
                            if comment.created_at.is_empty() {
                                &comment.submitted_at
                            } else {
                                &comment.created_at
                            }
                        });
                        // The thread keeps the same 24px under the header as the
                        // Summary's first card.
                        let mut thread = div()
                            .id("pr-activity-thread")
                            .debug_selector(|| "pr-activity-thread".into())
                            .mt(px(24.0))
                            // Authors line up with the title and metadata above;
                            // your own replies stay flush with the composer.
                            .pl(px(8.0))
                            .flex()
                            .flex_col();
                        if activity.is_empty() {
                            thread = thread.child(
                                div()
                                    .text_color(theme.text_muted)
                                    .child("No comments or reviews yet."),
                            );
                        }
                        let viewer_login = detail
                            .comments
                            .iter()
                            .chain(detail.reviews.iter())
                            .find(|comment| {
                                comment.viewer_did_author && !comment.author.login.is_empty()
                            })
                            .map(|comment| comment.author.login.as_str());
                        for (index, comment) in activity {
                            let own = comment.viewer_did_author
                                || viewer_login.is_some_and(|login| {
                                    login.eq_ignore_ascii_case(&comment.author.login)
                                });
                            thread = thread.child(
                                div()
                                    .w_full()
                                    .flex()
                                    .mb(px(24.0))
                                    .when(own, |el| el.justify_end())
                                    .child(
                                        div()
                                            .id(SharedString::from(format!("pr-message-{index}")))
                                            .debug_selector(move || format!("pr-message-{index}"))
                                            .min_w_0()
                                            .w(gpui::relative(0.9))
                                            .when(own, |el| {
                                                el.max_w(gpui::relative(0.8))
                                                    .px(px(16.0))
                                                    .py(px(10.0))
                                                    .rounded(px(Theme::BUBBLE_RADIUS))
                                                    .bg(crate::theme::user_bubble_bg())
                                            })
                                            .flex()
                                            .flex_col()
                                            .gap(px(8.0))
                                            .child(
                                                div()
                                                    .flex()
                                                    .flex_wrap()
                                                    .items_center()
                                                    .gap(px(8.0))
                                                    .child(super::pull_request_media::avatar(
                                                        &comment.author.login,
                                                        format!("pr-actor-{index}").into(),
                                                        24.0,
                                                        &theme,
                                                    ))
                                                    .child(
                                                        div()
                                                            .font_weight(gpui::FontWeight::MEDIUM)
                                                            .child(
                                                                if comment.author.login.is_empty() {
                                                                    "Deleted account".to_owned()
                                                                } else {
                                                                    comment.author.login.clone()
                                                                },
                                                            ),
                                                    )
                                                    .when(!comment.state.is_empty(), |el| {
                                                        el.child(status_chip(
                                                            &comment.state,
                                                            &theme,
                                                        ))
                                                    })
                                                    .child(
                                                        div()
                                                            .text_size(crate::typography::ui_rems(
                                                                11.0,
                                                            ))
                                                            .text_color(theme.text_muted)
                                                            .child(activity_time(
                                                                if comment.created_at.is_empty() {
                                                                    &comment.submitted_at
                                                                } else {
                                                                    &comment.created_at
                                                                },
                                                            )),
                                                    ),
                                            )
                                            .children(self.activity_bodies.get(index).map(
                                                |body| {
                                                    div().child(rich_text(
                                                        body,
                                                        format!("pr-activity-{index}"),
                                                        &self.url,
                                                        &theme,
                                                        window,
                                                        cx.weak_entity(),
                                                    ))
                                                },
                                            )),
                                    ),
                            );
                        }
                        column = column.child(thread);
                    }
                }
            } else if self.loading {
                // The loaded layout with placeholders, so nothing shifts when
                // the pull request arrives. The board's row supplies the header.
                if self.tab != Tab::Code {
                    column = column.child(match &self.preview {
                        Some(preview) => detail_header(
                            &preview.title,
                            &preview.author.login,
                            preview.repository.clone(),
                            preview.number,
                            None,
                            &theme,
                        )
                        .into_any_element(),
                        None => div()
                            .flex()
                            .flex_col()
                            .gap(px(12.0))
                            .px(px(8.0))
                            .child(crate::pull_request_skeleton::bar(
                                gpui::relative(0.6),
                                20.0,
                                0,
                                cx.entity_id(),
                                &theme,
                                cx,
                            ))
                            .child(crate::pull_request_skeleton::bar(
                                px(240.0),
                                10.0,
                                1,
                                cx.entity_id(),
                                &theme,
                                cx,
                            ))
                            .into_any_element(),
                    });
                }
                column = column.child(crate::pull_request_skeleton::summary(
                    CARD_GAP,
                    cx.entity_id(),
                    &theme,
                    cx,
                ));
            }
            let scroll = self.scroll.scroll.clone();
            let rail = crate::popover::rail(self, "pr-detail-scrollbar", &theme, cx);
            div()
                .id("pr-detail-scroll-host")
                .flex_1()
                .min_h_0()
                .relative()
                .on_hover(cx.listener(|page, hovered: &bool, _, cx| {
                    if page.scroll.set_list_hovered(*hovered) {
                        cx.notify();
                    }
                }))
                .child(
                    crate::edge_fade::edge_faded(
                        24.0,
                        true,
                        true,
                        div()
                            .id("pr-detail-scroll")
                            .debug_selector(|| "pr-detail-scroll".into())
                            .size_full()
                            .when(self.tab != Tab::Code, |el| el.overflow_y_scroll())
                            .when(self.tab == Tab::Code, |el| el.overflow_hidden())
                            .track_scroll(&scroll)
                            .child(column),
                    )
                    .fade_overflow_y(&scroll),
                )
                .when(self.tab != Tab::Code, |el| el.children(rail))
                .into_any_element()
        };
        let navigation = self.navigation(&theme, cx);
        if self.tab_slide.animating_at(Instant::now()) {
            window.request_animation_frame();
        }
        let composer = (self.tab == Tab::Activity).then(|| self.comment_composer(&theme, cx));
        let mut root = div()
            .size_full()
            .relative()
            .on_action(cx.listener(|page, action: &OpenPrImage, window, cx| {
                page.open_image(&action.0, window, cx)
            }))
            .flex()
            .flex_col()
            .pt(px(Theme::TITLEBAR_HEIGHT))
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(content)
                    .children(composer)
                    .child(navigation),
            );
        if let Some(preview) = &self.image_preview {
            let weak = cx.weak_entity();
            root = root.child(crate::attachments::lightbox(
                window,
                preview,
                &self.image_focus,
                move |window, cx| {
                    let _ = weak.update(cx, |page, cx| {
                        page.image_preview = None;
                        if let Some(focus) = page.image_previous_focus.take() {
                            window.focus(&focus, cx);
                        }
                        cx.notify();
                    });
                },
                cx,
            ));
        }
        if self.image_task.is_some() || self.image_failed.is_some() {
            root = root.child(
                div()
                    .id("pr-image-loading")
                    .absolute()
                    .inset_0()
                    .occlude()
                    .track_focus(&self.image_focus)
                    .bg(crate::popover::scrim_alpha(0.7))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(gpui::white())
                    .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, window, cx| {
                        if event.keystroke.key == "escape" {
                            page.close_image(window, cx);
                            cx.stop_propagation();
                        }
                    }))
                    .on_click(cx.listener(|page, _, window, cx| {
                        if let Some(source) = &page.image_failed {
                            cx.open_url(source);
                        }
                        page.close_image(window, cx)
                    }))
                    .child(if self.image_failed.is_some() {
                        "Couldn’t load image. Click to open it in your browser · Escape to close"
                    } else {
                        "Loading image… · Escape to cancel"
                    }),
            );
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(title: &str) -> DetailSnapshot {
        DetailSnapshot {
            detail: ChangeRequestDetail {
                title: title.into(),
                ..Default::default()
            },
            body: crate::markdown::parse_full("**Rich description**"),
            activity: vec![crate::markdown::parse_full("### Review\n\n- **important**")],
            fetched: Instant::now(),
            diff: None,
        }
    }

    #[test]
    fn pull_request_cache_is_bounded_and_separates_devices() {
        let mut cache = PullRequestCache::default();
        cache.put(None, "same".into(), snapshot("Local"));
        cache.put(Some("remote".into()), "same".into(), snapshot("Remote"));
        assert_eq!(cache.get(&None, "same").unwrap().detail.title, "Local");
        assert_eq!(
            cache
                .get(&Some("remote".into()), "same")
                .unwrap()
                .detail
                .title,
            "Remote"
        );
        for index in 0..12 {
            cache.put(None, format!("url-{index}"), snapshot("PR"));
        }
        assert_eq!(cache.entries.len(), 12);
        assert!(cache.get(&None, "same").is_none());
        cache.get(&None, "url-0");
        cache.put(None, "new".into(), snapshot("New"));
        assert!(cache.get(&None, "url-0").is_some());
        assert!(cache.get(&None, "url-1").is_none());
    }

    #[gpui::test]
    fn pull_request_reopening_uses_cached_content_without_a_network_request(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        cx.update(|cx| cx.set_global(Theme::default()));
        let (_, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|_| AppState::new());
            let cache = Rc::new(RefCell::new(PullRequestCache::default()));
            let url = "https://github.com/a/b/pull/1";
            let diff = ParsedDiff::new(
                "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n".into(),
            );
            let rows = diff.rows.clone();
            let mut cached = snapshot("Already loaded");
            cached.diff = Some(diff);
            cached.fetched = Instant::now() - Duration::from_secs(3600);
            cache.borrow_mut().put(None, url.into(), cached);
            let page = PullRequestDetailPage::new(state, url.into(), None, cache, None, window, cx);
            // No engine exists in this fixture. Attempting a fetch would set an error.
            assert!(!page.loading && page.error.is_none());
            assert_eq!(page.detail.as_ref().unwrap().title, "Already loaded");
            assert_eq!(page.activity_bodies.len(), 1);
            assert!(
                Arc::ptr_eq(&page.code_rows, &rows),
                "cached diff rows are reused without parsing or copying"
            );
            page
        });
        cx.run_until_parked();
    }

    /// Records each diff request's `refresh` flag.
    struct DiffRpc(Arc<std::sync::Mutex<Vec<bool>>>);
    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for DiffRpc {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
            match method {
                methods::GET_CHANGE_REQUEST => zeron_rpc::RpcReply::value(&ChangeRequestDetail {
                    title: "Fresh".into(),
                    ..Default::default()
                }),
                methods::GET_CHANGE_REQUEST_DIFF => {
                    self.0.lock().unwrap().push(params["refresh"] == true);
                    zeron_rpc::RpcReply::value(
                        &"diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-old\n+new\n",
                    )
                }
                other => Err(zeron_rpc::RpcError::UnknownMethod(other.into())),
            }
        }
    }

    #[gpui::test]
    fn pull_request_refresh_away_from_code_refreshes_the_next_diff(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        cx.update(|cx| cx.set_global(Theme::default()));
        let diffs = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (page, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|_| AppState::new());
            let cache = Rc::new(RefCell::new(PullRequestCache::default()));
            let mut cached = snapshot("Cached");
            cached.diff = Some(ParsedDiff::new(
                "diff --git a/a b/a\n--- a/a\n+++ b/a\n@@ -1 +1 @@\n-a\n+b\n".into(),
            ));
            cache
                .borrow_mut()
                .put(None, "https://github.com/a/b/pull/1".into(), cached);
            state.update(cx, |state, _| {
                state.set_test_engine(crate::state::EngineHandle::from_test_client(
                    zeron_rpc::memory_client(Arc::new(DiffRpc(diffs.clone()))),
                ))
            });
            PullRequestDetailPage::new(
                state,
                "https://github.com/a/b/pull/1".into(),
                None,
                cache,
                None,
                window,
                cx,
            )
        });
        let settle = |cx: &mut gpui::VisualTestContext| {
            for _ in 0..100 {
                cx.run_until_parked();
                runtime.block_on(async { tokio::task::yield_now().await });
                if page.read_with(cx, |page, _| {
                    page.task.is_none() && page.diff_task.is_none()
                }) {
                    break;
                }
            }
        };
        page.update(cx, |page, cx| page.refresh(cx));
        settle(cx);
        page.update(cx, |page, cx| page.select_tab(Tab::Code, cx));
        settle(cx);
        page.update(cx, |page, cx| {
            page.select_tab(Tab::Summary, cx);
            page.diff = None;
            page.select_tab(Tab::Code, cx);
        });
        settle(cx);
        assert_eq!(
            *diffs.lock().unwrap(),
            [true, false],
            "only the first diff load after a refresh bypasses the cache"
        );
    }

    #[test]
    fn pull_request_failures_read_as_words_not_codes() {
        use zeron_rpc::{RpcError, capability_errors as codes};
        for error in [
            RpcError::Capability(codes::PULL_REQUESTS_AUTHENTICATION.into()),
            RpcError::Capability(codes::PULL_REQUESTS_DECODE.into()),
            RpcError::Transport("socket reset by peer".into()),
            RpcError::UnknownMethod(methods::GET_CHANGE_REQUEST.into()),
        ] {
            let reason = failure_reason(&error);
            assert!(!reason.contains("pull_requests."), "{reason}");
            assert!(!reason.contains("socket"), "{reason}");
            assert!(!reason.contains("GetChangeRequest"), "{reason}");
        }
        assert_eq!(
            failure_reason(&RpcError::Capability(
                codes::PULL_REQUESTS_AUTHENTICATION.into()
            )),
            "Run gh auth login on this device, then refresh."
        );
    }

    #[test]
    fn ci_summary_handles_mixed_and_unreported_checks() {
        let check = |conclusion: &str, state: &str, status: &str| zeron_proto::ChangeRequestCheck {
            conclusion: conclusion.into(),
            state: state.into(),
            status: status.into(),
            ..Default::default()
        };
        assert_eq!(ci_status(&[]), "No checks reported");
        assert_eq!(
            ci_status(&[
                check("SUCCESS", "", "COMPLETED"),
                check("SKIPPED", "", "COMPLETED")
            ]),
            "SUCCESS"
        );
        assert_eq!(
            ci_status(&[check("", "SUCCESS", ""), check("", "", "IN_PROGRESS")]),
            "PENDING"
        );
        assert_eq!(
            ci_status(&[check("FAILURE", "", "COMPLETED"), check("", "", "QUEUED")]),
            "FAILURE"
        );
        assert_eq!(ci_status(&[check("CANCELLED", "", "COMPLETED")]), "FAILURE");
        assert_eq!(ci_status(&[check("SKIPPED", "", "COMPLETED")]), "SKIPPED");
        assert_eq!(ci_status(&[check("", "", "")]), "PENDING");
    }

    #[test]
    fn status_cues_distinguish_passed_pending_failed_and_skipped() {
        assert_eq!(status_style("SUCCESS").2, StatusTone::Positive);
        assert_eq!(status_style("IN_PROGRESS").2, StatusTone::Warning);
        assert_eq!(status_style("FAILURE").2, StatusTone::Negative);
        assert_eq!(status_style("SKIPPED").2, StatusTone::Neutral);
        assert_eq!(status_style("CANCELLED").2, StatusTone::Neutral);
        assert_eq!(status_style("MERGED").2, StatusTone::Merged);
        assert_eq!(status_style("CHANGES_REQUESTED").0, "Changes requested");
        assert_eq!(status_style("REVIEW_REQUIRED").0, "Awaiting review");
    }

    #[test]
    fn pull_request_code_rows_preserve_sides_and_file_jump_offsets() {
        let patch = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,2 +1,2 @@\n context\n-old\n+new\n";
        let (rows, files) = code_rows(patch);
        assert_eq!(files, vec![("a.rs".into(), 0)]);
        let added = rows
            .iter()
            .find(|row| row.kind == crate::changes::LineKind::Add)
            .unwrap();
        assert_eq!(added.old, "");
        assert_eq!(added.new, "2");
        let removed = rows
            .iter()
            .find(|row| row.kind == crate::changes::LineKind::Del)
            .unwrap();
        assert_eq!(removed.old, "2");
        assert_eq!(removed.new, "");
    }

    #[test]
    fn pull_request_diff_reuses_syntax_highlighting() {
        let (rows, _) = code_rows(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\n",
        );
        assert!(
            rows.iter()
                .filter(|row| row.kind != crate::changes::LineKind::Meta)
                .all(|row| !row.spans.is_empty())
        );
    }

    #[gpui::test]
    fn pull_request_code_stream_navigates_folds_and_follows_layout(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::default()));
        let (host, cx) = cx.add_window_view(|window, cx| DetailHost::new(window, cx, true));
        let page = host.read_with(cx, |host, _| host.page.clone());
        page.update(cx, |page, cx| {
            let patch = ["a.rs", "nested/b.rs", "c.rs"].iter().map(|name| format!(
                "diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -1 +1 @@\n-old\n+{}\n", "new ".repeat(100)
            )).collect::<String>();
            page.install_diff(ParsedDiff::new(patch), cx);
            let cached = page.code_rows.clone();
            page.select_tab(Tab::Code, cx);
            assert_eq!(page.code_ranges.len(), 3, "every file stays in the stream");
            for index in [2, 0, 1] {
                page.select_code_file(index, cx);
                assert_eq!(page.active_file(), Some(index));
                assert!(Arc::ptr_eq(&cached, &page.code_rows));
            }
            page.select_code_file(99, cx);
            assert_eq!(page.active_file(), Some(1));
        });
        cx.simulate_resize(gpui::size(px(1200.0), px(850.0)));
        page.update(cx, |page, cx| {
            page.file_search
                .update(cx, |input, cx| input.set_text("NESTED", cx));
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert_eq!(page.file_query, "nested"));
        assert!(cx.debug_bounds("pr-file-navigation").is_some());
        // Root files lead, so `nested/b.rs` is the last file.
        assert!(cx.debug_bounds("pr-file-0").is_none());
        assert!(cx.debug_bounds("pr-file-1").is_none());
        assert!(cx.debug_bounds("pr-file-2").is_some());
        page.update(cx, |page, cx| {
            page.file_search
                .update(cx, |input, cx| input.set_text("", cx));
        });
        for (width, height) in [(320.0, 600.0), (900.0, 400.0), (1200.0, 850.0)] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            cx.run_until_parked();
            let viewport = cx.debug_bounds("pr-code-viewport").unwrap();
            let toolbar = cx.debug_bounds("pr-file-navigation").unwrap();
            let nav = cx.debug_bounds("pr-detail-nav").unwrap();
            assert!(viewport.size.height >= px(60.0), "{viewport:?}");
            assert!(viewport.bottom() <= nav.top());
            assert!(toolbar.bottom() <= viewport.top());
            assert_eq!(
                toolbar.left(),
                viewport.left(),
                "toolbar and stream share an edge"
            );
            let header = cx.debug_bounds("pr-file-header-0").unwrap();
            assert_eq!(header.size.height, px(crate::changes::FILE_HEADER_HEIGHT));
            if width >= 900.0 {
                let browser = cx.debug_bounds("pr-file-browser").unwrap();
                assert!(
                    browser.right() < viewport.left(),
                    "files stay beside the diff"
                );
                assert_eq!(
                    browser.top(),
                    toolbar.top(),
                    "tree header aligns with the toolbar"
                );
                let search = cx.debug_bounds("pr-file-search").unwrap();
                assert_eq!(search.size.height, px(crate::surface_chrome::CONTROL_SIZE));
                let files = cx.debug_bounds("pr-file-list").unwrap();
                assert!(files.top() >= search.bottom() && files.size.height > px(40.0));
                assert!(
                    files.bottom() > viewport.bottom(),
                    "the tree runs to the window edge while the diff clears the navigation"
                );
                assert!(cx.debug_bounds("pr-files").is_none());
            } else {
                assert!(
                    cx.debug_bounds("pr-file-browser").is_none(),
                    "compact stream keeps its width"
                );
                let files = cx.debug_bounds("pr-files").unwrap();
                cx.simulate_mouse_down(
                    files.center(),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                cx.simulate_mouse_up(
                    files.center(),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                cx.run_until_parked();
                let picker = cx.debug_bounds("pr-file-picker").unwrap();
                assert!(picker.top() >= toolbar.bottom() && picker.right() <= px(width));
                let file = cx.debug_bounds("pr-file-2").unwrap();
                cx.simulate_mouse_down(
                    file.center(),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                cx.simulate_mouse_up(
                    file.center(),
                    gpui::MouseButton::Left,
                    gpui::Modifiers::default(),
                );
                cx.run_until_parked();
                page.read_with(cx, |page, _| {
                    assert_eq!(page.active_file(), Some(2));
                    assert!(!page.files_expanded, "picking a file closes the picker");
                });
            }
            let toggle = cx.debug_bounds("pr-split").unwrap();
            cx.simulate_mouse_down(
                toggle.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.simulate_mouse_up(
                toggle.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            page.update(cx, |page, cx| page.select_code_file(0, cx));
            cx.run_until_parked();
            page.read_with(cx, |page, _| assert!(page.code_split));
            // Stream rows: header, hunk, then the first paired line.
            let left = cx.debug_bounds("pr-split-cell-2-true").unwrap();
            let right = cx.debug_bounds("pr-split-cell-2-false").unwrap();
            assert!(left.left() >= viewport.left());
            assert!(right.right() <= viewport.right());
            assert!((left.size.width - right.size.width).abs() < px(1.0));
            assert!(left.right() <= right.left());
            cx.simulate_mouse_down(
                toggle.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.simulate_mouse_up(
                toggle.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.run_until_parked();
            page.read_with(cx, |page, _| assert!(!page.code_split));
            for selector in [
                "pr-copy-patch",
                "pr-previous-file",
                "pr-next-file",
                "pr-split",
                "pr-fold-all",
            ] {
                let control = cx.debug_bounds(selector).unwrap();
                assert!(
                    control.left() >= px(24.0) && control.right() <= px(width - 24.0),
                    "{selector}: {control:?}"
                );
            }
        }
        let file = cx.debug_bounds("pr-file-2").unwrap();
        cx.simulate_mouse_down(
            file.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            file.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert_eq!(page.active_file(), Some(2)));
        assert!(
            cx.debug_bounds("pr-file-browser").is_some(),
            "wide file navigator remains available after selection"
        );
        page.update(cx, |page, cx| page.select_code_file(0, cx));
        cx.run_until_parked();
        let header = cx.debug_bounds("pr-file-header-0").unwrap();
        cx.simulate_mouse_down(
            header.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            header.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.collapsed_files.contains(&0));
            assert_eq!(
                page.code_ranges[0],
                0..1,
                "a folded file keeps only its header"
            );
        });
        let fold_all = cx.debug_bounds("pr-fold-all").unwrap();
        cx.simulate_mouse_down(
            fold_all.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            fold_all.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert_eq!(page.code_stream.len(), 3));
    }

    #[gpui::test]
    fn pull_request_file_jump_hides_the_header_divider_under_the_card_border(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| cx.set_global(Theme::default()));
        let (host, cx) = cx.add_window_view(|window, cx| DetailHost::new(window, cx, true));
        let page = host.read_with(cx, |host, _| host.page.clone());
        cx.simulate_resize(gpui::size(px(1200.0), px(700.0)));
        page.update(cx, |page, cx| {
            let body = (0..80).map(|n| format!("+line {n}\n")).collect::<String>();
            let patch = ["a.rs", "b.rs", "c.rs"]
                .iter()
                .map(|name| format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -0,0 +1,80 @@\n{body}"))
                .collect::<String>();
            page.install_diff(ParsedDiff::new(patch), cx);
            page.select_tab(Tab::Code, cx);
        });
        cx.run_until_parked();
        page.update(cx, |page, cx| page.select_code_file(1, cx));
        cx.run_until_parked();
        let viewport = cx.debug_bounds("pr-code-viewport").unwrap();
        let header = cx.debug_bounds("pr-file-header-1").unwrap();
        assert_eq!(
            header.top(),
            viewport.top(),
            "the jumped-to header's divider hides under the card border"
        );
        assert!(
            cx.debug_bounds("pr-sticky-file-1").is_none(),
            "no pinned copy doubles the header"
        );
    }

    struct NavigationHitHost {
        page: Entity<PullRequestDetailPage>,
        presses: usize,
        clicks: usize,
        raw_presses: Rc<std::cell::Cell<usize>>,
    }

    impl Render for NavigationHitHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let theme = Theme::of(cx).clone();
            let navigation = self.page.update(cx, |page, cx| page.navigation(&theme, cx));
            let raw = self.raw_presses.clone();
            div()
                .size_full()
                .relative()
                .child(
                    div()
                        .id("under-tabs")
                        .size_full()
                        .relative()
                        .child(
                            gpui::canvas(
                                |_, _, _| (),
                                move |bounds, _, window, _| {
                                    window.on_mouse_event(
                                        move |event: &gpui::MouseDownEvent, phase, _, _| {
                                            if phase == gpui::DispatchPhase::Bubble
                                                && bounds.contains(&event.position)
                                            {
                                                raw.set(raw.get() + 1);
                                            }
                                        },
                                    );
                                },
                            )
                            .absolute()
                            .inset_0(),
                        )
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|host, _, _, _| host.presses += 1),
                        )
                        .on_click(cx.listener(|host, _, _, _| host.clicks += 1)),
                )
                .child(div().absolute().inset_0().child(navigation))
        }
    }

    #[gpui::test]
    fn pull_request_navigation_occludes_underlying_press_and_click(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        cx.update(|cx| cx.set_global(Theme::default()));
        let (host, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|_| AppState::new());
            let page = cx.new(|cx| {
                PullRequestDetailPage::new(
                    state,
                    "https://github.com/a/b/pull/1".into(),
                    None,
                    Default::default(),
                    None,
                    window,
                    cx,
                )
            });
            NavigationHitHost {
                page,
                presses: 0,
                clicks: 0,
                raw_presses: Default::default(),
            }
        });
        cx.simulate_resize(gpui::size(px(600.0), px(400.0)));
        cx.run_until_parked();
        let tab = cx.debug_bounds("pr-code").unwrap();
        let nav = cx.debug_bounds("pr-detail-nav").unwrap();
        for point in [
            tab.center(),
            gpui::point(nav.left() + px(2.0), nav.center().y),
        ] {
            cx.simulate_mouse_down(point, gpui::MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_up(point, gpui::MouseButton::Left, gpui::Modifiers::default());
        }
        host.read_with(cx, |host, cx| {
            assert_eq!(
                host.presses, 0,
                "tabs and glass padding must block selection starts beneath them"
            );
            assert_eq!(host.clicks, 0, "tabs must block underlying links/images");
            assert_eq!(
                host.raw_presses.get(),
                0,
                "raw text-selection listeners must not receive tab presses"
            );
            assert!(host.page.read(cx).tab == Tab::Code);
        });
        let outside = gpui::point(nav.center().x, nav.bottom() + px(10.0));
        cx.simulate_mouse_down(outside, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(outside, gpui::MouseButton::Left, gpui::Modifiers::default());
        host.read_with(cx, |host, _| {
            assert_eq!(
                host.presses, 1,
                "only the glass footprint should intercept input"
            );
            assert_eq!(host.clicks, 1);
            assert_eq!(host.raw_presses.get(), 1);
        });
    }

    #[gpui::test]
    fn pull_request_navigation_thumb_slides_onto_the_selected_tab(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        cx.update(|cx| cx.set_global(Theme::default()));
        let (host, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|_| AppState::new());
            let page = cx.new(|cx| {
                PullRequestDetailPage::new(
                    state,
                    "https://github.com/a/b/pull/1".into(),
                    None,
                    Default::default(),
                    None,
                    window,
                    cx,
                )
            });
            NavigationHitHost {
                page,
                presses: 0,
                clicks: 0,
                raw_presses: Default::default(),
            }
        });
        let page = host.read_with(cx, |host, _| host.page.clone());
        cx.update(|_, cx| crate::motion::set_reduced_motion(cx, true));
        for width in [320.0, 900.0] {
            cx.simulate_resize(gpui::size(px(width), px(400.0)));
            cx.run_until_parked();
            let nav = cx.debug_bounds("pr-detail-nav").unwrap();
            assert!(
                nav.left() >= px(16.0) && nav.right() <= px(width - 16.0),
                "{nav:?}"
            );
            let segments: Vec<_> = TABS
                .iter()
                .map(|(_, _, id, _)| cx.debug_bounds(id).unwrap())
                .collect();
            for pair in segments.windows(2) {
                assert!(
                    (pair[0].size.width - pair[1].size.width).abs() <= px(1.0),
                    "{segments:?}"
                );
            }
            for (tab, slot) in [(Tab::Code, 1), (Tab::Activity, 2), (Tab::Summary, 0)] {
                page.update(cx, |page, cx| page.select_tab(tab, cx));
                cx.run_until_parked();
                let thumb = cx.debug_bounds("pr-detail-nav-thumb").unwrap();
                assert!(
                    (thumb.origin.x - segments[slot].origin.x).abs() <= px(1.0),
                    "{thumb:?}"
                );
                assert!((thumb.size.width - segments[slot].size.width).abs() <= px(1.0));
            }
        }
    }

    #[gpui::test]
    fn pull_request_overview_rows_share_one_label_column(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::default()));
        let (_, cx) = cx.add_window_view(|window, cx| DetailHost::new(window, cx, true));
        for width in [900.0, 320.0] {
            cx.simulate_resize(gpui::size(px(width), px(1000.0)));
            cx.run_until_parked();
            let rows: Vec<_> = [
                "pr-field-Status",
                "pr-field-Review",
                "pr-field-Branch",
                "pr-field-Changes",
            ]
            .iter()
            .map(|id| cx.debug_bounds(id).unwrap())
            .collect();
            for pair in rows.windows(2) {
                assert_eq!(pair[0].left(), pair[1].left(), "{rows:?}");
                assert!(pair[1].top() >= pair[0].bottom(), "{rows:?}");
            }
            // Cards stack at one rhythm, with no floating labels between them.
            let cards: Vec<_> = [
                "pr-overview",
                "pr-checks-card",
                "pr-handoff",
                "pr-description",
            ]
            .iter()
            .map(|id| cx.debug_bounds(id).unwrap())
            .collect();
            for pair in cards.windows(2) {
                assert_eq!(pair[1].top() - pair[0].bottom(), px(CARD_GAP), "{cards:?}");
            }
            let meta = cx.debug_bounds("pr-detail-meta").unwrap();
            assert_eq!(
                cards[0].top() - meta.bottom(),
                px(24.0),
                "header sits 2× the card gap above"
            );
            // Handoff actions trail the title on wide rows, inside the card.
            let row = cx.debug_bounds("pr-handoff-row").unwrap();
            let actions = cx.debug_bounds("pr-handoff-actions").unwrap();
            assert!(
                actions.right() <= cards[2].right() - px(16.0),
                "{actions:?}"
            );
            if width > 600.0 {
                assert!(actions.top() >= row.top() && actions.bottom() <= row.bottom());
                assert!(
                    actions.size.height <= px(34.0),
                    "one line of actions: {actions:?}"
                );
            }
            // The long fixture title wraps inside the column instead of clipping.
            let title = cx.debug_bounds("pr-detail-title").unwrap();
            assert!(title.right() <= cards[0].right(), "{title:?}");
            if width < 600.0 {
                assert!(
                    title.size.height > px(30.0),
                    "narrow titles wrap: {title:?}"
                );
            }
        }
    }

    struct DetailHost {
        page: Entity<PullRequestDetailPage>,
        _subscription: Subscription,
        returned: bool,
    }

    impl DetailHost {
        fn new(window: &mut Window, cx: &mut Context<Self>, loaded: bool) -> Self {
            let state = cx.new(|_| AppState::new());
            let page = cx.new(|cx| {
                let mut page = PullRequestDetailPage::new(
                    state, "https://github.com/a/b/pull/1".into(),
                    Some("remote-device".into()),
                    Rc::new(RefCell::new(PullRequestCache::default())), None, window, cx,
                );
                if loaded {
                    page.error = None;
                    page.detail = Some(ChangeRequestDetail {
                        title: "Inspect a pull request with a very long title that must leave room for all actions".into(),
                        number: 1, body: "A description".into(),
                        status_check_rollup: vec![zeron_proto::ChangeRequestCheck {
                            name: "linux-browser".into(), conclusion: "SUCCESS".into(),
                            details_url: "https://github.com/a/b/actions/runs/1".into(),
                            ..Default::default()
                        }], ..Default::default()
                    });
                    page.body = Some(crate::markdown::parse_full("A description"));
                    let review = "### Review notes\n\n**Strong** text and [a link](https://github.com).\n\n```rust\nlet answer = 42;\n```";
                    page.detail.as_mut().unwrap().reviews.push(zeron_proto::ChangeRequestComment {
                        body: review.into(), ..Default::default()
                    });
                    page.detail.as_mut().unwrap().comments.push(zeron_proto::ChangeRequestComment {
                        viewer_did_author: true, body: "My comment".into(),
                        author: zeron_proto::ChangeRequestActor { login: "viewer".into() }, ..Default::default()
                    });
                    page.activity_bodies = vec![
                        super::super::pull_request_media::parse_description("My comment"),
                        super::super::pull_request_media::parse_description(review),
                    ];
                    let patch = format!("diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-old\n+{}\n", "long_expression_".repeat(30));
                    page.install_diff(ParsedDiff::new(patch), cx);
                }
                page
            });
            let subscription = cx.observe(&page, |_, _, cx| cx.notify());
            Self {
                page,
                _subscription: subscription,
                returned: false,
            }
        }
    }

    impl Render for DetailHost {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let header = self.page.update(cx, |page, cx| page.titlebar(cx));
            div()
                .size_full()
                .relative()
                .on_action(cx.listener(|host, _: &ClosePullRequest, _, cx| {
                    host.returned = true;
                    cx.notify();
                }))
                .child(self.page.clone())
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .w_full()
                        .h(px(Theme::TITLEBAR_HEIGHT))
                        .flex()
                        .items_center()
                        .child(header),
                )
        }
    }

    #[gpui::test]
    fn pull_request_detail_actions_fit_and_tabs_switch(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::default()));
        let (host, cx) = cx.add_window_view(|window, cx| DetailHost::new(window, cx, true));
        let page = host.read_with(cx, |host, _| host.page.clone());
        page.read_with(cx, |page, _| {
            assert_eq!(page.params(false)["targetDeviceId"], "remote-device")
        });
        for width in [240.0, 320.0, 600.0, 900.0] {
            cx.simulate_resize(gpui::size(px(width), px(800.0)));
            cx.run_until_parked();
            let mut previous_right = px(0.0);
            for selector in ["pr-back", "pr-detail-refresh", "pr-copy-url", "pr-external"] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert!(
                    bounds.left() >= previous_right && bounds.right() <= px(width),
                    "{selector}: {bounds:?}"
                );
                assert!(bounds.top() >= px(0.0) && bounds.bottom() <= px(Theme::TITLEBAR_HEIGHT));
                previous_right = bounds.right();
            }
            assert!(cx.debug_bounds("pr-immersive").is_none());
            assert!(cx.debug_bounds("pr-browser").is_none());
            let nav = cx.debug_bounds("pr-detail-nav").unwrap();
            assert_eq!(nav.bottom(), px(784.0));
            assert_eq!(nav.size.height, px(44.0));
            assert!(cx.debug_bounds("pr-checks").is_none());
            for selector in ["pr-summary", "pr-code", "pr-activity"] {
                let bounds = cx.debug_bounds(selector).unwrap();
                assert!(
                    bounds.left() >= px(0.0) && bounds.right() <= px(width),
                    "{selector}: {bounds:?}"
                );
            }
        }
        let checks = cx.debug_bounds("pr-checks-toggle").unwrap();
        cx.simulate_mouse_down(
            checks.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            checks.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.tab == Tab::Summary && page.checks_expanded)
        });
        let check = cx.debug_bounds("pr-check-0").unwrap();
        let check_name = cx.debug_bounds("pr-check-name-0").unwrap();
        let card = cx.debug_bounds("pr-checks-card").unwrap();
        assert_eq!(check.left(), card.left());
        assert_eq!(check_name.left() - check.left(), px(16.0));
        assert!(check.size.height <= px(40.0));
        assert!(check_name.top() > check.top() && check_name.bottom() < check.bottom());
        let code = cx.debug_bounds("pr-code").unwrap();
        cx.simulate_mouse_down(
            code.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            code.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert!(page.tab == Tab::Code));
        assert!(cx.debug_bounds("pr-copy-patch").is_some());
        for (width, height) in [(320.0, 600.0), (900.0, 400.0), (1200.0, 900.0)] {
            cx.simulate_resize(gpui::size(px(width), px(height)));
            cx.run_until_parked();
            let viewport = cx.debug_bounds("pr-code-viewport").unwrap();
            let nav = cx.debug_bounds("pr-detail-nav").unwrap();
            assert!(viewport.size.height > px(60.0), "{viewport:?}");
            assert!(
                viewport.bottom() <= nav.top(),
                "diff must clear floating navigation"
            );
            assert_eq!(
                viewport.size.width,
                px(width - 48.0 - if width - 48.0 >= 760.0 { 276.0 } else { 0.0 }),
                "Code shares available width with the file navigator"
            );
            assert!(nav.size.width <= px(width));
            assert_eq!(nav.size.height, px(44.0));
            page.read_with(cx, |page, _| {
                assert_eq!(
                    page.scroll.scroll.max_offset().y,
                    px(0.0),
                    "Code has one vertical scroller"
                );
            });
        }
        cx.simulate_resize(gpui::size(px(900.0), px(800.0)));
        cx.run_until_parked();
        let copy = cx.debug_bounds("pr-copy-url").unwrap();
        cx.simulate_mouse_down(
            copy.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            copy.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.update(|_, cx| {
            assert_eq!(
                cx.read_from_clipboard().unwrap().text().unwrap(),
                "https://github.com/a/b/pull/1"
            )
        });
        page.read_with(cx, |page, _| {
            assert!(page.code_horizontal.max_offset().x > px(0.0))
        });
        cx.simulate_resize(gpui::size(px(600.0), px(800.0)));
        cx.run_until_parked();
        let files = cx.debug_bounds("pr-files").unwrap();
        cx.simulate_mouse_down(
            files.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            files.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        page.read_with(cx, |page, _| assert!(page.files_expanded));
        let activity = cx.debug_bounds("pr-activity").unwrap();
        cx.simulate_mouse_down(
            activity.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            activity.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert!(page.tab == Tab::Activity));
        assert!(cx.debug_bounds("pr-rich-text").is_some());
        for width in [320.0, 900.0, 1200.0] {
            cx.simulate_resize(gpui::size(px(width), px(800.0)));
            cx.run_until_parked();
            let column = cx.debug_bounds("pr-content-column").unwrap();
            let composer = cx.debug_bounds("pr-comment-surface").unwrap();
            let nav = cx.debug_bounds("pr-detail-nav").unwrap();
            let meta = cx.debug_bounds("pr-detail-meta").unwrap();
            let thread = cx.debug_bounds("pr-activity-thread").unwrap();
            assert_eq!(
                thread.top() - meta.bottom(),
                px(24.0),
                "header gap matches Summary"
            );
            let mine = cx.debug_bounds("pr-message-0").unwrap();
            let other = cx.debug_bounds("pr-message-1").unwrap();
            assert_eq!(composer.left(), column.left() + px(40.0));
            assert_eq!(composer.right(), column.right() - px(40.0));
            assert_eq!(nav.center().x, composer.center().x);
            assert_eq!(
                nav.top() - composer.bottom(),
                px(16.0),
                "docked above the navigation"
            );
            let scroll = cx.debug_bounds("pr-detail-scroll").unwrap();
            assert!(
                scroll.bottom() <= composer.top(),
                "the thread never scrolls behind it"
            );
            if width >= 900.0 {
                assert_eq!(
                    composer.size.height,
                    px(crate::composer::COMPACT_TOTAL_HEIGHT),
                    "one line matches the chat composer's compact pill"
                );
                let send = cx.debug_bounds("pr-send-comment").unwrap();
                assert_eq!(send.center().y, composer.center().y);
                assert_eq!(composer.right() - send.right(), px(9.0));
            }
            assert_eq!(mine.right(), composer.right());
            assert_eq!(other.left(), composer.left() + px(8.0), "title's text edge");
            assert!(mine.left() > other.left());
        }

        cx.simulate_resize(gpui::size(px(900.0), px(400.0)));
        cx.run_until_parked();
        let nav = cx.debug_bounds("pr-detail-nav").unwrap();
        page.update(cx, |page, cx| {
            page.scroll
                .scroll
                .set_offset(gpui::point(px(0.0), px(-150.0)));
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(
            cx.debug_bounds("pr-detail-nav").unwrap(),
            nav,
            "navigation must not move with content"
        );
        page.read_with(cx, |page, _| {
            assert!(page.scroll.scroll.offset().y < px(0.0))
        });
        let title = cx.debug_bounds("pr-back-title").unwrap();
        let back = cx.debug_bounds("pr-back").unwrap();
        assert!(title.size.width > px(100.0) && back.contains(&title.center()));
        cx.simulate_mouse_down(
            title.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            title.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        host.read_with(cx, |host, _| {
            assert!(host.returned, "the title is part of the back action")
        });
    }

    #[gpui::test]
    fn pull_request_error_keeps_retry_and_browser_actions(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::default()));
        let (_, cx) = cx.add_window_view(|window, cx| DetailHost::new(window, cx, false));
        cx.simulate_resize(gpui::size(px(320.0), px(800.0)));
        cx.run_until_parked();
        for selector in ["pr-detail-refresh", "pr-external"] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector}");
        }
    }
}
