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
    ChangeRequestFilter, ChangeRequestListItem, ChangeRequestMergeability,
    ChangeRequestReviewDecision, Device,
};
use zeron_rpc::{RpcError, capability_errors, methods};

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
    if item.is_draft {
        PullRequestGroup::Drafts
    } else if item.mergeability == ChangeRequestMergeability::Conflicting
        || item.review_decision == ChangeRequestReviewDecision::ChangesRequested
    {
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

struct DashboardTooltip(SharedString);

impl Render for DashboardTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(5.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(px(11.0))
            .max_w(px(420.0))
            .text_color(theme.text)
            .child(self.0.clone())
    }
}

impl PullRequestSort {
    const DEFAULT: Self = Self {
        field: PullRequestSortField::Updated,
        direction: SortDirection::Descending,
    };

    #[cfg(test)]
    fn select(self, field: PullRequestSortField) -> Self {
        if self.field != field {
            return Self {
                field,
                direction: SortDirection::Descending,
            };
        }
        Self {
            field,
            direction: match self.direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            },
        }
    }
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
    _repository_events: Subscription,
    snapshots: Vec<(
        (Option<String>, String, ChangeRequestFilter),
        Vec<ChangeRequestListItem>,
        Instant,
    )>,
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
    filter_fades: crate::motion::HoverFades,
    load_state: PullRequestsLoadState,
    last_loaded_at: Option<Instant>,
    generation: u64,
    request_task: Option<Task<()>>,
    visible: bool,
    scroll: widgets::PageScroll,
    content_width: Option<f32>,
    device_menu: popover::Popup<()>,
    sort_menu: popover::Popup<()>,
    repository_menu: popover::Popup<()>,
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
        });
        let repository_events = cx.subscribe(&repository_input, |page: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                page.select_repository(cx);
            }
        });
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
            _repository_events: repository_events,
            snapshots: Vec::new(),
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
            filter_fades,
            load_state: PullRequestsLoadState::Idle,
            last_loaded_at: None,
            generation: 0,
            request_task: None,
            // The entity is created lazily only while this route is active.
            visible: true,
            scroll: widgets::PageScroll::default(),
            content_width: None,
            device_menu: popover::Popup::default(),
            sort_menu: popover::Popup::default(),
            repository_menu: popover::Popup::default(),
            _observe: observe,
        };
        page.initialize_repository(cx);
        page
    }

    fn initialize_repository(&mut self, cx: &mut Context<Self>) {
        if self.repository.is_some() || self.initial_scope_attempted || !self.visible {
            return;
        }
        let state = self.state.read(cx);
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let saved = crate::settings::current(cx);
        // The sidebar scope is independent of the retained conversation/project.
        let current = saved.space_filter.as_deref().and_then(|id| {
            state
                .spaces
                .iter()
                .find(|space| space.id == id)
                .map(|space| (space.path.clone(), space.device_id.clone()))
        });
        if saved.space_filter.is_some() && current.is_none() {
            // Wait for the project's first frame, rather than pinning a stale scope.
            return;
        }
        let fallback = saved
            .last_pull_request_repository
            .filter(|repo| valid_repository_filter(repo))
            .map(|repo| (repo, saved.last_pull_request_device));
        let Some((cwd, device)) = current else {
            if let Some((repo, target)) = fallback {
                self.apply_initial_repository(repo, target, cx);
            }
            return;
        };
        let local = state
            .local_device_id
            .as_deref()
            .unwrap_or(&engine.engine_info().device_id);
        let target = (device != local).then_some(device);
        let mut params = params_for_target(target.as_deref());
        params["cwd"] = cwd.into();
        self.initial_scope_attempted = true;
        self.load_state = PullRequestsLoadState::Loading;
        self.initial_scope_task = Some(cx.spawn(async move |this, cx| {
            let request = engine.client().call(methods::GET_CHANGE_REQUEST_REPOSITORY, params);
            let deadline = cx.background_executor().timer(std::time::Duration::from_secs(3));
            futures::pin_mut!(request, deadline);
            let result = match futures::future::select(request, deadline).await {
                futures::future::Either::Left((result, _)) => result,
                futures::future::Either::Right(_) => Err(RpcError::Failed("Repository detection timed out".into())),
            };
            let discovery_failed = result.is_err();
            let repository = result
                .ok()
                .and_then(|value| serde_json::from_value::<Option<String>>(value).ok())
                .flatten();
            let _ = this.update(cx, |page, cx| {
                page.initial_scope_task = None;
                // User selection cancels this task; never override their scope.
                if page.repository.is_some() {
                    return;
                }
                page.load_state = PullRequestsLoadState::Idle;
                if let Some((repo, target)) = initial_repository_scope(repository, target, fallback)
                {
                    page.apply_initial_repository(repo, target, cx);
                } else if discovery_failed {
                    page.repository_error = Some("Couldn’t detect this project’s repository. Choose a repository to continue.".into());
                }
                cx.notify();
            });
        }));
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
        // Seed one scope only; subsequent navigation reuses its snapshot.
        self.initialize_repository(cx);
        cx.notify();
    }

    /// Keep the retained route entity dormant while another outlet is active.
    pub fn on_hidden(&mut self) {
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
        self.close_device_menu(cx);
        if self.target_device == target {
            return;
        }

        self.initial_scope_task = None;
        self.initial_scope_attempted = true;
        self.reset_for_target(target);
        cx.notify();
    }

    fn reset_for_target(&mut self, target: Option<String>) {
        self.generation = self.generation.wrapping_add(1);
        self.request_task = None;
        self.target_device = target;
        self.collapsed_groups.clear();
        self.group_motions.clear();
        self.items.clear();
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
            .position(|((target, repo, filter), _, _)| {
                target == &self.target_device && repo == repository && filter == &self.filter
            })
        {
            let snapshot = self.snapshots.remove(index);
            self.items = snapshot.1.clone();
            self.last_loaded_at = Some(snapshot.2);
            self.load_state = PullRequestsLoadState::Ready;
            self.snapshots.push(snapshot);
        }
    }

    fn select_repository(&mut self, cx: &mut Context<Self>) {
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
        if self.repository.as_deref() != Some(&repository) {
            self.repository = Some(repository);
            self.reset_for_target(self.target_device.clone());
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
        self.close_device_menu(cx);
        self.reset_for_target(normalized);
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
        if self.filter == filter {
            return;
        }
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
        if self.repository.is_none() {
            cx.notify();
            return;
        }
        self.reset_for_target(self.target_device.clone());
        if self.last_loaded_at.is_none() {
            self.load(false, cx);
        }
        cx.notify();
    }

    fn close_repository_menu(&mut self, cx: &mut Context<Self>) {
        if self.repository_menu.begin_close() {
            popover::reap_popup(cx, |page: &mut Self| &mut page.repository_menu);
            cx.notify();
        }
    }

    fn render_repository_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let label = self.repository.clone().unwrap_or_else(|| {
            if self.initial_scope_task.is_some() {
                "Detecting repository…".into()
            } else {
                "Choose repository".into()
            }
        });
        let tooltip = label.clone();
        let mut trigger =
            crate::surface_chrome::tab("pr-repository", self.repository_menu.is_open(), theme)
                .debug_selector(|| "pr-repository".into())
                .h(px(28.0))
                .px(px(8.0))
                .max_w(px(220.0))
                .min_w_0()
                .aria_label(format!("Repository: {label}"))
                .aria_expanded(self.repository_menu.is_open())
                .tooltip(move |_, cx| cx.new(|_| DashboardTooltip(tooltip.clone().into())).into())
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|page, _, _, _| page.repository_menu.note_trigger_press()),
                )
                .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
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
                        page.repository_menu.open(());
                        window.focus(&page.repository_input.read(cx).focus_handle(cx), cx);
                    }
                    cx.notify();
                }))
                .child(
                    icon(icons::FOLDER_WITH_FILES)
                        .size(px(14.0))
                        .text_color(theme.text_muted),
                )
                .child(div().min_w_0().truncate().child(label))
                .child(
                    icon(icons::ALT_ARROW_DOWN)
                        .size(px(12.0))
                        .text_color(theme.text_muted),
                );
        if self.repository_menu.get().is_some() {
            let mut recent: Vec<String> = self
                .snapshots
                .iter()
                .rev()
                .filter(|((target, _, _), _, _)| target == &self.target_device)
                .map(|((_, repo, _), _, _)| repo.clone())
                .collect();
            let mut seen = HashSet::new();
            recent.retain(|repo| seen.insert(repo.clone()));
            recent.truncate(5);
            let menu = popover::popover_card(theme)
                .w(px(280.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .on_mouse_down_out(cx.listener(|page, _, _, cx| page.close_repository_menu(cx)))
                .child(popover::menu_heading(theme, "Repository"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .child(
                            crate::surface_chrome::input()
                                .h(px(30.0))
                                .child(self.repository_input.clone()),
                        )
                        .child(
                            crate::surface_chrome::tab("pr-repository-load", false, theme)
                                .debug_selector(|| "pr-repository-load".into())
                                .h(px(30.0))
                                .px(px(8.0))
                                .aria_label("Open repository")
                                .child("Open")
                                .on_click(cx.listener(|page, _, _, cx| {
                                    cx.stop_propagation();
                                    page.select_repository(cx);
                                })),
                        ),
                )
                .when_some(self.repository_error.clone(), |el, error| {
                    el.child(widgets::error_strip(theme, error))
                })
                .children(recent.into_iter().enumerate().map(|(index, repo)| {
                    popover::menu_row(
                        theme,
                        self.repository.as_ref() == Some(&repo),
                        format!("pr-recent-{index}"),
                    )
                    .id(("pr-recent", index))
                    .role(gpui::Role::Button)
                    .tab_index(0)
                    .aria_label(format!("Open {repo}"))
                    .focus_visible(|style| style.bg(theme.glass_hover()))
                    .child(div().truncate().child(repo.clone()))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        cx.stop_propagation();
                        page.repository_input
                            .update(cx, |input, cx| input.set_text(&repo, cx));
                        page.select_repository(cx);
                    }))
                }));
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
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|page, _, _, _| page.sort_menu.note_trigger_press()),
            )
            .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    page.close_sort_menu(cx);
                }
            }))
            .on_click(cx.listener(|page, event, _, cx| {
                cx.stop_propagation();
                let open = if matches!(event, gpui::ClickEvent::Keyboard(_)) {
                    page.sort_menu.is_open()
                } else {
                    page.sort_menu.take_press_was_open()
                };
                if open {
                    page.close_sort_menu(cx);
                } else {
                    page.sort_menu.open(());
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
            .tooltip(move |_, cx| cx.new(|_| DashboardTooltip(label.into())).into());
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
                        .debug_selector(move || id.into())
                        .role(gpui::Role::Button)
                        .tab_index(0)
                        .aria_selected(active)
                        .aria_label(label)
                        .focus_visible(|style| style.bg(theme.glass_hover()))
                        .child(label)
                        .on_click(cx.listener(move |page, _, _, cx| {
                            cx.stop_propagation();
                            page.sort = PullRequestSort { field, direction };
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
        self.load_state = PullRequestsLoadState::Loading;
        self.request_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::LIST_FILTERED_CHANGE_REQUESTS, params)
                .await;
            this.update(cx, |page, cx| {
                if !response_is_current(page.generation, generation) {
                    return;
                }

                page.request_task = None;
                let loaded = match result {
                    Ok(value) => serde_json::from_value::<Vec<ChangeRequestListItem>>(value)
                        .map_err(|_| PullRequestsPageError::Network),
                    Err(error) => Err(map_rpc_error(&error, &target_name)),
                };
                let succeeded = loaded.is_ok();
                page.load_state = settle_snapshot(&mut page.items, loaded);
                page.view_items = None;
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
                    let now = Instant::now();
                    page.last_loaded_at = Some(now);
                    let key = (
                        page.target_device.clone(),
                        page.repository.clone().unwrap(),
                        page.filter,
                    );
                    page.snapshots.retain(|(existing, _, _)| existing != &key);
                    page.snapshots.push((key, page.items.clone(), now));
                    if page.snapshots.len() > 12 {
                        page.snapshots.remove(0);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
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
            .tab_index(0)
            .border_1()
            .border_color(gpui::transparent_black())
            .focus_visible(|style| style.border_color(theme.accent))
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
                    page.close_device_menu(cx);
                }
            }))
            .on_click(cx.listener(|page, event, _, cx| {
                let was_open = if matches!(event, gpui::ClickEvent::Keyboard(_)) {
                    page.device_menu.is_open()
                } else {
                    page.device_menu.take_press_was_open()
                };
                if was_open {
                    page.close_device_menu(cx);
                } else {
                    page.device_menu.open(());
                }
                cx.notify();
            }))
            .child(icon(glyph).size(px(16.0)).text_color(theme.text_muted))
            .child(
                div()
                    .max_w(px(130.0))
                    .truncate()
                    .text_size(px(12.5))
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
                .child(popover::menu_heading(theme, "Desktop devices"))
                .children(devices.into_iter().enumerate().map(|(index, device)| {
                    let active = effective.as_deref() == Some(device.id.as_str());
                    let local = local_id.as_deref() == Some(device.id.as_str());
                    let device_id = device.id.clone();
                    let name: SharedString = device.name.clone().into();
                    let online = self.state.read(cx).device_online(&device.id, Utc::now());
                    popover::menu_row(theme, active, format!("pull-requests-device-row-{index}"))
                        .id(("pull-requests-device-row", index))
                        .role(gpui::Role::Button)
                        .aria_label(format!(
                            "{}{}",
                            device.name,
                            if online { "" } else { ", offline" }
                        ))
                        .aria_selected(active)
                        .tab_index(0)
                        .focus_visible(|style| style.bg(theme.glass_hover()))
                        .on_click(cx.listener(move |page, _, _, cx| {
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
                                    .text_size(px(10.5))
                                    .text_color(theme.text_muted)
                                    .child("This device"),
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

    fn render_empty_or_error(&self, theme: &Theme) -> AnyElement {
        let glyph = match &self.load_state {
            PullRequestsLoadState::Failed(PullRequestsPageError::Authentication) => {
                icons::KEY_MINIMALISTIC
            }
            PullRequestsLoadState::Failed(
                PullRequestsPageError::Network | PullRequestsPageError::RemoteOffline(_),
            ) => icons::WIFI_OFF,
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
            _ => (
                "No open pull requests".to_string(),
                "Pull requests matching this filter will appear here.".to_string(),
            ),
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
                    .text_size(px(15.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(SharedString::from(title)),
            )
            .child(
                div()
                    .mt(px(6.0))
                    .max_w(px(420.0))
                    .text_size(px(13.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(body)),
            )
            .into_any_element()
    }
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
        .then_some(self.items.len());
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
                    .child(widgets::page_header(&theme, "Pull requests", count).when(
                        self.items.len() == 50,
                        |el| {
                            el.child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme.text_muted)
                                    .child("latest 50"),
                            )
                        },
                    ))
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap(px(8.0))
                            .child(self.render_repository_menu(&theme, cx))
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
                                    .focus_visible(|style| style.border_color(theme.accent))
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
                                    .tooltip(move |_, cx| {
                                        cx.new(|_| DashboardTooltip(freshness.clone())).into()
                                    }),
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
                                                    style.bg(theme.glass_hover())
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
            .when(!self.query.is_empty(), |el| {
                el.child(
                    div()
                        .mt(px(Theme::SPACE_SM))
                        .text_size(crate::typography::ui_rems(11.0))
                        .text_color(theme.text_muted)
                        .child(format!(
                            "{} of {} pull requests",
                            items.len(),
                            self.items.len()
                        )),
                )
            });
        let content = if initial_loading {
            div()
                .mt(px(72.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.0))
                .text_size(crate::typography::ui_rems(13.0))
                .text_color(theme.text_muted)
                .child(crate::loaders::gradient_spinner(
                    "pull-requests-loading",
                    &theme,
                    3.0,
                    cx.entity_id(),
                    cx,
                ))
                .child(if self.initial_scope_task.is_some() {
                    "Finding repository…"
                } else {
                    "Loading pull requests…"
                })
                .into_any_element()
        } else if self.items.is_empty() {
            self.render_empty_or_error(&theme)
        } else if items.is_empty() {
            div()
                .id("pull-requests-no-results")
                .debug_selector(|| "pull-requests-no-results".to_string())
                .py(px(48.0))
                .text_center()
                .text_size(crate::typography::ui_rems(13.0))
                .text_color(theme.text_muted)
                .child("No matching pull requests")
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
                                        .child(content),
                                ),
                        )
                        .fade_overflow_y(&scroll),
                    )
                    .children(scrollbar),
            )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PullRequestTableLayout {
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
                            .focus_visible(|style| style.border_color(theme.accent))
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
                                    selected_url == Some(item.url.as_str()),
                                    theme,
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

fn render_table_row(
    item: &ChangeRequestListItem,
    layout: PullRequestTableLayout,
    sort_field: PullRequestSortField,
    first: bool,
    selected: bool,
    theme: &Theme,
) -> AnyElement {
    let url = item.url.clone();
    let row = widgets::card_row(theme, first)
        .flex_nowrap()
        .min_w_0()
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
        .min_h(px(PR_TABLE_ROW_HEIGHT))
        .when(selected, |el| el.bg(theme.glass_hover()))
        .focus_visible(|style| style.bg(theme.accent.opacity(0.12)))
        .flex_none()
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
            .tooltip(move |_, cx| cx.new(|_| DashboardTooltip(exact.clone())).into())
            .child(SharedString::from(
                if sort_field == PullRequestSortField::Opened {
                    format!("Opened {}", compact_relative_time(timestamp, Utc::now()))
                } else {
                    compact_relative_time(timestamp, Utc::now())
                },
            ))
    };
    if layout == PullRequestTableLayout::Narrow {
        row.flex()
            .flex_col()
            .items_stretch()
            .gap(px(Theme::SPACE_SM))
            .child(render_pr_identity(item, theme))
            .child(
                div()
                    .pl(px(22.0))
                    .flex()
                    .items_center()
                    .gap(px(Theme::SPACE_SM))
                    .child(render_diff_stats(item, theme))
                    .child(div().flex_1())
                    .child(updated()),
            )
            .into_any_element()
    } else {
        row.flex()
            .items_center()
            .gap(px(Theme::SPACE_LG))
            .child(render_pr_identity(item, theme))
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
            .into_any_element()
    }
}

fn status_description(item: &ChangeRequestListItem) -> String {
    let mut labels = vec![if item.is_draft { "Draft" } else { "Open" }];
    if item.mergeability == ChangeRequestMergeability::Conflicting {
        labels.push("Merge conflicts");
    }
    match item.review_decision {
        ChangeRequestReviewDecision::ChangesRequested => labels.push("Changes requested"),
        ChangeRequestReviewDecision::Approved => labels.push("Approved"),
        _ => {}
    }
    labels.join(" · ")
}

fn render_pr_identity(item: &ChangeRequestListItem, theme: &Theme) -> AnyElement {
    let title = SharedString::from(single_line(&item.title));
    let full_title = title.clone();
    let status = status_description(item);
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
                .tooltip(move |_, cx| cx.new(|_| DashboardTooltip(full_title.clone())).into())
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
                                .text_size(px(11.0))
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
                .when(status != "Open", |el| {
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
        .tooltip(move |_, cx| cx.new(|_| DashboardTooltip(exact.clone())).into())
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

fn initial_repository_scope(
    current: Option<String>,
    target: Option<String>,
    fallback: Option<(String, Option<String>)>,
) -> Option<(String, Option<String>)> {
    current
        .filter(|repo| valid_repository_filter(repo))
        .map(|repo| (repo, target))
        .or_else(|| fallback.filter(|(repo, _)| valid_repository_filter(repo)))
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
            "Check the connection and try again.".into(),
        ),
        PullRequestsPageError::RemoteOffline(name) => (
            format!("{name} is offline"),
            "Reconnect the device and try again.".into(),
        ),
        PullRequestsPageError::UpdateRequired(name) => (
            format!("Update Zeron on {name}"),
            "This device doesn’t support the pull request dashboard yet.".into(),
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

fn response_is_current(current: u64, response: u64) -> bool {
    current == response
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
                false,
                Theme::of(cx),
            ))
        }
    }

    #[test]
    fn pull_request_default_scope_prefers_current_and_preserves_fallback_device() {
        let saved = Some(("saved/repo".into(), Some("remote".into())));
        assert_eq!(
            initial_repository_scope(Some("current/repo".into()), None, saved.clone()),
            Some(("current/repo".into(), None))
        );
        assert_eq!(initial_repository_scope(None, None, saved.clone()), saved);
        assert!(initial_repository_scope(None, None, Some(("bad filter".into(), None))).is_none());
    }

    #[gpui::test]
    fn pull_request_default_scope_reuses_cache_and_persists_selection(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        let temp = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            crate::settings::init(crate::settings::UiSettings::default(), temp.path(), cx);
        });
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
    fn pull_request_board_never_fetches_on_entry_and_restores_repository_cache(
        cx: &mut gpui::TestAppContext,
    ) {
        use gpui::AppContext;
        cx.update(|cx| cx.set_global(Theme::default()));
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
            ));
            page.repository_input
                .update(cx, |input, cx| input.set_text("ACME/ZERON", cx));
            page.select_repository(cx);
            assert_eq!(page.items[0].number, 10);
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            page.on_visible(cx);
            assert_eq!(page.load_state, PullRequestsLoadState::Ready);
            page.reset_for_target(Some("other-device".into()));
            assert!(page.items.is_empty());
            assert_eq!(page.load_state, PullRequestsLoadState::Idle);
            page.reset_for_target(None);
            assert_eq!(page.items[0].number, 10);
            page.repository_input.update(cx, |input, cx| {
                input.set_text("acme/zeron repo:other/repo", cx)
            });
            page.select_repository(cx);
            assert!(page.repository_error.is_some());
            assert_eq!(page.repository.as_deref(), Some("acme/zeron"));
        });
    }

    struct InitialRepositoryRpc(std::sync::Arc<std::sync::Mutex<Vec<String>>>, bool);

    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for InitialRepositoryRpc {
        async fn handle(
            &self,
            method: &str,
            params: serde_json::Value,
        ) -> Result<zeron_rpc::RpcReply, zeron_rpc::RpcError> {
            self.0.lock().unwrap().push(method.into());
            assert!(
                params.get("targetDeviceId").is_none(),
                "local identity comes from engine before device frames"
            );
            match method {
                methods::GET_CHANGE_REQUEST_REPOSITORY => {
                    assert_eq!(params["cwd"], "/checkout");
                    if self.1 {
                        return futures::future::pending().await;
                    }
                    zeron_rpc::RpcReply::value(&Some("owner/repo"))
                }
                methods::LIST_FILTERED_CHANGE_REQUESTS => {
                    assert_eq!(params["repository"], "owner/repo");
                    assert_eq!(params["filter"], "authored");
                    zeron_rpc::RpcReply::value(&vec![pull_request("owner/canonical", 7, 1, 1, 1)])
                }
                _ => panic!("unexpected request {method}"),
            }
        }
    }

    #[gpui::test]
    fn pull_request_initial_load_waits_for_engine_and_project(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let settings_dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            crate::settings::init(
                {
                    let mut settings = crate::settings::UiSettings::default();
                    settings.space_filter = Some("project".into());
                    settings
                },
                settings_dir.path(),
                cx,
            );
        });
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let state = cx.new(|_| AppState::new());
        let page = cx.new(|cx| PullRequestsPage::new(state.clone(), cx));
        state.update(cx, |state, cx| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zeron_rpc::memory_client(std::sync::Arc::new(InitialRepositoryRpc(
                    calls.clone(),
                    false,
                ))),
            ));
            state.spaces = vec![zeron_proto::Space {
                id: "project".into(),
                device_id: "local".into(),
                path: "/checkout".into(),
                name: None,
                git_detected: true,
                git_checked_at: None,
                checkout_id: None,
                created_at: Utc::now(),
            }];
            state.selected_space = Some("project".into());
            state.no_project = false;
            cx.notify();
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if page.read_with(cx, |page, _| {
                page.load_state == PullRequestsLoadState::Ready
            }) {
                break;
            }
            runtime.block_on(async { tokio::task::yield_now().await });
        }
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
                methods::LIST_FILTERED_CHANGE_REQUESTS
            ]
        );
    }

    #[gpui::test]
    fn pull_request_stalled_discovery_falls_back_once(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let settings_dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            crate::settings::init(
                {
                    let mut settings = crate::settings::UiSettings::default();
                    settings.space_filter = Some("project".into());
                    settings.last_pull_request_repository = Some("owner/repo".into());
                    settings
                },
                settings_dir.path(),
                cx,
            );
        });
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let state = cx.new(|_| AppState::new());
        let page = cx.new(|cx| PullRequestsPage::new(state.clone(), cx));
        state.update(cx, |state, cx| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zeron_rpc::memory_client(std::sync::Arc::new(InitialRepositoryRpc(
                    calls.clone(),
                    true,
                ))),
            ));
            state.spaces = vec![zeron_proto::Space {
                id: "project".into(),
                device_id: "local".into(),
                path: "/checkout".into(),
                name: None,
                git_detected: true,
                git_checked_at: None,
                checkout_id: None,
                created_at: Utc::now(),
            }];
            state.selected_space = Some("project".into());
            state.no_project = false;
            cx.notify();
        });
        for iteration in 0..100 {
            cx.run_until_parked();
            if iteration == 2 {
                cx.executor()
                    .advance_clock(std::time::Duration::from_secs(4));
            }
            if page.read_with(cx, |page, _| {
                page.load_state == PullRequestsLoadState::Ready
            }) {
                break;
            }
            runtime.block_on(async { tokio::task::yield_now().await });
        }
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
                methods::LIST_FILTERED_CHANGE_REQUESTS
            ]
        );
    }

    #[gpui::test]
    fn pull_request_all_projects_loads_last_repo_without_discovery(cx: &mut gpui::TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let settings_dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            crate::settings::init(
                {
                    let mut settings = crate::settings::UiSettings::default();
                    settings.last_pull_request_repository = Some("owner/repo".into());
                    settings
                },
                settings_dir.path(),
                cx,
            );
        });
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let state = cx.new(|_| AppState::new());
        let page = cx.new(|cx| PullRequestsPage::new(state.clone(), cx));
        state.update(cx, |state, cx| {
            state.set_test_engine(crate::state::EngineHandle::from_test_client(
                zeron_rpc::memory_client(std::sync::Arc::new(InitialRepositoryRpc(
                    calls.clone(),
                    false,
                ))),
            ));
            state.spaces = vec![zeron_proto::Space {
                id: "project".into(),
                device_id: "local".into(),
                path: "/checkout".into(),
                name: None,
                git_detected: true,
                git_checked_at: None,
                checkout_id: None,
                created_at: Utc::now(),
            }];
            state.selected_space = Some("project".into());
            state.no_project = false;
            cx.notify();
        });
        for _ in 0..100 {
            cx.run_until_parked();
            if page.read_with(cx, |page, _| {
                page.load_state == PullRequestsLoadState::Ready
            }) {
                break;
            }
            runtime.block_on(async { tokio::task::yield_now().await });
        }
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
            [methods::LIST_FILTERED_CHANGE_REQUESTS]
        );
    }

    #[gpui::test]
    fn pull_request_filter_switches_restore_only_matching_snapshots(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::default()));
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
        cx.update(|cx| cx.set_global(Theme::default()));
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
            assert!(title.right() <= row.right(), "title overflow at {width}: row={row:?}, title={title:?}");
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
    fn narrow_sorting_and_refresh_stay_reachable(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        cx.update(|cx| cx.set_global(Theme::default()));
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
        // Popup exit uses wall-clock Instant as well as the executor timer.
        std::thread::sleep(
            crate::motion::MENU_OUT
                .total()
                .mul_f32(crate::motion::speed_scale())
                + std::time::Duration::from_millis(30),
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
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

    #[gpui::test]
    fn pull_request_column_matches_settings_width(cx: &mut gpui::TestAppContext) {
        use gpui::AppContext;
        cx.update(|cx| cx.set_global(Theme::default()));
        let (_, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            PullRequestsPage::new(state, cx)
        });
        cx.simulate_resize(gpui::size(px(1200.0), px(800.0)));
        cx.run_until_parked();
        let column = cx.debug_bounds("pull-requests-column").unwrap();
        assert_eq!(column.size.width, px(760.0));
        assert_eq!(column.left(), px(220.0));
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
        cx.update(|cx| cx.set_global(Theme::default()));
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
            "Draft · Merge conflicts · Changes requested"
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
    fn table_breakpoints_follow_the_measured_content_width() {
        assert_eq!(table_layout(639.0), PullRequestTableLayout::Narrow);
        assert_eq!(table_layout(640.0), PullRequestTableLayout::Compact);
        assert_eq!(table_layout(899.0), PullRequestTableLayout::Compact);
        assert_eq!(table_layout(900.0), PullRequestTableLayout::Wide);
        assert_eq!(
            table_layout(table_content_width(719.0)),
            PullRequestTableLayout::Narrow
        );
        assert_eq!(
            table_layout(table_content_width(720.0)),
            PullRequestTableLayout::Compact
        );
        assert_eq!(
            table_layout(table_content_width(948.0)),
            PullRequestTableLayout::Compact
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
    fn sorting_defaults_to_recent_updates_and_toggles_each_column() {
        assert_eq!(
            PullRequestSort::DEFAULT,
            PullRequestSort {
                field: PullRequestSortField::Updated,
                direction: SortDirection::Descending,
            }
        );
        assert_eq!(
            PullRequestSort::DEFAULT.select(PullRequestSortField::Updated),
            PullRequestSort {
                field: PullRequestSortField::Updated,
                direction: SortDirection::Ascending,
            }
        );
        assert_eq!(
            PullRequestSort::DEFAULT.select(PullRequestSortField::Changes),
            PullRequestSort {
                field: PullRequestSortField::Changes,
                direction: SortDirection::Descending,
            }
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
    fn stale_responses_cannot_replace_a_new_target_snapshot() {
        assert!(response_is_current(7, 7));
        assert!(!response_is_current(8, 7));
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
                &RpcError::UnknownMethod("ListOpenChangeRequests".into()),
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
}
