use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Instant,
};

use chrono::{DateTime, Utc};
use gpui::{
    AnyElement, Context, Entity, Focusable, IntoElement, Render, ScrollHandle, SharedString,
    Subscription, Task, Window, div, prelude::*, px,
};
use zeron_proto::{
    ChangeRequestFilter, ChangeRequestListItem, ChangeRequestMergeability, ChangeRequestPage,
    ChangeRequestReviewDecision, Device,
};
use zeron_rpc::{RpcError, capability_errors, methods};

use zeron_proto::change_request_assessment::{Blocker, CiState, Facts};

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::popover;
use crate::settings::widgets;
use crate::state::AppState;
use crate::theme::Theme;

const PR_PAGE_MAX_WIDTH: f32 = 760.0;
const PR_PAGE_HORIZONTAL_PADDING: f32 = 40.0;
const PR_TABLE_ROW_HEIGHT: f32 = 64.0;
const PR_SCROLL_FADE_BAND: f32 = 24.0;

/// Where the loaded listing stands relative to everything GitHub matched.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Paging {
    next_cursor: Option<String>,
    total: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PullRequestsPageError {
    CliUnavailable,
    Authentication,
    RateLimited,
    Network,
    RemoteOffline(String),
    UpdateRequired(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PullRequestsLoadState {
    Idle,
    Loading,
    Ready,
    Failed(PullRequestsPageError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PullRequestSortField {
    Changes,
    Opened,
    Updated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PullRequestSort {
    field: PullRequestSortField,
    direction: SortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum PullRequestGroup {
    Attention,
    Review,
    Approved,
    Drafts,
}

impl PullRequestGroup {
    const ALL: [Self; 4] = [Self::Attention, Self::Review, Self::Approved, Self::Drafts];
    fn label(self) -> &'static str {
        match self {
            Self::Attention => "Needs attention",
            Self::Review => "Awaiting review",
            Self::Approved => "Approved",
            Self::Drafts => "Drafts",
        }
    }
    fn key(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Review => "review",
            Self::Approved => "approved",
            Self::Drafts => "drafts",
        }
    }
}

fn request_group(item: &ChangeRequestListItem) -> PullRequestGroup {
    let assessment = Facts::from_item(item).assess();
    if item.is_draft {
        PullRequestGroup::Drafts
    } else if !assessment.blockers.is_empty() || !assessment.attention.is_empty() {
        PullRequestGroup::Attention
    } else if item.review_decision == ChangeRequestReviewDecision::Approved {
        PullRequestGroup::Approved
    } else {
        PullRequestGroup::Review
    }
}

fn request_glyph(item: &ChangeRequestListItem) -> &'static str {
    if item.is_draft {
        icons::PULL_REQUEST_DRAFT
    } else {
        icons::PULL_REQUEST
    }
}

fn request_color(item: &ChangeRequestListItem, theme: &Theme) -> gpui::Hsla {
    if item.is_draft {
        theme.text_muted
    } else if item.mergeability == ChangeRequestMergeability::Conflicting {
        theme.danger
    } else if item.review_decision == ChangeRequestReviewDecision::ChangesRequested {
        theme.warning
    } else {
        theme.success
    }
}

fn matches_query(item: &ChangeRequestListItem, query: &str) -> bool {
    let text = format!(
        "{} {} #{} {} {}",
        item.title,
        item.repository,
        item.number,
        status_description(item),
        item.author.login
    )
    .to_lowercase();
    query
        .split_whitespace()
        .all(|word| text.contains(&word.to_lowercase()))
}

impl PullRequestSort {
    const DEFAULT: Self = Self {
        field: PullRequestSortField::Updated,
        direction: SortDirection::Descending,
    };
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum PersonalView {
    #[default]
    Everything,
    NeedsYou,
    Starred,
}

/// Stable across devices and remote spellings; stars are local preferences.
pub(crate) fn star_key(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.trim_end_matches('/').to_ascii_lowercase();
    };
    parsed.set_fragment(None);
    parsed.set_query(None);
    let path = parsed.path().trim_end_matches('/').to_owned();
    parsed.set_path(&path);
    parsed.to_string().to_ascii_lowercase()
}

pub(crate) fn star_button(url: &str, theme: &Theme, cx: &gpui::App) -> gpui::Stateful<gpui::Div> {
    let key = star_key(url);
    let starred = crate::settings::current(cx)
        .pull_request_stars
        .contains(&key);
    let label = if starred {
        "Unstar pull request"
    } else {
        "Star pull request"
    };
    div()
        .id(SharedString::from(format!("pr-star-{key}")))
        .debug_selector(|| "pr-star".into())
        .role(gpui::Role::Button)
        .aria_label(label)
        .aria_toggled(if starred {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
        .tab_index(0)
        .size(px(28.0))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .border_1()
        .border_color(gpui::transparent_black())
        .focus_visible(|style| style.border_2().border_color(theme.accent))
        .cursor_pointer()
        .hover(|style| style.bg(theme.glass_hover()))
        .tooltip(widgets::text_tooltip(label))
        .child(
            icon(if starred {
                icons::STAR_BOLD
            } else {
                icons::STAR
            })
            .size(px(14.0))
            .text_color(if starred {
                theme.warning
            } else {
                theme.text_muted
            }),
        )
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            if crate::settings::update(crate::settings::SavePolicy::Debounced, cx, |settings| {
                if settings.pull_request_stars.contains(&key) {
                    settings.pull_request_stars.retain(|value| value != &key);
                } else {
                    settings.pull_request_stars.push(key.clone());
                }
            }) {
                cx.refresh_windows();
            }
        })
}

/// Repository-scoped dashboard with lazy, cached relationship filters.
pub struct PullRequestsPage {
    state: Entity<AppState>,
    search: Entity<ComposerInput>,
    query: String,
    repository_input: Entity<ComposerInput>,
    repository: Option<String>,
    repository_error: Option<String>,
    initial_scope_attempted: bool,
    initial_scope_task: Option<Task<()>>,
    resolving_project: Option<ProjectRef>,
    project_repositories: HashMap<ProjectRef, String>,
    project_icons: crate::shell::project_icon::ProjectIcons,
    /// The project the board was last opened from. Opening it from another
    /// project follows that project; an explicit choice stands until then.
    seen_context: Option<ProjectRef>,
    _repository_events: Subscription,
    snapshots: Vec<(
        (Option<String>, String, ChangeRequestFilter),
        Vec<ChangeRequestListItem>,
        Instant,
        Paging,
    )>,
    paging: Paging,
    /// The next page, while it loads; `None` when idle.
    more_task: Option<Task<()>>,
    more_error: Option<PullRequestsPageError>,
    selected_url: Option<String>,
    collapsed_groups: HashSet<PullRequestGroup>,
    group_motions: HashMap<PullRequestGroup, crate::motion::DisclosureMotion>,
    group_heights: HashMap<PullRequestGroup, Rc<Cell<f32>>>,
    _search_events: Subscription,
    /// `None` keeps local calls direct; a value is forwarded by the relay.
    target_device: Option<String>,
    items: Vec<ChangeRequestListItem>,
    view_items: Option<Rc<Vec<ChangeRequestListItem>>>,
    sort: PullRequestSort,
    filter: ChangeRequestFilter,
    personal_view: PersonalView,
    /// A new repository starts on Authored; only this automatic choice may
    /// fall back to All. Explicit choices and settled defaults survive visits.
    automatic_filter: bool,
    scope_filters: Vec<((Option<String>, String), ChangeRequestFilter)>,
    filter_fades: crate::motion::HoverFades,
    load_state: PullRequestsLoadState,
    last_loaded_at: Option<Instant>,
    /// Whether the first page bypassed the engine's cache.
    refreshed: bool,
    generation: u64,
    request_task: Option<Task<()>>,
    visible: bool,
    scroll: widgets::PageScroll,
    content_width: Option<f32>,
    device_menu: popover::Popup<()>,
    sort_menu: popover::Popup<()>,
    repository_menu: popover::Popup<()>,
    repository_scroll: widgets::PageScroll,
    /// Menu triggers, so a menu closed from the keyboard hands focus back.
    repository_focus: gpui::FocusHandle,
    sort_focus: gpui::FocusHandle,
    device_focus: gpui::FocusHandle,
    /// The current option of an open sort or device menu, so a menu opened
    /// from the keyboard starts there and Tab walks its options.
    option_focus: gpui::FocusHandle,
    return_focus: Option<gpui::FocusHandle>,
    _observe: Subscription,
}

impl PullRequestsPage {
    pub(crate) fn preview(&self, url: &str) -> Option<ChangeRequestListItem> {
        self.items.iter().find(|item| item.url == url).cloned()
    }

    pub(crate) fn select_url(&mut self, url: Option<String>, cx: &mut Context<Self>) {
        self.selected_url = url;
        cx.notify();
    }
    pub(crate) fn target_device(&self) -> Option<String> {
        self.target_device.clone()
    }
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            ComposerInput::with_context("Search pull requests", "PaletteSearch", cx)
                .with_text_metrics(12.0, 16.0)
                .with_accessibility_role(gpui::Role::SearchInput)
                .with_single_line()
                .with_tab_stop()
        });
        let search_events = cx.subscribe(&search, |page: &mut Self, input, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                page.query = input.read(cx).text().to_string();
                page.view_items = None;
                page.scroll.scroll.set_offset(gpui::Point::default());
                page.collapsed_groups.clear();
                page.group_motions.clear();
                cx.notify();
            }
        });
        let repository_input = cx.new(|cx| {
            ComposerInput::with_context("owner/repository", "PaletteSearch", cx)
                .with_text_metrics(12.0, 16.0)
                .with_single_line()
                .with_tab_stop()
        });
        let repository_events =
            cx.subscribe(
                &repository_input,
                |page: &mut Self, _, event, cx| match event {
                    ComposerInputEvent::Submitted => {
                        page.return_focus = Some(page.repository_focus.clone());
                        page.select_repository(cx);
                    }
                    ComposerInputEvent::Edited => {
                        page.cancel_project_detection();
                        page.repository_error = None;
                        cx.notify();
                    }
                    _ => {}
                },
            );
        let observe = cx.observe(&state, |page, _, cx| {
            if page.visible {
                page.reconcile_target_device(cx);
                page.initialize_repository(cx);
                cx.notify();
            }
        });
        let mut filter_fades = crate::motion::HoverFades::default();
        filter_fades.set_at("pr-filter-authored", true, true, Instant::now());
        let mut page = Self {
            state,
            search,
            query: String::new(),
            repository_input,
            repository: None,
            repository_error: None,
            initial_scope_attempted: false,
            initial_scope_task: None,
            resolving_project: None,
            project_repositories: HashMap::new(),
            project_icons: Default::default(),
            seen_context: None,
            _repository_events: repository_events,
            snapshots: Vec::new(),
            paging: Paging::default(),
            more_task: None,
            more_error: None,
            selected_url: None,
            collapsed_groups: HashSet::new(),
            group_motions: HashMap::new(),
            group_heights: PullRequestGroup::ALL
                .into_iter()
                .map(|group| (group, Rc::new(Cell::new(0.0))))
                .collect(),
            _search_events: search_events,
            target_device: None,
            items: Vec::new(),
            view_items: None,
            sort: PullRequestSort::DEFAULT,
            filter: ChangeRequestFilter::Authored,
            personal_view: PersonalView::Everything,
            automatic_filter: true,
            scope_filters: Vec::new(),
            filter_fades,
            load_state: PullRequestsLoadState::Idle,
            last_loaded_at: None,
            refreshed: false,
            generation: 0,
            request_task: None,
            // The entity is created lazily only while this route is active.
            visible: true,
            scroll: widgets::PageScroll::default(),
            content_width: None,
            device_menu: popover::Popup::default(),
            sort_menu: popover::Popup::default(),
            repository_menu: popover::Popup::default(),
            repository_scroll: widgets::PageScroll::default(),
            repository_focus: cx.focus_handle().tab_stop(true),
            sort_focus: cx.focus_handle().tab_stop(true),
            device_focus: cx.focus_handle().tab_stop(true),
            option_focus: cx.focus_handle().tab_stop(true),
            return_focus: None,
            _observe: observe,
        };
        page.initialize_repository(cx);
        page
    }

    /// Pick the board's repository without asking: the project it was opened
    /// from (sidebar project, else the open session's), else the repository
    /// viewed last, else the most recently active local Git project.
    fn initialize_repository(&mut self, cx: &mut Context<Self>) {
        if self.repository.is_some() || self.initial_scope_attempted || !self.visible {
            return;
        }
        let state = self.state.read(cx);
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let saved = crate::settings::current(cx);
        let context = match context_project(state, saved.space_filter.as_deref()) {
            // Wait for the project's first frame, rather than pinning a stale scope.
            ContextProject::Waiting => return,
            ContextProject::Project(project) => Some(project),
            ContextProject::None => None,
        };
        let local = state
            .local_device_id
            .clone()
            .unwrap_or_else(|| engine.engine_info().device_id.clone());
        let fallback = saved
            .last_pull_request_repository
            .filter(|repo| valid_repository_filter(repo))
            .map(|repo| (repo, saved.last_pull_request_device));
        let candidates =
            match &context {
                Some(project) => {
                    let mut candidates = vec![project.clone()];
                    if let Some(space) = state.spaces.iter().find(|space| {
                        space.path == project.path && space.device_id == project.device
                    }) {
                        candidates.extend(
                            state
                                .project_members(space)
                                .into_iter()
                                .filter(|space| space.git_detected)
                                .map(|space| ProjectRef {
                                    path: space.path.clone(),
                                    device: space.device_id.clone(),
                                })
                                .filter(|candidate| candidate != project),
                        );
                    }
                    candidates.truncate(4);
                    candidates
                }
                None if fallback.is_some() => Vec::new(),
                None => recent_git_projects(state, &local),
            };
        self.seen_context = context;
        if candidates.is_empty() {
            if let Some((repo, target)) = fallback {
                self.apply_initial_repository(repo, target, cx);
            }
            return;
        }
        let explicit_context = self.seen_context.is_some();
        self.initial_scope_attempted = true;
        self.load_state = PullRequestsLoadState::Loading;
        self.initial_scope_task = Some(cx.spawn(async move |this, cx| {
            let mut discovery_failed = false;
            let mut found = None;
            for project in candidates {
                let target = (project.device != local).then(|| project.device.clone());
                let mut params = params_for_target(target.as_deref());
                params["cwd"] = project.path.clone().into();
                let request = engine.client().call(methods::GET_CHANGE_REQUEST_REPOSITORY, params);
                let deadline = cx.background_executor().timer(std::time::Duration::from_secs(3));
                futures::pin_mut!(request, deadline);
                let result = match futures::future::select(request, deadline).await {
                    futures::future::Either::Left((result, _)) => result,
                    futures::future::Either::Right(_) => Err(RpcError::Failed("Repository detection timed out".into())),
                };
                discovery_failed |= result.is_err();
                if let Some(repository) = result
                    .ok()
                    .and_then(|value| serde_json::from_value::<Option<String>>(value).ok())
                    .flatten()
                    .filter(|repo| valid_repository_filter(repo))
                {
                    found = Some((repository, target, project));
                    break;
                }
            }
            let _ = this.update(cx, |page, cx| {
                page.initial_scope_task = None;
                // User selection cancels this task; never override their scope.
                if page.repository.is_some() {
                    return;
                }
                page.load_state = PullRequestsLoadState::Idle;
                let found = found.map(|(repo, target, project)| {
                    page.project_repositories.insert(project, repo.clone());
                    (repo, target)
                });
                if let Some((repo, target)) = found.or(fallback) {
                    page.apply_initial_repository(repo, target, cx);
                } else if discovery_failed && explicit_context {
                    page.repository_error = Some("Couldn’t detect this project’s repository. Choose a repository to continue.".into());
                }
                cx.notify();
            });
        }));
    }

    /// Opened from a different project than last time: show that project's
    /// pull requests instead of whatever the board held.
    fn follow_context(&mut self, cx: &mut Context<Self>) {
        if self.repository.is_none() {
            return;
        }
        let saved = crate::settings::current(cx);
        let ContextProject::Project(project) =
            context_project(self.state.read(cx), saved.space_filter.as_deref())
        else {
            return;
        };
        if self.seen_context.as_ref() == Some(&project) {
            return;
        }
        self.cancel_project_detection();
        self.repository = None;
        self.automatic_filter = true;
        self.initial_scope_task = None;
        self.initial_scope_attempted = false;
        self.repository_error = None;
        self.reset_for_target(self.target_device.clone(), cx);
    }

    fn apply_initial_repository(
        &mut self,
        repository: String,
        target: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.target_device = target;
        self.repository_input
            .update(cx, |input, cx| input.set_text(&repository, cx));
        self.select_repository(cx);
    }

    pub(crate) fn on_project_scope_changed(&mut self, cx: &mut Context<Self>) {
        self.cancel_project_detection();
        if self.visible {
            self.follow_context(cx);
        }
        if self.repository.is_none() {
            self.initial_scope_task = None;
            self.initial_scope_attempted = false;
            self.load_state = PullRequestsLoadState::Idle;
            self.repository_error = None;
            self.initialize_repository(cx);
            cx.notify();
        }
    }

    /// Called whenever shell navigation makes the already-owned entity visible again.
    pub fn on_visible(&mut self, cx: &mut Context<Self>) {
        self.visible = true;
        self.reconcile_target_device(cx);
        // Returning from the same project reuses the board as it was.
        self.follow_context(cx);
        self.initialize_repository(cx);
        cx.notify();
    }

    /// Keep the retained route entity dormant while another outlet is active.
    pub fn on_hidden(&mut self) {
        self.cancel_project_detection();
        self.visible = false;
        self.sort_menu = popover::Popup::default();
        self.repository_menu = popover::Popup::default();
        if self.initial_scope_task.take().is_some() {
            self.initial_scope_attempted = false;
            self.load_state = PullRequestsLoadState::Idle;
        }
    }

    fn close_device_menu(&mut self, cx: &mut Context<Self>) {
        if self.device_menu.begin_close() {
            popover::reap_popup(cx, |page: &mut Self| &mut page.device_menu);
            cx.notify();
        }
    }

    fn set_target_device(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.cancel_project_detection();
        self.close_device_menu(cx);
        if self.target_device == target {
            return;
        }

        self.initial_scope_task = None;
        self.initial_scope_attempted = true;
        self.reset_for_target(target, cx);
        cx.notify();
    }

    fn reset_for_target(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.request_task = None;
        self.target_device = target;
        let remembered = self.repository.as_ref().and_then(|repository| {
            self.scope_filters
                .iter()
                .find(|((target, repo), _)| {
                    target == &self.target_device && repo.eq_ignore_ascii_case(repository)
                })
                .map(|(_, filter)| *filter)
        });
        self.automatic_filter = remembered.is_none();
        self.set_filter(remembered.unwrap_or(ChangeRequestFilter::Authored), cx);
        self.collapsed_groups.clear();
        self.group_motions.clear();
        self.items.clear();
        self.paging = Paging::default();
        self.more_task = None;
        self.more_error = None;
        self.view_items = None;
        self.load_state = PullRequestsLoadState::Idle;
        self.last_loaded_at = None;
        self.scroll.scroll.set_offset(gpui::Point::default());
        self.restore_snapshot();
    }

    fn restore_snapshot(&mut self) {
        let Some(repository) = &self.repository else {
            return;
        };
        if let Some(index) = self
            .snapshots
            .iter()
            .position(|((target, repo, filter), ..)| {
                target == &self.target_device
                    && repo.eq_ignore_ascii_case(repository)
                    && filter == &self.filter
            })
        {
            let snapshot = self.snapshots.remove(index);
            self.items = snapshot.1.clone();
            self.last_loaded_at = Some(snapshot.2);
            self.paging = snapshot.3.clone();
            self.load_state = PullRequestsLoadState::Ready;
            self.snapshots.push(snapshot);
        }
    }

    fn select_repository(&mut self, cx: &mut Context<Self>) {
        self.cancel_project_detection();
        let repository = self
            .repository_input
            .read(cx)
            .text()
            .trim()
            .to_ascii_lowercase();
        if !valid_repository_filter(&repository) {
            self.repository_error = Some("Enter a repository as owner/name.".into());
            cx.notify();
            return;
        }
        self.repository_error = None;
        self.close_repository_menu(cx);
        self.initial_scope_task = None;
        self.initial_scope_attempted = true;
        let saved_repository = repository.clone();
        let saved_device = self.target_device.clone();
        crate::settings::update(crate::settings::SavePolicy::Debounced, cx, |settings| {
            settings.last_pull_request_repository = Some(saved_repository);
            settings.last_pull_request_device = saved_device;
        });
        // GitHub's casing, adopted after a load, names the same repository.
        if !self
            .repository
            .as_deref()
            .is_some_and(|current| current.eq_ignore_ascii_case(&repository))
        {
            let unscoped_choice = self.repository.is_none() && !self.automatic_filter;
            self.repository = Some(repository);
            if unscoped_choice {
                self.remember_filter(self.filter);
            }
            self.reset_for_target(self.target_device.clone(), cx);
        }
        if self.last_loaded_at.is_none() {
            self.load(false, cx);
        }
        cx.notify();
    }

    /// Heal a selected remote that is no longer present in an authoritative
    /// device frame. The local row proves the frame has landed, so the empty
    /// pre-sync list cannot accidentally discard a valid remote selection.
    fn reconcile_target_device(&mut self, cx: &mut Context<Self>) -> bool {
        let normalized = {
            let state = self.state.read(cx);
            normalized_target_device(
                self.target_device.as_deref(),
                &state.devices,
                state.local_device_id.as_deref(),
            )
        };
        if normalized == self.target_device {
            return false;
        }
        self.cancel_project_detection();
        self.close_device_menu(cx);
        self.reset_for_target(normalized, cx);
        true
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.repository.is_none() && self.initial_scope_task.is_none() {
            self.initial_scope_attempted = false;
            self.repository_error = None;
            self.initialize_repository(cx);
        }
        if !matches!(self.load_state, PullRequestsLoadState::Loading) {
            self.load(true, cx);
        }
    }

    fn select_filter(&mut self, filter: ChangeRequestFilter, cx: &mut Context<Self>) {
        self.close_sort_menu(cx);
        self.automatic_filter = false;
        self.remember_filter(filter);
        if self.filter == filter {
            return;
        }
        self.set_filter(filter, cx);
        if self.repository.is_none() {
            cx.notify();
            return;
        }
        self.reset_for_target(self.target_device.clone(), cx);
        if self.last_loaded_at.is_none() {
            self.load(false, cx);
        }
        cx.notify();
    }

    fn remember_filter(&mut self, filter: ChangeRequestFilter) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        self.scope_filters.retain(|((target, repo), _)| {
            target != &self.target_device || !repo.eq_ignore_ascii_case(&repository)
        });
        self.scope_filters
            .push(((self.target_device.clone(), repository), filter));
        if self.scope_filters.len() > 12 {
            self.scope_filters.remove(0);
        }
    }

    fn set_filter(&mut self, filter: ChangeRequestFilter, cx: &mut Context<Self>) {
        for (candidate, key) in [
            (ChangeRequestFilter::Authored, "pr-filter-authored"),
            (ChangeRequestFilter::Reviewing, "pr-filter-reviewing"),
            (ChangeRequestFilter::All, "pr-filter-all"),
        ] {
            self.filter_fades.set_at(
                key,
                candidate == filter,
                crate::motion::reduced_motion(cx),
                Instant::now(),
            );
        }
        self.filter = filter;
    }

    fn close_repository_menu(&mut self, cx: &mut Context<Self>) {
        self.cancel_project_detection();
        if self.repository_menu.begin_close() {
            popover::reap_popup(cx, |page: &mut Self| &mut page.repository_menu);
            cx.notify();
        }
    }

    fn cancel_project_detection(&mut self) {
        if self.resolving_project.take().is_some() {
            self.initial_scope_task = None;
            if self.repository.is_none() {
                self.load_state = PullRequestsLoadState::Idle;
            }
        }
    }

    fn select_project(&mut self, path: String, device: String, cx: &mut Context<Self>) {
        self.cancel_project_detection();
        let state = self.state.read(cx);
        let Some(engine) = state.engine().cloned() else {
            self.repository_error =
                Some("Connect to a device to open its project repository.".into());
            cx.notify();
            return;
        };
        if !state.device_online(&device, Utc::now()) {
            self.repository_error =
                Some("This device is offline. Reconnect it to open the project.".into());
            cx.notify();
            return;
        }
        let local = state
            .local_device_id
            .as_deref()
            .unwrap_or(&engine.engine_info().device_id);
        let target = (device != local).then(|| device.clone());
        let project = ProjectRef { path, device };
        let mut params = params_for_target(target.as_deref());
        params["cwd"] = project.path.clone().into();
        self.repository_error = None;
        self.initial_scope_attempted = true;
        self.project_repositories.remove(&project);
        self.resolving_project = Some(project.clone());
        self.initial_scope_task = Some(cx.spawn(async move |this, cx| {
            let request = engine
                .client()
                .call(methods::GET_CHANGE_REQUEST_REPOSITORY, params);
            let deadline = cx
                .background_executor()
                .timer(std::time::Duration::from_secs(3));
            futures::pin_mut!(request, deadline);
            let repository = match futures::future::select(request, deadline).await {
                futures::future::Either::Left((Ok(value), _)) => {
                    serde_json::from_value::<Option<String>>(value)
                        .map_err(|_| "The device returned an unreadable repository. Try again.")
                        .and_then(|repo| repo.filter(|repo| valid_repository_filter(repo))
                            .ok_or("This project has no GitHub remote. Enter owner/repository to open it directly."))
                }
                futures::future::Either::Left((Err(_), _)) => Err("Couldn’t read this project’s repository. Check the device connection and try again."),
                futures::future::Either::Right(_) => Err("Repository detection timed out. Try again or enter owner/repository."),
            };
            let _ = this.update(cx, |page, cx| {
                if page.resolving_project.as_ref() != Some(&project) {
                    return;
                }
                page.initial_scope_task = None;
                page.resolving_project = None;
                match repository {
                    Ok(repository) => {
                        page.project_repositories.insert(project, repository.clone());
                        if page.target_device != target {
                            page.reset_for_target(target.clone(), cx);
                        }
                        page.return_focus = Some(page.repository_focus.clone());
                        page.apply_initial_repository(repository, target, cx);
                    }
                    Err(error) => page.repository_error = Some(error.into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn project_is_selected(&self, project: &ProjectRef, local: Option<&str>) -> bool {
        self.target_device.as_deref().or(local) == Some(project.device.as_str())
            && self.project_repositories.get(project).is_some_and(|repo| {
                self.repository
                    .as_ref()
                    .is_some_and(|current| current.eq_ignore_ascii_case(repo))
            })
    }

    fn recent_repositories(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        self.repository
            .iter()
            .chain(
                self.snapshots
                    .iter()
                    .rev()
                    .filter(|((target, _, _), ..)| target == &self.target_device)
                    .map(|((_, repo, _), ..)| repo),
            )
            .filter(|repo| seen.insert(repo.to_ascii_lowercase()))
            .take(5)
            .cloned()
            .collect()
    }

    fn render_repository_menu(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label = self.repository.clone().unwrap_or_else(|| {
            if self.initial_scope_task.is_some() {
                "Detecting repository…".into()
            } else {
                "Choose repository".into()
            }
        });
        let (projects, local, profile, selected_space) = {
            let state = self.state.read(cx);
            let local = state.local_device_id.clone().or_else(|| {
                state
                    .engine()
                    .map(|engine| engine.engine_info().device_id.clone())
            });
            let projects: Vec<_> = if self.repository_menu.get().is_some() {
                state.projects()
            } else {
                Vec::new()
            }
            .into_iter()
            .filter_map(|members| {
                let name = state
                    .representative_space(members[0])
                    .display_name()
                    .to_owned();
                let checkouts: Vec<_> = members
                    .into_iter()
                    .filter(|space| space.git_detected)
                    .map(|space| {
                        (
                            space.clone(),
                            state
                                .device_name(&space.device_id)
                                .unwrap_or("This device")
                                .to_owned(),
                            state.device_online(&space.device_id, Utc::now()),
                        )
                    })
                    .collect();
                (!checkouts.is_empty()).then_some((name, checkouts))
            })
            .collect();
            let profile = match &state.auth {
                Some(zeron_proto::AuthState::SignedIn { user, org_id }) => {
                    Some(format!("{}:{org_id:?}", user.id))
                }
                _ => None,
            };
            let selected_space = state
                .spaces
                .iter()
                .find(|space| {
                    space.git_detected
                        && self.project_is_selected(
                            &ProjectRef {
                                path: space.path.clone(),
                                device: space.device_id.clone(),
                            },
                            local.as_deref(),
                        )
                })
                .cloned();
            (projects, local, profile, selected_space)
        };
        let trigger_icon = match selected_space.as_ref() {
            Some(space) => div()
                .size(px(16.0))
                .flex_none()
                .child(self.project_icons.render(
                    &self.state,
                    Some(space),
                    profile.clone(),
                    false,
                    cx,
                ))
                .into_any_element(),
            None => icon(icons::FOLDER_WITH_FILES)
                .size(px(14.0))
                .text_color(theme.text_muted)
                .into_any_element(),
        };
        let mut trigger =
            crate::surface_chrome::tab("pr-repository", self.repository_menu.is_open(), theme)
                .debug_selector(|| "pr-repository".into())
                .h(px(28.0))
                .px(px(8.0))
                .max_w(px(220.0))
                .min_w_0()
                .text_size(crate::typography::ui_rems(12.0))
                .aria_label(format!("Repository: {label}"))
                .aria_expanded(self.repository_menu.is_open())
                .track_focus(&self.repository_focus)
                .tooltip(widgets::text_tooltip(label.clone()))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|page, _, _, _| page.repository_menu.note_trigger_press()),
                )
                .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" && page.repository_menu.is_open() {
                        cx.stop_propagation();
                        page.return_focus = Some(page.repository_focus.clone());
                        page.close_repository_menu(cx);
                    }
                }))
                .on_click(cx.listener(|page, event, window, cx| {
                    cx.stop_propagation();
                    let open = if matches!(event, gpui::ClickEvent::Keyboard(_)) {
                        page.repository_menu.is_open()
                    } else {
                        page.repository_menu.take_press_was_open()
                    };
                    if open {
                        page.close_repository_menu(cx);
                    } else {
                        page.repository_error = None;
                        page.repository_scroll.reset();
                        page.repository_menu.open(());
                        window.focus(&page.repository_input.read(cx).focus_handle(cx), cx);
                    }
                    cx.notify();
                }))
                .child(trigger_icon)
                .child(div().min_w_0().truncate().child(label))
                .child(
                    icon(icons::ALT_ARROW_DOWN)
                        .size(px(12.0))
                        .text_color(theme.text_muted),
                );
        if self.repository_menu.get().is_some() {
            let recent: Vec<_> = self
                .recent_repositories()
                .into_iter()
                .filter(|repo| {
                    !projects
                        .iter()
                        .flat_map(|(_, members)| members)
                        .any(|(space, _, _)| {
                            let project = ProjectRef {
                                path: space.path.clone(),
                                device: space.device_id.clone(),
                            };
                            Some(space.device_id.as_str())
                                == self.target_device.as_deref().or(local.as_deref())
                                && self
                                    .project_repositories
                                    .get(&project)
                                    .is_some_and(|known| known.eq_ignore_ascii_case(repo))
                        })
                })
                .collect();
            let mut options = Vec::new();
            if !projects.is_empty() {
                let mut group = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(repository_menu_label(theme, "Projects"));
                for (name, members) in projects {
                    let multiple = members.len() > 1;
                    let artwork = self.project_icons.render(
                        &self.state,
                        Some(&members[0].0),
                        profile.clone(),
                        false,
                        cx,
                    );
                    let mut project_group = div().flex().flex_col().gap(px(2.0));
                    if multiple {
                        project_group = project_group.child(
                            div()
                                .px(px(8.0))
                                .pt(px(8.0))
                                .pb(px(4.0))
                                .flex()
                                .items_center()
                                .gap(px(10.0))
                                .child(div().size(px(20.0)).flex_none().child(artwork))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .child(name.clone()),
                                ),
                        );
                    }
                    for (space, device, online) in members {
                        let project = ProjectRef {
                            path: space.path.clone(),
                            device: space.device_id.clone(),
                        };
                        let selected = self.project_is_selected(&project, local.as_deref());
                        let loading = self.resolving_project.as_ref() == Some(&project);
                        let repository = self.project_repositories.get(&project);
                        let primary = if multiple {
                            device.clone()
                        } else {
                            name.clone()
                        };
                        let secondary = if loading {
                            "Finding GitHub repository…".into()
                        } else if !online {
                            "Device offline".into()
                        } else if multiple {
                            space.path.clone()
                        } else {
                            format!(
                                "{} · {}",
                                device,
                                repository.map(String::as_str).unwrap_or(&space.path)
                            )
                        };
                        let glyph = if multiple {
                            icon(icons::LAPTOP)
                                .size(px(16.0))
                                .text_color(theme.text_muted)
                                .into_any_element()
                        } else {
                            self.project_icons.render(
                                &self.state,
                                Some(&space),
                                profile.clone(),
                                selected,
                                cx,
                            )
                        };
                        let id = format!("pr-project-{}", space.id);
                        let selector = id.clone();
                        let row = popover::menu_row(theme, selected, id.clone())
                            .id(SharedString::from(id))
                            .debug_selector(move || selector.clone())
                            .role(gpui::Role::Button)
                            .aria_selected(selected)
                            .tab_index(0)
                            .aria_label(format!(
                                "Open {name} repository on {device}: {}{}",
                                space.path,
                                if online { "" } else { ", offline" }
                            ))
                            .focus_visible(|style| style.border_2().border_color(theme.accent))
                            .min_w_0()
                            .min_h(px(48.0))
                            .when(multiple, |row| row.ml(px(16.0)))
                            .tooltip(widgets::text_tooltip(format!(
                                "{name}\n{device}\n{}",
                                space.path
                            )))
                            .child(
                                div()
                                    .size(px(20.0))
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(glyph),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(crate::shell::sidebar_faded_label(
                                        format!("pr-project-name-{}", space.id).into(),
                                        false,
                                        primary,
                                    ))
                                    .child(crate::shell::sidebar_faded_label(
                                        format!("pr-project-path-{}", space.id).into(),
                                        false,
                                        div()
                                            .text_size(crate::typography::ui_rems(11.0))
                                            .text_color(theme.text_muted)
                                            .child(secondary),
                                    )),
                            )
                            .child(
                                div()
                                    .size(px(16.0))
                                    .flex_none()
                                    .when(loading, |el| {
                                        el.child(crate::loaders::mini_glyph_spinner(
                                            "pr-project-resolving",
                                            1.5,
                                            theme.glyph,
                                            cx.entity_id(),
                                            cx,
                                        ))
                                    })
                                    .when(!loading && selected, |el| {
                                        el.child(
                                            icon(icons::CHECK)
                                                .size(px(16.0))
                                                .text_color(theme.text_muted),
                                        )
                                    }),
                            )
                            .on_click(cx.listener(move |page, _, _, cx| {
                                cx.stop_propagation();
                                page.select_project(
                                    project.path.clone(),
                                    project.device.clone(),
                                    cx,
                                );
                            }));
                        project_group = project_group.child(row);
                    }
                    group = group.child(project_group);
                }
                options.push(group.into_any_element());
            }
            if !recent.is_empty() {
                let mut group = div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(repository_menu_label(theme, "Recent repositories"));
                for repo in recent {
                    let selected = self
                        .repository
                        .as_ref()
                        .is_some_and(|current| current.eq_ignore_ascii_case(&repo));
                    group = group.child(
                        popover::menu_row(theme, selected, format!("pr-recent-{repo}"))
                            .id(SharedString::from(format!("pr-recent-{repo}")))
                            .debug_selector(|| "pr-recent-option".into())
                            .role(gpui::Role::Button)
                            .aria_selected(selected)
                            .tab_index(0)
                            .aria_label(format!("Open {repo}"))
                            .focus_visible(|style| style.border_2().border_color(theme.accent))
                            .min_w_0()
                            .min_h(px(36.0))
                            .child(
                                icon(icons::FOLDER_WITH_FILES)
                                    .size(px(16.0))
                                    .flex_none()
                                    .text_color(theme.text_muted),
                            )
                            .child(crate::shell::sidebar_faded_label(
                                format!("pr-recent-label-{repo}").into(),
                                true,
                                repo.clone(),
                            ))
                            .child(div().size(px(16.0)).flex_none().when(selected, |el| {
                                el.child(
                                    icon(icons::CHECK)
                                        .size(px(16.0))
                                        .text_color(theme.text_muted),
                                )
                            }))
                            .on_click(cx.listener(move |page, _, _, cx| {
                                cx.stop_propagation();
                                page.return_focus = Some(page.repository_focus.clone());
                                page.repository_input
                                    .update(cx, |input, cx| input.set_text(&repo, cx));
                                page.select_repository(cx);
                            })),
                    );
                }
                options.push(group.into_any_element());
            }
            let scrollbar = widgets::rail(
                &mut self.repository_scroll,
                "pr-repository-scrollbar",
                theme,
                cx,
                |page| &mut page.repository_scroll,
            );
            let menu = popover::popover_card(theme)
                .id("pr-repository-card")
                .debug_selector(|| "pr-repository-card".into())
                .w(px((f32::from(window.viewport_size().width) - 16.0)
                    .min(344.0)
                    .max(240.0)))
                .flex()
                .flex_col()
                .gap(px(12.0))
                .on_mouse_down_out(cx.listener(|page, _, _, cx| page.close_repository_menu(cx)))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(6.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(
                                    crate::surface_chrome::input()
                                        .min_w_0()
                                        .flex_1()
                                        .h(px(32.0))
                                        .child(self.repository_input.clone()),
                                )
                                .child(
                                    crate::surface_chrome::tab("pr-repository-load", false, theme)
                                        .debug_selector(|| "pr-repository-load".into())
                                        .h(px(32.0))
                                        .px(px(10.0))
                                        .flex_none()
                                        .aria_label("Open repository")
                                        .child("Open")
                                        .on_click(cx.listener(|page, _, _, cx| {
                                            cx.stop_propagation();
                                            page.return_focus = Some(page.repository_focus.clone());
                                            page.select_repository(cx);
                                        })),
                                ),
                        )
                        .when_some(self.repository_error.clone(), |el, error| {
                            el.child(widgets::error_strip(theme, error))
                        }),
                )
                .when(!options.is_empty(), |el| {
                    el.child(
                        popover::menu_scroll_host("pr-repository-list-host")
                            .on_hover(cx.listener(|page, hovered: &bool, _, cx| {
                                if page.repository_scroll.set_list_hovered(*hovered) {
                                    cx.notify();
                                }
                            }))
                            .child(popover::faded_menu_list(
                                &self.repository_scroll.scroll,
                                popover::menu_scroll_list(
                                    "pr-repository-options",
                                    &self.repository_scroll.scroll,
                                )
                                .debug_selector(|| "pr-repository-options".into())
                                .min_w_0()
                                .max_h(px((f32::from(window.viewport_size().height) - 240.0)
                                    .clamp(96.0, 360.0)))
                                .flex()
                                .flex_col()
                                .gap(px(12.0))
                                .children(options),
                            ))
                            .children(scrollbar),
                    )
                });
            trigger = trigger.child(popover::anchored_menu_below_end(
                "pr-repository-menu",
                menu.into_any_element(),
                self.repository_menu.closing_since(),
            ));
        }
        trigger.into_any_element()
    }

    fn close_sort_menu(&mut self, cx: &mut Context<Self>) {
        if self.sort_menu.begin_close() {
            popover::reap_popup(cx, |page: &mut Self| &mut page.sort_menu);
            cx.notify();
        }
    }

    fn render_sort_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let options = [
            (
                PullRequestSortField::Updated,
                SortDirection::Descending,
                "Recently updated",
                "pull-requests-sort-updated",
            ),
            (
                PullRequestSortField::Updated,
                SortDirection::Ascending,
                "Least recently updated",
                "pr-sort-updated-asc",
            ),
            (
                PullRequestSortField::Opened,
                SortDirection::Descending,
                "Newest first",
                "pull-requests-sort-opened",
            ),
            (
                PullRequestSortField::Opened,
                SortDirection::Ascending,
                "Oldest first",
                "pr-sort-opened-asc",
            ),
            (
                PullRequestSortField::Changes,
                SortDirection::Descending,
                "Largest changes",
                "pull-requests-sort-changes",
            ),
            (
                PullRequestSortField::Changes,
                SortDirection::Ascending,
                "Smallest changes",
                "pr-sort-changes-asc",
            ),
        ];
        let label = options
            .iter()
            .find(|(field, direction, _, _)| {
                *field == self.sort.field && *direction == self.sort.direction
            })
            .unwrap()
            .2;
        let mut trigger = crate::surface_chrome::tab("pr-sort", self.sort_menu.is_open(), theme)
            .debug_selector(|| "pr-sort".into())
            .h(px(32.0))
            .w(px(32.0))
            .justify_center()
            .aria_label(format!("Sort pull requests: {label}"))
            .aria_expanded(self.sort_menu.is_open())
            .track_focus(&self.sort_focus)
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|page, _, _, _| page.sort_menu.note_trigger_press()),
            )
            .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && page.sort_menu.is_open() {
                    page.return_focus = Some(page.sort_focus.clone());
                    page.close_sort_menu(cx);
                }
            }))
            .on_click(cx.listener(|page, event, window, cx| {
                cx.stop_propagation();
                let keyboard = matches!(event, gpui::ClickEvent::Keyboard(_));
                let open = if keyboard {
                    page.sort_menu.is_open()
                } else {
                    page.sort_menu.take_press_was_open()
                };
                if open {
                    page.close_sort_menu(cx);
                } else {
                    page.sort_menu.open(());
                    if keyboard {
                        window.focus(&page.option_focus, cx);
                    }
                }
                cx.notify();
            }))
            .child(
                icon(if self.sort.direction == SortDirection::Ascending {
                    icons::ARROW_UP
                } else {
                    icons::ARROW_DOWN
                })
                .size(px(14.0))
                .text_color(theme.text_muted),
            )
            .tooltip(widgets::text_tooltip(label));
        if self.sort_menu.get().is_some() {
            let menu = popover::popover_card(theme)
                .w(px(220.0))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .on_mouse_down_out(cx.listener(|page, _, _, cx| page.close_sort_menu(cx)))
                .children(options.into_iter().map(|(field, direction, label, id)| {
                    let active = self.sort.field == field && self.sort.direction == direction;
                    popover::menu_row(theme, active, id)
                        .id(id)
                        .when(active, |row| row.track_focus(&self.option_focus))
                        .debug_selector(move || id.into())
                        .role(gpui::Role::Button)
                        .tab_index(0)
                        .aria_selected(active)
                        .aria_label(label)
                        .focus_visible(|style| style.border_2().border_color(theme.accent))
                        .child(label)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            cx.stop_propagation();
                            page.sort = PullRequestSort { field, direction };
                            page.return_focus = Some(page.sort_focus.clone());
                            page.view_items = None;
                            page.scroll.scroll.set_offset(gpui::Point::default());
                            page.close_sort_menu(cx);
                            cx.notify();
                        }))
                }));
            trigger = trigger.child(popover::anchored_menu_below_end(
                "pr-sort-menu",
                menu.into_any_element(),
                self.sort_menu.closing_since(),
            ));
        }
        trigger.into_any_element()
    }

    fn load(&mut self, refresh: bool, cx: &mut Context<Self>) {
        if self.repository.is_none() || matches!(self.load_state, PullRequestsLoadState::Loading) {
            return;
        }
        self.reconcile_target_device(cx);
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.load_state = PullRequestsLoadState::Failed(PullRequestsPageError::Network);
            cx.notify();
            return;
        };

        let target_name = selected_device_name(&self.state.read(cx), self.target_device.as_deref());
        if let Some(target) = self.target_device.as_deref()
            && !self.state.read(cx).device_online(target, Utc::now())
        {
            self.load_state =
                PullRequestsLoadState::Failed(PullRequestsPageError::RemoteOffline(target_name));
            cx.notify();
            return;
        }

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let mut params = params_for_target(self.target_device.as_deref());
        params["repository"] = self.repository.clone().unwrap().into();
        params["refresh"] = refresh.into();
        params["filter"] = serde_json::to_value(self.filter).unwrap();
        self.refreshed = refresh;
        self.load_state = PullRequestsLoadState::Loading;
        // A reload starts over from the first page.
        self.more_task = None;
        self.more_error = None;
        self.request_task = Some(cx.spawn(async move |this, cx| {
            let mut result = fetch_page(&engine, params.clone()).await;
            if result.as_ref().is_ok_and(|page| page.items.is_empty()) && !refresh {
                let fallback = this
                    .update(cx, |page, cx| {
                        if page.generation != generation
                            || !page.automatic_filter
                            || page.filter != ChangeRequestFilter::Authored
                        {
                            return false;
                        }
                        page.set_filter(ChangeRequestFilter::All, cx);
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if fallback {
                    params["filter"] = serde_json::to_value(ChangeRequestFilter::All).unwrap();
                    result = fetch_page(&engine, params).await;
                }
            }
            this.update(cx, |page, cx| {
                if page.generation != generation {
                    return;
                }

                page.request_task = None;
                let loaded = result.map_err(|error| map_rpc_error(&error, &target_name));
                let paging = loaded.as_ref().ok().map(|page| Paging {
                    next_cursor: page.next_cursor.clone(),
                    total: page.total_count,
                });
                let succeeded = loaded.is_ok();
                page.load_state = settle_snapshot(&mut page.items, loaded.map(|page| page.items));
                page.view_items = None;
                if let Some(paging) = paging {
                    page.paging = paging;
                }
                if succeeded {
                    if let Some(first) = page.items.first()
                        && valid_repository_filter(&first.repository)
                        && page
                            .items
                            .iter()
                            .all(|item| item.repository == first.repository)
                        && page.repository.as_deref() != Some(first.repository.as_str())
                    {
                        let canonical = first.repository.clone();
                        page.repository = Some(canonical.clone());
                        page.repository_input
                            .update(cx, |input, cx| input.set_text(&canonical, cx));
                        let device = page.target_device.clone();
                        crate::settings::update(
                            crate::settings::SavePolicy::Debounced,
                            cx,
                            |settings| {
                                settings.last_pull_request_repository = Some(canonical);
                                settings.last_pull_request_device = device;
                            },
                        );
                    }
                    page.automatic_filter = false;
                    page.remember_filter(page.filter);
                    let now = Instant::now();
                    page.last_loaded_at = Some(now);
                    page.store_snapshot(now);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn store_snapshot(&mut self, at: Instant) {
        let Some(repository) = self.repository.clone() else {
            return;
        };
        let key = (self.target_device.clone(), repository, self.filter);
        self.snapshots.retain(|((target, repo, filter), ..)| {
            !(target == &key.0 && repo.eq_ignore_ascii_case(&key.1) && filter == &key.2)
        });
        self.snapshots
            .push((key, self.items.clone(), at, self.paging.clone()));
        if self.snapshots.len() > 12 {
            self.snapshots.remove(0);
        }
    }

    /// Append the next page. Items already on the board keep their place;
    /// a PR that moved between pages since the first load is not repeated.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        let Some(cursor) = self.paging.next_cursor.clone() else {
            return;
        };
        if self.more_task.is_some()
            || self.repository.is_none()
            || matches!(self.load_state, PullRequestsLoadState::Loading)
        {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.more_error = Some(PullRequestsPageError::Network);
            cx.notify();
            return;
        };
        let target_name = selected_device_name(&self.state.read(cx), self.target_device.as_deref());
        let generation = self.generation;
        let mut params = params_for_target(self.target_device.as_deref());
        params["repository"] = self.repository.clone().unwrap().into();
        params["filter"] = serde_json::to_value(self.filter).unwrap();
        params["after"] = cursor.into();
        // Pages after a refreshed first page must be as fresh as it is.
        params["refresh"] = self.refreshed.into();
        self.more_error = None;
        self.more_task = Some(cx.spawn(async move |this, cx| {
            let result = fetch_page(&engine, params).await;
            this.update(cx, |page, cx| {
                if page.generation != generation {
                    return;
                }
                page.more_task = None;
                match result {
                    Ok(next) => {
                        let known: HashSet<String> =
                            page.items.iter().map(|item| item.url.clone()).collect();
                        page.items.extend(
                            next.items
                                .into_iter()
                                .filter(|item| !known.contains(&item.url)),
                        );
                        page.paging = Paging {
                            next_cursor: next.next_cursor,
                            total: next.total_count.or(page.paging.total),
                        };
                        page.view_items = None;
                        page.store_snapshot(page.last_loaded_at.unwrap_or_else(Instant::now));
                    }
                    Err(error) => page.more_error = Some(map_rpc_error(&error, &target_name)),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// "Load more" beneath the last group, with the remaining count when
    /// GitHub reports it.
    fn render_load_more(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.paging.next_cursor.as_ref()?;
        let loading = self.more_task.is_some();
        let remaining = self
            .paging
            .total
            .map(|total| total.saturating_sub(self.items.len() as u64))
            .filter(|remaining| *remaining > 0);
        let label = match (loading, remaining) {
            (true, _) => "Loading more…".to_owned(),
            (false, Some(remaining)) => format!("Load {} more", remaining.min(50)),
            (false, None) => "Load more".to_owned(),
        };
        Some(
            div()
                .mt(px(Theme::SPACE_LG))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(Theme::SPACE_SM))
                .when_some(self.more_error.as_ref(), |el, error| {
                    let (title, body) = error_copy(error);
                    el.child(widgets::error_strip(theme, format!("{title}. {body}")))
                })
                .child(
                    widgets::ghost_action(theme)
                        .id("pull-requests-load-more")
                        .debug_selector(|| "pull-requests-load-more".to_string())
                        .role(gpui::Role::Button)
                        .aria_label(label.clone())
                        .tab_index(0)
                        .h(px(32.0))
                        .px(px(14.0))
                        .border_1()
                        .border_color(theme.border)
                        .focus_visible(|style| style.border_2().border_color(theme.accent))
                        .when(loading, |el| el.opacity(0.6))
                        .gap(px(8.0))
                        .when(loading, |el| {
                            el.child(crate::loaders::mini_glyph_spinner(
                                "pull-requests-more-spinner",
                                1.5,
                                theme.glyph,
                                cx.entity_id(),
                                cx,
                            ))
                        })
                        .child(label)
                        .on_click(cx.listener(|page, _, _, cx| page.load_more(cx))),
                )
                .into_any_element(),
        )
    }

    fn render_device_switcher(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let (devices, local_id) = {
            let state = self.state.read(cx);
            (
                eligible_desktop_devices(&state.devices),
                state.local_device_id.clone(),
            )
        };
        if devices.len() <= 1 {
            return div().into_any_element();
        }

        let effective = self.target_device.clone().or_else(|| local_id.clone());
        let selected = devices
            .iter()
            .find(|device| Some(device.id.as_str()) == effective.as_deref());
        let label: SharedString = selected
            .map(|device| device.name.clone().into())
            .unwrap_or_else(|| SharedString::from("This device"));
        let glyph = selected
            .map(|device| platform_icon(&device.platform))
            .unwrap_or(icons::LAPTOP);
        let open = self.device_menu.is_open();

        let mut trigger = div()
            .id("pull-requests-device-switcher")
            .role(gpui::Role::Button)
            .aria_label(format!("Desktop device: {label}"))
            .aria_expanded(open)
            .track_focus(&self.device_focus)
            .tab_index(0)
            .border_1()
            .border_color(gpui::transparent_black())
            .focus_visible(|style| style.border_2().border_color(theme.accent))
            .flex_none()
            .h(px(32.0))
            .px(px(8.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .cursor_pointer()
            .bg(if open {
                crate::theme::ink(0.06)
            } else {
                gpui::transparent_black()
            })
            .when(!open, |element| {
                element.hover(|style| style.bg(crate::theme::ink(0.04)))
            })
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|page, _, _, _| page.device_menu.note_trigger_press()),
            )
            .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" && page.device_menu.is_open() {
                    cx.stop_propagation();
                    page.return_focus = Some(page.device_focus.clone());
                    page.close_device_menu(cx);
                }
            }))
            .on_click(cx.listener(|page, event, window, cx| {
                let keyboard = matches!(event, gpui::ClickEvent::Keyboard(_));
                let was_open = if keyboard {
                    page.device_menu.is_open()
                } else {
                    page.device_menu.take_press_was_open()
                };
                if was_open {
                    page.close_device_menu(cx);
                } else {
                    page.device_menu.open(());
                    if keyboard {
                        window.focus(&page.option_focus, cx);
                    }
                }
                cx.notify();
            }))
            .child(icon(glyph).size(px(16.0)).text_color(theme.text_muted))
            .child(
                div()
                    .max_w(px(130.0))
                    .truncate()
                    .text_size(crate::typography::ui_rems(12.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(label),
            )
            .child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(px(14.0))
                    .text_color(theme.text_muted.opacity(0.5)),
            );

        if self.device_menu.get().is_some() {
            let closing = self.device_menu.closing_since();
            let menu = popover::popover_card(theme)
                .w(px(240.0))
                .on_mouse_down_out(cx.listener(|page, _, _, cx| page.close_device_menu(cx)))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(repository_menu_label(theme, "Desktop devices"))
                .children(devices.into_iter().enumerate().map(|(index, device)| {
                    let active = effective.as_deref() == Some(device.id.as_str());
                    let local = local_id.as_deref() == Some(device.id.as_str());
                    let device_id = device.id.clone();
                    let name: SharedString = device.name.clone().into();
                    let online = self.state.read(cx).device_online(&device.id, Utc::now());
                    popover::menu_row(theme, active, format!("pull-requests-device-row-{index}"))
                        .id(("pull-requests-device-row", index))
                        .when(active, |row| row.track_focus(&self.option_focus))
                        .role(gpui::Role::Button)
                        .aria_label(format!(
                            "{}{}",
                            device.name,
                            if online { "" } else { ", offline" }
                        ))
                        .aria_selected(active)
                        .tab_index(0)
                        .focus_visible(|style| style.border_2().border_color(theme.accent))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.return_focus = Some(page.device_focus.clone());
                            page.set_target_device((!local).then(|| device_id.clone()), cx);
                        }))
                        .child(
                            icon(platform_icon(&device.platform))
                                .size(px(16.0))
                                .text_color(theme.text_muted),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(name))
                        .when(local, |element| {
                            element.child(
                                div()
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .text_color(theme.text_muted)
                                    .child("This device"),
                            )
                        })
                        .when(!online, |element| {
                            element.child(
                                div()
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .text_color(theme.text_muted)
                                    .child("Offline"),
                            )
                        })
                        .child(div().size(px(6.0)).rounded_full().bg(if online {
                            theme.success
                        } else {
                            crate::theme::ink(0.2)
                        }))
                }));
            trigger = trigger.child(popover::anchored_menu(
                "pull-requests-device-menu",
                menu.into_any_element(),
                closing,
            ));
        }

        trigger.into_any_element()
    }

    fn render_empty_or_error(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let glyph = match &self.load_state {
            PullRequestsLoadState::Failed(PullRequestsPageError::Authentication) => {
                icons::KEY_MINIMALISTIC
            }
            PullRequestsLoadState::Failed(PullRequestsPageError::RemoteOffline(_)) => {
                icons::WIFI_OFF
            }
            PullRequestsLoadState::Failed(_) => icons::INFO_CIRCLE,
            _ => icons::PULL_REQUEST,
        };
        let (title, body) = match &self.load_state {
            PullRequestsLoadState::Failed(error) => error_copy(error),
            _ if self.repository.is_none() => (
                "Choose a repository".into(),
                "No GitHub repository was found for this project. Choose one to load your open pull requests.".into(),
            ),
            PullRequestsLoadState::Idle => (
                "Ready when you are".into(),
                "Refresh to load this repository on the selected device.".into(),
            ),
            _ => match self.filter {
                ChangeRequestFilter::Authored => (
                    "No pull requests authored by you".into(),
                    "Your open pull requests in this repository will appear here.".into(),
                ),
                ChangeRequestFilter::Reviewing => (
                    "No reviews requested".into(),
                    "Pull requests waiting for your review will appear here.".into(),
                ),
                ChangeRequestFilter::All => (
                    "No open pull requests".into(),
                    "This repository has no open pull requests. New ones will appear here.".into(),
                ),
            },
        };
        div()
            .mt(px(72.0))
            .flex()
            .flex_col()
            .items_center()
            .text_center()
            .child(
                div()
                    .mb(px(Theme::SPACE_LG))
                    .child(icon(glyph).size(px(24.0)).text_color(theme.text_muted)),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(420.0))
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(SharedString::from(title)),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .w_full()
                    .max_w(px(420.0))
                    .text_size(px(13.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(body)),
            )
            .when(
                self.repository.is_none()
                    && !matches!(self.load_state, PullRequestsLoadState::Failed(_)),
                |el| {
                    el.child(
                        empty_action("pr-empty-choose-repository", "Choose repository", theme)
                            .on_click(cx.listener(|page, _, window, cx| {
                                page.repository_menu.open(());
                                window.focus(&page.repository_input.read(cx).focus_handle(cx), cx);
                                cx.notify();
                            })),
                    )
                },
            )
            .when(
                matches!(self.load_state, PullRequestsLoadState::Ready)
                    && self.filter != ChangeRequestFilter::All,
                |el| {
                    el.child(
                        empty_action("pr-empty-show-all", "Show all pull requests", theme)
                            .on_click(cx.listener(|page, _, _, cx| {
                                page.select_filter(ChangeRequestFilter::All, cx)
                            })),
                    )
                },
            )
            .when(
                self.repository.is_some() && matches!(self.load_state, PullRequestsLoadState::Idle),
                |el| {
                    el.child(
                        empty_action("pr-empty-load", "Load pull requests", theme)
                            .on_click(cx.listener(|page, _, _, cx| page.refresh(cx))),
                    )
                },
            )
            .when(
                matches!(
                    self.load_state,
                    PullRequestsLoadState::Failed(ref error)
                        if *error != PullRequestsPageError::RateLimited
                ),
                |el| {
                    el.child(
                        empty_action("pull-requests-retry", "Try again", theme)
                            .on_click(cx.listener(|page, _, _, cx| page.refresh(cx))),
                    )
                },
            )
            .into_any_element()
    }
}

fn empty_action(id: &'static str, label: &'static str, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    widgets::ghost_action(theme)
        .id(id)
        .debug_selector(move || id.into())
        .role(gpui::Role::Button)
        .tab_index(0)
        .mt(px(Theme::SPACE_LG))
        .h(px(32.0))
        .px(px(14.0))
        .border_1()
        .border_color(theme.border)
        .focus_visible(|style| style.border_2().border_color(theme.accent))
        .child(label)
}

impl popover::ScrollRailHost for PullRequestsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }
    fn rail_scroll(&self) -> Option<ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for PullRequestsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(focus) = self.return_focus.take() {
            window.focus(&focus, cx);
        }
        let theme = Theme::of(cx).clone();
        let initial_loading =
            self.items.is_empty() && matches!(self.load_state, PullRequestsLoadState::Loading);
        let refreshing =
            !self.items.is_empty() && matches!(self.load_state, PullRequestsLoadState::Loading);
        let refresh_error =
            !self.items.is_empty() && matches!(self.load_state, PullRequestsLoadState::Failed(_));
        let refresh_message = match &self.load_state {
            PullRequestsLoadState::Failed(error) => {
                let (title, body) = error_copy(error);
                format!("{title}. {body} Showing the last loaded results.")
            }
            _ => String::new(),
        };
        let count = (!initial_loading
            && !matches!(self.load_state, PullRequestsLoadState::Failed(_))
            || !self.items.is_empty())
        .then(|| {
            self.paging.total.map_or(self.items.len(), |total| {
                (total as usize).max(self.items.len())
            })
        });
        let items = self
            .view_items
            .get_or_insert_with(|| {
                let mut items: Vec<_> = self
                    .items
                    .iter()
                    .filter(|item| matches_query(item, &self.query))
                    .cloned()
                    .collect();
                sort_pull_requests(&mut items, self.sort);
                Rc::new(items)
            })
            .clone();
        let stars = crate::settings::current(cx).pull_request_stars;
        let items = if self.personal_view == PersonalView::Everything {
            items
        } else {
            Rc::new(
                items
                    .iter()
                    .filter(|item| match self.personal_view {
                        PersonalView::Everything => true,
                        PersonalView::NeedsYou => {
                            !Facts::from_item(item).assess().attention.is_empty()
                        }
                        PersonalView::Starred => stars.contains(&star_key(&item.url)),
                    })
                    .cloned()
                    .collect(),
            )
        };
        let layout = self
            .content_width
            .map(table_layout)
            .unwrap_or(PullRequestTableLayout::Narrow);
        let scroll = self.scroll.scroll.clone();
        let width_probe = {
            let page = cx.weak_entity();
            gpui::canvas(
                move |bounds, window, cx| {
                    let width = table_content_width(f32::from(bounds.size.width));
                    // Notify after layout completes so the next frame uses the
                    // new outlet width even when resizing across a breakpoint.
                    window.defer(cx, move |_, cx| {
                        page.update(cx, |page, cx| {
                            if page
                                .content_width
                                .is_none_or(|current| (current - width).abs() > 0.5)
                            {
                                page.content_width = Some(width);
                                cx.notify();
                            }
                        })
                        .ok();
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };

        let loading = initial_loading || refreshing;
        let freshness: SharedString = match self.last_loaded_at {
            Some(at) if at.elapsed().as_secs() < 60 => {
                "Loaded just now · Refresh pull requests".into()
            }
            Some(at) => format!(
                "Loaded {}m ago · Refresh pull requests",
                at.elapsed().as_secs() / 60
            )
            .into(),
            None => "Refresh pull requests".into(),
        };

        let header = widgets::page_column()
            .id("pull-requests-column")
            .debug_selector(|| "pull-requests-column".to_owned())
            .max_w(px(PR_PAGE_MAX_WIDTH))
            .pt_0()
            .pb(px(Theme::SPACE_LG))
            .flex_none()
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(12.0))
                    .child(
                        widgets::page_header(&theme, "Pull requests", count)
                            .items_center()
                            .when(self.paging.next_cursor.is_some(), |el| {
                                el.child(
                                    div()
                                        .id("pull-requests-loaded-count")
                                        .debug_selector(|| "pull-requests-loaded-count".into())
                                        .text_size(crate::typography::ui_rems(11.0))
                                        .text_color(theme.text_muted)
                                        .child(format!("{} loaded", self.items.len())),
                                )
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap(px(8.0))
                            .child(self.render_repository_menu(&theme, window, cx))
                            .child(self.render_device_switcher(&theme, cx))
                            .child(
                                widgets::ghost_action(&theme)
                                    .id("pull-requests-refresh")
                                    .debug_selector(|| "pull-requests-refresh".to_string())
                                    .role(gpui::Role::Button)
                                    .aria_label("Refresh pull requests")
                                    .aria_description(if loading {
                                        "Loading pull requests"
                                    } else {
                                        "Fetch the latest results from GitHub"
                                    })
                                    .tab_index(0)
                                    .border_1()
                                    .border_color(gpui::transparent_black())
                                    .focus_visible(|style| {
                                        style.border_2().border_color(theme.accent)
                                    })
                                    .h(px(32.0))
                                    .flex_none()
                                    .when(loading, |el| el.opacity(0.5))
                                    .on_click(cx.listener(|page, _, _, cx| page.refresh(cx)))
                                    .child(if refreshing {
                                        crate::loaders::mini_glyph_spinner(
                                            "pull-requests-refresh-spinner",
                                            1.75,
                                            theme.glyph,
                                            cx.entity_id(),
                                            cx,
                                        )
                                        .into_any_element()
                                    } else {
                                        icon(icons::REFRESH)
                                            .size(px(16.0))
                                            .text_color(theme.text_muted)
                                            .into_any_element()
                                    })
                                    .tooltip(widgets::text_tooltip(freshness)),
                            ),
                    ),
            )
            .when(refresh_error, |el| {
                el.child(widgets::error_strip(&theme, refresh_message))
            })
            .when_some(self.repository_error.clone(), |el, error| {
                el.child(widgets::error_strip(&theme, error))
            })
            .child(
                div()
                    .mt(px(12.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div().flex().items_center().gap(px(2.0)).children(
                            [
                                (
                                    ChangeRequestFilter::Authored,
                                    "Authored",
                                    "pr-filter-authored",
                                ),
                                (
                                    ChangeRequestFilter::Reviewing,
                                    "Reviewing",
                                    "pr-filter-reviewing",
                                ),
                                (ChangeRequestFilter::All, "All", "pr-filter-all"),
                            ]
                            .into_iter()
                            .map(|(filter, label, id)| {
                                let selected = self.filter_fades.value_at(id, Instant::now());
                                let hover_key = format!("pr-filter-{}-{id}", cx.entity_id());
                                let hover = crate::motion::hover_t(&hover_key);
                                crate::surface_chrome::tab_frame(id, self.filter == filter, &theme)
                                    .bg(crate::theme::wash(
                                        0.10 * selected + 0.06 * hover * (1.0 - selected),
                                    ))
                                    .text_color(crate::motion::mix(
                                        theme.text_muted,
                                        theme.text,
                                        selected.max(hover),
                                    ))
                                    .on_hover(crate::motion::hover_listener(hover_key))
                                    .hover(move |style| {
                                        style.bg(crate::theme::wash(
                                            0.10 * selected + 0.06 * hover * (1.0 - selected),
                                        ))
                                    })
                                    .debug_selector(move || id.into())
                                    .text_size(crate::typography::ui_rems(12.0))
                                    .px(px(10.0))
                                    .child(label)
                                    .on_click(cx.listener(move |page, _, _, cx| {
                                        page.select_filter(filter, cx)
                                    }))
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(180.0))
                            .items_center()
                            .gap(px(Theme::SPACE_SM))
                            .child(
                                crate::surface_chrome::input()
                                    .h(px(32.0))
                                    .min_w(px(160.0))
                                    .child(
                                        icon(icons::MAGNIFER)
                                            .size(px(14.0))
                                            .text_color(theme.text_muted),
                                    )
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .h(px(16.0))
                                            .overflow_hidden()
                                            .child(self.search.clone()),
                                    )
                                    .when(!self.query.is_empty(), |el| {
                                        el.child(
                                            div()
                                                .id("pull-requests-clear-search")
                                                .role(gpui::Role::Button)
                                                .aria_label("Clear search")
                                                .tab_index(0)
                                                .size(px(24.0))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .rounded(px(4.0))
                                                .focus_visible(|style| {
                                                    style.border_2().border_color(theme.accent)
                                                })
                                                .cursor_pointer()
                                                .on_click(cx.listener(|page, _, _, cx| {
                                                    page.search.update(cx, |input, cx| {
                                                        input.set_text("", cx)
                                                    });
                                                    page.query.clear();
                                                    page.view_items = None;
                                                    page.scroll
                                                        .scroll
                                                        .set_offset(gpui::Point::default());
                                                    cx.notify();
                                                }))
                                                .child(
                                                    icon(icons::CLOSE)
                                                        .size(px(12.0))
                                                        .text_color(theme.text_muted),
                                                ),
                                        )
                                    }),
                            )
                            .child(self.render_sort_menu(&theme, cx)),
                    ),
            )
            .child(
                div()
                    .mt(px(12.0))
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(8.0))
                    .children(
                        [
                            (PersonalView::NeedsYou, "Needs you", "pr-view-needs-you"),
                            (PersonalView::Starred, "Starred", "pr-view-starred"),
                        ]
                        .into_iter()
                        .map(|(view, label, id)| {
                            let selected = self.personal_view == view;
                            crate::surface_chrome::tab_frame(id, selected, &theme)
                                .debug_selector(move || id.into())
                                .role(gpui::Role::Button)
                                .aria_label(label)
                                .aria_selected(selected)
                                .text_size(crate::typography::ui_rems(12.0))
                                .px(px(10.0))
                                .child(label)
                                .on_click(cx.listener(move |page, _, _, cx| {
                                    page.personal_view = if selected {
                                        PersonalView::Everything
                                    } else {
                                        view
                                    };
                                    page.scroll.scroll.set_offset(gpui::Point::default());
                                    if !selected {
                                        page.select_filter(ChangeRequestFilter::All, cx);
                                    }
                                    cx.notify();
                                }))
                        }),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(if initial_loading {
                                "Loading pull requests…".to_owned()
                            } else {
                                format!("{} shown · {} loaded", items.len(), self.items.len())
                            }),
                    ),
            )
            .when(!self.query.is_empty(), |el| {
                el.child(
                    div()
                        .mt(px(Theme::SPACE_SM))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child(if self.paging.next_cursor.is_some() {
                            format!(
                                "{} of {} loaded pull requests · load more to search further",
                                items.len(),
                                self.items.len()
                            )
                        } else {
                            format!("{} of {} pull requests", items.len(), self.items.len())
                        }),
                )
            });
        let content = if initial_loading {
            crate::pull_request_skeleton::board(layout, cx.entity_id(), &theme, cx)
        } else if self.items.is_empty() {
            self.render_empty_or_error(&theme, cx)
        } else if items.is_empty() {
            div()
                .id("pull-requests-no-results")
                .debug_selector(|| "pull-requests-no-results".to_string())
                .py(px(48.0))
                .text_center()
                .text_size(crate::typography::ui_rems(13.0))
                .text_color(theme.text_muted)
                .flex()
                .flex_col()
                .items_center()
                .child(match self.personal_view {
                    PersonalView::NeedsYou => "Nothing needs your attention in the loaded PRs",
                    PersonalView::Starred => "No starred pull requests in the loaded PRs",
                    PersonalView::Everything => "No matching pull requests",
                })
                .child(
                    empty_action("pr-empty-clear-search", "Show loaded pull requests", &theme)
                        .on_click(cx.listener(|page, _, _, cx| {
                            page.search.update(cx, |input, cx| input.set_text("", cx));
                            page.query.clear();
                            page.personal_view = PersonalView::Everything;
                            page.view_items = None;
                            cx.notify();
                        }),
                    ),
                )
                .into_any_element()
        } else {
            render_grouped_requests(
                &items,
                layout,
                self.sort.field,
                &self.collapsed_groups,
                &self.group_motions,
                &self.group_heights,
                self.selected_url.as_deref(),
                &theme,
                cx,
            )
        };
        let load_more = (!initial_loading && !self.items.is_empty())
            .then(|| self.render_load_more(&theme, cx))
            .flatten();
        let scrollbar = popover::rail(self, "pull-requests-scrollbar", &theme, cx);
        if self.filter_fades.tick_at(Instant::now())
            || self.group_motions.values().any(|motion| motion.animating())
        {
            window.request_animation_frame();
        }
        // Keep the page actions reachable while browsing a long list. Only the
        // results scroll; the shared edges remain identical in every layout.
        div()
            .id("pull-requests-page")
            .size_full()
            .flex()
            .flex_col()
            .pt(px(Theme::TITLEBAR_HEIGHT + Theme::SPACE_LG))
            .child(header)
            .child(
                div()
                    .id("pull-requests-scroll-host")
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
                            PR_SCROLL_FADE_BAND,
                            true,
                            true,
                            div()
                                .id("pull-requests-scroll")
                                .debug_selector(|| "pull-requests-scroll".into())
                                .size_full()
                                .overflow_y_scroll()
                                .track_scroll(&scroll)
                                .child(
                                    div()
                                        .w_full()
                                        .max_w(px(PR_PAGE_MAX_WIDTH))
                                        .mx_auto()
                                        .relative()
                                        .px(px(PR_PAGE_HORIZONTAL_PADDING))
                                        .pt(px(8.0))
                                        .pb(px(32.0))
                                        .child(width_probe)
                                        .child(content)
                                        .children(load_more),
                                ),
                        )
                        .fade_overflow_y(&scroll),
                    )
                    .children(scrollbar),
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PullRequestTableLayout {
    Narrow,
    Compact,
    Wide,
}

fn table_layout(width: f32) -> PullRequestTableLayout {
    if width < 640.0 {
        PullRequestTableLayout::Narrow
    } else if width < 900.0 {
        PullRequestTableLayout::Compact
    } else {
        PullRequestTableLayout::Wide
    }
}

fn table_content_width(container_width: f32) -> f32 {
    (container_width - PR_PAGE_HORIZONTAL_PADDING * 2.0)
        .max(0.0)
        .min(PR_PAGE_MAX_WIDTH - PR_PAGE_HORIZONTAL_PADDING * 2.0)
}

fn sort_pull_requests(items: &mut [ChangeRequestListItem], sort: PullRequestSort) {
    items.sort_by(|left, right| {
        let primary = match sort.field {
            PullRequestSortField::Changes => left
                .additions
                .saturating_add(left.deletions)
                .cmp(&right.additions.saturating_add(right.deletions)),
            PullRequestSortField::Opened => left.created_at.cmp(&right.created_at),
            PullRequestSortField::Updated => left.updated_at.cmp(&right.updated_at),
        };
        let primary = match sort.direction {
            SortDirection::Ascending => primary,
            SortDirection::Descending => primary.reverse(),
        };
        primary
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.repository.cmp(&right.repository))
            .then_with(|| left.number.cmp(&right.number))
    });
}

fn pull_request_key(item: &ChangeRequestListItem) -> String {
    format!("{}#{}", item.repository, item.number)
}

fn render_grouped_requests(
    items: &[ChangeRequestListItem],
    layout: PullRequestTableLayout,
    sort_field: PullRequestSortField,
    collapsed: &HashSet<PullRequestGroup>,
    motions: &HashMap<PullRequestGroup, crate::motion::DisclosureMotion>,
    heights: &HashMap<PullRequestGroup, Rc<Cell<f32>>>,
    selected_url: Option<&str>,
    theme: &Theme,
    cx: &mut Context<PullRequestsPage>,
) -> AnyElement {
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(32.0))
        .children(PullRequestGroup::ALL.into_iter().filter_map(|group| {
            let rows: Vec<_> = items
                .iter()
                .filter(|item| request_group(item) == group)
                .collect();
            if rows.is_empty() {
                return None;
            }
            let closed = collapsed.contains(&group);
            let measured = heights[&group].clone();
            let full_height = measured.get().max(1.0);
            let tween = motions
                .get(&group)
                .copied()
                .filter(|motion| motion.animating());
            let reveal = tween.map_or(if closed { 0.0 } else { 1.0 }, |motion| {
                (motion.current() / full_height).clamp(0.0, 1.0)
            });
            Some(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(
                        div()
                            .id(SharedString::from(format!(
                                "pull-requests-group-{}",
                                group.key()
                            )))
                            .debug_selector(move || format!("pull-requests-group-{}", group.key()))
                            .role(gpui::Role::Button)
                            .aria_label(format!("{}, {} pull requests", group.label(), rows.len()))
                            .aria_expanded(!closed)
                            .tab_index(0)
                            .min_h(px(24.0))
                            .px(px(8.0))
                            .flex()
                            .items_center()
                            .gap(px(Theme::SPACE_SM))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(gpui::transparent_black())
                            .focus_visible(|style| style.border_2().border_color(theme.accent))
                            .cursor_pointer()
                            .hover(|style| style.bg(crate::theme::ink(0.025)))
                            .on_click(cx.listener(move |page, _, _, cx| {
                                let height = page.group_heights[&group].get();
                                let previous = page.group_motions.get(&group).copied();
                                let from = previous
                                    .filter(|motion| motion.animating())
                                    .map(|motion| motion.current())
                                    .unwrap_or(if closed { 0.0 } else { height });
                                if crate::motion::reduced_motion(cx) {
                                    page.group_motions.remove(&group);
                                } else {
                                    page.group_motions.insert(
                                        group,
                                        crate::motion::DisclosureMotion::new(
                                            previous.map_or(1, |motion| motion.epoch + 1),
                                            from,
                                            if closed { height } else { 0.0 },
                                        ),
                                    );
                                }
                                if !page.collapsed_groups.remove(&group) {
                                    page.collapsed_groups.insert(group);
                                }
                                cx.notify();
                            }))
                            .child(
                                icon(icons::ALT_ARROW_RIGHT)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted)
                                    .with_transformation(gpui::Transformation::rotate(
                                        gpui::percentage(reveal * 0.25),
                                    )),
                            )
                            .child(widgets::section_label(theme, group.label()).px_0())
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(11.0))
                                    .text_color(theme.text_muted)
                                    .child(rows.len().to_string()),
                            ),
                    )
                    .when(!closed || tween.is_some(), |el| {
                        let row_count = rows.len();
                        let content = div()
                            .w_full()
                            .flex_none()
                            .relative()
                            .child(
                                gpui::canvas(
                                    move |bounds, _, _| {
                                        measured.set(f32::from(bounds.size.height));
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            )
                            .children(rows.into_iter().enumerate().map(|(index, item)| {
                                render_table_row(
                                    item,
                                    layout,
                                    sort_field,
                                    index == 0,
                                    index + 1 == row_count,
                                    selected_url == Some(item.url.as_str()),
                                    theme,
                                    cx,
                                )
                            }));
                        el.child(
                            widgets::section_card(theme)
                                .mt_0()
                                .w_full()
                                .when_some(tween, |el, motion| {
                                    el.h(px(motion.current()))
                                        .opacity(0.35 + 0.65 * reveal)
                                        .relative()
                                        .top(px(-3.0 * (1.0 - reveal)))
                                })
                                .child(content),
                        )
                    })
                    .into_any_element(),
            )
        }))
        .into_any_element()
}

pub(crate) fn table_row_shell(
    layout: PullRequestTableLayout,
    first: bool,
    last: bool,
    theme: &Theme,
) -> gpui::Div {
    widgets::card_row(theme, first)
        .mx_0()
        .px(px(16.0))
        .when(first, |row| row.rounded_t(px(12.0)))
        .when(last, |row| row.rounded_b(px(12.0)))
        .flex_nowrap()
        .min_w_0()
        .min_h(px(PR_TABLE_ROW_HEIGHT))
        .flex_none()
        .flex()
        .when(layout == PullRequestTableLayout::Narrow, |row| {
            row.flex_col().items_stretch().gap(px(Theme::SPACE_SM))
        })
        .when(layout != PullRequestTableLayout::Narrow, |row| {
            row.items_center().gap(px(Theme::SPACE_LG))
        })
}

fn render_table_row(
    item: &ChangeRequestListItem,
    layout: PullRequestTableLayout,
    sort_field: PullRequestSortField,
    first: bool,
    last: bool,
    selected: bool,
    theme: &Theme,
    cx: &gpui::App,
) -> AnyElement {
    let star = star_button(&item.url, theme, cx);
    let url = item.url.clone();
    let row = table_row_shell(layout, first, last, theme)
        .id(SharedString::from(format!(
            "pull-request-row-{}",
            pull_request_key(item)
        )))
        .debug_selector(|| "pull-request-row".to_string())
        .role(gpui::Role::Link)
        .aria_label(format!(
            "{}: {} #{}. {}. Open pull request",
            item.title,
            item.repository,
            item.number,
            status_description(item)
        ))
        .tab_index(0)
        .when(selected, |el| el.bg(theme.glass_hover()))
        .focus_visible(|style| style.border_2().border_color(theme.accent))
        .cursor_pointer()
        .hover(|style| style.bg(theme.glass_hover()))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            crate::pull_request_detail::open(&url, window, cx);
        });
    let (date_label, timestamp) = if sort_field == PullRequestSortField::Opened {
        ("Opened", item.created_at)
    } else {
        ("Updated", item.updated_at)
    };
    let updated = || {
        let exact = SharedString::from(format!(
            "{date_label} {} · {}",
            relative_time(timestamp, Utc::now()),
            timestamp.format("%b %d, %Y at %H:%M UTC")
        ));
        div()
            .id(SharedString::from(format!(
                "board-pr-date-{}",
                pull_request_key(item)
            )))
            .text_size(crate::typography::ui_rems(11.0))
            .text_color(theme.text_muted)
            .tooltip(widgets::text_tooltip(exact))
            .child(SharedString::from(
                if sort_field == PullRequestSortField::Opened {
                    format!("Opened {}", compact_relative_time(timestamp, Utc::now()))
                } else {
                    compact_relative_time(timestamp, Utc::now())
                },
            ))
    };
    if layout == PullRequestTableLayout::Narrow {
        row.child(render_pr_identity(item, theme))
            .child(
                div()
                    .pl(px(22.0))
                    .flex()
                    .items_center()
                    .gap(px(Theme::SPACE_SM))
                    .child(render_diff_stats(item, theme))
                    .child(div().flex_1())
                    .child(updated())
                    .child(star),
            )
            .into_any_element()
    } else {
        row.child(render_pr_identity(item, theme))
            .child(
                div()
                    .w(px(112.0))
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(Theme::SPACE_XS))
                    .child(updated())
                    .child(render_diff_stats(item, theme)),
            )
            .child(star)
            .into_any_element()
    }
}

fn status_description(item: &ChangeRequestListItem) -> String {
    let assessment = Facts::from_item(item).assess();
    let mut labels = vec![if item.is_draft { "Draft" } else { "Open" }];
    labels.extend(assessment.blockers.iter().map(|blocker| blocker.label()));
    if item.review_decision == ChangeRequestReviewDecision::Approved {
        labels.push("Approved");
    }
    labels.extend(
        assessment
            .attention
            .iter()
            .filter(|reason| {
                **reason != zeron_proto::change_request_assessment::AttentionReason::FailingCi
            })
            .map(|reason| reason.label()),
    );
    labels.join(" · ")
}

fn render_pr_identity(item: &ChangeRequestListItem, theme: &Theme) -> AnyElement {
    let title = SharedString::from(single_line(&item.title));
    let full_title = title.clone();
    let assessment = Facts::from_item(item).assess();
    let mut labels: Vec<_> = assessment
        .blockers
        .iter()
        .filter(|blocker| **blocker != Blocker::FailingCi)
        .map(|blocker| blocker.label())
        .collect();
    if item.is_draft {
        labels.insert(0, "Draft");
    }
    if item.review_decision == ChangeRequestReviewDecision::Approved {
        labels.push("Approved");
    }
    labels.extend(
        assessment
            .attention
            .iter()
            .filter(|reason| {
                **reason != zeron_proto::change_request_assessment::AttentionReason::FailingCi
            })
            .map(|reason| reason.label()),
    );
    let status = labels.join(" · ");
    let tone = if item.mergeability == ChangeRequestMergeability::Conflicting {
        theme.danger_muted
    } else if item.review_decision == ChangeRequestReviewDecision::ChangesRequested {
        theme.warning
    } else {
        theme.text_muted
    };
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(Theme::SPACE_XS))
        .child(
            div()
                .id(SharedString::from(format!(
                    "pull-request-title-{}",
                    pull_request_key(item)
                )))
                .debug_selector(|| "pull-request-title".to_string())
                .min_w_0()
                .truncate()
                .text_size(crate::typography::ui_rems(widgets::ROW_TITLE_SIZE))
                .text_color(theme.text)
                .tooltip(widgets::text_tooltip(full_title))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    icon(request_glyph(item))
                        .size(px(14.0))
                        .text_color(request_color(item, theme)),
                )
                .child(div().min_w_0().truncate().child(title)),
        )
        .child(
            div()
                .flex()
                .flex_wrap()
                .pl(px(22.0))
                .items_center()
                .gap(px(Theme::SPACE_SM))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.0))
                        .min_w_0()
                        .max_w(px(150.0))
                        .child(super::pull_request_media::avatar(
                            &item.author.login,
                            format!("pr-list-author-{}", pull_request_key(item)).into(),
                            16.0,
                            theme,
                        ))
                        .child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(if item.author.login.is_empty() {
                                    "Unknown author".to_owned()
                                } else {
                                    item.author.login.clone()
                                }),
                        ),
                )
                .child(crate::change_requests::pull_request_list_badge(
                    SharedString::from(format!("board-pr-badge-{}", pull_request_key(item))),
                    item,
                    theme,
                ))
                .child(
                    div()
                        .id(SharedString::from(format!(
                            "pr-ci-{}",
                            pull_request_key(item)
                        )))
                        .debug_selector(|| "pr-row-ci".into())
                        .min_w_0()
                        .truncate()
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(match item.ci.state {
                            CiState::Failed => theme.danger_muted,
                            CiState::Passed => theme.success_muted,
                            _ => theme.text_muted,
                        })
                        .tooltip(widgets::text_tooltip(format!(
                            "{} · Commit: {}\n{}",
                            item.ci.state.label(),
                            if item.head_ref_oid.is_empty() {
                                "unavailable"
                            } else {
                                &item.head_ref_oid
                            },
                            format!(
                                "{}\nNext: {}",
                                assessment
                                    .missing
                                    .iter()
                                    .map(|missing| missing.label())
                                    .collect::<Vec<_>>()
                                    .join(" · "),
                                assessment
                                    .actions
                                    .first()
                                    .map_or("No action suggested", |action| action.label())
                            )
                        )))
                        .child(format!(
                            "{}{}",
                            item.ci.state.label(),
                            if item.head_ref_oid.is_empty() {
                                String::new()
                            } else {
                                format!(
                                    " · {}",
                                    item.head_ref_oid.chars().take(7).collect::<String>()
                                )
                            }
                        )),
                )
                .when(!status.is_empty(), |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(tone)
                            .child(status.trim_start_matches("Open · ").to_string()),
                    )
                }),
        )
        .into_any_element()
}

