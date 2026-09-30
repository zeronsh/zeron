//! Tool groups, laid out like the desktop transcript (`ui/src/transcript.rs`):
//! a 26pt header (chevron on the rail's trunk, summary title), then one 32pt
//! row per call — rail elbow, tool icon, verb, detail or file badge — each
//! with an inline detail (invocation, output / diff / stats, thought text).
//! Subagent spawns render as their own card group. Desktop metrics scale by
//! [`T`] so 12pt desktop text lands on the phone's 13.5pt small size.

use std::sync::Arc;

use zeron_doc::parts::{MessagePart, SubagentStatus};
use zeron_proto::ToolCall;
use zeron_text::WhiteSpace;

use super::display::{ColorRole, DisplayBuilder, LinkHit, WidgetKind};
use super::file_icons::{basename, file_icon_asset};
use super::markdown::{Ctx, PText, Px, place_text, prepare_plain};
use super::rows::{Content, RowBuilder, RowCore, RowKind, next_version, place_text_lines, row_key};
use super::style::{Family, Weight};

/// Desktop → phone scale (12pt tool text → 13.5pt).
const T: f32 = 1.125;
const HEADER_H: f32 = 26.0;
const TOP_PAD: f32 = 2.0;
const ROW_H: f32 = 32.0;
const TRUNK_X: f32 = 12.5;
const BEND: f32 = 6.0;
const BRANCH_END: f32 = 28.0;
const ICON_LEFT: f32 = 32.0;
const ICON: f32 = 16.0;
const TEXT_X: f32 = 56.0;
const TITLE_X: f32 = 28.0;
const OUT_LH: f32 = 18.0;
const BODY_PAD: f32 = 6.0;
const SEPARATOR: f32 = 1.0;
const MAX_LINES: usize = 24;
const DIFF_MAX_LINES: usize = 600;
const CALL_WRAP_COLS: usize = 80;
const AGENT_ROW: f32 = 38.0;
const AGENT_CARD: f32 = 30.0;

fn d(px: Px, v: f32) -> f32 {
    px.v(v * T)
}

pub(crate) struct ToolLine {
    pub icon: String,
    pub label: PText,
    pub detail: Option<PText>,
    /// File calls show a badge: (file-icon asset, basename, path as called).
    pub badge: Option<(String, PText, String)>,
    pub failed: bool,
    pub running: bool,
    pub key: u64,
    pub open: bool,
    pub body: Vec<DetailBlock>,
    /// A spawned subagent's card opens its transcript: the card's tap link
    /// (`zeron-subagent:{docId}`).
    pub link: Option<String>,
}

pub(crate) struct ToolGroup {
    pub summary: PText,
    pub lines: Vec<ToolLine>,
    pub expanded: bool,
    /// Auto-open live tail (drives the title shimmer).
    pub live: bool,
    /// Subagent spawns: bordered cards, no header, no rail.
    pub agents: bool,
}

pub(crate) enum DetailBlock {
    /// Mono lines (invocation / output), single-line each, scrolling sideways.
    Lines { lines: Vec<PText>, more: Option<PText> },
    /// Thought text, wrapped.
    Prose(PText),
    Stats(Vec<StatRow>),
    Diff { rows: Vec<DiffRowP>, notice: Option<PText>, digits: usize },
}

