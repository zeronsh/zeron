//! File navigation and the read-only diff share one bounded review workspace.
use super::*;

const FILES_WIDTH: f32 = 224.0;
const WORKSPACE_GAP: f32 = 12.0;
const WIDE_MIN: f32 = 760.0;

fn code_gutter(rows: &[CodeRow]) -> f32 {
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
            let mut cursor = start + 1; // The selected path is already in the toolbar.
            let mut pairs = Vec::new();
            while cursor < end {
                if rows[cursor].kind == crate::changes::LineKind::Meta {
                    pairs.push((Some(cursor as u32), Some(cursor as u32)));
                    cursor += 1;
                    continue;
                }
                let first = cursor;
                while cursor < end
                    && (rows[cursor].kind != crate::changes::LineKind::Meta
                        || rows[cursor].text.as_ref() == "No newline at end of file")
                {
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

impl PullRequestDetailPage {
    pub(super) fn code_workspace(
        &self,
        detail: &ChangeRequestDetail,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let wide = self.code_pane_width.is_some_and(|width| width >= WIDE_MIN);
        let current = self.selected_code_file;
        let path = self
            .code_files
            .get(current)
            .map(|(path, _)| path.clone())
            .unwrap_or_default();
        let selected_stats = detail.files.iter().find(|file| file.path == path);
        let file_count = self.code_files.len();
        let mut workspace = div()
            .id("pr-code-workspace")
            .debug_selector(|| "pr-code-workspace".into())
            .relative()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .flex()
            .gap(px(WORKSPACE_GAP))
            .when(!wide, |el| el.flex_col())
            .child({
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
            });
        if wide || self.files_expanded {
            let mut files = div()
                .id("pr-file-list")
                .debug_selector(|| "pr-file-list".into())
                .flex_1()
                .min_h_0()
                .overflow_y_scroll();
            let mut matches = 0;
            for (index, (path, _)) in self.code_files.iter().enumerate() {
                if !path.to_lowercase().contains(&self.file_query) {
                    continue;
                }
                matches += 1;
                let (directory, name) = path.rsplit_once('/').unwrap_or(("", path));
                let stats = detail.files.iter().find(|file| file.path == *path);
                files = files.child(
                    div()
                        .id(SharedString::from(format!("pr-file-{index}")))
                        .debug_selector(move || format!("pr-file-{index}"))
                        .role(gpui::Role::Button)
                        .aria_label(format!("Open diff for {path}"))
                        .aria_selected(index == current)
                        .tab_index(0)
                        .w_full()
                        .min_w_0()
                        .px(px(10.0))
                        .py(px(8.0))
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .cursor_pointer()
                        .when(index == current, |el| el.bg(theme.glass_hover()))
                        .hover(|style| style.bg(theme.glass_hover()))
                        .focus_visible(|style| style.bg(theme.glass_hover()))
                        .child(
                            crate::icons::icon(crate::icons::FILE_CODE)
                                .size(px(14.0))
                                .text_color(theme.text_muted),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(div().truncate().text_size(px(12.0)).child(name.to_owned()))
                                .when(!directory.is_empty(), |el| {
                                    el.child(
                                        div()
                                            .truncate()
                                            .text_size(px(10.0))
                                            .text_color(theme.text_muted)
                                            .child(directory.to_owned()),
                                    )
                                }),
                        )
                        .when_some(stats, |el, stats| {
                            el.child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.0))
                                    .flex()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_color(theme.success)
                                            .child(format!("+{}", stats.additions)),
                                    )
                                    .child(
                                        div()
                                            .text_color(theme.danger)
                                            .child(format!("−{}", stats.deletions)),
                                    ),
                            )
                        })
                        .on_click(cx.listener(move |page, _, window, cx| {
                            page.select_code_file(index, cx);
                            if !wide {
                                page.files_expanded = false;
                                window.blur();
                            }
                            cx.notify();
                        })),
                );
            }
            if matches == 0 {
                files = files.child(
                    div()
                        .p(px(12.0))
                        .text_color(theme.text_muted)
                        .child("No matching files"),
                );
            }
            workspace = workspace.child(
                div()
                    .id("pr-file-browser")
                    .debug_selector(|| "pr-file-browser".into())
                    .flex_none()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .when(wide, |el| el.w(px(FILES_WIDTH)).h_full())
                    .when(!wide, |el| el.w_full().h(px(136.0)))
                    .child(
                        div()
                            .h(px(crate::surface_chrome::CONTROL_SIZE))
                            .flex_none()
                            .flex()
                            .items_center()
                            .px(px(10.0))
                            .text_size(px(12.0))
                            .text_color(theme.text_muted)
                            .child(format!("Files changed · {file_count}")),
                    )
                    .child(
                        // The shared input grows along its parent axis; this parent is a column.
                        crate::surface_chrome::input()
                            .flex_none()
                            .id("pr-file-search")
                            .debug_selector(|| "pr-file-search".into())
                            .child(div().flex_1().min_w_0().child(self.file_search.clone())),
                    )
                    .child(files),
            );
        }
        let patch = self.diff.clone().unwrap();
        let mut editor = div().flex_1().min_w_0().min_h_0().flex().flex_col().child(
            div()
                .id("pr-file-navigation")
                .debug_selector(|| "pr-file-navigation".into())
                .flex_none()
                .min_w_0()
                .pb(px(8.0))
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    action("pr-files", "Choose a changed file", theme)
                        .aria_expanded(wide || self.files_expanded)
                        .on_click(cx.listener(move |page, _, window, cx| {
                            if wide {
                                window.focus(&page.file_search.read(cx).focus_handle(cx), cx);
                            } else {
                                page.toggle_files(cx);
                                if page.files_expanded {
                                    window.focus(&page.file_search.read(cx).focus_handle(cx), cx);
                                }
                            }
                        })),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.0))
                        .child(if path.is_empty() {
                            "No changed files".into()
                        } else {
                            path
                        }),
                )
                .when_some(selected_stats.filter(|_| wide), |el, stats| {
                    el.child(
                        div()
                            .flex_none()
                            .flex()
                            .gap(px(6.0))
                            .text_size(px(11.0))
                            .child(
                                div()
                                    .text_color(theme.success)
                                    .child(format!("+{}", stats.additions)),
                            )
                            .child(
                                div()
                                    .text_color(theme.danger)
                                    .child(format!("−{}", stats.deletions)),
                            ),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(theme.text_muted)
                        .child(format!(
                            "{} / {file_count}",
                            if file_count == 0 { 0 } else { current + 1 }
                        )),
                )
                .child(
                    action("pr-previous-file", "Previous file", theme)
                        .when(current == 0, |el| el.opacity(0.4))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            if current > 0 {
                                page.select_code_file(current - 1, cx);
                            }
                        })),
                )
                .child(
                    action("pr-next-file", "Next file", theme)
                        .when(current + 1 >= file_count, |el| el.opacity(0.4))
                        .on_click(cx.listener(move |page, _, _, cx| {
                            page.select_code_file(current + 1, cx)
                        })),
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
                    .on_click(cx.listener(|page, _, _, cx| {
                        page.code_split = !page.code_split;
                        let split = page.code_split;
                        crate::settings::update(
                            crate::settings::SavePolicy::Immediate,
                            cx,
                            |settings| {
                                settings.diff_split = split;
                            },
                        );
                        page.select_code_file(page.selected_code_file, cx);
                    })),
                )
                .child(
                    action("pr-copy-patch", "Copy diff", theme).on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                            patch.as_ref().clone(),
                        ));
                    }),
                ),
        );
        let mut visible = self.code_range();
        if !visible.is_empty() {
            visible.start += 1;
        }
        let gutter = code_gutter(&self.code_rows[visible.clone()]);
        let code_width =
            (self.code_width - 128.0) / 7.0 * crate::changes::diff_text_size(theme) * 0.7
                + 144.0
                + 2.0 * (gutter - crate::changes::GUTTER_WIDTH);
        let text_width =
            (self.code_width - 128.0) / 7.0 * crate::changes::diff_text_size(theme) * 0.7;
        let horizontal = self.code_horizontal.clone();
        let split = self.code_split;
        let pairs = self.code_pairs.clone();
        let row_count = if split {
            pairs.get(current).map_or(0, Vec::len)
        } else {
            visible.len()
        };
        let rows = self.code_rows.clone();
        let colors = theme.clone();
        let scroll = self.code_scroll.0.borrow().base_handle.clone();
        editor =
            editor.child(
                div()
                    .id("pr-code-viewport")
                    .debug_selector(|| "pr-code-viewport".into())
                    .flex_1()
                    .min_h_0()
                    .border_t_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .when(!split, |el| {
                        el.overflow_x_scroll()
                            .track_scroll(&self.code_horizontal)
                            .map(|mut el| {
                                el.style().restrict_scroll_to_axis = Some(true);
                                el
                            })
                    })
                    .child(
                        crate::edge_fade::edge_faded(
                            12.0,
                            true,
                            true,
                            gpui::uniform_list("pr-code-lines", row_count, move |range, _, _| {
                                range
                                    .map(|index| {
                                        let (left, right) = if split {
                                            pairs[current][index]
                                        } else {
                                            (Some((index + visible.start) as u32), None)
                                        };
                                        let row = &rows[left.or(right).unwrap() as usize];
                                        if row.kind == crate::changes::LineKind::Meta {
                                            div()
                                                .w_full()
                                                .h(px(crate::changes::diff_line_height(&colors)))
                                                .px(px(12.0))
                                                .bg(crate::theme::wash(0.035))
                                                .font_family(colors.font_mono.clone())
                                                .text_size(px(11.0))
                                                .text_color(colors.text_muted)
                                                .child(row.text.clone())
                                                .into_any_element()
                                        } else if split {
                                            let left_row = left.map(|i| &rows[i as usize]);
                                            let right_row = right.map(|i| &rows[i as usize]);
                                            let left_line = left_row.map(diff_line);
                                            let right_line = right_row.map(diff_line);
                                            crate::changes::readonly_split_line(
                                                left_line.as_ref().zip(left_row).map(
                                                    |(line, row)| (line, row.spans.as_slice()),
                                                ),
                                                right_line.as_ref().zip(right_row).map(
                                                    |(line, row)| (line, row.spans.as_slice()),
                                                ),
                                                &colors,
                                                gutter,
                                                text_width,
                                                &horizontal,
                                                index,
                                            )
                                        } else {
                                            crate::changes::readonly_diff_line(
                                                &crate::changes::DiffLine {
                                                    kind: row.kind,
                                                    old_no: row.old.parse().ok(),
                                                    new_no: row.new.parse().ok(),
                                                    text: row.text.to_string(),
                                                },
                                                &row.spans,
                                                &colors,
                                                gutter,
                                            )
                                        }
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .debug_selector(|| "pr-code-lines".into())
                            .when(split, |el| el.w_full())
                            .when(!split, |el| el.w(px(code_width)))
                            .min_w_full()
                            .h_full()
                            .track_scroll(&self.code_scroll),
                        )
                        .fade_overflow_y(&scroll),
                    ),
            );
        workspace.child(editor).into_any_element()
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
}