fn render_diff_stats(item: &ChangeRequestListItem, theme: &Theme) -> AnyElement {
    let exact = SharedString::from(format!(
        "{} additions, {} deletions",
        item.additions, item.deletions
    ));
    div()
        .id(SharedString::from(format!(
            "pull-request-diff-{}",
            pull_request_key(item)
        )))
        .flex()
        .items_center()
        .gap(px(8.0))
        .font_family(theme.font_mono.clone())
        .text_size(crate::typography::ui_rems(11.0))
        .tooltip(widgets::text_tooltip(exact))
        .child(
            div()
                .text_color(theme.success_muted)
                .child(SharedString::from(format!(
                    "+{}",
                    format_compact_count(item.additions)
                ))),
        )
        .child(
            div()
                .text_color(theme.danger_muted)
                .child(SharedString::from(format!(
                    "−{}",
                    format_compact_count(item.deletions)
                ))),
        )
        .into_any_element()
}

fn eligible_desktop_devices(devices: &[Device]) -> Vec<Device> {
    let mut devices: Vec<_> = devices
        .iter()
        .filter(|device| !matches!(device.platform.as_str(), "ios" | "android"))
        .cloned()
        .collect();
    devices.sort_by(|left, right| {
        left.created_at
            .cmp(&right.created_at)
            .then_with(|| left.id.cmp(&right.id))
    });
    devices
}