pub(crate) struct StatRow {
    pub icon: String,
    pub path: PText,
    pub add: PText,
    pub del: PText,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DiffKind {
    Hunk,
    Add,
    Del,
    Ctx,
}

pub(crate) struct DiffRowP {
    pub kind: DiffKind,
    pub old: Option<PText>,
    pub new: Option<PText>,
    pub marker: Option<PText>,
    pub text: PText,
}

// MARK: - Summary / invocation / detail (desktop ports)

/// Desktop summary: thought segment first, then the shared tool summary.
fn group_summary(thoughts: usize, calls: &[(ToolCall, bool)]) -> String {
    let mut segments: Vec<String> = Vec::new();
    match thoughts {
        0 => {}
        1 => segments.push("thought process".into()),
        n => segments.push(format!("thought {n} times")),
    }
    if !calls.is_empty() {
        let s = zeron_proto::view::tool_group_summary(calls);
        let mut c = s.chars();
        let lowered = c.next().map(|f| f.to_lowercase().collect::<String>() + c.as_str()).unwrap_or_default();
        segments.push(lowered);
    }
    let mut out = segments.join(" · ");
    if let Some(first) = out.get(0..1) {
        let up = first.to_uppercase();
        out.replace_range(0..1, &up);
    }
    out
}

fn tool_icon(call: &ToolCall) -> &'static str {
    match call {
        ToolCall::Exec { .. } => "tool-terminal",
        ToolCall::ReadFile { .. } | ToolCall::ApplyPatch { .. } => "tool-document",
        ToolCall::WriteFile { .. } => "tool-document-add",
        ToolCall::EditFile { .. } => "tool-pen",
        ToolCall::Search { .. } => "tool-magnifer",
        ToolCall::Glob { .. } => "tool-folder-with-files",
        ToolCall::WebFetch { .. } | ToolCall::WebSearch { .. } => "tool-global",
        ToolCall::Todo { .. } => "tool-checklist",
        ToolCall::Unknown { name, .. } if name == "Agent" || name.starts_with("Agent: ") || name.eq_ignore_ascii_case("wait for agents") => "tool-bot",
        ToolCall::Mcp { .. } | ToolCall::Unknown { .. } => "tool-widget",
    }
}

fn file_path(call: &ToolCall) -> Option<&str> {
    match call {
        ToolCall::ReadFile { path } | ToolCall::WriteFile { path, .. } | ToolCall::EditFile { path, .. } => Some(path),
        ToolCall::ApplyPatch { path } => path.as_deref(),
        _ => None,
    }
}

fn wrap_cols(line: &str, cols: usize) -> Vec<String> {
    if line.chars().count() <= cols {
        return vec![line.to_owned()];
    }
    line.chars().collect::<Vec<_>>().chunks(cols).map(|c| c.iter().collect()).collect()
}

/// The full invocation the header truncates (desktop `call_block`).
fn call_text(call: &ToolCall) -> String {
    match call {
        ToolCall::Exec { command } => command.clone(),
        ToolCall::ReadFile { path } => path.clone(),
        ToolCall::WriteFile { path, content } => match content {
            Some(c) => format!("{path}\n{c}"),
            None => path.clone(),
        },
        ToolCall::EditFile { path, .. } => path.clone(),
        ToolCall::ApplyPatch { path } => path.clone().unwrap_or_else(|| "workspace".into()),
        ToolCall::Search { pattern, path } => match path {
            Some(p) => format!("{pattern} in {p}"),
            None => pattern.clone(),
        },
        ToolCall::Glob { pattern } => pattern.clone(),
        ToolCall::WebFetch { url, prompt } => match prompt {
            Some(p) => format!("{url}\n{p}"),
            None => url.clone(),
        },
        ToolCall::WebSearch { query } => query.clone(),
        ToolCall::Todo { items } => items
            .iter()
            .map(|i| format!("{} {}", if i.done { "[x]" } else { "[ ]" }, i.text))
            .collect::<Vec<_>>()
            .join("\n"),
        ToolCall::Mcp { server, tool, input } => match input {
            Some(i) => format!("{server} · {tool}\n{}", serde_json::to_string_pretty(i).unwrap_or_default()),
            None => format!("{server} · {tool}"),
        },
        ToolCall::Unknown { name, input } => match input {
            Some(i) => format!("{name}\n{}", serde_json::to_string_pretty(i).unwrap_or_default()),
            None => name.clone(),
        },
    }
}

struct Styles {
    label: super::style::Resolved,
    mono: super::style::Resolved,
    mono_small: super::style::Resolved,
    lh: f32,
    out_lh: f32,
}

fn lines_block(ctx: &mut Ctx, st: &Styles, text: &str, wrap: Option<usize>) -> Option<DetailBlock> {
    let mut lines: Vec<String> = text
        .lines()
        .flat_map(|l| match wrap {
            Some(cols) => wrap_cols(l, cols),
            None => vec![l.to_owned()],
        })
        .collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.is_empty() {
        return None;
    }
    let extra = lines.len().saturating_sub(MAX_LINES);
    lines.truncate(MAX_LINES);
    let lines = lines
        .iter()
        .map(|l| prepare_plain(ctx, l, st.mono, st.out_lh, ColorRole::TextFaint, WhiteSpace::Pre))
        .collect();
    let more = (extra > 0).then(|| prepare_plain(ctx, &format!("… {extra} more lines"), st.label, st.out_lh, ColorRole::TextFaint, WhiteSpace::Pre));
    Some(DetailBlock::Lines { lines, more })
}

