//! The artifact viewer: markdown, table, metrics and file artifacts of a run.
//!
//! The parsing is pure (and tested); the view fetches over
//! `WorkflowArtifactData` / `WorkflowArtifactRead` and draws what arrives,
//! with loading, error and per-version states.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    AnyElement, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    SharedString, StatefulInteractiveElement as _, Styled as _, Task, Window, div, prelude::*, px,
};
use serde::Deserialize;
use serde_json::Value;
use zeron_proto::ArtifactKind;

use super::model::format_bytes;
use super::widgets::kind_word;
use crate::icons::icon;
use crate::markdown::parser::{BlockTree, parse_full};
use crate::markdown::render::{self, RenderCache, RenderOptions};
use crate::state::AppState;
use crate::theme::Theme;
use crate::typography::ui_rems;

/// Table rows drawn per page.
pub const TABLE_ROWS_PER_PAGE: usize = 200;
/// Longest a file artifact is drawn, in lines.
pub const FILE_MAX_LINES: usize = 2000;

// ── parsing ───────────────────────────────────────────────────────────────

/// `1234567` → `1,234,567`; floats keep up to 4 decimals; null is a dash.
pub fn format_value(v: &Value) -> String {
    match v {
        Value::Null => "—".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                group_digits(&i.to_string())
            } else if let Some(u) = n.as_u64() {
                group_digits(&u.to_string())
            } else {
                let f = n.as_f64().unwrap_or_default();
                let mut s = format!("{f:.4}");
                while s.ends_with('0') {
                    s.pop();
                }
                if s.ends_with('.') {
                    s.pop();
                }
                match s.split_once('.') {
                    Some((int, frac)) => format!("{}.{frac}", group_digits(int)),
                    None => group_digits(&s),
                }
            }
        }
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn group_digits(s: &str) -> String {
    let (sign, digits) = match s.strip_prefix('-') {
        Some(rest) => ("-", rest),
        None => ("", s),
    };
    if digits.len() <= 4 {
        // 1999 reads better ungrouped (years, ids)
        return s.to_owned();
    }
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    format!("{sign}{out}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableData {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    /// Right-aligned: every non-null cell of the column is a number.
    pub numeric: Vec<bool>,
    /// Suggested pixel width per column.
    pub widths: Vec<f32>,
}

pub fn parse_table(json: &str) -> Result<TableData, String> {
    #[derive(Deserialize)]
    struct Raw {
        columns: Vec<String>,
        rows: Vec<Vec<Value>>,
    }
    let raw: Raw = serde_json::from_str(json).map_err(|e| format!("not a table: {e}"))?;
    let n = raw.columns.len();
    let mut numeric = vec![true; n];
    let mut longest: Vec<usize> = raw.columns.iter().map(|c| c.chars().count()).collect();
    let rows: Vec<Vec<String>> = raw
        .rows
        .iter()
        .map(|row| {
            (0..n)
                .map(|c| {
                    let cell = row.get(c).unwrap_or(&Value::Null);
                    if !matches!(cell, Value::Number(_) | Value::Null) {
                        numeric[c] = false;
                    }
                    let text = format_value(cell);
                    longest[c] = longest[c].max(text.chars().take(80).count());
                    text
                })
                .collect()
        })
        .collect();
    let widths = longest
        .iter()
        .map(|chars| (*chars as f32 * 7.4 + 28.0).clamp(72.0, 320.0))
        .collect();
    Ok(TableData {
        columns: raw.columns,
        rows,
        numeric,
        widths,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricTile {
    pub label: String,
    pub value: String,
    pub unit: Option<String>,
}

pub fn parse_metrics(json: &str) -> Result<Vec<MetricTile>, String> {
    #[derive(Deserialize)]
    struct Raw {
        label: String,
        #[serde(default)]
        value: Value,
        #[serde(default)]
        unit: Option<String>,
    }
    let raw: Vec<Raw> = serde_json::from_str(json).map_err(|e| format!("not metrics: {e}"))?;
    Ok(raw
        .into_iter()
        .map(|m| MetricTile {
            label: m.label,
            value: format_value(&m.value),
            unit: m.unit.filter(|u| !u.is_empty()),
        })
        .collect())
}

/// What the viewer draws.
pub enum Content {
    Markdown(Arc<BlockTree>),
    Table { data: TableData, pages: usize },
    Metrics(Vec<MetricTile>),
    Text { lines: Vec<SharedString>, cut: bool },
    Binary,
}

/// Decode a page of artifact bytes for its kind.
pub fn decode(
    kind: ArtifactKind,
    content_type: &str,
    text: Option<&str>,
) -> Result<Content, String> {
    match (kind, text) {
        (ArtifactKind::Markdown, Some(t)) => Ok(Content::Markdown(Arc::new(parse_full(t)))),
        (ArtifactKind::Table, Some(t)) => Ok(Content::Table {
            data: parse_table(t)?,
            pages: 1,
        }),
        (ArtifactKind::Metrics, Some(t)) => Ok(Content::Metrics(parse_metrics(t)?)),
        (ArtifactKind::File, Some(t)) if content_type != "application/octet-stream" => {
            let mut lines: Vec<SharedString> = t
                .lines()
                .take(FILE_MAX_LINES + 1)
                .map(|l| SharedString::from(l.replace('\t', "    ")))
                .collect();
            let cut = lines.len() > FILE_MAX_LINES;
            lines.truncate(FILE_MAX_LINES);
            Ok(Content::Text { lines, cut })
        }
        (_, None) | (ArtifactKind::File, _) => Ok(Content::Binary),
    }
}

// ── view ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionInfo {
    version: u32,
    #[serde(default)]
    bytes: u64,
}

enum Load {
    Loading,
    Failed(String),
    Ready {
        content: Content,
        total: u64,
        shown: u64,
    },
}

/// One artifact, loaded on demand.
pub struct ArtifactView {
    state: gpui::Entity<AppState>,
    run_id: String,
    artifact_id: String,
    kind: ArtifactKind,
    title: String,
    versions: Vec<VersionInfo>,
    /// The version drawn (latest until the user picks another).
    version: Option<u32>,
    load: Load,
    task: Option<Task<()>>,
    cache: Rc<std::cell::RefCell<RenderCache>>,
    scroll: gpui::ScrollHandle,
    table_scroll: gpui::ScrollHandle,
}

impl ArtifactView {
    pub fn new(
        state: gpui::Entity<AppState>,
        run_id: String,
        artifact_id: String,
        kind: ArtifactKind,
        title: String,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            state,
            run_id,
            artifact_id,
            kind,
            title,
            versions: Vec::new(),
            version: None,
            load: Load::Loading,
            task: None,
            cache: Rc::default(),
            scroll: gpui::ScrollHandle::new(),
            table_scroll: gpui::ScrollHandle::new(),
        };
        view.fetch(None, cx);
        view
    }

    pub fn artifact_id(&self) -> &str {
        &self.artifact_id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn kind(&self) -> ArtifactKind {
        self.kind
    }

    /// Tell the view a newer version was published (the run is still going):
    /// follow it when the latest is on screen.
    pub fn refresh_if_newer(&mut self, latest: u32, cx: &mut Context<Self>) {
        let on_latest = self
            .versions
            .iter()
            .map(|v| v.version)
            .max()
            .is_none_or(|m| Some(m) == self.version);
        let known = self.versions.iter().map(|v| v.version).max().unwrap_or(0);
        if latest > known && on_latest && !matches!(self.load, Load::Loading) {
            self.fetch(None, cx);
        }
    }

    fn fetch(&mut self, version: Option<u32>, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.load = Load::Failed("Engine not connected".into());
            return;
        };
        self.load = Load::Loading;
        let (run_id, artifact_id, kind) =
            (self.run_id.clone(), self.artifact_id.clone(), self.kind);
        self.task = Some(cx.spawn(async move |this, cx| {
            let index = engine
                .client()
                .call(
                    zeron_rpc::methods::WORKFLOW_ARTIFACT_DATA,
                    serde_json::json!({ "runId": run_id, "artifactId": artifact_id }),
                )
                .await;
            let read = engine
                .client()
                .call(
                    zeron_rpc::methods::WORKFLOW_ARTIFACT_READ,
                    serde_json::json!({
                        "runId": run_id, "artifactId": artifact_id,
                        "version": version, "offset": 0, "limit": 1_048_576,
                    }),
                )
                .await;
            let parsed = cx
                .background_executor()
                .spawn(async move {
                    let versions: Vec<VersionInfo> = index
                        .ok()
                        .and_then(|v| serde_json::from_value(v.get("versions")?.clone()).ok())
                        .unwrap_or_default();
                    let reply = read.map_err(|e| e.to_string())?;
                    let shown_version = reply
                        .pointer("/version/version")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as u32;
                    let content_type = reply
                        .pointer("/version/contentType")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let title = reply
                        .pointer("/version/title")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let total = reply.get("total").and_then(Value::as_u64).unwrap_or(0);
                    let data = reply.get("data").and_then(Value::as_str).unwrap_or("");
                    let utf8 = reply.get("encoding").and_then(Value::as_str) == Some("utf8");
                    let shown = if utf8 { data.len() as u64 } else { total };
                    let content = decode(kind, &content_type, utf8.then_some(data))?;
                    Ok::<_, String>((versions, shown_version, title, content, total, shown))
                })
                .await;
            this.update(cx, |view, cx| {
                match parsed {
                    Ok((versions, shown_version, title, content, total, shown)) => {
                        view.versions = versions;
                        view.version = Some(shown_version);
                        if let Some(title) = title {
                            view.title = title;
                        }
                        view.load = Load::Ready {
                            content,
                            total,
                            shown,
                        };
                    }
                    Err(err) => view.load = Load::Failed(err),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for ArtifactView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let versions = self.versions.clone();
        let current = self.version;
        let version_bar = (versions.len() > 1).then(|| {
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(4.0))
                .pb(px(8.0))
                .child(
                    div()
                        .text_size(ui_rems(11.5))
                        .text_color(theme.text_faint)
                        .child("Version"),
                )
                .children(versions.iter().map(|v| {
                    let selected = Some(v.version) == current;
                    let number = v.version;
                    let accent = theme.accent;
                    div()
                        .id(SharedString::from(format!("artifact-v{number}")))
                        .role(gpui::Role::Button)
                        .aria_label(SharedString::from(format!("Show version {number}")))
                        .h(px(22.0))
                        .px(px(8.0))
                        .rounded(px(6.0))
                        .flex()
                        .items_center()
                        .text_size(ui_rems(11.5))
                        .cursor_pointer()
                        .tab_index(0)
                        .focus_visible(move |s| s.bg(accent.opacity(0.18)))
                        .when(selected, |el| {
                            el.bg(crate::theme::ink(0.09)).text_color(theme.text)
                        })
                        .when(!selected, |el| {
                            el.text_color(theme.text_muted)
                                .hover(|s| s.bg(crate::theme::ink(0.05)))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.version != Some(number) {
                                this.fetch(Some(number), cx);
                            }
                        }))
                        .child(SharedString::from(format!("v{number}")))
                }))
        });
        let body: AnyElement = match &self.load {
            Load::Loading => div()
                .py(px(24.0))
                .text_size(ui_rems(12.5))
                .text_color(theme.text_faint)
                .child("Loading…")
                .into_any_element(),
            Load::Failed(err) => div()
                .py(px(12.0))
                .flex()
                .flex_col()
                .gap(px(8.0))
                .child(
                    div()
                        .text_size(ui_rems(12.5))
                        .text_color(theme.danger)
                        .child(SharedString::from(format!(
                            "Could not load this artifact: {err}"
                        ))),
                )
                .child(
                    div()
                        .id("artifact-retry")
                        .role(gpui::Role::Button)
                        .aria_label("Try again")
                        .h(px(26.0))
                        .px(px(10.0))
                        .w(px(80.0))
                        .rounded(px(6.0))
                        .border_1()
                        .border_color(crate::theme::hairline(0.14))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(ui_rems(12.0))
                        .text_color(theme.text)
                        .cursor_pointer()
                        .tab_index(0)
                        .hover(|s| s.bg(crate::theme::ink(0.06)))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let v = this.version;
                            this.fetch(v, cx)
                        }))
                        .child("Try again"),
                )
                .into_any_element(),
            Load::Ready {
                content,
                total,
                shown,
            } => {
                let note = (shown < total).then(|| {
                    div()
                        .pt(px(8.0))
                        .text_size(ui_rems(11.5))
                        .text_color(theme.warning)
                        .child(SharedString::from(format!(
                            "Showing the first {} of {}.",
                            format_bytes(*shown),
                            format_bytes(*total)
                        )))
                });
                let content_el = match content {
                    Content::Markdown(tree) => {
                        let opts = RenderOptions {
                            tasks: None,
                            media: None,
                            row_key: SharedString::from(format!(
                                "wf-artifact-{}-{}",
                                self.run_id, self.artifact_id
                            )),
                            veil: None,
                            cache: Some(self.cache.clone()),
                            now: std::time::Instant::now(),
                            copy: None,
                            link: None,
                            workspace_root: None,
                            code: None,
                        };
                        render::render_tree(tree, &opts, &theme, window, &|_| None)
                    }
                    Content::Table { data, pages } => {
                        let pages = *pages;
                        Self::render_table(data, pages, &self.table_scroll, &theme, cx)
                    }
                    Content::Metrics(tiles) => metrics_grid(tiles, &theme),
                    Content::Text { lines, cut } => text_block(lines, *cut, &theme),
                    Content::Binary => div()
                        .py(px(12.0))
                        .text_size(ui_rems(12.5))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(format!(
                            "This is a binary file ({}); it cannot be previewed here.",
                            format_bytes(*total)
                        )))
                        .into_any_element(),
                };
                div()
                    .flex()
                    .flex_col()
                    .child(content_el)
                    .children(note)
                    .into_any_element()
            }
        };
        let meta = {
            let mut bits = vec![kind_word(self.kind).to_owned()];
            if let Some(v) = self
                .versions
                .iter()
                .find(|v| Some(v.version) == self.version)
                && v.bytes > 0
            {
                bits.push(format_bytes(v.bytes));
            }
            bits.join(" · ")
        };
        div()
            .id(SharedString::from(format!("artifact-{}", self.artifact_id)))
            .size_full()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .flex()
            .flex_col()
            .child(
                div()
                    .pb(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        icon(super::widgets::kind_icon(self.kind))
                            .size(px(14.0))
                            .text_color(theme.text_muted),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(ui_rems(14.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(SharedString::from(self.title.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(ui_rems(11.5))
                            .text_color(theme.text_faint)
                            .child(SharedString::from(meta)),
                    ),
            )
            .children(version_bar)
            .child(body)
    }
}

impl ArtifactView {
    fn render_table(
        data: &TableData,
        pages: usize,
        scroll: &gpui::ScrollHandle,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let limit = (pages * TABLE_ROWS_PER_PAGE).min(data.rows.len());
        let total_width: f32 = data.widths.iter().sum();
        let last = data.columns.len().saturating_sub(1);
        // The last column takes the slack, so the table spans the pane when
        // its content is narrower and scrolls sideways when it is wider.
        let column = |c: usize| {
            let col = div().min_w(px(data.widths[c]));
            if c == last {
                col.flex_1()
            } else {
                col.w(px(data.widths[c])).flex_none()
            }
        };
        let header = div()
            .flex()
            .flex_row()
            .w_full()
            .h(px(28.0))
            .items_center()
            .border_b_1()
            .border_color(crate::theme::hairline(0.14))
            .children(data.columns.iter().enumerate().map(|(c, name)| {
                column(c)
                    .px(px(10.0))
                    .truncate()
                    .when(data.numeric[c], |el| el.text_right())
                    .text_size(ui_rems(11.5))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text_muted)
                    .child(SharedString::from(name.clone()))
            }));
        let rows = data.rows.iter().take(limit).enumerate().map(|(r, row)| {
            div()
                .flex()
                .flex_row()
                .w_full()
                .min_h(px(26.0))
                .items_center()
                .when(r % 2 == 1, |el| el.bg(crate::theme::ink(0.025)))
                .children(row.iter().enumerate().map(|(c, cell)| {
                    column(c)
                        .px(px(10.0))
                        .py(px(4.0))
                        .when(data.numeric[c], |el| el.text_right())
                        .text_size(ui_rems(12.0))
                        .line_height(px(17.0))
                        .text_color(theme.text)
                        .child(SharedString::from(cell.clone()))
                }))
        });
        let more = data.rows.len().saturating_sub(limit);
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .id("artifact-table-scroll")
                    .w_full()
                    .overflow_x_scroll()
                    .track_scroll(scroll)
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(crate::theme::hairline(0.1))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .w_full()
                            .min_w(px(total_width))
                            .child(header)
                            .children(rows),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .text_size(ui_rems(11.5))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(format!(
                        "{} {}",
                        data.rows.len(),
                        if data.rows.len() == 1 { "row" } else { "rows" }
                    )))
                    .when(more > 0, |el| {
                        el.child(
                            div()
                                .id("artifact-table-more")
                                .role(gpui::Role::Button)
                                .aria_label("Show more rows")
                                .h(px(22.0))
                                .px(px(8.0))
                                .rounded(px(6.0))
                                .flex()
                                .items_center()
                                .text_color(theme.text_muted)
                                .cursor_pointer()
                                .tab_index(0)
                                .hover(|s| s.bg(crate::theme::ink(0.06)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Load::Ready {
                                        content: Content::Table { pages, .. },
                                        ..
                                    } = &mut this.load
                                    {
                                        *pages += 1;
                                    }
                                    cx.notify();
                                }))
                                .child(SharedString::from(format!(
                                    "Show {} more",
                                    more.min(TABLE_ROWS_PER_PAGE)
                                ))),
                        )
                    }),
            )
            .into_any_element()
    }
}

fn metrics_grid(tiles: &[MetricTile], theme: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(10.0))
        .children(tiles.iter().map(|tile| {
            div()
                .flex_1()
                .flex_basis(px(150.0))
                .min_w(px(130.0))
                .max_w(px(260.0))
                .px(px(14.0))
                .py(px(12.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(crate::theme::hairline(0.1))
                .bg(crate::theme::ink(0.03))
                .flex()
                .flex_col()
                .gap(px(4.0))
                .child(
                    div()
                        .truncate()
                        .text_size(ui_rems(11.5))
                        .text_color(theme.text_muted)
                        .child(SharedString::from(tile.label.clone())),
                )
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(ui_rems(22.0))
                                .line_height(px(28.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(SharedString::from(tile.value.clone())),
                        )
                        .when_some(tile.unit.clone(), |el, unit| {
                            el.child(
                                div()
                                    .text_size(ui_rems(12.0))
                                    .text_color(theme.text_faint)
                                    .child(SharedString::from(unit)),
                            )
                        }),
                )
        }))
        .into_any_element()
}

fn text_block(lines: &[SharedString], cut: bool, theme: &Theme) -> AnyElement {
    div()
        .rounded(px(8.0))
        .border_1()
        .border_color(crate::theme::hairline(0.1))
        .bg(crate::theme::ink(0.03))
        .py(px(8.0))
        .flex()
        .flex_col()
        .font_family(theme.font_mono.clone())
        .text_size(px(theme.code_font_size))
        .text_color(theme.text)
        .children(lines.iter().enumerate().map(|(i, line)| {
            div()
                .flex()
                .flex_row()
                .gap(px(10.0))
                .px(px(10.0))
                .child(
                    div()
                        .flex_none()
                        .w(px(36.0))
                        .text_right()
                        .text_color(theme.text_faint)
                        .child(SharedString::from((i + 1).to_string())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .child(if line.is_empty() {
                            SharedString::from(" ")
                        } else {
                            line.clone()
                        }),
                )
        }))
        .when(cut, |el| {
            el.child(
                div()
                    .px(px(10.0))
                    .pt(px(6.0))
                    .text_color(theme.text_faint)
                    .child(SharedString::from(format!(
                        "… cut at {FILE_MAX_LINES} lines"
                    ))),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_group_thousands_and_trim_fractions() {
        let f = |s: &str| format_value(&serde_json::from_str(s).unwrap());
        assert_eq!(f("1234567"), "1,234,567");
        assert_eq!(f("-98765"), "-98,765");
        assert_eq!(f("1999"), "1999", "short numbers (years, ids) stay whole");
        assert_eq!(f("12.50"), "12.5");
        assert_eq!(f("3.0"), "3");
        assert_eq!(f("1234567.8912"), "1,234,567.8912");
        assert_eq!(f("0.000049"), "0");
        assert_eq!(f("null"), "—");
        assert_eq!(f("true"), "true");
        assert_eq!(f("\"hi\""), "hi");
        assert_eq!(f("[1,2]"), "[1,2]");
    }

    #[test]
    fn a_table_parses_with_numeric_columns_and_widths() {
        let t = parse_table(
            r#"{"columns":["Region","Revenue","Note"],
                "rows":[["EMEA",1200000,"strong"],["APAC",null,"n/a"],["AMER",980000.5,null]]}"#,
        )
        .unwrap();
        assert_eq!(t.columns, ["Region", "Revenue", "Note"]);
        assert_eq!(t.rows[0], ["EMEA", "1,200,000", "strong"]);
        assert_eq!(t.rows[1][1], "—");
        assert_eq!(t.rows[2][1], "980,000.5");
        assert_eq!(t.numeric, [false, true, false]);
        assert!(t.widths.iter().all(|w| (72.0..=320.0).contains(w)));
        // short rows are padded, never panic
        let t = parse_table(r#"{"columns":["a","b"],"rows":[["x"]]}"#).unwrap();
        assert_eq!(t.rows[0], ["x", "—"]);
        assert!(parse_table("{}").is_err());
        assert!(parse_table("not json").is_err());
    }

    #[test]
    fn long_cells_cap_their_column_width() {
        let long = "x".repeat(500);
        let t = parse_table(&format!(r#"{{"columns":["c"],"rows":[["{long}"]]}}"#)).unwrap();
        assert_eq!(t.widths, [320.0]);
    }

    #[test]
    fn metrics_parse_with_optional_units() {
        let m = parse_metrics(
            r#"[{"label":"Files","value":412},{"label":"Coverage","value":87.5,"unit":"%"},{"label":"x","value":"n/a","unit":""}]"#,
        )
        .unwrap();
        assert_eq!(m[0].value, "412");
        assert_eq!(m[0].unit, None);
        assert_eq!(m[1].value, "87.5");
        assert_eq!(m[1].unit.as_deref(), Some("%"));
        assert_eq!(m[2].unit, None, "an empty unit is no unit");
        assert!(parse_metrics("{}").is_err());
    }

    #[test]
    fn decode_picks_the_viewer_for_the_kind() {
        assert!(matches!(
            decode(ArtifactKind::Markdown, "text/markdown", Some("# hi")),
            Ok(Content::Markdown(_))
        ));
        assert!(matches!(
            decode(ArtifactKind::Metrics, "application/json", Some("[]")),
            Ok(Content::Metrics(_))
        ));
        assert!(decode(ArtifactKind::Table, "application/json", Some("oops")).is_err());
        assert!(matches!(
            decode(ArtifactKind::File, "application/octet-stream", Some("zz")),
            Ok(Content::Binary)
        ));
        assert!(matches!(
            decode(ArtifactKind::File, "text/plain", None),
            Ok(Content::Binary)
        ));
        let long = "line\n".repeat(FILE_MAX_LINES + 50);
        match decode(ArtifactKind::File, "text/plain", Some(&long)).unwrap() {
            Content::Text { lines, cut } => {
                assert_eq!(lines.len(), FILE_MAX_LINES);
                assert!(cut);
            }
            _ => panic!("text expected"),
        }
    }
}