fn normalized_target_device(
    target: Option<&str>,
    devices: &[Device],
    local_id: Option<&str>,
) -> Option<String> {
    let target = target?;
    if local_id == Some(target) {
        return None;
    }

    // The local row is inserted before the first device snapshot is emitted.
    // Until it arrives, keep the selection instead of judging an empty or
    // partial pre-sync list as authoritative.
    let local_is_present = local_id
        .is_some_and(|local_id| devices.iter().any(|device| device.id.as_str() == local_id));
    if !local_is_present
        || devices.iter().any(|device| {
            device.id == target && !matches!(device.platform.as_str(), "ios" | "android")
        })
    {
        Some(target.to_string())
    } else {
        None
    }
}

/// A project folder on a device.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProjectRef {
    path: String,
    device: String,
}

#[derive(Debug, PartialEq, Eq)]
enum ContextProject {
    /// The sidebar names a project whose row has not synced yet.
    Waiting,
    Project(ProjectRef),
    None,
}

/// Where the board is opened from: the sidebar's project, else the open
/// session's project.
fn context_project(state: &AppState, space_filter: Option<&str>) -> ContextProject {
    let project = |id: &str| {
        state
            .spaces
            .iter()
            .find(|space| space.id == id)
            .map(|space| ProjectRef {
                path: space.path.clone(),
                device: space.device_id.clone(),
            })
    };
    if let Some(id) = space_filter {
        let Some(space) = state.spaces.iter().find(|space| space.id == id) else {
            return ContextProject::Waiting;
        };
        let members = state.project_members(space);
        let local = state.local_device_id.as_deref().or_else(|| {
            state
                .engine()
                .map(|engine| engine.engine_info().device_id.as_str())
        });
        let selected = state
            .selected_chat_row()
            .and_then(|chat| chat.space_id.as_deref())
            .and_then(|id| members.iter().find(|member| member.id == id).copied());
        let preferred = selected
            .or_else(|| {
                members
                    .iter()
                    .copied()
                    .find(|member| local == Some(member.device_id.as_str()) && member.git_detected)
            })
            .unwrap_or(space);
        return ContextProject::Project(ProjectRef {
            path: preferred.path.clone(),
            device: preferred.device_id.clone(),
        });
    }
    state
        .selected_chat_row()
        .and_then(|chat| chat.space_id.as_deref())
        .and_then(project)
        .map_or(ContextProject::None, ContextProject::Project)
}

