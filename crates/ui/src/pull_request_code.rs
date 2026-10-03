//! The Code tab: a file tree beside one continuous review stream. Every file
//! keeps its place in the stream under a foldable header (the Changes pane's
//! convention), the current file's header stays pinned while its body
//! scrolls, and the tree follows the scroll position.
use super::*;
use std::ops::Range;

const FILES_WIDTH: f32 = 260.0;
const WORKSPACE_GAP: f32 = 16.0;
const WIDE_MIN: f32 = 760.0;
const TREE_ROW_HEIGHT: f32 = 28.0;
const TREE_INDENT: f32 = 14.0;
const PICKER_WIDTH: f32 = 320.0;
const PICKER_HEIGHT: f32 = 360.0;
/// Frosted pinned header: blur and veil strong enough to erase the text
/// scrolling under it, light enough to keep the header rows' tone.
const STICKY_BLUR: f32 = 32.0;
const STICKY_VEIL: f32 = 0.18;
/// The stream card's 12px radius inside its 1px border.
const CARD_INNER_RADIUS: f32 = 11.0;

/// One entry of the virtualized review stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum StreamRow {
    /// A file's header (fold toggle, path, counts).
    File(usize),
    /// A unified notice, hunk header, or line: an index into the patch rows.
    Row(usize),
    /// A split row: a file and an index into its cached pairs.
    Pair(usize, usize),
}

/// Flatten every file into stream rows plus each file's span. A folded
/// file contributes only its header.
pub(super) fn stream(
    diff: &ParsedDiff,
    split: bool,
    collapsed: &std::collections::HashSet<usize>,
) -> (Vec<StreamRow>, Vec<Range<usize>>) {
    let mut rows = Vec::with_capacity(diff.rows.len());
    let mut ranges = Vec::with_capacity(diff.files.len());
    for (file, (_, start)) in diff.files.iter().enumerate() {
        let first = rows.len();
        rows.push(StreamRow::File(file));
        if !collapsed.contains(&file) {
            if split {
                let pairs = diff.pairs.get(file).map_or(0, Vec::len);
                rows.extend((0..pairs).map(|pair| StreamRow::Pair(file, pair)));
            } else {
                let end = diff
                    .files
                    .get(file + 1)
                    .map_or(diff.rows.len(), |(_, end)| *end);
                rows.extend((start + 1..end).map(StreamRow::Row));
            }
        }
        ranges.push(first..rows.len());
    }
    (rows, ranges)
}

/// Height of the divider above a file header; the first file has none.
fn header_divider(file: usize) -> f32 {
    if file > 0 { 1.0 } else { 0.0 }
}

/// The file whose span holds stream row `row`.
fn file_at(ranges: &[Range<usize>], row: usize) -> Option<usize> {
    ranges
        .partition_point(|range| range.start <= row)
        .checked_sub(1)
}

pub(super) fn code_gutter(rows: &[CodeRow]) -> f32 {
    let digits = rows
        .iter()
        .flat_map(|row| [row.old.len(), row.new.len()])
        .max()
        .unwrap_or(1);
    (digits as f32 * 6.6 + 14.0).max(crate::changes::GUTTER_WIDTH)
}

fn diff_line(row: &CodeRow) -> crate::changes::DiffLine {
    crate::changes::DiffLine {
        kind: row.kind,
        old_no: row.old.parse().ok(),
        new_no: row.new.parse().ok(),
        text: row.text.to_string(),
    }
}

/// Cache index pairs once per patch, using the Changes pane's hunk pairing.
pub(super) fn split_files(
    rows: &[CodeRow],
    files: &[(String, usize)],
) -> Vec<Vec<crate::changes::LinePair>> {
    files
        .iter()
        .enumerate()
        .map(|(file, (_, start))| {
            let end = files.get(file + 1).map_or(rows.len(), |(_, start)| *start);
            let mut cursor = start + 1; // The path is the file's header row.
            let mut pairs = Vec::new();
            while cursor < end {
                if rows[cursor].role != RowRole::Line {
                    pairs.push((Some(cursor as u32), Some(cursor as u32)));
                    cursor += 1;
                    continue;
                }
                let first = cursor;
                while cursor < end && rows[cursor].role == RowRole::Line {
                    cursor += 1;
                }
                let lines = rows[first..cursor]
                    .iter()
                    .map(diff_line)
                    .collect::<Vec<_>>();
                pairs.extend(crate::changes::split_pairs(&lines).into_iter().map(
                    |(left, right)| {
                        (
                            left.map(|i| i + first as u32),
                            right.map(|i| i + first as u32),
                        )
                    },
                ));
            }
            pairs
        })
        .collect()
}

