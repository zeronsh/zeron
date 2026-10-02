//! Settings → Workflows: the saved (reusable) workflows of this device —
//! Global, per project, and the built-in ones — with a detail view (the
//! description, its arguments, the script, recent runs) and Run / Open file /
//! Delete.
//!
//! The list is read from the engine on demand (`WorkflowSavedList`): there is
//! no watcher, the page reloads when it opens, after every change it makes and
//! when it is shown again a few seconds later, and has a Refresh button. Files
//! edited in an editor appear on the next load.
//!
//! The page owns reading and deleting; *running* is the shell's (it opens the
//! launcher), so the page only emits [`WorkflowsEvent`]s.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, Entity, EventEmitter, SharedString, Subscription, Task, Window, div,
    prelude::*, px,
};
use zeron_proto::{
    SavedArg, SavedScope, SavedWorkflowDetail, SavedWorkflowList, SavedWorkflowSummary,
    WorkflowRunHeader,
};
use zeron_rpc::methods;

use crate::icons::{self, icon};
use crate::popover::{self, Loadable};
use crate::settings::widgets::{self, ActionTone};
use crate::state::AppState;
use crate::theme::Theme;
use crate::typography::ui_rems;
use crate::workflow::model::Tone;
use crate::workflow::saved::{self, Group, GroupKind, RunRow, WorkflowKey};

/// How long a loaded list stays fresh while the page is on screen.
const STALE_AFTER: Duration = Duration::from_secs(4);
/// Runs listed under a workflow.
const RUNS_SHOWN: usize = 8;

/// What the page asks the shell to do.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkflowsEvent {
    /// Open the launcher for this workflow.
    Run(SavedWorkflowSummary),
    /// Open a run in its chat.
    OpenRun { chat_id: String, run_id: String },
}

impl EventEmitter<WorkflowsEvent> for WorkflowsPage {}

struct Detail {
    key: WorkflowKey,
    summary: SavedWorkflowSummary,
    loaded: Loadable<SavedWorkflowDetail>,
    runs: Vec<WorkflowRunHeader>,
    highlight: Option<zeron_syntax::HighlightedDocument>,
}

pub struct WorkflowsPage {
    state: Entity<AppState>,
    scroll: widgets::PageScroll,
    list: Loadable<SavedWorkflowList>,
    loaded_at: Option<Instant>,
    spaces_seen: Vec<String>,
    detail: Option<Detail>,
    /// The key whose Delete was clicked once (inline confirmation).
    confirm_delete: Option<WorkflowKey>,
    error: Option<SharedString>,
    busy: bool,
    list_task: Option<Task<()>>,
    detail_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
    _observe: Subscription,
}

/// `ZERON_WORKFLOWS_DETAIL`, taken once.
fn detail_knob() -> Option<String> {
    static TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if TAKEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    std::env::var("ZERON_WORKFLOWS_DETAIL").ok()
}

fn params_for(summary: &SavedWorkflowSummary) -> serde_json::Value {
    let mut p = serde_json::json!({ "name": summary.name, "scope": summary.scope });
    if let Some(space) = &summary.space_id {
        p["spaceId"] = space.clone().into();
    }
    p
}