/// Git projects to try when nothing points at one: this device's first,
/// most recent session activity first. Bounded: each costs a lookup.
fn recent_git_projects(state: &AppState, local: &str) -> Vec<ProjectRef> {
    let mut projects: Vec<_> = state
        .spaces
        .iter()
        .filter(|space| space.git_detected)
        .map(|space| {
            let active = state
                .chats
                .iter()
                .filter(|chat| chat.space_id.as_deref() == Some(space.id.as_str()))
                .filter_map(|chat| chat.last_message_at)
                .max()
                .unwrap_or(space.created_at);
            (
                space.device_id != local,
                space.device_id != local && !state.device_online(&space.device_id, Utc::now()),
                std::cmp::Reverse(active),
                space,
            )
        })
        .collect();
    projects.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));
    let mut seen = HashSet::new();
    let (primary, alternatives): (Vec<_>, Vec<_>) = projects
        .into_iter()
        .partition(|(.., space)| seen.insert(zeron_proto::view::project_key(space)));
    primary
        .into_iter()
        .chain(alternatives)
        .take(4)
        .map(|(.., space)| ProjectRef {
            path: space.path.clone(),
            device: space.device_id.clone(),
        })
        .collect()
}

fn repository_menu_label(theme: &Theme, label: &str) -> gpui::Div {
    widgets::section_label(theme, label.to_owned())
        .py(px(4.0))
        .text_size(crate::typography::ui_rems(11.0))
}