fn diff_block(ctx: &mut Ctx, st: &Styles, diff: &zeron_proto::ToolDiff) -> Option<DetailBlock> {
    let old = diff.old_text.as_deref().unwrap_or("");
    // Bounded: a huge rewrite falls back to a coarser diff instead of
    // stalling the layout worker (the transcript would stop updating).
    let text_diff = similar::TextDiff::configure()
        .timeout(std::time::Duration::from_millis(100))
        .diff_lines(old, &diff.new_text);
    let mut raw: Vec<(DiffKind, Option<u32>, Option<u32>, String)> = Vec::new();
    for group in text_diff.grouped_ops(3) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else { continue };
        let o = first.old_range().start..last.old_range().end;
        let n = first.new_range().start..last.new_range().end;
        raw.push((DiffKind::Hunk, None, None, format!("@@ -{},{} +{},{} @@", o.start + 1, o.len(), n.start + 1, n.len())));
        for op in &group {
            for change in text_diff.iter_changes(op) {
                let kind = match change.tag() {
                    similar::ChangeTag::Delete => DiffKind::Del,
                    similar::ChangeTag::Insert => DiffKind::Add,
                    similar::ChangeTag::Equal => DiffKind::Ctx,
                };
                raw.push((
                    kind,
                    change.old_index().map(|i| i as u32 + 1),
                    change.new_index().map(|i| i as u32 + 1),
                    change.value().trim_end_matches('\n').to_owned(),
                ));
            }
        }
    }
    if raw.is_empty() {
        return None;
    }
    let total = raw.iter().filter(|r| r.0 != DiffKind::Hunk).count();
    let mut notice = if diff.old_text.is_none() { Some("New file".to_owned()) } else { None };
    if total > DIFF_MAX_LINES {
        notice = Some(format!("Diff truncated — showing first {DIFF_MAX_LINES} of {total} lines"));
        let mut kept = 0;
        raw.retain(|r| {
            if r.0 != DiffKind::Hunk {
                kept += 1;
            }
            kept <= DIFF_MAX_LINES
        });
    }
    let max_no = raw.iter().map(|r| r.1.unwrap_or(0).max(r.2.unwrap_or(0))).max().unwrap_or(0);
    let digits = max_no.max(1).to_string().len();
    let rows = raw
        .into_iter()
        .map(|(kind, o, n, text)| {
            let num = |ctx: &mut Ctx, v: Option<u32>, color| v.map(|v| prepare_plain(ctx, &v.to_string(), st.mono_small, st.out_lh, color, WhiteSpace::Pre));
            let (gutter_color, marker) = match kind {
                DiffKind::Add => (ColorRole::Success, Some(("+", ColorRole::Success))),
                DiffKind::Del => (ColorRole::Danger, Some(("−", ColorRole::Danger))),
                DiffKind::Ctx => (ColorRole::TextFaint, Some(("·", ColorRole::TextFaint))),
                DiffKind::Hunk => (ColorRole::TextFaint, None),
            };
            DiffRowP {
                kind,
                old: num(ctx, o, gutter_color),
                new: num(ctx, n, gutter_color),
                marker: marker.map(|(m, c)| prepare_plain(ctx, m, st.mono, st.out_lh, c, WhiteSpace::Pre)),
                text: if kind == DiffKind::Hunk {
                    prepare_plain(ctx, &text, st.mono_small, st.out_lh, ColorRole::TextFaint, WhiteSpace::Pre)
                } else {
                    prepare_plain(ctx, &text, st.mono, st.out_lh, ColorRole::Text, WhiteSpace::Pre)
                },
            }
        })
        .collect();
    let notice = notice.map(|n| prepare_plain(ctx, &n, st.label, st.out_lh, ColorRole::TextFaint, WhiteSpace::Pre));
    Some(DetailBlock::Diff { rows, notice, digits })
}

