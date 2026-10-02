//! Settings → Files: local preferences for workspace-file editing.

use gpui::{Context, EventEmitter, Window, div, prelude::*, px};

use super::widgets;
use crate::popover;
use crate::theme::Theme;

const DELAY_OPTIONS: [u64; 5] = [300, 600, 900, 1_500, 3_000];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilesSettingsEvent {
    AutosaveChanged(bool),
    AutosaveDelayChanged(u64),
    WordWrapChanged(bool),
    ShowAllFilesChanged(bool),
}

pub struct FilesSettingsPage {
    scroll: widgets::PageScroll,
    autosave_enabled: bool,
    autosave_delay_ms: u64,
    word_wrap: bool,
    show_all_files: bool,
    delay_select: widgets::SelectState,
}

impl EventEmitter<FilesSettingsEvent> for FilesSettingsPage {}

impl FilesSettingsPage {
    pub fn new(
        autosave_enabled: bool,
        autosave_delay_ms: u64,
        word_wrap: bool,
        show_all_files: bool,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            scroll: widgets::PageScroll::default(),
            autosave_enabled,
            autosave_delay_ms,
            word_wrap,
            show_all_files,
            delay_select: widgets::SelectState::default(),
        }
    }

    pub fn set_word_wrap(&mut self, word_wrap: bool, cx: &mut Context<Self>) {
        if self.word_wrap == word_wrap {
            return;
        }
        self.word_wrap = word_wrap;
        cx.notify();
    }

    pub fn set_show_all_files(&mut self, show_all_files: bool, cx: &mut Context<Self>) {
        if self.show_all_files == show_all_files {
            return;
        }
        self.show_all_files = show_all_files;
        cx.notify();
    }

    fn on_scroll_hovered(&mut self, hovered: &bool, _: &mut Window, cx: &mut Context<Self>) {
        if self.scroll.set_list_hovered(*hovered) {
            cx.notify();
        }
    }
}

impl popover::ScrollRailHost for FilesSettingsPage {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        self.scroll.rail_bar()
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.scroll.rail_scroll()
    }
}

impl Render for FilesSettingsPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if cx.has_global::<crate::app_runtime::AppRuntime>() {
            let settings = crate::settings::current(cx);
            self.autosave_enabled = settings.files_autosave_enabled;
            self.autosave_delay_ms = settings.files_autosave_delay_ms;
            self.word_wrap = settings.files_word_wrap;
            self.show_all_files = settings.files_show_all;
        }
        let theme = Theme::of(cx).for_settings_surface();
        let autosave_enabled = self.autosave_enabled;
        let selected = self.autosave_delay_ms;
        let word_wrap = self.word_wrap;
        let show_all_files = self.show_all_files;
        let delay_control = widgets::select(
            "files-autosave-delay",
            "Autosave delay",
            &theme,
            |page: &mut Self| &mut page.delay_select,
        )
        .options(
            DELAY_OPTIONS.into_iter().map(|delay| {
                widgets::SelectOption::new(if delay >= 1_000 {
                    format!("{} s", delay as f32 / 1_000.0)
                } else {
                    format!("{delay} ms")
                })
            }),
            DELAY_OPTIONS
                .iter()
                .position(|delay| *delay == selected)
                .unwrap_or_default(),
        )
        .width(112.0)
        .on_select(|page, ix, _, cx| {
            let delay = DELAY_OPTIONS[ix];
            page.autosave_delay_ms = delay;
            cx.emit(FilesSettingsEvent::AutosaveDelayChanged(delay));
            cx.notify();
        })
        .render(&self.delay_select, cx);
        let card = widgets::section_card(&theme)
            .child(
                widgets::card_row(&theme, true)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Autosave")),
                    )
                    .child(
                        widgets::toggle_switch(&theme, autosave_enabled, "files-autosave")
                            .id("files-autosave-toggle")
                            .cursor_pointer()
                            .tab_index(0)
                            .role(gpui::Role::Switch)
                            .aria_label("Autosave")
                            .aria_toggled(if autosave_enabled {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.autosave_enabled = !this.autosave_enabled;
                                cx.emit(FilesSettingsEvent::AutosaveChanged(this.autosave_enabled));
                                cx.notify();
                            })),
                    ),
            )
            .when(autosave_enabled, |card| {
                card.child(
                    widgets::card_row(&theme, true)
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(160.0))
                                .flex()
                                .flex_col()
                                .child(widgets::row_title(&theme, "Autosave delay")),
                        )
                        .child(delay_control),
                )
            })
            .child(
                widgets::card_row(&theme, false)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Word wrap")),
                    )
                    .child(
                        widgets::toggle_switch(&theme, word_wrap, "files-word-wrap")
                            .id("files-word-wrap-toggle")
                            .cursor_pointer()
                            .tab_index(0)
                            .role(gpui::Role::Switch)
                            .aria_label("Word wrap")
                            .aria_toggled(if word_wrap {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.word_wrap = !this.word_wrap;
                                cx.emit(FilesSettingsEvent::WordWrapChanged(this.word_wrap));
                                cx.notify();
                            })),
                    ),
            )
            .child(
                widgets::card_row(&theme, false)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(160.0))
                            .flex()
                            .flex_col()
                            .child(widgets::row_title(&theme, "Show hidden and ignored files")),
                    )
                    .child(
                        widgets::toggle_switch(&theme, show_all_files, "files-show-all")
                            .id("files-show-all-toggle")
                            .cursor_pointer()
                            .tab_index(0)
                            .role(gpui::Role::Switch)
                            .aria_label("Show hidden and ignored files")
                            .aria_toggled(if show_all_files {
                                gpui::Toggled::True
                            } else {
                                gpui::Toggled::False
                            })
                            .focus_visible(|s| s.border_2().border_color(theme.accent).opacity(1.0))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.show_all_files = !this.show_all_files;
                                cx.emit(FilesSettingsEvent::ShowAllFilesChanged(
                                    this.show_all_files,
                                ));
                                cx.notify();
                            })),
                    ),
            );

        let scrollbar = popover::rail(self, "files-settings-page-scrollbar", &theme, cx);
        div()
            .id("files-settings-page-host")
            .relative()
            .size_full()
            .on_hover(cx.listener(Self::on_scroll_hovered))
            .child(
                crate::edge_fade::edge_faded(
                    16.0,
                    true,
                    true,
                    div()
                        .id("files-settings-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll.scroll)
                        .child(
                            widgets::page_column()
                                .child(widgets::page_header(&theme, "Files", None))
                                .child(card),
                        ),
                )
                .fade_overflow_y(&self.scroll.scroll),
            )
            .children(scrollbar)
    }
}