fn valid_repository_filter(value: &str) -> bool {
    let parts: Vec<_> = value.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && *part != "."
                && *part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        })
}

fn params_for_target(target: Option<&str>) -> serde_json::Value {
    match target {
        Some(target) => serde_json::json!({ "targetDeviceId": target }),
        None => serde_json::json!({}),
    }
}

fn selected_device_name(state: &AppState, target: Option<&str>) -> String {
    target
        .or(state.local_device_id.as_deref())
        .and_then(|id| state.device_name(id))
        .map(single_line)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "This device".to_string())
}

fn map_rpc_error(error: &RpcError, target_name: &str) -> PullRequestsPageError {
    match error {
        RpcError::Capability(code) if code == capability_errors::PULL_REQUESTS_CLI_UNAVAILABLE => {
            PullRequestsPageError::CliUnavailable
        }
        RpcError::Capability(code) if code == capability_errors::PULL_REQUESTS_AUTHENTICATION => {
            PullRequestsPageError::Authentication
        }
        RpcError::Capability(code) if code == capability_errors::PULL_REQUESTS_RATE_LIMITED => {
            PullRequestsPageError::RateLimited
        }
        RpcError::UnknownMethod(_) => {
            PullRequestsPageError::UpdateRequired(target_name.to_string())
        }
        RpcError::Capability(_)
        | RpcError::BadParams(_)
        | RpcError::Failed(_)
        | RpcError::Transport(_)
        | RpcError::Closed => PullRequestsPageError::Network,
    }
}

fn error_copy(error: &PullRequestsPageError) -> (String, String) {
    match error {
        PullRequestsPageError::CliUnavailable => (
            "GitHub CLI isn’t available on this device".into(),
            "Install gh and sign in to view your pull requests.".into(),
        ),
        PullRequestsPageError::Authentication => (
            "Sign in to GitHub on this device".into(),
            "Run gh auth login, then refresh this page.".into(),
        ),
        PullRequestsPageError::RateLimited => (
            "GitHub’s rate limit was reached".into(),
            "Requests are paused for 15 minutes. Your loaded results stay available.".into(),
        ),
        PullRequestsPageError::Network => (
            "Couldn’t load pull requests".into(),
            "Check the repository name and your connection, then try again.".into(),
        ),
        PullRequestsPageError::RemoteOffline(name) => (
            format!("{name} is offline"),
            "Reconnect the device and try again.".into(),
        ),
        PullRequestsPageError::UpdateRequired(name) => (
            format!("Update Zeron on {name}"),
            "This device doesn’t support pull requests yet.".into(),
        ),
    }
}

fn platform_icon(platform: &str) -> &'static str {
    match platform {
        "macos" | "darwin" => icons::LAPTOP,
        _ => icons::MONITOR,
    }
}