/// Result detail (desktop `tool_detail`): a diff wins, then stats, then output.
fn result_block(ctx: &mut Ctx, st: &Styles, part: &MessagePart) -> Option<DetailBlock> {
    let MessagePart::Tool { output, diff, diff_stats, .. } = part else { return None };
    if let Some(diff) = diff
        && let Some(b) = diff_block(ctx, st, diff)
    {
        return Some(b);
    }
    if let Some(stats) = diff_stats.as_ref().filter(|s| !s.is_empty()) {
        return Some(DetailBlock::Stats(
            stats
                .iter()
                .map(|s| StatRow {
                    icon: file_icon_asset(&s.path),
                    path: prepare_plain(ctx, &s.path, st.label, st.out_lh, ColorRole::TextFaint, WhiteSpace::Pre),
                    add: prepare_plain(ctx, &format!("+{}", s.additions), st.label, st.out_lh, ColorRole::Success, WhiteSpace::Pre),
                    del: prepare_plain(ctx, &format!("−{}", s.deletions), st.label, st.out_lh, ColorRole::Danger, WhiteSpace::Pre),
                })
                .collect(),
        ));
    }
    lines_block(ctx, st, output.as_deref()?, None)
}

// MARK: - Building

impl RowBuilder {
    /// One group of consecutive tool / thought parts (`{msg}#g{n}`).
    pub(crate) fn tool_row(&mut self, ctx: &mut Ctx, entry_id: &str, id: &str, parts: &[&MessagePart], live: bool, agents: bool) -> RowCore {
        let key = row_key(id);
        let calls: Vec<(ToolCall, bool)> = parts
            .iter()
            .filter_map(|p| match p {
                MessagePart::Tool { call, is_error, .. } => Some((call.clone(), *is_error)),
                _ => None,
            })
            .collect();
        let thoughts = parts.iter().filter(|p| matches!(p, MessagePart::Reasoning { .. })).count();
        let expanded = agents
            || if self.expanded.contains(&key) {
                true
            } else if self.collapsed.contains(&key) {
                false
            } else {
                live // Auto-open while it's the live tail, like desktop.
            };
        let size = 12.0 * T;
        let st = Styles {
            label: ctx.typo.style(Family::Sans, Weight::Regular, false, size),
            mono: ctx.typo.style(Family::Mono, Weight::Regular, false, size),
            mono_small: ctx.typo.style(Family::Mono, Weight::Regular, false, 11.0 * T),
            lh: ctx.typo.px(18.0 * T),
            out_lh: ctx.typo.px(OUT_LH * T),
        };
        let medium = ctx.typo.style(Family::Sans, Weight::Medium, false, size);
        let summary = prepare_plain(ctx, &group_summary(thoughts, &calls), st.label, st.lh, ColorRole::TextSecondary, WhiteSpace::Pre);
        let mut lines = Vec::new();
        if expanded {
            let last = parts.len().saturating_sub(1);
            for (i, part) in parts.iter().enumerate() {
                let dkey = row_key(&format!("{id}/{}", part.id()));
                match part {
                    MessagePart::Tool { call, is_error, resolved, subagent_ref, subagent_status, .. } => {
                        let (label, detail) = zeron_proto::view::tool_chip_content(call);
                        // Subagent lifecycle is distinct from `resolved`: under
                        // eager-done the spawn call resolves while the subagent
                        // still runs (desktop transcript.rs `running`/`failed`).
                        let (running, is_error) = if agents {
                            let spawned = subagent_ref.is_some();
                            (
                                spawned && matches!(subagent_status, Some(SubagentStatus::Running)) || !spawned && !*resolved,
                                &(*is_error || spawned && matches!(subagent_status, Some(SubagentStatus::Failed))),
                            )
                        } else {
                            (!*resolved, is_error)
                        };
                        let color = if *is_error { ColorRole::Danger } else { ColorRole::TextSecondary };
                        let badge = if agents {
                            None
                        } else {
                            file_path(call).map(|p| (file_icon_asset(p), prepare_plain(ctx, basename(p), st.label, st.lh, if *is_error { ColorRole::Danger } else { ColorRole::TextSoft }, WhiteSpace::Pre), p.to_owned()))
                        };
                        let detail_color = if agents && !*is_error { ColorRole::TextSoft } else { color };
                        let detail = (badge.is_none() && !detail.is_empty()).then(|| prepare_plain(ctx, &detail, st.label, st.lh, detail_color, WhiteSpace::Pre));
                        let open = !agents && self.detail_open.get(&dkey).copied().unwrap_or(false);
                        let mut body = Vec::new();
                        if open {
                            body.extend(lines_block(ctx, &st, &call_text(call), Some(CALL_WRAP_COLS)));
                            body.extend(result_block(ctx, &st, part));
                        }
                        lines.push(ToolLine {
                            icon: tool_icon(call).to_owned(),
                            label: prepare_plain(ctx, label, if agents { medium } else { st.label }, st.lh, color, WhiteSpace::Pre),
                            detail,
                            badge,
                            failed: *is_error,
                            running,
                            key: dkey,
                            open,
                            body,
                            link: subagent_ref
                                .as_ref()
                                .filter(|_| agents)
                                .map(|doc| format!("{}{doc}", crate::client_ffi::SUBAGENT_LINK_SCHEME)),
                        });
                    }
                    MessagePart::Reasoning { text, .. } => {
                        // A streaming thought opens by default (desktop).
                        let open = self.detail_open.get(&dkey).copied().unwrap_or(live && i == last);
                        let body = if open && !text.trim().is_empty() {
                            vec![DetailBlock::Prose(prepare_plain(ctx, text.trim(), st.label, st.out_lh, ColorRole::TextFaint, WhiteSpace::PreWrap))]
                        } else {
                            Vec::new()
                        };
                        lines.push(ToolLine {
                            icon: "tool-chat-round-line".into(),
                            label: prepare_plain(ctx, "Thought process", st.label, st.lh, ColorRole::TextSecondary, WhiteSpace::Pre),
                            detail: None,
                            badge: None,
                            failed: false,
                            running: false,
                            key: dkey,
                            open,
                            body,
                            link: None,
                        });
                    }
                    _ => {}
                }
            }
        }
        RowCore {
            key,
            version: next_version(),
            kind: RowKind::Tools,
            entry_id: Arc::from(entry_id),
            content: Content::Tools(ToolGroup { summary, lines, expanded, live, agents }),
            copy_text: String::new(),
        }
    }
}