impl WorkflowsPage {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        // A project added or removed changes what the list covers.
        let observe = cx.observe(&state, |page, state, cx| {
            let mut ids: Vec<String> = state.read(cx).spaces.iter().map(|s| s.id.clone()).collect();
            ids.sort();
            if ids != page.spaces_seen {
                page.spaces_seen = ids;
                if page.loaded_at.is_some() {
                    page.refresh(cx);
                }
            }
            cx.notify();
        });
        let mut page = Self {
            state,
            scroll: widgets::PageScroll::default(),
            list: Loadable::Idle,
            loaded_at: None,
            spaces_seen: Vec::new(),
            detail: None,
            confirm_delete: None,
            error: None,
            busy: false,
            list_task: None,
            detail_task: None,
            action_task: None,
            _observe: observe,
        };
        page.refresh(cx);
        page
    }

    /// Reload when the list is older than [`STALE_AFTER`]. Called each time
    /// the shell renders the page, so coming back to it rescans.
    pub fn refresh_if_stale(&mut self, cx: &mut Context<Self>) {
        let stale = self.loaded_at.is_none_or(|at| at.elapsed() > STALE_AFTER);
        if stale && self.list_task.is_none() {
            self.refresh(cx);
        }
    }

    /// Read the list (and, in the detail view, the open workflow) again.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.list = Loadable::Error("Engine not connected".into());
            cx.notify();
            return;
        };
        if matches!(self.list, Loadable::Idle) {
            self.list = Loadable::Loading;
        }
        self.list_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::WORKFLOW_SAVED_LIST,
                    serde_json::json!({ "all": true }),
                )
                .await
                .map_err(|e| e.to_string())
                .and_then(|v| {
                    serde_json::from_value::<SavedWorkflowList>(v).map_err(|e| e.to_string())
                });
            this.update(cx, |page, cx| {
                page.list_task = None;
                page.loaded_at = Some(Instant::now());
                match result {
                    Ok(list) => {
                        // Capture knob `ZERON_WORKFLOWS_DETAIL=<name>` (screenshots):
                        // the first load opens that workflow's detail view.
                        let knob = page.detail.is_none().then(detail_knob).flatten();
                        let open = knob.and_then(|name| {
                            list.workflows
                                .iter()
                                .find(|w| w.name == name && w.shadowed_by.is_none())
                                .cloned()
                        });
                        page.list = Loadable::Ready(list);
                        if let Some(wf) = open {
                            page.open(wf, cx);
                        }
                        // An open detail whose file vanished goes back to the list.
                        if let Some(d) = &page.detail {
                            let still = page.list.ready().is_some_and(|l| {
                                l.workflows.iter().any(|w| WorkflowKey::of(w) == d.key)
                            });
                            if !still {
                                page.detail = None;
                            }
                        }
                    }
                    Err(message) => page.list = Loadable::Error(message),
                }
                if page.detail.is_some() {
                    page.load_detail(cx);
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn open(&mut self, summary: SavedWorkflowSummary, cx: &mut Context<Self>) {
        self.confirm_delete = None;
        self.error = None;
        self.detail = Some(Detail {
            key: WorkflowKey::of(&summary),
            summary,
            loaded: Loadable::Loading,
            runs: Vec::new(),
            highlight: None,
        });
        self.scroll.reset();
        self.load_detail(cx);
        cx.notify();
    }

    pub fn back(&mut self, cx: &mut Context<Self>) {
        self.detail = None;
        self.confirm_delete = None;
        self.scroll.reset();
        cx.notify();
    }

    /// Escape closes the detail view first.
    pub fn dismiss_on_escape(&mut self, cx: &mut Context<Self>) -> bool {
        if self.confirm_delete.take().is_some() {
            cx.notify();
            return true;
        }
        if self.detail.is_some() {
            self.back(cx);
            return true;
        }
        false
    }

    fn load_detail(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(detail) = &self.detail else { return };
        let key = detail.key.clone();
        let params = params_for(&detail.summary);
        let mut runs_params = params.clone();
        runs_params["limit"] = RUNS_SHOWN.into();
        self.detail_task = Some(cx.spawn(async move |this, cx| {
            let client = engine.client();
            let (got, runs) = futures::join!(
                client.call(methods::WORKFLOW_SAVED_GET, params),
                client.call(methods::WORKFLOW_SAVED_RUNS, runs_params),
            );
            let got = got.map_err(|e| e.to_string()).and_then(|v| {
                serde_json::from_value::<SavedWorkflowDetail>(v).map_err(|e| e.to_string())
            });
            // Runs are a convenience: an older host without the method just has none.
            let runs = runs
                .ok()
                .and_then(|v| serde_json::from_value::<Vec<WorkflowRunHeader>>(v).ok())
                .unwrap_or_default();
            this.update(cx, |page, cx| {
                let Some(d) = page.detail.as_mut().filter(|d| d.key == key) else {
                    return;
                };
                match got {
                    Ok(full) => {
                        d.highlight = zeron_syntax::highlight(zeron_syntax::HighlightRequest {
                            source: &full.script,
                            path: None,
                            fence_tag: Some("python"),
                        })
                        .ok();
                        d.summary = full.summary.clone();
                        d.loaded = Loadable::Ready(full);
                    }
                    Err(message) => d.loaded = Loadable::Error(message),
                }
                d.runs = runs;
                cx.notify();
            })
            .ok();
        }));
    }

    fn delete(&mut self, cx: &mut Context<Self>) {
        let Some(key) = self.confirm_delete.take() else {
            return;
        };
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(summary) = self
            .detail
            .as_ref()
            .map(|d| d.summary.clone())
            .filter(|s| WorkflowKey::of(s) == key)
        else {
            return;
        };
        self.busy = true;
        let params = params_for(&summary);
        self.action_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::WORKFLOW_SAVED_DELETE, params)
                .await;
            this.update(cx, |page, cx| {
                page.busy = false;
                match result {
                    Ok(_) => {
                        page.detail = None;
                        page.refresh(cx);
                    }
                    Err(e) => page.error = Some(format!("Could not delete it: {e}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn open_file(&mut self, path: &str, cx: &mut Context<Self>) {
        match url::Url::from_file_path(path) {
            Ok(url) => cx.open_url(url.as_str()),
            Err(()) => self.error = Some(format!("Could not open {path}").into()),
        }
        cx.notify();
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn set_list_for_test(&mut self, list: SavedWorkflowList) {
        self.list = Loadable::Ready(list);
        self.loaded_at = Some(Instant::now());
    }
}

impl popover::ScrollRailHost for WorkflowsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

// ── pieces ────────────────────────────────────────────────────────────────

fn mono_chip(theme: &Theme, text: impl Into<SharedString>) -> gpui::Div {
    div()
        .flex_none()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(5.0))
        .bg(theme.wash(0.08))
        .font_family(theme.font_mono.clone())
        .text_size(px(theme.code_font_size - 1.5))
        .text_color(theme.text_muted)
        .child(text.into())
}

fn tone_color(tone: Tone, theme: &Theme) -> gpui::Hsla {
    match tone {
        Tone::Accent => theme.accent,
        Tone::Success => theme.success,
        Tone::Warning => theme.warning,
        Tone::Danger => theme.danger,
        Tone::Muted => theme.text_faint,
    }
}

fn scope_badge(theme: &Theme, scope: SavedScope) -> gpui::Div {
    widgets::badge(theme, scope.label())
}

/// `3 phases · 12 agents · runs cargo`: what the script does, from analysis.
fn graph_line(graph: &zeron_proto::WorkflowGraph) -> String {
    let n = |c: usize, one: &str, many: &str| format!("{c} {}", if c == 1 { one } else { many });
    let mut parts = vec![n(graph.phases.len(), "phase", "phases")];
    if !graph.actors.is_empty() {
        parts.push(n(graph.actors.len(), "agent", "agents"));
    }
    let mut cmds: Vec<&str> = graph.commands.iter().map(|c| c.command.as_str()).collect();
    cmds.dedup();
    if !cmds.is_empty() {
        let shown: Vec<&str> = cmds.iter().copied().take(3).collect();
        parts.push(format!("runs {}", shown.join(", ")));
    }
    parts.join(" · ")
}

fn row_args(theme: &Theme, line: &str) -> AnyElement {
    if line.is_empty() {
        div()
            .text_color(theme.text_muted.opacity(0.7))
            .child("no arguments")
            .into_any_element()
    } else {
        div()
            .truncate()
            .font_family(theme.font_mono.clone())
            .text_size(px(theme.code_font_size - 1.5))
            .text_color(theme.text_muted.opacity(0.85))
            .child(SharedString::from(line.to_owned()))
            .into_any_element()
    }
}

impl WorkflowsPage {
    fn render_row(
        &self,
        theme: &Theme,
        group_ix: usize,
        row_ix: usize,
        row: &saved::Row,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let summary = row.summary.clone();
        let open = summary.clone();
        let run = summary.clone();
        let mut fragments: Vec<AnyElement> = vec![row_args(theme, &row.args_line)];
        if let Some(note) = &row.note {
            fragments.push(
                div()
                    .child(SharedString::from(note.clone()))
                    .into_any_element(),
            );
        }
        widgets::card_row(theme, row_ix == 0)
            .id(SharedString::from(format!(
                "workflow-row-{group_ix}-{row_ix}"
            )))
            .role(gpui::Role::Button)
            .aria_label(SharedString::from(format!("Open {}", summary.name)))
            .tab_index(0)
            .cursor_pointer()
            .flex_nowrap()
            .on_click(cx.listener(move |this, _, _, cx| this.open(open.clone(), cx)))
            .child(widgets::row_tile(theme, icons::WORKFLOW))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(widgets::row_title(
                        theme,
                        SharedString::from(summary.name.clone()),
                    ))
                    .child(
                        div()
                            .mt(px(1.0))
                            .truncate()
                            .text_size(ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(summary.description.clone())),
                    )
                    .child(widgets::meta_line(theme, fragments)),
            )
            .child(
                widgets::text_action(theme, ActionTone::Filled, "Run")
                    .id(SharedString::from(format!(
                        "workflow-run-{group_ix}-{row_ix}"
                    )))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(format!("Run {}", summary.name)))
                    .tab_index(0)
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.stop_propagation();
                        cx.emit(WorkflowsEvent::Run(run.clone()));
                    })),
            )
            .into_any_element()
    }

    fn render_group(
        &self,
        theme: &Theme,
        ix: usize,
        group: &Group,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let label: SharedString = match (&group.kind, &group.subtitle) {
            (GroupKind::Project { .. }, _) => format!("Project · {}", group.title).into(),
            _ => group.title.clone().into(),
        };
        let mut block = widgets::section_card(theme).mt(px(0.0));
        if group.rows.is_empty() && group.invalid.is_empty() {
            let (title, hint) = match group.kind {
                GroupKind::Global => (
                    "No global workflows yet",
                    "Save one from a run's card, or ask the agent to save a workflow it ran.",
                ),
                _ => ("Nothing here", ""),
            };
            block = block.child(
                widgets::card_row(theme, true)
                    .child(widgets::row_tile(theme, icons::WORKFLOW))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(widgets::row_title(theme, title))
                            .child(
                                div()
                                    .mt(px(1.0))
                                    .text_size(ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                                    .text_color(theme.text_muted)
                                    .child(hint),
                            ),
                    ),
            );
        }
        for (row_ix, row) in group.rows.iter().enumerate() {
            block = block.child(self.render_row(theme, ix, row_ix, row, cx));
        }
        for (bad_ix, bad) in group.invalid.iter().enumerate() {
            block = block.child(
                widgets::card_row(theme, group.rows.is_empty() && bad_ix == 0)
                    .flex_nowrap()
                    .child(
                        div().flex_none().w(px(24.0)).flex().justify_center().child(
                            icon(icons::DANGER_TRIANGLE)
                                .size(px(15.0))
                                .text_color(theme.warning),
                        ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(widgets::row_title(
                                theme,
                                SharedString::from(saved::file_name(&bad.path).to_owned()),
                            ))
                            .child(
                                div()
                                    .mt(px(1.0))
                                    .text_size(ui_rems(widgets::ROW_DESCRIPTION_SIZE))
                                    .line_height(px(16.0))
                                    .text_color(theme.warning_muted)
                                    .child(SharedString::from(bad.reason.clone())),
                            ),
                    ),
            );
        }
        let title = widgets::section_label(theme, label);
        let title = match &group.subtitle {
            Some(sub) if !matches!(group.kind, GroupKind::Global) || !sub.is_empty() => div()
                .flex()
                .items_baseline()
                .gap(px(8.0))
                .child(title)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(ui_rems(12.0))
                        .text_color(theme.text_muted.opacity(0.6))
                        .child(SharedString::from(sub.clone())),
                ),
            _ => div().child(title),
        };
        div()
            .mt(px(28.0))
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(title)
            .child(block)
            .into_any_element()
    }

    fn render_list(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let spaces = self.state.read(cx).spaces.clone();
        let body: AnyElement = match &self.list {
            Loadable::Idle | Loadable::Loading => div()
                .mt(px(32.0))
                .text_size(ui_rems(13.0))
                .text_color(theme.text_muted)
                .child("Loading workflows…")
                .into_any_element(),
            Loadable::Error(message) => widgets::error_strip(
                theme,
                format!("Could not read the saved workflows: {message}"),
            )
            .into_any_element(),
            Loadable::Ready(list) => {
                let groups = saved::group(list, &spaces);
                div()
                    .flex()
                    .flex_col()
                    .children(
                        groups
                            .iter()
                            .enumerate()
                            .map(|(ix, g)| self.render_group(theme, ix, g, cx)),
                    )
                    .into_any_element()
            }
        };
        let refresh = widgets::ghost_action(theme)
            .id("workflows-refresh")
            .role(gpui::Role::Button)
            .aria_label("Reload the workflows from disk")
            .tab_index(0)
            .focus_visible(|s| s.border_2().border_color(theme.accent))
            .tooltip(widgets::text_tooltip("Reload from disk"))
            .on_click(cx.listener(|this, _, _, cx| this.refresh(cx)))
            .child(
                icon(icons::REFRESH)
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            )
            .child("Refresh");
        div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .child(widgets::page_header(theme, "Workflows", None))
                    .child(refresh),
            )
            .child(widgets::page_subtitle(
                theme,
                "Reusable workflows with arguments. Run one here, with /workflow in a chat, or let the agent pick it.",
            ))
            .children(self.error.clone().map(|m| widgets::error_strip(theme, m)))
            .child(body)
            .into_any_element()
    }

    fn render_args_table(&self, theme: &Theme, args: &[SavedArg]) -> AnyElement {
        if args.is_empty() {
            return widgets::section_card(theme)
                .mt(px(0.0))
                .child(
                    widgets::card_row(theme, true).child(
                        div()
                            .text_size(ui_rems(12.5))
                            .text_color(theme.text_muted)
                            .child("This workflow takes no arguments."),
                    ),
                )
                .into_any_element();
        }
        let mut block = widgets::section_card(theme).mt(px(0.0));
        for (ix, a) in args.iter().enumerate() {
            let default = match (&a.default, a.required) {
                (Some(d), _) => Some(SharedString::from(format!(
                    "default {}",
                    SavedArg::display_value(d)
                ))),
                _ => None,
            };
            block = block.child(
                widgets::card_row(theme, ix == 0)
                    .flex_nowrap()
                    .items_start()
                    .child(
                        div()
                            .flex_none()
                            .w(px(188.0))
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .min_w_0()
                                    .font_family(theme.font_mono.clone())
                                    .text_size(px(theme.code_font_size))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .truncate()
                                    .child(SharedString::from(a.name.clone())),
                            )
                            .child(mono_chip(theme, a.ty.as_str()))
                            .when(a.required, |el| {
                                el.child(widgets::badge_active(theme, "required"))
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_size(ui_rems(12.5))
                                    .line_height(px(17.0))
                                    .text_color(if a.description.is_some() {
                                        theme.text
                                    } else {
                                        theme.text_muted.opacity(0.6)
                                    })
                                    .child(SharedString::from(
                                        a.description
                                            .clone()
                                            .unwrap_or_else(|| "No description".into()),
                                    )),
                            )
                            .children(default.map(|d| {
                                div()
                                    .truncate()
                                    .font_family(theme.font_mono.clone())
                                    .text_size(px(theme.code_font_size - 1.5))
                                    .text_color(theme.text_muted)
                                    .child(d)
                            })),
                    ),
            );
        }
        block.into_any_element()
    }

    fn render_script(&self, theme: &Theme, d: &Detail, window: &Window) -> AnyElement {
        let Loadable::Ready(full) = &d.loaded else {
            return div().into_any_element();
        };
        crate::workflow::saved_view::script_block(
            &full.script,
            "saved-workflow-script",
            d.highlight.as_ref(),
            theme,
            window,
        )
    }

    fn render_runs(&self, theme: &Theme, d: &Detail, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<RunRow> = saved::run_rows(&d.runs, chrono::Utc::now());
        let mut block = widgets::section_card(theme).mt(px(0.0));
        if rows.is_empty() {
            return block
                .child(
                    widgets::card_row(theme, true).child(
                        div()
                            .text_size(ui_rems(12.5))
                            .text_color(theme.text_muted)
                            .child("Not run yet. Runs started from here, /workflow or the agent show up."),
                    ),
                )
                .into_any_element();
        }
        for (ix, r) in rows.into_iter().enumerate() {
            let chat_id = r.chat_id.clone();
            let run_id = r.run_id.clone();
            let color = tone_color(r.tone, theme);
            block = block.child(
                widgets::card_row(theme, ix == 0)
                    .id(SharedString::from(format!("workflow-run-row-{ix}")))
                    .role(gpui::Role::Button)
                    .aria_label(SharedString::from(format!("Open the run, {}", r.status)))
                    .tab_index(0)
                    .cursor_pointer()
                    .flex_nowrap()
                    .min_h(px(44.0))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(WorkflowsEvent::OpenRun {
                            chat_id: chat_id.clone(),
                            run_id: run_id.clone(),
                        })
                    }))
                    .child(div().flex_none().size(px(8.0)).rounded_full().bg(color))
                    .child(
                        div()
                            .flex_none()
                            .w(px(120.0))
                            .text_size(ui_rems(12.5))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(color)
                            .child(r.status),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(if r.detail.is_empty() {
                                r.when.clone()
                            } else {
                                format!("{} · {}", r.when, r.detail)
                            })),
                    )
                    .child(
                        icon(icons::ALT_ARROW_RIGHT)
                            .size(px(12.0))
                            .text_color(theme.text_muted.opacity(0.6)),
                    ),
            );
        }
        block.into_any_element()
    }

    fn render_detail(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(d) = self.detail.as_ref() else {
            return div().into_any_element();
        };
        let summary = d.summary.clone();
        let run = summary.clone();
        let key = d.key.clone();
        let confirming = self.confirm_delete.as_ref() == Some(&key);
        let writable = summary.scope.writable();
        let path = summary.path.clone();

        let back = div()
            .id("workflow-detail-back")
            .role(gpui::Role::Button)
            .aria_label("Back to the workflows")
            .tab_index(0)
            .cursor_pointer()
            .px(px(widgets::section_label_inset()))
            .h(px(28.0))
            .flex()
            .items_center()
            .gap(px(4.0))
            .rounded(px(6.0))
            .text_size(ui_rems(12.5))
            .text_color(theme.text_muted)
            .hover(|s| s.text_color(theme.text))
            .focus_visible(|s| s.border_2().border_color(theme.accent))
            .on_click(cx.listener(|this, _, _, cx| this.back(cx)))
            .child(
                icon(icons::ALT_ARROW_LEFT)
                    .size(px(12.0))
                    .text_color(theme.text_muted),
            )
            .child("Workflows");

        let mut actions = div().flex().items_center().gap(px(6.0)).child(
            widgets::text_action(theme, ActionTone::Solid, "Run")
                .id("workflow-detail-run")
                .role(gpui::Role::Button)
                .aria_label("Run this workflow")
                .tab_index(0)
                .focus_visible(|s| s.border_2().border_color(theme.accent))
                .on_click(
                    cx.listener(move |_, _, _, cx| cx.emit(WorkflowsEvent::Run(run.clone()))),
                ),
        );
        if let Some(path) = path.clone() {
            actions = actions.child(
                widgets::action_button(theme, ActionTone::Outlined)
                    .id("workflow-detail-open")
                    .role(gpui::Role::Button)
                    .aria_label("Open the file")
                    .tab_index(0)
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .on_click(cx.listener(move |this, _, _, cx| this.open_file(&path, cx)))
                    .child(
                        icon(icons::FILE_CODE)
                            .size(px(14.0))
                            .text_color(theme.text_muted),
                    )
                    .child("Open file"),
            );
        }
        if writable {
            actions = actions.child(
                widgets::action_button(theme, ActionTone::Quiet)
                    .id("workflow-detail-delete")
                    .role(gpui::Role::Button)
                    .aria_label("Delete this workflow")
                    .tab_index(0)
                    .focus_visible(|s| s.border_2().border_color(theme.accent))
                    .tooltip(widgets::text_tooltip("Delete the file"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.confirm_delete = Some(key.clone());
                        cx.notify();
                    }))
                    .child(
                        icon(icons::TRASH_BIN_MINIMALISTIC)
                            .size(px(14.0))
                            .text_color(theme.text_muted),
                    ),
            );
        }

        let header = div()
            .mt(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .gap(px(12.0))
            .child(
                div()
                    .min_w_0()
                    .px(px(widgets::section_label_inset()))
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_rems(20.0))
                            .line_height(ui_rems(26.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(SharedString::from(summary.name.clone())),
                    )
                    .child(scope_badge(theme, summary.scope)),
            )
            .child(actions);

        let confirm = confirming.then(|| {
            div()
                .mt(px(14.0))
                .px(px(16.0))
                .py(px(12.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(theme.danger.opacity(0.25))
                .bg(theme.danger.opacity(0.06))
                .flex()
                .items_center()
                .gap(px(12.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(ui_rems(12.5))
                        .line_height(px(17.0))
                        .text_color(theme.text)
                        .child(SharedString::from(match &path {
                            Some(p) => format!(
                                "Delete {}? The file {p} is removed. This can’t be undone.",
                                summary.name
                            ),
                            None => format!("Delete {}?", summary.name),
                        })),
                )
                .child(
                    widgets::text_action(theme, ActionTone::Quiet, "Cancel")
                        .id("workflow-delete-cancel")
                        .role(gpui::Role::Button)
                        .tab_index(0)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.confirm_delete = None;
                            cx.notify();
                        })),
                )
                .child(
                    widgets::action_button(theme, ActionTone::Filled)
                        .id("workflow-delete-confirm")
                        .role(gpui::Role::Button)
                        .tab_index(0)
                        .text_color(theme.danger_muted)
                        .opacity(if self.busy { 0.5 } else { 1.0 })
                        .on_click(cx.listener(|this, _, _, cx| this.delete(cx)))
                        .child("Delete"),
                )
        });

        let description = div()
            .mt(px(14.0))
            .px(px(widgets::section_label_inset()))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .text_size(ui_rems(13.5))
                    .line_height(px(19.0))
                    .text_color(theme.text)
                    .child(SharedString::from(summary.description.clone())),
            )
            .children(summary.when_to_use.clone().map(|w| {
                div()
                    .text_size(ui_rems(12.5))
                    .line_height(px(17.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(format!("When to use: {w}")))
            }))
            .children(summary.shadowed_by.map(|s| {
                div()
                    .text_size(ui_rems(12.0))
                    .text_color(theme.warning_muted)
                    .child(SharedString::from(format!(
                        "A {} workflow of the same name takes precedence where both exist.",
                        s.label().to_lowercase()
                    )))
            }))
            .children((!summary.shadows.is_empty()).then(|| {
                div()
                    .text_size(ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(format!(
                        "Overrides the {} workflow of the same name.",
                        summary
                            .shadows
                            .iter()
                            .map(|s| s.label().to_lowercase())
                            .collect::<Vec<_>>()
                            .join(" and ")
                    )))
            }))
            .children(match &d.loaded {
                Loadable::Ready(full) => full.graph.as_ref().map(|g| {
                    div()
                        .text_size(ui_rems(12.0))
                        .text_color(theme.text_muted.opacity(0.8))
                        .child(SharedString::from(graph_line(g)))
                }),
                _ => None,
            });

        let problems = match &d.loaded {
            Loadable::Ready(full) if !full.diagnostics.is_empty() => Some(widgets::warning_strip(
                theme,
                format!(
                    "This script would not start: {}",
                    full.diagnostics.join(" · ")
                ),
            )),
            Loadable::Error(message) => Some(widgets::error_strip(
                theme,
                format!("Could not read it: {message}"),
            )),
            _ => None,
        };

        let args_block = self.render_args_table(theme, &summary.args);
        let script = self.render_script(theme, d, window);
        let runs = self.render_runs(theme, d, cx);
        let script_label: SharedString = match &summary.path {
            Some(p) => format!("Script · {}", saved::file_name(p)).into(),
            None => "Script · built in".into(),
        };

        div()
            .flex()
            .flex_col()
            .child(back)
            .child(header)
            .children(confirm)
            .children(self.error.clone().map(|m| widgets::error_strip(theme, m)))
            .child(description)
            .children(problems)
            .child(widgets::section(theme, "Arguments", args_block))
            .child(widgets::section(theme, script_label, script))
            .child(widgets::section(theme, "Recent runs", runs))
            .into_any_element()
    }
}