pub(crate) fn relative_time(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = now.signed_duration_since(timestamp).num_seconds().max(0);
    let (amount, unit) = if seconds < 60 {
        return "just now".into();
    } else if seconds < 3_600 {
        (seconds / 60, "minute")
    } else if seconds < 86_400 {
        (seconds / 3_600, "hour")
    } else if seconds < 604_800 {
        (seconds / 86_400, "day")
    } else if seconds < 2_592_000 {
        (seconds / 604_800, "week")
    } else if seconds < 31_536_000 {
        (seconds / 2_592_000, "month")
    } else {
        (seconds / 31_536_000, "year")
    };
    format!("{amount} {unit}{} ago", if amount == 1 { "" } else { "s" })
}

fn compact_relative_time(timestamp: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let seconds = now.signed_duration_since(timestamp).num_seconds().max(0);
    if seconds < 60 {
        "now".into()
    } else if seconds < 3_600 {
        format!("{}m ago", seconds / 60)
    } else if seconds < 86_400 {
        format!("{}h ago", seconds / 3_600)
    } else if seconds < 604_800 {
        format!("{}d ago", seconds / 86_400)
    } else if seconds < 2_592_000 {
        format!("{}w ago", seconds / 604_800)
    } else {
        format!("{}mo ago", seconds / 2_592_000)
    }
}