// MARK: - Placement

pub(crate) fn place_tools(t: &ToolGroup, px: Px, x: f32, y: f32, cw: f32, mut out: Option<&mut DisplayBuilder>) -> f32 {
    if t.agents {
        return place_agents(t, px, x, y, cw, out);
    }
    let hh = d(px, HEADER_H);
    if let Some(o) = out.as_deref_mut() {
        let cs = d(px, 14.0);
        o.widget(WidgetKind::Chevron { expanded: t.expanded }, (x + d(px, TRUNK_X) - cs / 2.0, y + (hh - cs) / 2.0, cs, cs), None);
        let tx = x + d(px, TITLE_X);
        let tw = (cw - d(px, TITLE_X) - d(px, 4.0)).max(1.0);
        let sw = t.summary.p.max_content_width().min(tw);
        let ty = y + (hh - t.summary.lh) / 2.0;
        place_text_lines(&t.summary, tx, ty, tw, 1, px, o);
        if t.live {
            o.widget(WidgetKind::Shimmer, (tx, ty, sw, t.summary.lh), None);
        }
        o.widget(WidgetKind::Disclosure { expanded: t.expanded }, (x, y, (d(px, TITLE_X) + sw + d(px, 12.0)).min(cw), hh), None);
    }
    if !t.expanded || t.lines.is_empty() {
        return hh;
    }
    let row_h = d(px, ROW_H);
    let mut ry = y + hh + d(px, TOP_PAD);
    let mut tops = Vec::with_capacity(t.lines.len());
    let mut heights = Vec::with_capacity(t.lines.len());
    let bx = x + d(px, TEXT_X);
    let bw = (cw - d(px, TEXT_X)).max(1.0);
    for line in &t.lines {
        if let Some(o) = out.as_deref_mut() {
            place_line_header(line, px, x, ry, cw, o);
        }
        let body = place_body(line, px, bx, ry + row_h, bw, out.as_deref_mut());
        tops.push(ry - y);
        heights.push(row_h + body);
        ry += row_h + body;
    }
    if let Some(o) = out {
        o.widget(
            WidgetKind::ToolRail {
                trunk_x: d(px, TRUNK_X),
                bend: d(px, BEND),
                branch_end: d(px, BRANCH_END),
                row_mid: row_h / 2.0,
                tops,
                heights,
            },
            (x, y, d(px, TEXT_X), ry - y),
            None,
        );
    }
    ry - y
}