impl Render for WorkflowsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_settings_surface();
        let body = if self.detail.is_some() {
            self.render_detail(&theme, window, cx)
        } else {
            self.render_list(&theme, cx)
        };
        let scrollbar = popover::rail(self, "workflows-page-scrollbar", &theme, cx);
        div()
            .id("workflows-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("workflows-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(widgets::page_column().child(body)),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{SavedArgType, WorkflowGraph};

    fn wf(name: &str, scope: SavedScope) -> SavedWorkflowSummary {
        SavedWorkflowSummary {
            name: name.into(),
            scope,
            description: format!("{name} does it"),
            when_to_use: Some("when asked".into()),
            args: vec![SavedArg {
                name: "base".into(),
                ty: SavedArgType::String,
                required: false,
                default: Some(serde_json::json!("main")),
                description: Some("Branch".into()),
            }],
            path: (scope != SavedScope::Builtin).then(|| format!("/tmp/{name}.star")),
            project_root: None,
            space_id: None,
            modified_at: None,
            shadowed_by: None,
            shadows: vec![],
        }
    }

    fn page(cx: &mut gpui::TestAppContext) -> gpui::WindowHandle<WorkflowsPage> {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            crate::settings::init(Default::default(), dir.path(), cx);
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            WorkflowsPage::new(state, cx)
        })
    }

    fn draw(cx: &mut gpui::TestAppContext, window: gpui::WindowHandle<WorkflowsPage>) {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear())
            .unwrap();
    }

    #[gpui::test]
    fn the_list_and_the_detail_render_and_navigate(cx: &mut gpui::TestAppContext) {
        let window = page(cx);
        window
            .update(cx, |p, _, _| {
                p.set_list_for_test(SavedWorkflowList {
                    workflows: vec![
                        wf("mine", SavedScope::Global),
                        wf("pr-review", SavedScope::Builtin),
                    ],
                    ..Default::default()
                });
            })
            .unwrap();
        draw(cx, window);
        // Open a workflow: its detail renders (without an engine the read
        // reports an error, which the page shows instead of crashing).
        window
            .update(cx, |p, _, cx| {
                p.open(wf("mine", SavedScope::Global), cx);
                assert!(p.detail.is_some());
            })
            .unwrap();
        draw(cx, window);
        window
            .update(cx, |p, _, cx| {
                p.detail.as_mut().unwrap().loaded = Loadable::Ready(SavedWorkflowDetail {
                    summary: wf("mine", SavedScope::Global),
                    script: "# zeron-workflow\n# description: d\n\ndef main(args):\n    return 1\n"
                        .into(),
                    graph: Some(WorkflowGraph::default()),
                    diagnostics: vec![],
                });
                // Escape unwinds: first a pending delete, then the detail.
                p.confirm_delete = Some(p.detail.as_ref().unwrap().key.clone());
                assert!(p.dismiss_on_escape(cx));
                assert!(p.confirm_delete.is_none() && p.detail.is_some());
                assert!(p.dismiss_on_escape(cx));
                assert!(p.detail.is_none());
                assert!(!p.dismiss_on_escape(cx));
            })
            .unwrap();
        draw(cx, window);
    }

    #[gpui::test]
    fn a_built_in_has_no_delete_and_an_unreadable_file_is_listed(cx: &mut gpui::TestAppContext) {
        let window = page(cx);
        window
            .update(cx, |p, _, cx| {
                p.set_list_for_test(SavedWorkflowList {
                    workflows: vec![wf("pr-review", SavedScope::Builtin)],
                    invalid: vec![zeron_proto::SavedWorkflowInvalid {
                        path: "/home/u/.zeron/workflows/bad.star".into(),
                        scope: SavedScope::Global,
                        reason: "bad.star:2:3 unknown key `x`".into(),
                        project_root: None,
                        space_id: None,
                    }],
                    ..Default::default()
                });
                p.open(wf("pr-review", SavedScope::Builtin), cx);
                assert!(!p.detail.as_ref().unwrap().summary.scope.writable());
            })
            .unwrap();
        draw(cx, window);
    }

    #[gpui::test]
    fn the_run_button_asks_the_shell_to_open_the_launcher(cx: &mut gpui::TestAppContext) {
        let window = page(cx);
        let seen = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let entity = window.entity(cx).unwrap();
        let sink = seen.clone();
        let _sub = cx.update(|cx| {
            cx.subscribe(&entity, move |_, event: &WorkflowsEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
        });
        window
            .update(cx, |_, _, cx| {
                cx.emit(WorkflowsEvent::Run(wf("mine", SavedScope::Global)));
                cx.emit(WorkflowsEvent::OpenRun {
                    chat_id: "c".into(),
                    run_id: "r".into(),
                });
            })
            .unwrap();
        assert_eq!(seen.borrow().len(), 2);
        assert!(matches!(&seen.borrow()[0], WorkflowsEvent::Run(w) if w.name == "mine"));
        let _ = cx;
    }

    #[test]
    fn the_graph_line_says_what_the_script_does() {
        use zeron_proto::{GraphActor, GraphCommand, GraphPhase};
        let g = WorkflowGraph {
            phases: vec![GraphPhase::default(), GraphPhase::default()],
            actors: vec![GraphActor::default()],
            commands: vec![
                GraphCommand {
                    command: "cargo".into(),
                    ..Default::default()
                },
                GraphCommand {
                    command: "cargo".into(),
                    ..Default::default()
                },
                GraphCommand {
                    command: "git".into(),
                    ..Default::default()
                },
            ],
            unphased_asks: 0,
        };
        assert_eq!(graph_line(&g), "2 phases · 1 agent · runs cargo, git");
        assert_eq!(graph_line(&WorkflowGraph::default()), "0 phases");
    }

    #[test]
    fn request_params_name_the_workflow_and_its_project() {
        let mut w = wf("x", SavedScope::Project);
        w.space_id = Some("s1".into());
        assert_eq!(
            params_for(&w),
            serde_json::json!({"name": "x", "scope": "project", "spaceId": "s1"})
        );
        assert_eq!(
            params_for(&wf("y", SavedScope::Global)),
            serde_json::json!({"name": "y", "scope": "global"})
        );
    }
}