/// Directory rows and file rows of the tree, in patch order. A directory
/// row names the whole path segment it stands for, so single-child chains
/// (`crates/ui/src`) stay one row, as in GitHub's file tree.
#[derive(Debug, PartialEq, Eq)]
enum TreeEntry<'a> {
    Directory {
        path: &'a str,
        depth: usize,
    },
    File {
        index: usize,
        name: &'a str,
        depth: usize,
    },
}

fn tree<'a>(files: &'a [(String, usize)], query: &str) -> Vec<TreeEntry<'a>> {
    let mut entries = Vec::new();
    let mut current: Option<&str> = None;
    for (index, (path, _)) in files.iter().enumerate() {
        if !query.is_empty() && !path.to_lowercase().contains(query) {
            continue;
        }
        let (directory, name) = match path.rsplit_once('/') {
            Some((directory, name)) => (Some(directory), name),
            None => (None, path.as_str()),
        };
        if directory != current {
            if let Some(path) = directory {
                entries.push(TreeEntry::Directory { path, depth: 0 });
            }
            current = directory;
        }
        entries.push(TreeEntry::File {
            index,
            name,
            depth: usize::from(directory.is_some()),
        });
    }
    entries
}

fn counts(additions: u64, deletions: u64, size: f32, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .gap(px(6.0))
        .font_family(theme.font_mono.clone())
        .text_size(px(size))
        .when(additions > 0, |el| {
            el.child(
                div()
                    .text_color(crate::changes::add_color(theme))
                    .child(format!("+{additions}")),
            )
        })
        .when(deletions > 0, |el| {
            el.child(
                div()
                    .text_color(crate::changes::del_color(theme))
                    .child(format!("−{deletions}")),
            )
        })
}

impl PullRequestDetailPage {
    /// Rebuild the stream after the patch, layout, or folds change, keeping
    /// the file at the top of the viewport in place.
    pub(super) fn rebuild_stream(&mut self, cx: &mut Context<Self>) {
        let Some(diff) = self.diff_snapshot() else {
            return;
        };
        let anchor = self.active_file();
        let (rows, ranges) = stream(&diff, self.code_split, &self.collapsed_files);
        self.code_list.reset(rows.len());
        self.code_stream = Rc::new(rows);
        self.code_ranges = ranges;
        self.code_horizontal.set_offset(gpui::Point::default());
        if let Some(file) = anchor.filter(|file| *file > 0) {
            self.scroll_to_file(file);
        }
        cx.notify();
    }

    /// The file under the top of the viewport, or the one just jumped to
    /// (the last files may be too short to reach the top).
    pub(super) fn active_file(&self) -> Option<usize> {
        if self.code_ranges.is_empty() {
            return None;
        }
        self.jumped_file
            .or_else(|| {
                file_at(
                    &self.code_ranges,
                    self.code_list.logical_scroll_top().item_ix,
                )
            })
            .map(|file| file.min(self.code_ranges.len() - 1))
    }

    fn scroll_to_file(&self, file: usize) {
        if let Some(range) = self.code_ranges.get(file) {
            // Land just below the divider every header after the first
            // carries, so it never doubles the stream card's own border.
            self.code_list.scroll_to(gpui::ListOffset {
                item_ix: range.start,
                offset_in_item: px(header_divider(file)),
            });
        }
    }