fn place_line_header(line: &ToolLine, px: Px, x: f32, ry: f32, cw: f32, o: &mut DisplayBuilder) {
    let row_h = d(px, ROW_H);
    let is = d(px, ICON);
    let icon_color = if line.failed { ColorRole::Danger } else { ColorRole::TextSecondary };
    o.widget(WidgetKind::Icon { name: line.icon.clone(), color: icon_color }, (x + d(px, ICON_LEFT), ry + (row_h - is) / 2.0, is, is), None);
    let tx = x + d(px, TEXT_X);
    let lw = line.label.p.max_content_width();
    place_text(&line.label, tx, ry + (row_h - line.label.lh) / 2.0, lw + 1.0, Some(o));
    let dx = tx + lw + d(px, 8.0);
    let avail = (x + cw - dx).max(0.0);
    let mut badge_hit = None;
    if let Some((asset, name, path)) = &line.badge {
        let bh = d(px, 22.0);
        let by = ry + (row_h - bh) / 2.0;
        let nw = name.p.max_content_width();
        let bw = (d(px, 1.0 + 20.0 + 6.0 + 6.0) + nw).min(avail);
        if bw > d(px, 34.0) {
            o.fill(dx, by, bw, bh, d(px, 5.0), ColorRole::ToolBadge);
            let well = d(px, 20.0);
            o.fill(dx + d(px, 1.0), by + d(px, 1.0), well, well, d(px, 4.0), ColorRole::ToolWell);
            let fs = d(px, 14.0);
            o.widget(
                WidgetKind::Icon { name: asset.clone(), color: ColorRole::TextSoft },
                (dx + d(px, 1.0) + (well - fs) / 2.0, by + d(px, 1.0) + (well - fs) / 2.0, fs, fs),
                None,
            );
            place_text_lines(name, dx + d(px, 27.0), ry + (row_h - name.lh) / 2.0, (bw - d(px, 33.0)).max(1.0), 1, px, o);
            badge_hit = Some((path.clone(), (dx, by, bw, bh)));
        }
    } else if let Some(detail) = &line.detail
        && avail > 1.0
    {
        place_text_lines(detail, dx, ry + (row_h - detail.lh) / 2.0, avail, 1, px, o);
    }
    o.widget(WidgetKind::ToolToggle { detail: line.key, open: line.open }, (x, ry, cw, row_h), None);
    // Above the row toggle: tapping the file badge opens the file.
    if let Some((path, rect)) = badge_hit {
        o.widget(WidgetKind::OpenFile { path }, rect, None);
    }
}

/// Inline detail under a row: blocks in the text column, each preceded by a
/// (transparent) 1pt separator.
fn place_body(line: &ToolLine, px: Px, bx: f32, by: f32, bw: f32, mut out: Option<&mut DisplayBuilder>) -> f32 {
    if !line.open || line.body.is_empty() {
        return 0.0;
    }
    let mut y = by;
    for block in &line.body {
        y += d(px, SEPARATOR);
        y += place_block(block, px, bx, y, bw, out.as_deref_mut());
    }
    y - by
}