fn format_compact_count(value: u64) -> String {
    if value < 1_000 {
        value.to_string()
    } else if value < 1_000_000 {
        let tenths = value / 100;
        if tenths % 10 == 0 {
            format!("{}k", value / 1_000)
        } else {
            format!("{}.{:01}k", value / 1_000, tenths % 10)
        }
    } else {
        let tenths = value / 100_000;
        if tenths % 10 == 0 {
            format!("{}m", value / 1_000_000)
        } else {
            format!("{}.{:01}m", value / 1_000_000, tenths % 10)
        }
    }
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn fetch_page(
    engine: &crate::state::EngineHandle,
    params: serde_json::Value,
) -> Result<ChangeRequestPage, RpcError> {
    let value = engine
        .client()
        .call(methods::LIST_CHANGE_REQUEST_PAGE, params)
        .await?;
    serde_json::from_value(value).map_err(|error| RpcError::Failed(error.to_string()))
}

fn settle_snapshot<T>(
    snapshot: &mut Vec<T>,
    result: Result<Vec<T>, PullRequestsPageError>,
) -> PullRequestsLoadState {
    match result {
        Ok(items) => {
            *snapshot = items;
            PullRequestsLoadState::Ready
        }
        Err(error) => PullRequestsLoadState::Failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pull_request_test_support::{self as fixture, ScriptedRpc};
    use chrono::{TimeDelta, TimeZone};

    fn device(id: &str, platform: &str) -> Device {
        Device {
            id: id.into(),
            name: id.into(),
            platform: platform.into(),
            last_seen_at: None,
            created_at: None,
            version: None,
            cursor_sdk_version: None,
            capabilities: Vec::new(),
        }
    }

    fn pull_request(
        repository: &str,
        number: u64,
        changes: u64,
        opened_day: u32,
        updated_hour: u32,
    ) -> ChangeRequestListItem {
        ChangeRequestListItem {
            provider: "github".into(),
            author: Default::default(),
            head_ref_oid: String::new(),
            ci: Default::default(),
            viewer_did_author: None,
            viewer_review_requested: None,
            repository: repository.into(),
            number,
            title: format!("Pull request {number}"),
            url: format!("https://github.com/{repository}/pull/{number}"),
            state: zeron_proto::ChangeRequestState::Open,
            is_draft: false,
            review_decision: ChangeRequestReviewDecision::Unknown,
            additions: changes,
            deletions: 0,
            mergeability: ChangeRequestMergeability::Mergeable,
            created_at: Utc.with_ymd_and_hms(2026, 8, opened_day, 8, 0, 0).unwrap(),
            updated_at: Utc
                .with_ymd_and_hms(2026, 8, 19, updated_hour, 0, 0)
                .unwrap(),
        }
    }

    struct RowLayoutFixture {
        item: ChangeRequestListItem,
    }

    impl Render for RowLayoutFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let width = f32::from(window.viewport_size().width);
            div().w_full().child(render_table_row(
                &self.item,
                table_layout(width),
                PullRequestSortField::Updated,
                true,
                true,
                false,
                Theme::of(cx),
                cx,
            ))
        }
    }

    fn space(id: &str, device: &str, git: bool, created_minutes_ago: i64) -> zeron_proto::Space {
        zeron_proto::Space {
            id: id.into(),
            device_id: device.into(),
            path: format!("/{id}"),
            name: None,
            git_detected: git,
            git_checked_at: None,
            checkout_id: None,
            repository_id: None,
            created_at: Utc::now() - TimeDelta::minutes(created_minutes_ago),
        }
    }

    fn session(space: &str, last_message_at: Option<DateTime<Utc>>) -> zeron_proto::Chat {
        zeron_proto::Chat {
            id: "chat".into(),
            device_id: "local".into(),
            title: None,
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at,
            created_at: Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            parent_chat_id: None,
            space_id: Some(space.into()),
            last_seen_at: None,
            room_gen: None,
        }
    }

    fn project(id: &str, device: &str) -> ProjectRef {
        ProjectRef {
            path: format!("/{id}"),
            device: device.into(),
        }
    }

    #[test]
    fn pull_request_board_opens_on_the_sidebar_or_session_project() {
        let mut state = AppState::new();
        state.spaces = vec![
            space("comet", "local", true, 60),
            space("other", "local", true, 60),
        ];
        let mut chat = session("comet", None);
        state.chats = vec![chat.clone()];
        state.selected_chat = Some("chat".into());
        assert_eq!(
            context_project(&state, None),
            ContextProject::Project(project("comet", "local")),
            "the open session's project"
        );
        assert_eq!(
            context_project(&state, Some("other")),
            ContextProject::Project(project("other", "local")),
            "the sidebar's project wins"
        );
        assert_eq!(
            context_project(&state, Some("unsynced")),
            ContextProject::Waiting
        );
        chat.space_id = None;
        state.chats = vec![chat];
        assert_eq!(context_project(&state, None), ContextProject::None);
        state.local_device_id = Some("local".into());
        state.spaces[0].repository_id = Some("commit:shared".into());
        let mut representative = space("representative", "remote", true, 900);
        representative.repository_id = Some("commit:shared".into());
        state.spaces.push(representative);
        assert_eq!(
            context_project(&state, Some("representative")),
            ContextProject::Project(project("comet", "local"))
        );
    }

    #[test]
    fn pull_request_board_first_run_tries_recent_local_git_projects() {
        let mut state = AppState::new();
        state.spaces = vec![
            space("old", "local", true, 600),
            space("remote", "remote", true, 1),
            space("notes", "local", false, 1),
            space("active", "local", true, 900),
            space("new", "local", true, 5),
        ];
        state.chats = vec![session("active", Some(Utc::now()))];
        state
            .spaces
            .iter_mut()
            .find(|space| space.id == "active")
            .unwrap()
            .repository_id = Some("commit:shared".into());
        for id in ["clone-a", "clone-b", "worktree"] {
            let mut duplicate = space(id, "local", true, 1);
            duplicate.repository_id = Some("commit:shared".into());
            state.spaces.push(duplicate);
        }
        assert_eq!(
            recent_git_projects(&state, "local"),
            [
                project("active", "local"),
                project("new", "local"),
                project("old", "local"),
                project("remote", "remote"),
            ],
            "session activity first, this device before others, Git only"
        );
    }

    #[gpui::test]
    fn pull_request_board_defaults_to_useful_results_and_respects_explicit_filters(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let _settings = fixture::settings(cx, crate::settings::UiSettings::default());
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let delayed = release.clone();
        let rpc = ScriptedRpc::new(move |method, params| {
            let recorded = recorded.clone();
            let delayed = delayed.clone();
            async move {
                assert_eq!(method, methods::LIST_CHANGE_REQUEST_PAGE);
                let filter = params["filter"].as_str().unwrap().to_owned();
                recorded.lock().unwrap().push(filter.clone());
                let repository = params["repository"].as_str().unwrap();
                if repository == "failed/repo" {
                    return Err(RpcError::Capability(
                        capability_errors::PULL_REQUESTS_AUTHENTICATION.into(),
                    ));
                }
                if repository == "slow/repo" {
                    delayed.notified().await;
                }
                zeron_rpc::RpcReply::value(&ChangeRequestPage {
                    items: if repository == "authored/repo"
                        || (filter == "all" && repository != "empty/repo")
                    {
                        vec![pull_request(repository, 7, 1, 1, 1)]
                    } else {
                        Vec::new()
                    },
                    next_cursor: None,
                    total_count: None,
                })
            }
        });
        let client = rpc.client();
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = fixture::state(cx, Some(client));
            PullRequestsPage::new(state, cx)
        });
        page.update(cx, |page, cx| {
            page.apply_initial_repository("owner/repo".into(), None, cx)
        });
        settle(
            cx,
            &runtime,
            |page| page.load_state == PullRequestsLoadState::Ready,
            &page,
            &rpc,
        );
        page.read_with(cx, |page, _| {
            assert_eq!(page.filter, ChangeRequestFilter::All);
            assert_eq!(page.items[0].number, 7);
        });
        assert_eq!(*calls.lock().unwrap(), ["authored", "all"]);
        page.update(cx, |page, cx| {
            page.select_filter(ChangeRequestFilter::Authored, cx)
        });
        settle(
            cx,
            &runtime,
            |page| page.load_state == PullRequestsLoadState::Ready,
            &page,
            &rpc,
        );
        page.update(cx, |page, cx| {
            assert_eq!(page.filter, ChangeRequestFilter::Authored);
            assert!(page.items.is_empty());
            page.refresh(cx);
        });
        settle(
            cx,
            &runtime,
            |page| page.load_state == PullRequestsLoadState::Ready,
            &page,
            &rpc,
        );
        assert_eq!(
            *calls.lock().unwrap(),
            ["authored", "all", "authored", "authored"]
        );
        page.update(cx, |page, cx| {
            page.apply_initial_repository("other/repo".into(), None, cx);
            page.apply_initial_repository("owner/repo".into(), None, cx);
            assert_eq!(page.filter, ChangeRequestFilter::Authored);
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            assert!(page.items.is_empty());
        });
        calls.lock().unwrap().clear();
        for repository in ["authored/repo", "failed/repo", "empty/repo"] {
            page.update(cx, |page, cx| {
                page.apply_initial_repository(repository.into(), None, cx)
            });
            settle(
                cx,
                &runtime,
                |page| !matches!(page.load_state, PullRequestsLoadState::Loading),
                &page,
                &rpc,
            );
            page.read_with(cx, |page, _| match repository {
                "authored/repo" => {
                    assert_eq!(page.filter, ChangeRequestFilter::Authored);
                    assert_eq!(page.items.len(), 1);
                }
                "failed/repo" => {
                    assert_eq!(page.filter, ChangeRequestFilter::Authored);
                    assert_eq!(
                        page.load_state,
                        PullRequestsLoadState::Failed(PullRequestsPageError::Authentication)
                    );
                }
                _ => {
                    assert_eq!(page.filter, ChangeRequestFilter::All);
                    assert!(page.items.is_empty());
                }
            });
        }
        assert_eq!(
            *calls.lock().unwrap(),
            ["authored", "authored", "authored", "all"]
        );
        calls.lock().unwrap().clear();
        page.update(cx, |page, cx| {
            page.apply_initial_repository("slow/repo".into(), None, cx)
        });
        rpc.settle(cx, &runtime, |_| !calls.lock().unwrap().is_empty());
        page.update(cx, |page, cx| {
            assert_eq!(page.load_state, PullRequestsLoadState::Loading);
            // Clicking the already selected Authored filter is still an explicit choice.
            page.select_filter(ChangeRequestFilter::Authored, cx);
        });
        release.notify_one();
        settle(
            cx,
            &runtime,
            |page| page.load_state == PullRequestsLoadState::Ready,
            &page,
            &rpc,
        );
        page.read_with(cx, |page, _| {
            assert_eq!(page.filter, ChangeRequestFilter::Authored);
            assert!(page.items.is_empty());
        });
        assert_eq!(*calls.lock().unwrap(), ["authored"]);
        cx.run_until_parked();
        assert!(cx.debug_bounds("pr-empty-show-all").is_some());
    }

    #[gpui::test]
    fn pull_request_default_scope_reuses_cache_and_persists_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        let _settings = fixture::settings(cx, crate::settings::UiSettings::default());
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            PullRequestsPage::new(state, cx)
        });
        page.update(cx, |page, cx| {
            page.snapshots.push((
                (
                    Some("remote".into()),
                    "saved/repo".into(),
                    ChangeRequestFilter::Authored,
                ),
                vec![pull_request("saved/repo", 10, 1, 1, 1)],
                Instant::now(),
                Paging::default(),
            ));
            page.apply_initial_repository("saved/repo".into(), Some("remote".into()), cx);
            assert_eq!(page.items[0].number, 10);
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            let settings = crate::settings::current(cx);
            let restored: crate::settings::UiSettings =
                serde_json::from_value(serde_json::to_value(settings).unwrap()).unwrap();
            assert_eq!(
                restored.last_pull_request_repository.as_deref(),
                Some("saved/repo")
            );
            assert_eq!(restored.last_pull_request_device.as_deref(), Some("remote"));
        });
    }

    #[gpui::test]
    fn pull_request_board_restores_the_selected_repository_cache(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        fixture::init(cx);
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            PullRequestsPage::new(state, cx)
        });
        page.update(cx, |page, cx| {
            assert_eq!(page.load_state, PullRequestsLoadState::Idle);
            assert_eq!(page.filter, ChangeRequestFilter::Authored);
            page.on_hidden();
            page.on_visible(cx);
            assert_eq!(page.load_state, PullRequestsLoadState::Idle);
            // No engine exists. Any accidental request would produce Network.
            page.snapshots.push((
                (None, "acme/zeron".into(), ChangeRequestFilter::Authored),
                vec![pull_request("acme/zeron", 10, 1, 1, 1)],
                Instant::now(),
                Paging::default(),
            ));
            page.repository_input
                .update(cx, |input, cx| input.set_text("ACME/ZERON", cx));
            page.select_repository(cx);
            assert_eq!(page.items[0].number, 10);
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            page.on_visible(cx);
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            page.reset_for_target(Some("other-device".into()), cx);
            assert!(page.items.is_empty());
            assert_eq!(page.load_state, PullRequestsLoadState::Idle);
            page.reset_for_target(None, cx);
            assert_eq!(page.items[0].number, 10);
            page.repository_input.update(cx, |input, cx| {
                input.set_text("acme/zeron repo:other/repo", cx)
            });
            page.select_repository(cx);
            assert!(page.repository_error.is_some());
            assert_eq!(page.repository.as_deref(), Some("acme/zeron"));
        });
    }

    fn initial_repository_rpc(
        calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        stalled: bool,
    ) -> std::sync::Arc<ScriptedRpc> {
        ScriptedRpc::new(move |method, params| {
            let calls = calls.clone();
            async move {
                calls.lock().unwrap().push(method.clone());
                assert!(
                    params.get("targetDeviceId").is_none(),
                    "local identity comes from engine before device frames"
                );
                match method.as_str() {
                    methods::GET_CHANGE_REQUEST_REPOSITORY => {
                        assert_eq!(params["cwd"], "/checkout");
                        if stalled {
                            return futures::future::pending().await;
                        }
                        zeron_rpc::RpcReply::value(&Some("owner/repo"))
                    }
                    methods::LIST_CHANGE_REQUEST_PAGE => {
                        assert_eq!(params["repository"], "owner/repo");
                        assert_eq!(params["filter"], "authored");
                        zeron_rpc::RpcReply::value(&ChangeRequestPage {
                            items: vec![pull_request("owner/canonical", 7, 1, 1, 1)],
                            next_cursor: None,
                            total_count: Some(1),
                        })
                    }
                    _ => panic!("unexpected request {method}"),
                }
            }
        })
    }

    #[gpui::test]
    fn pull_request_repository_picker_keeps_selection_and_cancels_detection(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let _settings = fixture::settings(cx, crate::settings::UiSettings::default());
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let rpc = ScriptedRpc::new({
            let calls = calls.clone();
            let release = release.clone();
            move |method, params| {
                let calls = calls.clone();
                let release = release.clone();
                async move {
                    match method.as_str() {
                        methods::GET_CHANGE_REQUEST_REPOSITORY => {
                            let path = params["cwd"].as_str().unwrap().to_owned();
                            calls.lock().unwrap().push(path.clone());
                            if path == "/slow" {
                                release.notified().await;
                            }
                            zeron_rpc::RpcReply::value(
                                &(path != "/scratch").then(|| format!("owner{}", path)),
                            )
                        }
                        methods::LIST_CHANGE_REQUEST_PAGE => {
                            zeron_rpc::RpcReply::value(&ChangeRequestPage {
                                items: vec![pull_request(
                                    params["repository"].as_str().unwrap(),
                                    7,
                                    1,
                                    1,
                                    1,
                                )],
                                next_cursor: None,
                                total_count: Some(1),
                            })
                        }
                        other => panic!("unexpected request {other}"),
                    }
                }
            }
        });
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = fixture::state(cx, None);
            let mut page = PullRequestsPage::new(state.clone(), cx);
            page.repository = Some("owner/current".into());
            page.load_state = PullRequestsLoadState::Ready;
            state.update(cx, |state, _| {
                state.set_test_engine(crate::state::EngineHandle::from_test_client(rpc.client()));
                state.local_device_id = Some("local".into());
                state.spaces = vec![
                    space("slow", "local", true, 1),
                    space("next", "local", true, 1),
                    space("scratch", "local", true, 1),
                    space("next-worktree", "local", true, 1),
                ];
                state.spaces[1].repository_id = Some("next-repository".into());
                state.spaces[3].repository_id = Some("next-repository".into());
            });
            page.snapshots.push((
                (None, "OWNER/CURRENT".into(), ChangeRequestFilter::All),
                Vec::new(),
                Instant::now(),
                Paging::default(),
            ));
            page.snapshots.push((
                (
                    Some("remote".into()),
                    "remote/repo".into(),
                    ChangeRequestFilter::All,
                ),
                Vec::new(),
                Instant::now(),
                Paging::default(),
            ));
            assert_eq!(page.recent_repositories(), ["owner/current"]);
            page
        });
        let click = |selector: &'static str, cx: &mut gpui::VisualTestContext| {
            let bounds = cx.debug_bounds(selector).unwrap();
            cx.simulate_mouse_down(
                bounds.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.simulate_mouse_up(
                bounds.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.run_until_parked();
        };
        cx.simulate_resize(gpui::size(px(900.0), px(700.0)));
        cx.run_until_parked();
        click("pr-repository", cx);
        for width in [320.0, 900.0] {
            cx.simulate_resize(gpui::size(px(width), px(700.0)));
            cx.run_until_parked();
            let menu = cx.debug_bounds("pr-repository-card").unwrap();
            assert!(menu.left() >= px(0.0) && menu.right() <= px(width));
            for selector in ["pr-project-slow", "pr-project-next", "pr-repository-load"] {
                let row = cx.debug_bounds(selector).unwrap();
                assert!(row.left() >= menu.left() && row.right() <= menu.right());
                assert!(row.bottom() <= menu.bottom());
            }
        }
        click("pr-project-slow", cx);
        rpc.settle(cx, &runtime, |_| !calls.lock().unwrap().is_empty());
        page.read_with(cx, |page, _| assert!(page.resolving_project.is_some()));
        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(!page.repository_menu.is_open());
            assert!(page.resolving_project.is_none());
            assert!(page.initial_scope_task.is_none());
            assert_eq!(page.repository.as_deref(), Some("owner/current"));
        });
        let focus = page.read_with(cx, |page, _| page.repository_focus.clone());
        cx.update(|window, _| assert!(focus.is_focused(window)));
        cx.simulate_keystrokes("enter");
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                    keystroke: gpui::Keystroke::parse("enter").unwrap(),
                }),
                cx,
            );
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| assert!(page.repository_menu.is_open()));
        click("pr-project-next", cx);
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| {
                page.repository.as_deref() == Some("owner/next")
                    && page.load_state == PullRequestsLoadState::Ready
            })
        });
        release.notify_one();
        // The host may finish its bounded read after the UI task is cancelled;
        // that old result must never replace the later selection.
        rpc.settle(cx, &runtime, |_| rpc.completed() >= 3);
        page.read_with(cx, |page, _| {
            assert_eq!(page.repository.as_deref(), Some("owner/next"))
        });
        cx.simulate_keystrokes("enter");
        cx.update(|window, cx| {
            window.dispatch_event(
                gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                    keystroke: gpui::Keystroke::parse("enter").unwrap(),
                }),
                cx,
            );
        });
        cx.run_until_parked();
        click("pr-project-scratch", cx);
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| page.repository_error.is_some())
        });
        page.read_with(cx, |page, _| {
            assert!(page.repository_menu.is_open());
            assert!(
                page.repository_error
                    .as_deref()
                    .unwrap()
                    .contains("no GitHub remote")
            );
            assert_eq!(page.repository.as_deref(), Some("owner/next"));
        });
        page.update(cx, |page, cx| {
            page.repository_input
                .update(cx, |input, cx| input.set_text("owner/typed", cx))
        });
        page.read_with(cx, |page, _| assert!(page.repository_error.is_none()));
        // The production menu tracks overflow independently of the board.
        page.update(cx, |page, cx| {
            page.state.update(cx, |state, cx| {
                state.spaces = (0..30)
                    .map(|index| space(&format!("overflow-{index:02}"), "local", true, 1))
                    .collect();
                cx.notify();
            });
        });
        cx.run_until_parked();
        let scroll = page.read_with(cx, |page, _| page.repository_scroll.scroll.clone());
        assert!(scroll.max_offset().y > px(0.0));
        let input = cx.debug_bounds("pr-repository-load").unwrap();
        scroll.set_offset(gpui::point(px(0.0), -scroll.max_offset().y));
        cx.update(|window, cx| window.draw(cx).clear());
        assert_eq!(
            cx.debug_bounds("pr-repository-load").unwrap(),
            input,
            "the input stays above the scroll region"
        );
        page.update(cx, |page, cx| {
            page.state.update(cx, |state, cx| {
                state.spaces = vec![space("next", "local", true, 1)];
                cx.notify();
            });
        });
        cx.run_until_parked();
        assert_eq!(scroll.max_offset().y, px(0.0));
        assert_eq!(scroll.offset().y, px(0.0));
    }

    #[gpui::test]
    fn pull_request_initial_load_waits_for_engine_and_project(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let _settings = fixture::settings(cx, {
            let mut settings = crate::settings::UiSettings::default();
            settings.space_filter = Some("project".into());
            settings
        });
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let rpc = initial_repository_rpc(calls.clone(), false);
        let state = cx.new(|_| AppState::new());
        let page = cx.new(|cx| PullRequestsPage::new(state.clone(), cx));
        state.update(cx, |state, cx| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(rpc.client()));
            state.spaces = vec![zeron_proto::Space {
                id: "project".into(),
                device_id: "local".into(),
                path: "/checkout".into(),
                name: None,
                git_detected: true,
                git_checked_at: None,
                checkout_id: None,
                repository_id: None,
                created_at: Utc::now(),
            }];
            state.selected_space = Some("project".into());
            state.no_project = false;
            cx.notify();
        });
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| {
                page.load_state == PullRequestsLoadState::Ready
            })
        });
        page.update(cx, |page, cx| {
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            assert_eq!(page.items[0].number, 7);
            assert_eq!(page.repository.as_deref(), Some("owner/canonical"));
            assert_eq!(
                crate::settings::current(cx)
                    .last_pull_request_repository
                    .as_deref(),
                Some("owner/canonical")
            );
            assert_eq!(page.snapshots.last().unwrap().0.1, "owner/canonical");
            page.on_hidden();
            page.on_visible(cx);
        });
        cx.run_until_parked();
        assert_eq!(
            *calls.lock().unwrap(),
            [
                methods::GET_CHANGE_REQUEST_REPOSITORY,
                methods::LIST_CHANGE_REQUEST_PAGE
            ]
        );
        calls.lock().unwrap().clear();
        page.update(cx, |page, cx| {
            page.select_project("/checkout".into(), "local".into(), cx);
        });
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| {
                page.initial_scope_task.is_none() && page.load_state == PullRequestsLoadState::Ready
            })
        });
        page.read_with(cx, |page, _| {
            assert_eq!(page.repository.as_deref(), Some("owner/canonical"));
            assert!(page.repository_error.is_none());
            assert!(page.target_device.is_none());
        });
        assert_eq!(
            *calls.lock().unwrap(),
            [
                methods::GET_CHANGE_REQUEST_REPOSITORY,
                methods::LIST_CHANGE_REQUEST_PAGE
            ]
        );
    }

    /// `/<name>` checkouts belong to `owner/<name>`, except `/scratch`.
    fn project_repository_rpc(
        calls: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    ) -> std::sync::Arc<ScriptedRpc> {
        ScriptedRpc::new(move |method, params| {
            let calls = calls.clone();
            async move {
                match method.as_str() {
                    methods::GET_CHANGE_REQUEST_REPOSITORY => {
                        let cwd = params["cwd"].as_str().unwrap().to_owned();
                        calls.lock().unwrap().push(cwd.clone());
                        let name = cwd.trim_start_matches('/');
                        zeron_rpc::RpcReply::value(
                            &(name != "scratch").then(|| format!("owner/{name}")),
                        )
                    }
                    methods::LIST_CHANGE_REQUEST_PAGE => {
                        let repository = params["repository"].as_str().unwrap();
                        zeron_rpc::RpcReply::value(&ChangeRequestPage {
                            items: vec![pull_request(repository, 7, 1, 1, 1)],
                            next_cursor: None,
                            total_count: Some(1),
                        })
                    }
                    _ => panic!("unexpected request {method}"),
                }
            }
        })
    }

    #[gpui::test]
    fn pull_request_board_finds_and_follows_the_repository_it_is_opened_from(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let _settings = fixture::settings(cx, crate::settings::UiSettings::default());
        let lookups = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let rpc = project_repository_rpc(lookups.clone());
        let state = cx.new(|_| AppState::new());
        state.update(cx, |state, _| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(rpc.client()));
            state.local_device_id = Some("local".into());
            state.spaces = vec![
                space("comet", "local", true, 60),
                space("scratch", "local", true, 1),
                space("docs", "local", true, 120),
            ];
        });
        let page = cx.new(|cx| PullRequestsPage::new(state.clone(), cx));
        let page_ready = {
            let page = page.clone();
            move |cx: &mut gpui::TestAppContext| {
                page.read_with(cx, |page, _| {
                    page.initial_scope_task.is_none()
                        && page.load_state == PullRequestsLoadState::Ready
                })
            }
        };
        let settle = |cx: &mut gpui::TestAppContext| {
            rpc.settle(cx, &runtime, &page_ready);
        };
        settle(cx);
        // First run, All projects, nothing saved: the newest Git project that
        // is on GitHub, without asking.
        page.read_with(cx, |page, _| {
            assert_eq!(page.repository.as_deref(), Some("owner/comet"));
            assert!(page.repository_error.is_none());
        });
        assert_eq!(*lookups.lock().unwrap(), ["/scratch", "/comet"]);

        // Opened from a session in another project: that project's PRs.
        page.update(cx, |page, _| page.on_hidden());
        state.update(cx, |state, _| {
            state.chats = vec![session("docs", None)];
            state.selected_chat = Some("chat".into());
        });
        page.update(cx, |page, cx| page.on_visible(cx));
        settle(cx);
        page.read_with(cx, |page, _| {
            assert_eq!(page.repository.as_deref(), Some("owner/docs"))
        });

        // An explicit choice stands while the board reopens from the same project.
        page.update(cx, |page, cx| {
            page.repository_input
                .update(cx, |input, cx| input.set_text("elsewhere/repo", cx));
            page.select_repository(cx);
        });
        settle(cx);
        page.update(cx, |page, cx| {
            page.on_hidden();
            page.on_visible(cx);
        });
        settle(cx);
        page.read_with(cx, |page, _| {
            assert_eq!(page.repository.as_deref(), Some("elsewhere/repo"))
        });
        assert_eq!(*lookups.lock().unwrap(), ["/scratch", "/comet", "/docs"]);
    }

    #[gpui::test]
    fn pull_request_stalled_discovery_falls_back_once(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let _settings = fixture::settings(cx, {
            let mut settings = crate::settings::UiSettings::default();
            settings.space_filter = Some("project".into());
            settings.last_pull_request_repository = Some("owner/repo".into());
            settings
        });
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let rpc = initial_repository_rpc(calls.clone(), true);
        let state = cx.new(|_| AppState::new());
        let page = cx.new(|cx| PullRequestsPage::new(state.clone(), cx));
        state.update(cx, |state, cx| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(rpc.client()));
            state.spaces = vec![zeron_proto::Space {
                id: "project".into(),
                device_id: "local".into(),
                path: "/checkout".into(),
                name: None,
                git_detected: true,
                git_checked_at: None,
                checkout_id: None,
                repository_id: None,
                created_at: Utc::now(),
            }];
            state.selected_space = Some("project".into());
            state.no_project = false;
            cx.notify();
        });
        rpc.settle(cx, &runtime, |_| !calls.lock().unwrap().is_empty());
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(4));
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| {
                page.load_state == PullRequestsLoadState::Ready
            })
        });
        page.update(cx, |page, cx| {
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            assert_eq!(page.items[0].number, 7);
            assert_eq!(page.repository.as_deref(), Some("owner/canonical"));
            assert_eq!(
                crate::settings::current(cx)
                    .last_pull_request_repository
                    .as_deref(),
                Some("owner/canonical")
            );
            assert_eq!(page.snapshots.last().unwrap().0.1, "owner/canonical");
            page.on_hidden();
            page.on_visible(cx);
        });
        cx.run_until_parked();
        assert_eq!(
            *calls.lock().unwrap(),
            [
                methods::GET_CHANGE_REQUEST_REPOSITORY,
                methods::LIST_CHANGE_REQUEST_PAGE
            ]
        );
    }

    #[gpui::test]
    fn pull_request_all_projects_loads_last_repo_without_discovery(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let _settings = fixture::settings(cx, {
            let mut settings = crate::settings::UiSettings::default();
            settings.last_pull_request_repository = Some("owner/repo".into());
            settings
        });
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let rpc = initial_repository_rpc(calls.clone(), false);
        let state = cx.new(|_| AppState::new());
        let page = cx.new(|cx| PullRequestsPage::new(state.clone(), cx));
        state.update(cx, |state, cx| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(rpc.client()));
            state.spaces = vec![zeron_proto::Space {
                id: "project".into(),
                device_id: "local".into(),
                path: "/checkout".into(),
                name: None,
                git_detected: true,
                git_checked_at: None,
                checkout_id: None,
                repository_id: None,
                created_at: Utc::now(),
            }];
            state.selected_space = Some("project".into());
            state.no_project = false;
            cx.notify();
        });
        rpc.settle(cx, &runtime, |cx| {
            page.read_with(cx, |page, _| {
                page.load_state == PullRequestsLoadState::Ready
            })
        });
        page.update(cx, |page, cx| {
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            assert_eq!(page.items[0].number, 7);
            assert_eq!(page.repository.as_deref(), Some("owner/canonical"));
            assert_eq!(
                crate::settings::current(cx)
                    .last_pull_request_repository
                    .as_deref(),
                Some("owner/canonical")
            );
            assert_eq!(page.snapshots.last().unwrap().0.1, "owner/canonical");
            page.on_hidden();
            page.on_visible(cx);
        });
        cx.run_until_parked();
        assert_eq!(*calls.lock().unwrap(), [methods::LIST_CHANGE_REQUEST_PAGE]);
    }

    #[gpui::test]
    fn pull_request_filter_switches_restore_only_matching_snapshots(cx: &mut gpui::TestAppContext) {
        fixture::init(cx);
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            PullRequestsPage::new(state, cx)
        });
        page.update(cx, |page, cx| {
            page.repository = Some("owner/repo".into());
            for (filter, number) in [
                (ChangeRequestFilter::All, 1),
                (ChangeRequestFilter::Authored, 2),
                (ChangeRequestFilter::Reviewing, 3),
            ] {
                page.snapshots.push((
                    (None, "owner/repo".into(), filter),
                    vec![pull_request("owner/repo", number, 1, 1, 1)],
                    Instant::now(),
                    Paging::default(),
                ));
            }
            for (filter, number) in [
                (ChangeRequestFilter::Reviewing, 3),
                (ChangeRequestFilter::All, 1),
                (ChangeRequestFilter::Authored, 2),
            ] {
                page.select_filter(filter, cx);
                assert_eq!(page.items[0].number, number);
                // No engine is attached: a request would fail with Network.
                assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            }
        });
    }

    #[gpui::test]
    fn long_rows_fit_at_every_layout_and_do_not_resize_on_hover(cx: &mut gpui::TestAppContext) {
        fixture::init(cx);
        let mut item = pull_request(
            "a-very-long-organization/a-very-long-repository-name",
            181,
            123456,
            1,
            1,
        );
        item.title =
            "A long pull request title that must remain readable beside every possible status "
                .repeat(3);
        item.is_draft = true;
        item.mergeability = ChangeRequestMergeability::Conflicting;
        item.review_decision = ChangeRequestReviewDecision::ChangesRequested;
        let (_, cx) = cx.add_window_view(|_, _| RowLayoutFixture { item });
        for width in [272.0, 639.0, 640.0, 899.0, 900.0, 1072.0] {
            cx.simulate_resize(gpui::size(px(width), px(480.0)));
            cx.run_until_parked();
            let row = cx.debug_bounds("pull-request-row").expect("row rendered");
            let title = cx
                .debug_bounds("pull-request-title")
                .expect("title rendered");
            assert!(row.size.width <= px(width), "row overflow at {width}");
            assert!(
                title.size.width > px(120.0),
                "statuses squeezed title at {width}"
            );
            assert!(
                title.right() <= row.right(),
                "title overflow at {width}: row={row:?}, title={title:?}"
            );
            cx.simulate_mouse_move(title.center(), None, gpui::Modifiers::default());
            cx.run_until_parked();
            assert_eq!(
                cx.debug_bounds("pull-request-title").unwrap(),
                title,
                "hover changed text geometry"
            );
        }
    }

    #[gpui::test]
    fn pull_request_personal_views_star_locally_without_opening_a_pull_request(
        cx: &mut gpui::TestAppContext,
    ) {
        assert_eq!(
            star_key("https://github.com/Owner/Repo/pull/1/?tab=checks#discussion"),
            star_key("https://github.com/owner/repo/pull/1")
        );
        let directory = fixture::settings(cx, crate::settings::UiSettings::default());
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = fixture::state(cx, None);
            let mut page = PullRequestsPage::new(state, cx);
            let mut mine = pull_request("owner/repo", 1, 1, 1, 1);
            mine.head_ref_oid = "a".repeat(40);
            mine.ci = zeron_proto::change_request_assessment::ChangeRequestCi {
                state: CiState::Failed,
                total_count: 3,
            };
            mine.viewer_did_author = Some(true);
            mine.viewer_review_requested = Some(false);
            let mut theirs = mine.clone();
            theirs.number = 2;
            theirs.url = "https://github.com/owner/repo/pull/2".into();
            theirs.viewer_did_author = Some(false);
            page.items = vec![mine, theirs];
            page.filter = ChangeRequestFilter::All;
            page.load_state = PullRequestsLoadState::Ready;
            page
        });
        let click = |selector: &'static str, cx: &mut gpui::VisualTestContext| {
            let bounds = cx.debug_bounds(selector).unwrap();
            cx.simulate_mouse_down(
                bounds.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.simulate_mouse_up(
                bounds.center(),
                gpui::MouseButton::Left,
                gpui::Modifiers::default(),
            );
            cx.run_until_parked();
        };
        cx.simulate_resize(gpui::size(px(320.0), px(600.0)));
        cx.run_until_parked();
        click("pr-view-starred", cx);
        assert!(cx.debug_bounds("pull-requests-no-results").is_some());
        click("pr-view-starred", cx);
        click("pr-view-needs-you", cx);
        assert!(cx.debug_bounds("pull-requests-no-results").is_none());
        let star = cx.debug_bounds("pr-star").unwrap();
        let row = cx.debug_bounds("pull-request-row").unwrap();
        assert!(row.contains(&star.center()));
        click("pr-star", cx);
        cx.update(|_, cx| {
            assert_eq!(
                crate::settings::current(cx).pull_request_stars,
                ["https://github.com/owner/repo/pull/1"]
            );
            crate::settings::flush(cx);
        });
        assert_eq!(
            crate::settings::UiSettings::load(directory.path()).pull_request_stars,
            ["https://github.com/owner/repo/pull/1"]
        );
        click("pr-view-starred", cx);
        assert!(cx.debug_bounds("pull-request-row").is_some());
        for width in [320.0, 900.0, 1200.0] {
            cx.simulate_resize(gpui::size(px(width), px(600.0)));
            cx.run_until_parked();
            let row = cx.debug_bounds("pull-request-row").unwrap();
            let star = cx.debug_bounds("pr-star").unwrap();
            let ci = cx.debug_bounds("pr-row-ci").unwrap();
            assert!(star.left() >= row.left() && star.right() <= row.right());
            assert!(ci.left() >= row.left() && ci.right() <= row.right());
            assert!(ci.bottom() <= row.bottom());
        }
        click("pr-star", cx);
        assert!(cx.debug_bounds("pull-requests-no-results").is_some());
        page.read_with(cx, |page, _| {
            assert_eq!(page.items.len(), 2, "local views keep the fetched snapshot")
        });
    }

    #[gpui::test]
    fn narrow_sorting_and_refresh_stay_reachable(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        fixture::init(cx);
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let mut page = PullRequestsPage::new(state, cx);
            page.items = (1..=50)
                .map(|n| pull_request("owner/repo", n, n, 1, 1))
                .collect();
            page.load_state = PullRequestsLoadState::Ready;
            page
        });
        cx.simulate_resize(gpui::size(px(320.0), px(480.0)));
        cx.run_until_parked();
        let refresh = cx
            .debug_bounds("pull-requests-refresh")
            .expect("refresh rendered");
        let sort = cx.debug_bounds("pr-sort").unwrap();
        cx.simulate_mouse_down(
            sort.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            sort.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        for selector in [
            "pull-requests-sort-changes",
            "pull-requests-sort-opened",
            "pull-requests-sort-updated",
        ] {
            let bounds = cx.debug_bounds(selector).expect("sort rendered");
            assert!(
                bounds.left() >= px(0.0) && bounds.right() <= px(320.0),
                "{selector}: {bounds:?}"
            );
        }
        let changes = cx.debug_bounds("pull-requests-sort-changes").unwrap();
        cx.simulate_mouse_down(
            changes.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            changes.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        page.read_with(cx, |page, _| {
            assert_eq!(page.sort.field, PullRequestSortField::Changes)
        });
        cx.run_until_parked();
        page.update(cx, |page, cx| {
            page.sort_menu = Default::default();
            cx.notify();
        });
        cx.run_until_parked();
        let viewport = cx.debug_bounds("pull-requests-scroll").unwrap();
        assert!(
            viewport.size.height > px(100.0),
            "the filter must leave room for results: {viewport:?}"
        );
        page.read_with(cx, |page, _| assert!(page.sort_menu.get().is_none()));
        cx.simulate_mouse_move(viewport.center(), None, gpui::Modifiers::default());
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: viewport.center(),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-800.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.scroll.scroll.offset().y < px(0.0))
        });
        assert_eq!(cx.debug_bounds("pull-requests-refresh").unwrap(), refresh);
        let repository = cx.debug_bounds("pr-repository").unwrap();
        cx.simulate_mouse_down(
            repository.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            repository.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        let open = cx.debug_bounds("pr-repository-load").unwrap();
        assert!(open.left() >= px(0.0) && open.right() <= px(320.0));
        page.update(cx, |page, cx| {
            page.repository_input
                .update(cx, |input, cx| input.set_text("another/repo", cx))
        });
        cx.simulate_mouse_down(
            open.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            open.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        page.read_with(cx, |page, _| {
            assert_eq!(page.repository.as_deref(), Some("another/repo"))
        });
    }

    #[test]
    fn groups_prioritize_attention_without_claiming_merge_readiness() {
        let mut item = pull_request("owner/repo", 181, 1, 1, 1);
        item.review_decision = ChangeRequestReviewDecision::Unknown;
        assert_eq!(request_group(&item), PullRequestGroup::Review);
        item.review_decision = ChangeRequestReviewDecision::Approved;
        assert_eq!(request_group(&item), PullRequestGroup::Approved);
        item.mergeability = ChangeRequestMergeability::Conflicting;
        assert_eq!(request_group(&item), PullRequestGroup::Attention);
        item.is_draft = true;
        assert_eq!(request_group(&item), PullRequestGroup::Drafts);
        item.is_draft = false;
        item.mergeability = ChangeRequestMergeability::Unknown;
        item.review_decision = ChangeRequestReviewDecision::ChangesRequested;
        assert_eq!(request_group(&item), PullRequestGroup::Attention);
    }

    #[test]
    fn search_matches_all_words_across_title_repo_number_and_status() {
        let mut item = pull_request("ZeronSH/Zeron", 181, 1, 1, 1);
        item.title = "Add a pull request dashboard".into();
        item.is_draft = true;
        assert!(matches_query(&item, "ZERON dashboard #181 draft"));
        assert!(matches_query(&item, "  "));
        assert!(!matches_query(&item, "dashboard approved"));
    }

    #[gpui::test]
    fn searching_reveals_collapsed_matches_and_handles_no_results(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        fixture::init(cx);
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            let mut page = PullRequestsPage::new(state, cx);
            page.items = vec![pull_request("owner/repo", 181, 1, 1, 1)];
            page.load_state = PullRequestsLoadState::Ready;
            page
        });
        cx.run_until_parked();
        let group = cx.debug_bounds("pull-requests-group-review").unwrap();
        cx.simulate_mouse_down(
            group.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            group.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(page.collapsed_groups.contains(&PullRequestGroup::Review));
            let motion = page.group_motions[&PullRequestGroup::Review];
            assert!(motion.from > 0.0, "collapse uses measured row height");
            assert_eq!(motion.to, 0.0);
        });
        page.update(cx, |page, cx| {
            page.search
                .update(cx, |input, cx| input.set_text("#181", cx))
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert_eq!(page.query, "#181");
            assert!(page.collapsed_groups.is_empty());
        });
        assert!(cx.debug_bounds("pull-request-title").is_some());
        page.update(cx, |page, cx| {
            page.search
                .update(cx, |input, cx| input.set_text("no matches", cx))
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("pull-requests-no-results").is_some());
        assert!(cx.debug_bounds("pull-requests-refresh").is_some());
    }

    #[test]
    fn status_description_preserves_simultaneous_states() {
        let mut item = pull_request("owner/repo", 181, 10, 1, 1);
        item.is_draft = true;
        item.mergeability = ChangeRequestMergeability::Conflicting;
        item.review_decision = ChangeRequestReviewDecision::ChangesRequested;
        assert_eq!(
            status_description(&item),
            "Draft · Changes requested · Merge conflicts"
        );
        item.is_draft = false;
        item.mergeability = ChangeRequestMergeability::Unknown;
        item.review_decision = ChangeRequestReviewDecision::Approved;
        assert_eq!(status_description(&item), "Open · Approved");
    }

    fn numbers(items: &[ChangeRequestListItem]) -> Vec<u64> {
        items.iter().map(|item| item.number).collect()
    }

    #[test]
    fn only_desktop_devices_are_eligible_targets() {
        let eligible = eligible_desktop_devices(&[
            device("mac", "macos"),
            device("linux", "linux"),
            device("phone", "ios"),
            device("tablet", "android"),
        ]);
        assert_eq!(
            eligible.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            ["linux", "mac"]
        );
    }

    #[test]
    fn target_params_keep_local_calls_direct() {
        assert_eq!(params_for_target(None), serde_json::json!({}));
        assert_eq!(
            params_for_target(Some("host")),
            serde_json::json!({ "targetDeviceId": "host" })
        );
    }

    #[test]
    fn missing_remote_target_falls_back_only_after_an_authoritative_device_frame() {
        let local = device("local", "macos");
        let remote = device("remote", "linux");

        assert_eq!(
            normalized_target_device(Some("remote"), &[local.clone(), remote], Some("local")),
            Some("remote".into())
        );
        assert_eq!(
            normalized_target_device(Some("remote"), &[local], Some("local")),
            None
        );
        assert_eq!(
            normalized_target_device(Some("remote"), &[], Some("local")),
            Some("remote".into()),
            "the empty pre-sync list must not discard the selection"
        );
        assert_eq!(
            normalized_target_device(Some("local"), &[], Some("local")),
            None,
            "local calls stay direct even before the device frame"
        );
    }

    #[test]
    fn table_sorting_uses_changes_opened_and_updated_values() {
        let original = vec![
            pull_request("beta/repo", 2, 30, 3, 9),
            pull_request("alpha/repo", 1, 10, 1, 10),
            pull_request("alpha/repo", 3, 30, 2, 11),
        ];

        let mut items = original.clone();
        sort_pull_requests(&mut items, PullRequestSort::DEFAULT);
        assert_eq!(numbers(&items), [3, 1, 2]);

        let mut items = original.clone();
        sort_pull_requests(
            &mut items,
            PullRequestSort {
                field: PullRequestSortField::Changes,
                direction: SortDirection::Descending,
            },
        );
        assert_eq!(numbers(&items), [3, 2, 1]);

        let mut items = original.clone();
        sort_pull_requests(
            &mut items,
            PullRequestSort {
                field: PullRequestSortField::Opened,
                direction: SortDirection::Ascending,
            },
        );
        assert_eq!(numbers(&items), [1, 3, 2]);

        let mut items = original;
        sort_pull_requests(
            &mut items,
            PullRequestSort {
                field: PullRequestSortField::Updated,
                direction: SortDirection::Ascending,
            },
        );
        assert_eq!(numbers(&items), [2, 1, 3]);
    }

    #[test]
    fn table_sorting_breaks_ties_by_repository_and_number() {
        let mut items = vec![
            pull_request("zeta/repo", 2, 10, 1, 10),
            pull_request("alpha/repo", 3, 10, 1, 10),
            pull_request("alpha/repo", 1, 10, 1, 10),
        ];
        sort_pull_requests(
            &mut items,
            PullRequestSort {
                field: PullRequestSortField::Changes,
                direction: SortDirection::Descending,
            },
        );
        assert_eq!(numbers(&items), [1, 3, 2]);
    }

    #[test]
    fn relative_dates_are_readable_and_pluralized() {
        let now = Utc::now();
        assert_eq!(relative_time(now, now), "just now");
        assert_eq!(relative_time(now - TimeDelta::hours(1), now), "1 hour ago");
        assert_eq!(relative_time(now - TimeDelta::days(2), now), "2 days ago");
        assert_eq!(
            compact_relative_time(now - TimeDelta::hours(2), now),
            "2h ago"
        );
        assert_eq!(compact_relative_time(now + TimeDelta::hours(2), now), "now");
    }

    #[test]
    fn diff_counts_stay_compact_without_losing_scale() {
        assert_eq!(format_compact_count(999), "999");
        assert_eq!(format_compact_count(1_000), "1k");
        assert_eq!(format_compact_count(13_223), "13.2k");
        assert_eq!(format_compact_count(1_250_000), "1.2m");
    }

    #[test]
    fn successful_empty_refresh_replaces_the_previous_snapshot() {
        let mut snapshot = vec![1, 2];
        let state = settle_snapshot(&mut snapshot, Ok(Vec::new()));
        assert!(snapshot.is_empty());
        assert_eq!(state, PullRequestsLoadState::Ready);
    }

    #[test]
    fn failed_refresh_preserves_the_previous_snapshot() {
        let mut snapshot = vec![1, 2];
        let state = settle_snapshot(&mut snapshot, Err(PullRequestsPageError::Authentication));
        assert_eq!(snapshot, [1, 2]);
        assert_eq!(
            state,
            PullRequestsLoadState::Failed(PullRequestsPageError::Authentication)
        );
    }

    #[test]
    fn error_mapping_uses_stable_codes_and_hides_transport_details() {
        assert_eq!(
            map_rpc_error(
                &RpcError::Capability(capability_errors::PULL_REQUESTS_AUTHENTICATION.into()),
                "Studio Mac",
            ),
            PullRequestsPageError::Authentication
        );
        assert_eq!(
            map_rpc_error(
                &RpcError::UnknownMethod(methods::LIST_CHANGE_REQUEST_PAGE.into()),
                "Studio Mac"
            ),
            PullRequestsPageError::UpdateRequired("Studio Mac".into())
        );
        assert_eq!(
            map_rpc_error(&RpcError::Transport("secret detail".into()), "Studio Mac"),
            PullRequestsPageError::Network
        );
    }

    #[test]
    fn visible_device_names_and_titles_are_sanitized() {
        assert_eq!(single_line("MacBook\n Pro"), "MacBook Pro");
        assert_eq!(single_line(&"a".repeat(200)), "a".repeat(200));
    }

    /// Serves two pages and records each request's cursor and `refresh` flag.
    /// The `reviewing` filter never answers until `release` fires.
    fn paged_rpc(
        calls: std::sync::Arc<std::sync::Mutex<Vec<(Option<String>, bool)>>>,
        release: std::sync::Arc<tokio::sync::Notify>,
    ) -> std::sync::Arc<ScriptedRpc> {
        ScriptedRpc::new(move |method, params| {
            let calls = calls.clone();
            let release = release.clone();
            async move {
                assert_eq!(method.as_str(), methods::LIST_CHANGE_REQUEST_PAGE);
                let after = params["after"].as_str().map(str::to_owned);
                calls
                    .lock()
                    .unwrap()
                    .push((after.clone(), params["refresh"] == true));
                if params["filter"] == "reviewing" {
                    release.notified().await;
                    return zeron_rpc::RpcReply::value(&ChangeRequestPage {
                        items: vec![pull_request("owner/repo", 99, 1, 1, 1)],
                        next_cursor: None,
                        total_count: Some(1),
                    });
                }
                match after.as_deref() {
                    None => zeron_rpc::RpcReply::value(&ChangeRequestPage {
                        items: (1..=50)
                            .map(|n| pull_request("Owner/Repo", n, 1, 1, 1))
                            .collect(),
                        next_cursor: Some("Y3Vyc29yOjUw".into()),
                        total_count: Some(52),
                    }),
                    Some("Y3Vyc29yOjUw") => zeron_rpc::RpcReply::value(&ChangeRequestPage {
                        // #50 moved onto the second page; it must not repeat.
                        items: [50, 51, 52]
                            .map(|n| pull_request("Owner/Repo", n, 1, 1, 1))
                            .into(),
                        next_cursor: None,
                        total_count: Some(52),
                    }),
                    other => panic!("unexpected cursor {other:?}"),
                }
            }
        })
    }

    fn paged_page(
        cx: &mut gpui::TestAppContext,
    ) -> (
        Entity<PullRequestsPage>,
        &mut gpui::VisualTestContext,
        std::sync::Arc<std::sync::Mutex<Vec<(Option<String>, bool)>>>,
        std::sync::Arc<tokio::sync::Notify>,
        std::sync::Arc<ScriptedRpc>,
    ) {
        fixture::init(cx);
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let release = std::sync::Arc::new(tokio::sync::Notify::new());
        let rpc = paged_rpc(calls.clone(), release.clone());
        let client = rpc.client();
        let (page, cx) = cx.add_window_view(|_, cx| {
            let state = fixture::state(cx, Some(client));
            PullRequestsPage::new(state, cx)
        });
        page.update(cx, |page, cx| {
            page.repository = Some("owner/repo".into());
            page.load(false, cx);
        });
        (page, cx, calls, release, rpc)
    }

    fn settle(
        cx: &mut gpui::VisualTestContext,
        runtime: &tokio::runtime::Runtime,
        done: impl Fn(&PullRequestsPage) -> bool,
        page: &Entity<PullRequestsPage>,
        rpc: &ScriptedRpc,
    ) {
        rpc.settle(cx, runtime, |cx| page.read_with(cx, |page, _| done(page)));
    }

    #[gpui::test]
    fn pull_request_board_loads_more_pages_without_repeating_items(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let (page, cx, calls, _, rpc) = paged_page(cx);
        settle(
            cx,
            &runtime,
            |page| page.load_state == PullRequestsLoadState::Ready,
            &page,
            &rpc,
        );
        cx.simulate_resize(gpui::size(px(900.0), px(800.0)));
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert_eq!(page.items.len(), 50);
            assert_eq!(page.paging.total, Some(52));
        });
        assert!(cx.debug_bounds("pull-requests-loaded-count").is_some());
        page.update(cx, |page, cx| {
            let bottom = page.scroll.scroll.max_offset().y;
            page.scroll.scroll.set_offset(gpui::point(px(0.0), -bottom));
            cx.notify();
        });
        cx.run_until_parked();
        let more = cx.debug_bounds("pull-requests-load-more").unwrap();
        assert!(
            more.bottom() <= px(800.0),
            "the button is reachable at the end of the list"
        );
        cx.simulate_mouse_down(
            more.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(
            more.center(),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        settle(
            cx,
            &runtime,
            |page| page.more_task.is_none() && page.items.len() > 50,
            &page,
            &rpc,
        );
        page.read_with(cx, |page, _| {
            assert_eq!(page.items.len(), 52, "the moved #50 appears once");
            assert_eq!(page.paging.next_cursor, None);
            assert_eq!(
                page.snapshots.last().unwrap().1.len(),
                52,
                "snapshots keep every page"
            );
        });
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("pull-requests-load-more").is_none(),
            "the last page ends the list"
        );
        assert_eq!(
            *calls.lock().unwrap(),
            [(None, false), (Some("Y3Vyc29yOjUw".to_owned()), false)]
        );

        // GitHub's casing was adopted; reselecting the repository as typed
        // reuses the loaded board instead of fetching it again.
        page.update(cx, |page, cx| {
            assert_eq!(page.repository.as_deref(), Some("Owner/Repo"));
            page.repository_input
                .update(cx, |input, cx| input.set_text("owner/repo", cx));
            page.select_repository(cx);
            assert_eq!(page.items.len(), 52);
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
        });
        cx.run_until_parked();
        assert_eq!(calls.lock().unwrap().len(), 2);

        // Refresh restarts from the first page, and later pages stay as fresh.
        page.update(cx, |page, cx| page.refresh(cx));
        settle(
            cx,
            &runtime,
            |page| page.load_state == PullRequestsLoadState::Ready,
            &page,
            &rpc,
        );
        page.update(cx, |page, cx| page.load_more(cx));
        settle(cx, &runtime, |page| page.more_task.is_none(), &page, &rpc);
        assert_eq!(
            calls.lock().unwrap()[2..],
            [(None, true), (Some("Y3Vyc29yOjUw".to_owned()), true)]
        );
    }

    #[gpui::test]
    fn pull_request_board_drops_a_reply_for_a_filter_it_has_left(cx: &mut gpui::TestAppContext) {
        let runtime = fixture::runtime();
        let _guard = runtime.enter();
        let (page, cx, calls, release, rpc) = paged_page(cx);
        settle(
            cx,
            &runtime,
            |page| page.load_state == PullRequestsLoadState::Ready,
            &page,
            &rpc,
        );
        page.update(cx, |page, cx| {
            page.select_filter(ChangeRequestFilter::Reviewing, cx)
        });
        settle(
            cx,
            &runtime,
            |_| calls.lock().unwrap().len() == 2,
            &page,
            &rpc,
        );
        page.update(cx, |page, cx| {
            assert_eq!(page.load_state, PullRequestsLoadState::Loading);
            page.select_filter(ChangeRequestFilter::Authored, cx);
            assert_eq!(page.items.len(), 50, "the loaded filter comes back at once");
        });
        let completed = rpc.completed();
        release.notify_one();
        rpc.settle(cx, &runtime, |_| rpc.completed() > completed);
        page.read_with(cx, |page, _| {
            assert_eq!(page.filter, ChangeRequestFilter::Authored);
            assert_eq!(page.items.len(), 50, "the late Reviewing reply is ignored");
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
        });
    }
}