    /// Arrow keys in the file tree: the previous or next file it shows.
    fn step_tree(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let files: Vec<usize> = tree(&self.code_files, &self.file_query)
            .into_iter()
            .filter_map(|entry| match entry {
                TreeEntry::File { index, .. } => Some(index),
                TreeEntry::Directory { .. } => None,
            })
            .collect();
        let current = self
            .active_file()
            .and_then(|file| files.iter().position(|index| *index == file));
        let next = match (key, current) {
            ("down", Some(at)) => (at + 1).min(files.len().saturating_sub(1)),
            ("up", Some(at)) => at.saturating_sub(1),
            ("down" | "up" | "home", None) | ("home", _) => 0,
            ("end", _) => files.len().saturating_sub(1),
            _ => return false,
        };
        if let Some(index) = files.get(next) {
            self.select_code_file(*index, cx);
        }
        true
    }

    pub(super) fn select_code_file(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.code_ranges.len() {
            return;
        }
        self.scroll_to_file(index);
        self.jumped_file = Some(index);
        cx.notify();
    }

    fn toggle_fold(&mut self, file: usize, cx: &mut Context<Self>) {
        if !self.collapsed_files.remove(&file) {
            self.collapsed_files.insert(file);
        }
        // Keep the toggled header where the reader is looking.
        let anchor = self.active_file();
        self.jumped_file = None;
        self.rebuild_stream(cx);
        if anchor == Some(file) {
            self.scroll_to_file(file);
        }
    }

    fn toggle_all_folds(&mut self, cx: &mut Context<Self>) {
        if self.collapsed_files.len() == self.code_ranges.len() {
            self.collapsed_files.clear();
        } else {
            self.collapsed_files = (0..self.code_ranges.len()).collect();
        }
        self.jumped_file = None;
        self.rebuild_stream(cx);
    }