fn place_block(block: &DetailBlock, px: Px, bx: f32, by: f32, bw: f32, out: Option<&mut DisplayBuilder>) -> f32 {
    let pad = d(px, BODY_PAD);
    let lh = d(px, OUT_LH);
    match block {
        DetailBlock::Lines { lines, more } => {
            let n = lines.len() + more.is_some() as usize;
            let h = pad * 2.0 + n as f32 * lh;
            if let Some(o) = out {
                let content = lines.iter().map(|l| l.p.max_content_width()).fold(0.0f32, f32::max) + d(px, 8.0);
                o.begin_scroller(bx, by + pad, bw, lines.len() as f32 * lh, content.max(bw));
                for (i, l) in lines.iter().enumerate() {
                    place_text(l, 0.0, i as f32 * lh + (lh - l.lh) / 2.0, l.p.max_content_width() + 1.0, Some(o));
                }
                o.end_scroller();
                if let Some(m) = more {
                    place_text(m, bx, by + pad + lines.len() as f32 * lh + (lh - m.lh) / 2.0, m.p.max_content_width() + 1.0, Some(o));
                }
            }
            h
        }
        DetailBlock::Prose(p) => {
            let shown = p.p.line_count(bw).min(MAX_LINES);
            if let Some(o) = out {
                place_text_lines(p, bx, by + pad, bw, MAX_LINES, px, o);
            }
            pad * 2.0 + shown as f32 * p.lh
        }
        DetailBlock::Stats(rows) => {
            let h = pad * 2.0 + rows.len() as f32 * lh;
            if let Some(o) = out {
                let fs = d(px, 14.0);
                for (i, r) in rows.iter().enumerate() {
                    let ry = by + pad + i as f32 * lh;
                    o.widget(WidgetKind::Icon { name: r.icon.clone(), color: ColorRole::TextSoft }, (bx, ry + (lh - fs) / 2.0, fs, fs), None);
                    let px0 = bx + fs + d(px, 8.0);
                    let aw = r.add.p.max_content_width();
                    let dw = r.del.p.max_content_width();
                    let tail = aw + dw + d(px, 16.0);
                    let pw = r.path.p.max_content_width().min((bx + bw - px0 - tail).max(1.0));
                    place_text_lines(&r.path, px0, ry + (lh - r.path.lh) / 2.0, pw.max(1.0), 1, px, o);
                    let ax = px0 + pw + d(px, 8.0);
                    place_text(&r.add, ax, ry + (lh - r.add.lh) / 2.0, aw + 1.0, Some(o));
                    place_text(&r.del, ax + aw + d(px, 8.0), ry + (lh - r.del.lh) / 2.0, dw + 1.0, Some(o));
                }
            }
            h
        }
        DetailBlock::Diff { rows, notice, digits } => {
            let notice_h = if notice.is_some() { d(px, 24.0) } else { 0.0 };
            let row_h = |k: DiffKind| if k == DiffKind::Hunk { d(px, 28.0) } else { d(px, 21.0) };
            let body: f32 = rows.iter().map(|r| row_h(r.kind)).sum();
            let h = notice_h + body + d(px, 8.0);
            if let Some(o) = out {
                if let Some(n) = notice {
                    place_text(n, bx, by + (notice_h - n.lh) / 2.0, n.p.max_content_width() + 1.0, Some(o));
                }
                let bar = 3.0;
                let gw = (*digits as f32 * 6.6 + 14.0).max(36.0) * T * px.v(1.0);
                let marker_w = d(px, 28.0);
                let code_x = bar + gw * 2.0 + marker_w + d(px, 12.0);
                let code_w = rows.iter().map(|r| r.text.p.max_content_width()).fold(0.0f32, f32::max);
                let content = (code_x + code_w + d(px, 12.0)).max(bw);
                o.begin_scroller(bx, by + notice_h, bw, body, content);
                let mut ry = 0.0;
                for r in rows {
                    let rh = row_h(r.kind);
                    match r.kind {
                        DiffKind::Hunk => {
                            o.fill(0.0, ry, content, rh, 0.0, ColorRole::DiffHunk);
                            place_text(&r.text, d(px, 16.0), ry + (rh - r.text.lh) / 2.0, r.text.p.max_content_width() + 1.0, Some(o));
                        }
                        kind => {
                            match kind {
                                DiffKind::Add => {
                                    o.fill(0.0, ry, content, rh, 0.0, ColorRole::DiffAddWash);
                                    o.fill(0.0, ry, bar, rh, 0.0, ColorRole::DiffAddBar);
                                }
                                DiffKind::Del => {
                                    o.fill(0.0, ry, content, rh, 0.0, ColorRole::DiffDelWash);
                                    o.fill(0.0, ry, bar, rh, 0.0, ColorRole::DiffDelBar);
                                }
                                _ => {}
                            }
                            for (i, n) in [&r.old, &r.new].into_iter().enumerate() {
                                if let Some(n) = n {
                                    let w = n.p.max_content_width();
                                    place_text(n, bar + gw * (i as f32 + 1.0) - d(px, 8.0) - w, ry + (rh - n.lh) / 2.0, w + 1.0, Some(o));
                                }
                            }
                            if let Some(m) = &r.marker {
                                let w = m.p.max_content_width();
                                place_text(m, bar + gw * 2.0 + (marker_w - w) / 2.0, ry + (rh - m.lh) / 2.0, w + 1.0, Some(o));
                            }
                            place_text(&r.text, code_x, ry + (rh - r.text.lh) / 2.0, r.text.p.max_content_width() + 1.0, Some(o));
                        }
                    }
                    ry += rh;
                }
                o.end_scroller();
            }
            h
        }
    }
}

/// Subagent spawns: 38pt rows holding a bordered 30pt card — icon tile,
/// "Agent", the description, a mini spinner while it runs.
fn place_agents(t: &ToolGroup, px: Px, x: f32, y: f32, cw: f32, mut out: Option<&mut DisplayBuilder>) -> f32 {
    let row = d(px, AGENT_ROW);
    let h = row * t.lines.len() as f32;
    let Some(o) = out.as_mut() else { return h };
    for (i, line) in t.lines.iter().enumerate() {
        let ry = y + i as f32 * row;
        let card_y = ry + (row - d(px, AGENT_CARD)) / 2.0;
        let card_h = d(px, AGENT_CARD);
        o.fill(x, card_y, cw, card_h, d(px, 9.0), ColorRole::AgentCard);
        if let Some(url) = &line.link {
            o.links.push(LinkHit { x, y: card_y, w: cw, h: card_h, url: url.clone(), scroller: o.scroller });
        }
        o.hairline(x, card_y, cw, card_h, d(px, 9.0), ColorRole::AgentCardBorder);
        let tile = d(px, 18.0);
        let tx = x + d(px, 8.0);
        let ty = card_y + (card_h - tile) / 2.0;
        o.fill(tx, ty, tile, tile, d(px, 5.0), ColorRole::AgentTile);
        let is = d(px, 12.0);
        o.widget(WidgetKind::Icon { name: line.icon.clone(), color: ColorRole::TextSecondary }, (tx + (tile - is) / 2.0, ty + (tile - is) / 2.0, is, is), None);
        let lx = tx + tile + d(px, 8.0);
        let lw = line.label.p.max_content_width();
        place_text(&line.label, lx, card_y + (card_h - line.label.lh) / 2.0, lw + 1.0, Some(o));
        let spin = if line.running { d(px, 14.0) + d(px, 8.0) } else { 0.0 };
        if let Some(detail) = &line.detail {
            let dx = lx + lw + d(px, 8.0);
            let avail = x + cw - d(px, 8.0) - spin - dx;
            if avail > 1.0 {
                place_text_lines(detail, dx, card_y + (card_h - detail.lh) / 2.0, avail, 1, px, o);
            }
        }
        if line.running {
            let s = d(px, 14.0);
            o.widget(WidgetKind::Spinner, (x + cw - d(px, 8.0) - s, card_y + (card_h - s) / 2.0, s, s), None);
        }
    }
    h
}

pub(crate) fn heap_bytes(t: &ToolGroup) -> usize {
    t.summary.p.heap_bytes()
        + t.lines
            .iter()
            .map(|l| {
                l.label.p.heap_bytes()
                    + l.detail.as_ref().map_or(0, |d| d.p.heap_bytes())
                    + l.badge.as_ref().map_or(0, |b| b.1.p.heap_bytes() + b.2.capacity())
                    + l.link.as_ref().map_or(0, String::capacity)
                    + l.body
                        .iter()
                        .map(|b| match b {
                            DetailBlock::Lines { lines, .. } => lines.iter().map(|p| p.p.heap_bytes()).sum(),
                            DetailBlock::Prose(p) => p.p.heap_bytes(),
                            DetailBlock::Stats(rows) => rows.iter().map(|r| r.path.p.heap_bytes()).sum(),
                            DetailBlock::Diff { rows, .. } => rows.iter().map(|r| r.text.p.heap_bytes()).sum(),
                        })
                        .sum::<usize>()
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_matches_desktop_wording() {
        let calls = vec![
            (ToolCall::Exec { command: "ls".into() }, false),
            (ToolCall::ReadFile { path: "a.rs".into() }, false),
            (ToolCall::Exec { command: "x".into() }, true),
        ];
        assert_eq!(group_summary(0, &calls), "Ran 2 commands · read 1 file · 1 failed");
        assert_eq!(group_summary(1, &calls), "Thought process · ran 2 commands · read 1 file · 1 failed");
        assert_eq!(group_summary(3, &[]), "Thought 3 times");
    }
}