    pub(super) fn toggle_split(&mut self, cx: &mut Context<Self>) {
        self.code_split = !self.code_split;
        let split = self.code_split;
        crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
            settings.diff_split = split;
        });
        self.rebuild_stream(cx);
    }

    /// A stream row. The list owns virtualization; rows are pure functions
    /// of the cached patch.
    pub(super) fn render_code_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).clone();
        let Some(row) = self.code_stream.get(index).copied() else {
            return div().into_any_element();
        };
        let text_width = self.code_text_width(&theme, window);
        let gutter = self.code_gutter;
        let rows = self.code_rows.clone();
        let meta = |row: &CodeRow, theme: &Theme| match row.role {
            RowRole::Hunk => Some(crate::changes::hunk_header_row(&row.text, theme)),
            RowRole::Notice => Some(crate::changes::notice_row(row.text.to_string(), theme)),
            RowRole::Line if row.kind == crate::changes::LineKind::Meta => Some(
                crate::changes::readonly_diff_line(&diff_line(row), &[], theme, gutter),
            ),
            _ => None,
        };
        match row {
            StreamRow::File(file) => self.file_header(file, false, &theme, cx),
            StreamRow::Row(ix) => {
                let row = &rows[ix];
                meta(row, &theme).unwrap_or_else(|| {
                    crate::changes::readonly_scrolled_diff_line(
                        &diff_line(row),
                        &row.spans,
                        &theme,
                        gutter,
                        text_width,
                        &self.code_horizontal,
                        index,
                    )
                })
            }
            StreamRow::Pair(file, pair) => {
                let (left, right) = self.code_pairs[file][pair];
                let first = &rows[left.or(right).unwrap_or_default() as usize];
                meta(first, &theme).unwrap_or_else(|| {
                    let left_row = left.map(|i| &rows[i as usize]);
                    let right_row = right.map(|i| &rows[i as usize]);
                    let left_line = left_row.map(diff_line);
                    let right_line = right_row.map(diff_line);
                    crate::changes::readonly_split_line(
                        left_line
                            .as_ref()
                            .zip(left_row)
                            .map(|(line, row)| (line, row.spans.as_slice())),
                        right_line
                            .as_ref()
                            .zip(right_row)
                            .map(|(line, row)| (line, row.spans.as_slice())),
                        &theme,
                        gutter,
                        text_width,
                        &self.code_horizontal,
                        index,
                    )
                })
            }
        }
    }

    /// The shared horizontal extent of every code plane in the stream.
    fn code_text_width(&self, theme: &Theme, window: &Window) -> f32 {
        let mono = gpui::font(theme.font_mono.clone());
        let size = px(crate::changes::diff_text_size(theme));
        let text = window.text_system();
        let advance = text
            .ch_advance(text.resolve_font(&mono), size)
            .map(|advance| advance.as_f32())
            .unwrap_or(size.as_f32() * 0.6);
        // Slack covers ligatures and wide glyphs the column count misses.
        self.code_columns as f32 * advance * 1.05
    }

    fn file_header(
        &self,
        file: usize,
        sticky: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let path = self
            .code_files
            .get(file)
            .map(|(path, _)| path.clone())
            .unwrap_or_default();
        let (directory, name) = match path.rsplit_once('/') {
            Some((directory, name)) => (format!("{directory}/"), name.to_owned()),
            None => (String::new(), path.clone()),
        };
        let collapsed = self.collapsed_files.contains(&file);
        let (additions, deletions) = self.code_stats.get(file).copied().unwrap_or_default();
        let paint = crate::changes::sticky_file_header_paint(theme);
        // Frosted: the pinned copy blurs what scrolls beneath it and keeps the
        // rows' own translucent wash, so both read as one header. Opaque
        // surfaces need the solid equivalent to hide the rows.
        let (rest, hover) = if sticky && !theme.is_frost() {
            (paint.rest_bg, paint.hover_bg)
        } else {
            (theme.ink(0.025), theme.ink(0.05))
        };
        let prefix = if sticky {
            "pr-sticky-file"
        } else {
            "pr-file-header"
        };
        let copy_path = path.clone();
        div()
            .id(SharedString::from(format!("{prefix}-{file}")))
            .debug_selector(move || format!("{prefix}-{file}"))
            .role(gpui::Role::Button)
            .aria_label(format!(
                "{path}, {} ",
                if collapsed { "collapsed" } else { "expanded" }
            ))
            .aria_expanded(!collapsed)
            .tab_index(0)
            .w_full()
            .h(px(crate::changes::FILE_HEADER_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(Theme::SPACE_MD))
            .bg(rest)
            .when(!sticky && file > 0, |el| {
                el.border_t_1().border_color(theme.border)
            })
            .when(sticky || collapsed, |el| {
                el.border_b_1()
                    .border_color(paint.border.opacity(if sticky { 1.0 } else { 0.0 }))
            })
            .when(sticky, |el| el.block_mouse_except_scroll())
            // Follow the stream card's inner corner radius at the top edge.
            .when(sticky || file == 0, |el| {
                el.rounded_t(px(CARD_INNER_RADIUS))
            })
            .cursor_pointer()
            .hover(move |style| style.bg(hover))
            .focus_visible(|style| style.border_2().border_color(theme.accent))
            .on_click(cx.listener(move |page, _, _, cx| page.toggle_fold(file, cx)))
            .child(
                crate::icons::icon(if collapsed {
                    crate::icons::ALT_ARROW_RIGHT
                } else {
                    crate::icons::ALT_ARROW_DOWN
                })
                .size(px(13.0))
                .flex_none()
                .text_color(theme.text_muted.opacity(0.7)),
            )
            .child(
                crate::file_icons::icon(
                    crate::file_icons::FileIconIdentity::file(&path),
                    theme.appearance,
                )
                .size(px(14.0))
                .flex_none(),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .overflow_hidden()
                    .font_family(theme.font_mono.clone())
                    .text_size(px(12.0))
                    .whitespace_nowrap()
                    .child(
                        div()
                            .flex_shrink(1.0)
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text_muted)
                            .child(directory),
                    )
                    .child(div().flex_none().text_color(theme.text).child(name)),
            )
            .child(counts(additions, deletions, 11.0, theme))
            .child(
                action("pr-copy-path", "Copy path", theme).on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy_path.clone()));
                }),
            )
            .into_any_element()
    }

    /// The current file's header, pinned over the stream while its body
    /// scrolls and pushed up by the next file's header.
    fn sticky_header(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        let top = self.code_list.logical_scroll_top();
        let file = file_at(&self.code_ranges, top.item_ix)?;
        let range = self.code_ranges.get(file)?;
        if !range.contains(&top.item_ix)
            || (top.item_ix == range.start && top.offset_in_item <= px(header_divider(file)))
        {
            return None;
        }
        let next = self.code_ranges.get(file + 1).and_then(|next| {
            let bounds = self.code_list.bounds_for_item(next.start)?;
            Some((bounds.origin.y - self.code_list.viewport_bounds().origin.y).as_f32())
        });
        let header = self.file_header(file, true, theme, cx);
        // A faint veil of the content plane melts the blurred glyphs beneath
        // without lifting the header above the rows' own tone.
        let header = if theme.is_frost() {
            div()
                .w_full()
                .rounded_t(px(CARD_INNER_RADIUS))
                .bg(theme.bg.opacity(STICKY_VEIL))
                .child(header)
                .into_any_element()
        } else {
            header
        };
        Some(
            div()
                .absolute()
                .top(px(crate::changes::sticky_header_push_offset(next)))
                .left_0()
                .w_full()
                .child(crate::frost::frosted(
                    CARD_INNER_RADIUS,
                    STICKY_BLUR,
                    header,
                ))
                .into_any_element(),
        )
    }

    fn file_tree(
        &self,
        active: Option<usize>,
        wide: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let entries = tree(&self.code_files, &self.file_query);
        let mut list = div()
            .id("pr-file-list")
            .debug_selector(|| "pr-file-list".into())
            .role(gpui::Role::Tree)
            .aria_label("Changed files")
            .track_focus(&self.tree_focus)
            .rounded(px(6.0))
            // A resting transparent ring, so focusing never shifts the rows.
            .border_2()
            .border_color(gpui::transparent_black())
            .focus_visible(|style| style.border_color(theme.accent))
            .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, _, cx| {
                if page.step_tree(&event.keystroke.key, cx) {
                    cx.stop_propagation();
                }
            }))
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.file_tree_scroll)
            .flex()
            .flex_col()
            .gap(px(1.0))
            .pt(px(4.0))
            .pb(px(if wide { NAV_CLEARANCE } else { 8.0 }));
        let loading = self.diff.is_none();
        if loading {
            list = list.child(crate::pull_request_skeleton::tree(
                cx.entity_id(),
                theme,
                cx,
            ));
        } else if entries.is_empty() {
            list = list.child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .child("No matching files"),
            );
        }
        for entry in entries {
            list = list.child(match entry {
                TreeEntry::Directory { path, depth } => div()
                    .h(px(TREE_ROW_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .pl(px(8.0 + depth as f32 * TREE_INDENT))
                    .pr(px(8.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .child(
                        crate::file_icons::icon(
                            crate::file_icons::FileIconIdentity::directory(path, true),
                            theme.appearance,
                        )
                        .size(px(14.0))
                        .flex_none(),
                    )
                    .child(div().min_w_0().truncate().child(path.to_owned()))
                    .into_any_element(),
                TreeEntry::File { index, name, depth } => {
                    let selected = active == Some(index);
                    let (additions, deletions) =
                        self.code_stats.get(index).copied().unwrap_or_default();
                    let path = &self.code_files[index].0;
                    div()
                        .id(SharedString::from(format!("pr-file-{index}")))
                        .debug_selector(move || format!("pr-file-{index}"))
                        .role(gpui::Role::TreeItem)
                        .aria_label(format!("Open diff for {path}"))
                        .aria_selected(selected)
                        .h(px(TREE_ROW_HEIGHT))
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .pl(px(8.0 + depth as f32 * TREE_INDENT))
                        .pr(px(8.0))
                        .rounded(px(6.0))
                        .text_size(px(12.0))
                        .text_color(if selected { theme.text } else { theme.text_dim })
                        .when(selected, |el| el.bg(theme.glass_hover()))
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.glass_hover()))
                        .child(
                            crate::file_icons::icon(
                                crate::file_icons::FileIconIdentity::file(path),
                                theme.appearance,
                            )
                            .size(px(14.0))
                            .flex_none(),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(name.to_owned()))
                        .child(counts(additions, deletions, 10.0, theme))
                        .on_click(cx.listener(move |page, _, window, cx| {
                            page.select_code_file(index, cx);
                            if !wide {
                                page.files_expanded = false;
                                window.focus(&page.files_focus, cx);
                            }
                        }))
                        .into_any_element()
                }
            });
        }
        // The PR detail already knows the totals while the patch loads.
        let (file_count, additions, deletions) = match (&self.detail, loading) {
            (Some(detail), true) => (detail.files.len(), detail.additions, detail.deletions),
            _ => self.code_stats.iter().fold(
                (self.code_files.len(), 0, 0),
                |(files, a, d), (add, del)| (files, a + add, d + del),
            ),
        };
        div()
            .id("pr-file-browser")
            .debug_selector(|| "pr-file-browser".into())
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .h(px(crate::surface_chrome::CONTROL_SIZE))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .px(px(8.0))
                    .text_size(px(12.0))
                    .child(div().font_weight(gpui::FontWeight::MEDIUM).child("Files"))
                    .child(
                        div()
                            .flex_1()
                            .text_color(theme.text_muted)
                            .child(file_count.to_string()),
                    )
                    .child(counts(additions, deletions, 11.0, theme)),
            )
            .child(
                // The shared input grows along its parent axis; this parent is a column.
                crate::surface_chrome::input()
                    .flex_none()
                    .id("pr-file-search")
                    .debug_selector(|| "pr-file-search".into())
                    .child(div().flex_1().min_w_0().child(self.file_search.clone())),
            )
            .child(
                div().flex_1().min_h_0().relative().child(
                    crate::edge_fade::edge_faded(16.0, true, true, list)
                        .fade_overflow_y(&self.file_tree_scroll),
                ),
            )
            .into_any_element()
    }

    pub(super) fn code_workspace(
        &self,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = window;
        let wide = self.code_pane_width.is_some_and(|width| width >= WIDE_MIN);
        let loading = self.diff.is_none();
        let active = self.active_file();
        let file_count = self.code_files.len();
        let all_folded = file_count > 0 && self.collapsed_files.len() == file_count;
        let patch = self.diff.clone().unwrap_or_default();
        let measure = {
            let page = cx.weak_entity();
            gpui::canvas(
                move |bounds, window, cx| {
                    let width = f32::from(bounds.size.width);
                    window.defer(cx, move |_, cx| {
                        let _ = page.update(cx, |page, cx| {
                            if page
                                .code_pane_width
                                .is_none_or(|old| (old - width).abs() > 0.5)
                            {
                                page.code_pane_width = Some(width);
                                cx.notify();
                            }
                        });
                    });
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0()
        };
        let position = active.map_or(0, |file| file + 1);
        let toolbar = div()
            .id("pr-file-navigation")
            .debug_selector(|| "pr-file-navigation".into())
            .h(px(crate::surface_chrome::CONTROL_SIZE))
            .flex_none()
            .min_w_0()
            .flex()
            .items_center()
            .gap(px(4.0))
            .when(!wide, |el| {
                el.child(
                    action("pr-files", "Choose a changed file", theme)
                        .track_focus(&self.files_focus)
                        .aria_expanded(self.files_expanded)
                        .when(self.files_expanded, |el| el.bg(theme.glass_hover()))
                        .on_click(cx.listener(move |page, _, window, cx| {
                            page.toggle_files(cx);
                            if page.files_expanded {
                                window.focus(&page.file_search.read(cx).focus_handle(cx), cx);
                            }
                        })),
                )
            })
            .child(
                action("pr-previous-file", "Previous file", theme)
                    .when(position <= 1, |el| el.opacity(0.4))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        if let Some(file) = page.active_file().filter(|file| *file > 0) {
                            page.select_code_file(file - 1, cx);
                        }
                    })),
            )
            .child(
                action("pr-next-file", "Next file", theme)
                    .when(position >= file_count, |el| el.opacity(0.4))
                    .on_click(cx.listener(move |page, _, _, cx| {
                        let next = page.active_file().map_or(0, |file| file + 1);
                        page.select_code_file(next, cx);
                    })),
            )
            .child(
                div()
                    .id("pr-file-position")
                    .debug_selector(|| "pr-file-position".into())
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .px(px(4.0))
                    .text_size(px(12.0))
                    .text_color(theme.text_muted)
                    .child(if loading {
                        "Loading changes…".to_owned()
                    } else if file_count == 0 {
                        "No changed files".to_owned()
                    } else {
                        format!("File {position} of {file_count}")
                    }),
            )
            .child(
                action(
                    "pr-fold-all",
                    if all_folded {
                        "Expand all files"
                    } else {
                        "Collapse all files"
                    },
                    theme,
                )
                .on_click(cx.listener(|page, _, _, cx| page.toggle_all_folds(cx))),
            )
            .child(
                action(
                    "pr-split",
                    if self.code_split {
                        "Show unified diff"
                    } else {
                        "Show split diff"
                    },
                    theme,
                )
                .aria_selected(self.code_split)
                .when(self.code_split, |el| el.bg(theme.glass_hover()))
                .on_click(cx.listener(|page, _, _, cx| page.toggle_split(cx))),
            )
            .child(
                action("pr-copy-patch", "Copy diff", theme).on_click(move |_, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(patch.as_ref().clone()));
                }),
            );
        let viewport = div()
            .id("pr-code-viewport")
            .debug_selector(|| "pr-code-viewport".into())
            .relative()
            .flex_1()
            .min_h_0()
            .rounded(px(12.0))
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .map(|el| {
                if loading {
                    el.child(crate::pull_request_skeleton::diff(
                        cx.entity_id(),
                        theme,
                        cx,
                    ))
                } else {
                    el.child(
                        gpui::list(self.code_list.clone(), cx.processor(Self::render_code_row))
                            .size_full()
                            .with_sizing_behavior(gpui::ListSizingBehavior::Auto),
                    )
                    .children(self.sticky_header(theme, cx))
                }
            });
        let editor = div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .pb(px(NAV_CLEARANCE))
            .child(toolbar)
            .child(viewport)
            .when(!wide && self.files_expanded, |el| {
                el.child(
                    // Dismiss layer, then the picker anchored under the toolbar.
                    div()
                        .id("pr-file-picker-dismiss")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_mouse_down(
                            gpui::MouseButton::Left,
                            cx.listener(|page, _, _, cx| {
                                page.files_expanded = false;
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .id("pr-file-picker")
                        .debug_selector(|| "pr-file-picker".into())
                        .absolute()
                        .top(px(crate::surface_chrome::CONTROL_SIZE + 4.0))
                        .left_0()
                        .w(px(PICKER_WIDTH))
                        .max_w_full()
                        .h(px(PICKER_HEIGHT))
                        .max_h(gpui::relative(0.9))
                        .p(px(8.0))
                        .rounded(px(12.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.surface_raised)
                        .shadow_lg()
                        .occlude()
                        .on_key_down(cx.listener(|page, event: &gpui::KeyDownEvent, window, cx| {
                            if event.keystroke.key == "escape" {
                                page.files_expanded = false;
                                window.focus(&page.files_focus, cx);
                                cx.stop_propagation();
                                cx.notify();
                            }
                        }))
                        .child(self.file_tree(active, false, theme, cx)),
                )
            });
        div()
            .id("pr-code-workspace")
            .debug_selector(|| "pr-code-workspace".into())
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .gap(px(WORKSPACE_GAP))
            .child(measure)
            .when(wide, |el| {
                el.child(
                    div()
                        .w(px(FILES_WIDTH))
                        .flex_none()
                        .h_full()
                        .min_h_0()
                        .child(self.file_tree(active, true, theme, cx)),
                )
            })
            .child(editor)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pull_request_split_pairs_preserve_hunks_files_and_missing_newlines() {
        let patch = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1,3 +1,2 @@\n same\n-old\n-removed\n+new\n@@ -10 +9 @@\n-before\n\\ No newline at end of file\n+after\n\\ No newline at end of file\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -0,0 +1 @@\n+added\n";
        let parsed = ParsedDiff::new(patch.into());
        let text = |side: Option<u32>| side.map(|i| parsed.rows[i as usize].text.as_ref());
        let first = parsed.pairs[0]
            .iter()
            .map(|(a, b)| (text(*a), text(*b)))
            .collect::<Vec<_>>();
        assert_eq!(
            first,
            vec![
                (Some("@@ -1,3 +1,2 @@"), Some("@@ -1,3 +1,2 @@")),
                (Some("same"), Some("same")),
                (Some("old"), Some("new")),
                (Some("removed"), None),
                (Some("@@ -10 +9 @@"), Some("@@ -10 +9 @@")),
                (Some("before"), Some("after")),
                (
                    Some("No newline at end of file"),
                    Some("No newline at end of file")
                ),
            ]
        );
        assert_eq!(parsed.pairs[1].len(), 2);
        assert_eq!(parsed.pairs[1][1].0, None);
        assert_eq!(text(parsed.pairs[1][1].1), Some("added"));
    }

    #[test]
    fn pull_request_stream_keeps_every_file_and_folds_to_headers() {
        let patch = ["a.rs", "b.rs"]
            .iter()
            .map(|name| format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -1 +1 @@\n-old\n+new\n"))
            .collect::<String>();
        let parsed = ParsedDiff::new(patch);
        assert_eq!(*parsed.stats, vec![(1, 1), (1, 1)]);
        let order = ["crates/ui/build.rs", "crates/ui/assets/a.svg", "Cargo.toml", "crates/ui/Cargo.toml"]
            .iter()
            .map(|name| format!("diff --git a/{name} b/{name}\n--- a/{name}\n+++ b/{name}\n@@ -1 +1 @@\n-old\n+new\n"))
            .collect::<String>();
        let paths: Vec<_> = ParsedDiff::new(order)
            .files
            .iter()
            .map(|(path, _)| path.clone())
            .collect();
        assert_eq!(
            paths,
            [
                "Cargo.toml",
                "crates/ui/Cargo.toml",
                "crates/ui/build.rs",
                "crates/ui/assets/a.svg"
            ],
            "root files lead and each directory's files stay together"
        );
        let (rows, ranges) = stream(&parsed, false, &Default::default());
        assert_eq!(ranges, vec![0..4, 4..8]);
        assert_eq!(rows[0], StreamRow::File(0));
        assert_eq!(rows[4], StreamRow::File(1));
        assert_eq!(file_at(&ranges, 6), Some(1));
        let (rows, ranges) = stream(&parsed, true, &[0].into_iter().collect());
        assert_eq!(ranges, vec![0..1, 1..4]);
        assert_eq!(
            rows[1..],
            [
                StreamRow::File(1),
                StreamRow::Pair(1, 0),
                StreamRow::Pair(1, 1)
            ]
        );
    }

    #[test]
    fn pull_request_tree_groups_files_under_their_directory() {
        let files: Vec<(String, usize)> = [
            "Cargo.toml",
            "crates/ui/src/a.rs",
            "crates/ui/src/b.rs",
            "docs/c.md",
        ]
        .iter()
        .map(|path| (path.to_string(), 0))
        .collect();
        assert_eq!(
            tree(&files, ""),
            vec![
                TreeEntry::File {
                    index: 0,
                    name: "Cargo.toml",
                    depth: 0
                },
                TreeEntry::Directory {
                    path: "crates/ui/src",
                    depth: 0
                },
                TreeEntry::File {
                    index: 1,
                    name: "a.rs",
                    depth: 1
                },
                TreeEntry::File {
                    index: 2,
                    name: "b.rs",
                    depth: 1
                },
                TreeEntry::Directory {
                    path: "docs",
                    depth: 0
                },
                TreeEntry::File {
                    index: 3,
                    name: "c.md",
                    depth: 1
                },
            ]
        );
        assert_eq!(
            tree(&files, "b.rs"),
            vec![
                TreeEntry::Directory {
                    path: "crates/ui/src",
                    depth: 0
                },
                TreeEntry::File {
                    index: 2,
                    name: "b.rs",
                    depth: 1
                },
            ]
        );
    }
}
