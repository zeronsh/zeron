//! Workspace file browsing surface.

use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use gpui::{
    Context, Entity, EventEmitter, FocusHandle, ListAlignment, ListState, Pixels, Point, Render,
    SharedString, Subscription, Task, Window, div, prelude::*, px,
};
use zeron_proto::ListWorkspaceDirectoryRequest;

use crate::{
    composer::{ComposerInput, ComposerInputEvent},
    state::AppState,
};

pub mod client;
pub mod document;
pub mod editor;
pub mod editor_adapter;
mod git_status;
mod image_preview;
pub(crate) mod markdown_media;
mod markdown_preview;
pub mod model;
mod pane;
pub mod preview;
pub mod search;
mod sections;
pub mod tree;
pub mod watch;

use client::{FilesRequestContext, WorkspaceFilesClient};
use model::{DirectoryLoadState, FileTreeModel};
pub(crate) use pane::ExplorerPage;
use preview::FilePreviewState;
use search::FileSearchState;

static NEXT_REVIEW_COMMENT_FLUSH_SOURCE: AtomicU64 = AtomicU64::new(1);
use crate::surface_chrome::{
    CONTROL_RADIUS as TOOLBAR_BUTTON_RADIUS, CONTROL_SIZE as TOOLBAR_BUTTON_SIZE, toolbar,
};

pub(super) fn toolbar_button(id: &'static str, label: &'static str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .size(px(TOOLBAR_BUTTON_SIZE))
        .flex_none()
        .rounded(px(TOOLBAR_BUTTON_RADIUS))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .role(gpui::Role::Button)
        .aria_label(label)
        .occlude()
        .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.prevent_default()
        })
        .hover(|style| style.bg(crate::theme::wash(0.14)))
        .tooltip(move |_, cx| {
            cx.new(|_| preview::FileEditorTooltip { text: label.into() })
                .into()
        })
        .tooltip_show_delay(Duration::from_millis(350))
}

/// A workspace-relative file or directory dragged out of a Files surface.
///
/// Keeping the payload relative is important: the composer may target a
/// remote device, and its existing file-mention transport resolves paths in
/// that workspace instead of leaking a path from the UI machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkspacePathDrag {
    pub path: String,
    pub is_directory: bool,
}

impl WorkspacePathDrag {
    pub(crate) fn new(path: String, is_directory: bool) -> Self {
        Self { path, is_directory }
    }

    fn title(&self) -> SharedString {
        self.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or(&self.path)
            .to_string()
            .into()
    }
}

/// Compact drag preview shared by tree and search rows. It deliberately uses
/// the same raised surface, hairline, type scale, and opacity as surface tabs.
pub(crate) struct WorkspacePathDragGhost {
    payload: WorkspacePathDrag,
}

impl Render for WorkspacePathDragGhost {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = crate::theme::Theme::of(cx);
        div()
            .h(px(24.0))
            .max_w(px(220.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .gap(px(5.0))
            .rounded(px(6.0))
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .text_size(px(11.5))
            .text_color(theme.text)
            .opacity(0.85)
            .child({
                let identity = if self.payload.is_directory {
                    crate::file_icons::FileIconIdentity::directory(&self.payload.path, false)
                } else {
                    crate::file_icons::FileIconIdentity::file(&self.payload.path)
                };
                crate::file_icons::icon(identity, theme.appearance)
                    .size(px(14.0))
                    .flex_none()
            })
            .child(div().min_w_0().truncate().child(self.payload.title()))
    }
}

pub(crate) fn workspace_path_drag_ghost(
    payload: &WorkspacePathDrag,
    cx: &mut gpui::App,
) -> gpui::Entity<WorkspacePathDragGhost> {
    let payload = payload.clone();
    cx.new(|_| WorkspacePathDragGhost { payload })
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilesEvent {
    OpenFile(String),
    RevealFile(String),
    OpenWebLink(crate::markdown::render::LinkActivation),
    TitleChanged,
    FileRenamed {
        old_path: String,
        new_path: String,
    },
    WordWrapChanged(bool),
    ShowAllFilesChanged(bool),
    CloseReady,
    CloseCancelled,
    ExplorerPageChanged,
    /// An activity row: open this subagent's transcript in the right pane.
    OpenSubagent {
        doc_id: String,
        title: String,
        frozen: bool,
    },
    /// An activity row: open this side chat (by id) in the right pane.
    OpenChildChat(String),
    ChildChatContextMenu {
        chat_id: String,
        position: Point<Pixels>,
    },
    /// The Chats header's "+": start a fresh side chat of the active chat.
    NewChildChat,
    /// The Chats header's fork: fork the active chat into a side chat.
    ForkChat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesCloseDisposition {
    Allow,
    Pending,
    Blocked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilesPresentation {
    Explorer,
    Editor,
}

impl FilesPresentation {
    fn is_editor(self) -> bool {
        matches!(self, Self::Editor)
    }
}

impl EventEmitter<FilesEvent> for FilesSurface {}

struct EditorContextMenu {
    editor: Entity<editor::FileEditorState>,
    position: Point<Pixels>,
    availability: editor::EditorMenuAvailability,
}

pub struct FilesSurface {
    state: Entity<AppState>,
    chat_id: String,
    review_comment_flush_source: u64,
    presentation: FilesPresentation,
    editor_path: Option<String>,
    request_context: Option<FilesRequestContext>,
    target_change_pending: bool,
    selected_editor_path: Option<String>,
    pending_request_context: Option<FilesRequestContext>,
    tree: FileTreeModel,
    tree_list: ListState,
    tree_list_rows: Vec<model::VisibleTreeRow>,
    tree_list_generation: u64,
    /// Floating rail state for the file tree (the menu-scrollbar treatment).
    tree_bar: crate::popover::MenuScrollbarState,
    tree_focus: FocusHandle,
    search: Entity<ComposerInput>,
    search_restore_tree_focus: bool,
    search_state: FileSearchState,
    search_list: ListState,
    git_status: Option<Entity<git_status::GitStatusSource>>,
    git_status_subscription: Option<Subscription>,
    watch_task: Option<Task<()>>,
    watch_sequence: Option<u64>,
    watch_error: Option<SharedString>,
    preview: FilePreviewState,
    editor_context_menu: crate::popover::Popup<EditorContextMenu>,
    loads: HashMap<(String, Option<String>), Task<()>>,
    error: Option<SharedString>,
    started: bool,
    /// The file tree and session activity share a horizontally paged pane.
    pane: pane::ExplorerPane,
    sections: sections::ExplorerSections,
    _observe: Subscription,
    _search_events: Subscription,
}

impl Render for FilesSurface {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.search_restore_tree_focus) {
            self.tree_focus.focus(window, cx);
        }
        let theme = crate::theme::Theme::of(cx).clone();
        let is_editor = self.presentation.is_editor();
        let body = if is_editor {
            let header = self.render_editor_header(&theme, cx);
            let preview = self.render_preview(window, cx);
            div()
                .size_full()
                .flex()
                .flex_col()
                .children(header)
                .child(div().flex_1().min_h_0().w_full().child(preview))
                .into_any_element()
        } else {
            self.render_explorer_pane(window, &theme, cx)
        };
        let editor_context_menu = self.render_editor_context_menu(&theme, cx);
        div()
            .id(SharedString::from(format!(
                "files-surface-{}",
                self.chat_id
            )))
            .role(gpui::Role::Group)
            .aria_label(if is_editor {
                "Workspace files"
            } else {
                "Files and subagents"
            })
            .size_full()
            .relative()
            .flex()
            .bg(crate::theme::ink(0.0))
            .flex_col()
            .child(div().flex_1().min_h_0().w_full().child(body))
            .children(editor_context_menu)
    }
}

impl FilesSurface {
    fn render_explorer(
        &mut self,
        theme: &crate::theme::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let phase = self.tree.node("").map(|root| root.load.clone());
        let content = if !self.search_state.query.is_empty() {
            self.render_search_results(cx)
        } else if let Some(error) = self.error.clone().filter(|_| !self.tree_has_content()) {
            div()
                .flex_1()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .px(px(28.0))
                .child(
                    div()
                        .text_center()
                        .text_size(px(12.0))
                        .text_color(theme.text_muted)
                        .child(error),
                )
                .child(
                    div()
                        .id("files-retry-root")
                        .h(px(28.0))
                        .px(px(12.0))
                        .rounded(px(7.0))
                        .border_1()
                        .border_color(theme.border)
                        .bg(crate::theme::wash(0.04))
                        .hover(|style| style.bg(crate::theme::wash(0.09)))
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .text_size(px(11.5))
                        .text_color(theme.text)
                        .child("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.retry_root(cx))),
                )
                .into_any_element()
        } else if !self.tree_has_content()
            && matches!(
                phase.as_ref(),
                Some(DirectoryLoadState::Unloaded | DirectoryLoadState::Loading { .. })
            )
        {
            div().flex_1().into_any_element()
        } else {
            self.render_tree(cx)
        };
        let watch_error = self.watch_error.clone();
        div()
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .when_some(self.git_status_notice(cx), |element, notice| {
                element.child(
                    div()
                        .flex_none()
                        .px(px(10.0))
                        .py(px(4.0))
                        .text_size(px(10.0))
                        .text_color(theme.text_faint)
                        .child(notice),
                )
            })
            .when_some(watch_error, |element, error| {
                element.child(
                    div()
                        .h(px(27.0))
                        .flex_none()
                        .px(px(10.0))
                        .border_b_1()
                        .border_color(theme.warning.opacity(0.22))
                        .bg(theme.warning.opacity(0.045))
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(10.0))
                        .text_color(theme.warning_muted)
                        .child(
                            crate::icons::icon(crate::icons::REFRESH)
                                .size(px(10.5))
                                .flex_none(),
                        )
                        .child(div().min_w_0().flex_1().truncate().child(error))
                        .child(
                            div()
                                .id("files-watch-refresh-now")
                                .h(px(20.0))
                                .flex_none()
                                .px(px(6.0))
                                .rounded(px(5.0))
                                .flex()
                                .items_center()
                                .cursor_pointer()
                                .role(gpui::Role::Button)
                                .aria_label("Refresh workspace files now")
                                .text_color(theme.text_muted)
                                .hover(|style| style.bg(crate::theme::wash(0.07)))
                                .child("Refresh now")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.refresh(cx);
                                    this.reconcile_open_documents(cx);
                                })),
                        ),
                )
            })
            .child(content)
    }

    /// A persistent explorer: opening a path always delegates to the shell.
    pub fn new_explorer(
        state: Entity<AppState>,
        chat_id: String,
        show_all_files: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_presentation(
            state,
            chat_id,
            FilesPresentation::Explorer,
            None,
            false,
            1000,
            13.0,
            false,
            show_all_files,
            cx,
        )
    }

    #[cfg(test)]
    pub fn new(
        state: Entity<AppState>,
        chat_id: String,
        autosave_enabled: bool,
        autosave_delay_ms: u64,
        editor_font_size: f32,
        word_wrap: bool,
        show_all_files: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_presentation(
            state,
            chat_id,
            FilesPresentation::Editor,
            None,
            autosave_enabled,
            autosave_delay_ms,
            editor_font_size,
            word_wrap,
            show_all_files,
            cx,
        )
    }

    pub fn new_editor(
        state: Entity<AppState>,
        chat_id: String,
        path: String,
        autosave_enabled: bool,
        autosave_delay_ms: u64,
        editor_font_size: f32,
        word_wrap: bool,
        show_all_files: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new_with_presentation(
            state,
            chat_id,
            FilesPresentation::Editor,
            Some(path),
            autosave_enabled,
            autosave_delay_ms,
            editor_font_size,
            word_wrap,
            show_all_files,
            cx,
        )
    }

    fn new_with_presentation(
        state: Entity<AppState>,
        chat_id: String,
        presentation: FilesPresentation,
        editor_path: Option<String>,
        autosave_enabled: bool,
        autosave_delay_ms: u64,
        editor_font_size: f32,
        word_wrap: bool,
        show_all_files: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            ComposerInput::new("Search files", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::SearchInput)
                .with_text_metrics(11.0, 16.0)
        });
        let search_events = cx.subscribe(&search, |this: &mut Self, _, event, cx| match event {
            ComposerInputEvent::Edited => this.on_search_edited(cx),
            ComposerInputEvent::Submitted
            | ComposerInputEvent::ModifiedSubmitted
            | ComposerInputEvent::MentionAccept => this.activate_search_result(cx),
            ComposerInputEvent::MentionNavigate(delta) => {
                let len = this.search_state.visible_len();
                if len > 0 {
                    this.search_state.active = if *delta < 0 {
                        this.search_state.active.saturating_sub(1)
                    } else {
                        (this.search_state.active + 1).min(len - 1)
                    };
                    this.search_list
                        .scroll_to_reveal_item(this.search_state.active);
                    cx.notify();
                }
            }
            ComposerInputEvent::MentionDismiss => this.close_search(cx),
            ComposerInputEvent::PastedImages(_)
            | ComposerInputEvent::PastedPaths(_)
            | ComposerInputEvent::PastedText { .. }
            | ComposerInputEvent::CursorMoved
            | ComposerInputEvent::ViewportChanged => {}
        });
        let observe = cx.observe(&state, |this: &mut Self, _, cx| {
            if this.sync_target(cx) {
                this.ensure_loaded(cx);
            }
            this.sync_active_markdown_comments(cx);
            if !this.presentation.is_editor() {
                this.refresh_sections(cx);
            }
        });
        // The list state exposes no scroll handle, so the floating rail
        // bridges scroll activity through the scroll handler (the
        // transcript's pattern) and reads its geometry from the state's own
        // scrollbar accessors.
        let tree_list = ListState::new(0, ListAlignment::Top, px(560.0));
        let weak = cx.entity().downgrade();
        tree_list.set_scroll_handler(move |_, _, cx| {
            weak.update(cx, |this: &mut Self, cx| this.on_tree_scrolled(cx))
                .ok();
        });
        let mut surface = Self {
            state,
            chat_id,
            review_comment_flush_source: NEXT_REVIEW_COMMENT_FLUSH_SOURCE
                .fetch_add(1, Ordering::Relaxed),
            presentation,
            editor_path: editor_path.clone(),
            request_context: None,
            target_change_pending: false,
            selected_editor_path: None,
            pending_request_context: None,
            tree: FileTreeModel::with_include_ignored(show_all_files),
            tree_list,
            tree_list_rows: Vec::new(),
            tree_list_generation: 0,
            tree_bar: crate::popover::MenuScrollbarState::default(),
            tree_focus: cx.focus_handle(),
            search,
            search_restore_tree_focus: false,
            search_state: FileSearchState::default(),
            search_list: ListState::new(0, ListAlignment::Top, px(420.0)),
            git_status: None,
            git_status_subscription: None,
            watch_task: None,
            watch_sequence: None,
            watch_error: None,
            preview: FilePreviewState::new(
                autosave_enabled,
                autosave_delay_ms,
                word_wrap,
                editor_font_size,
            ),
            editor_context_menu: crate::popover::Popup::default(),
            loads: HashMap::new(),
            error: None,
            started: false,
            pane: pane::ExplorerPane::new(cx),
            sections: sections::ExplorerSections::default(),
            _observe: observe,
            _search_events: search_events,
        };
        surface.sync_target(cx);
        surface
    }

    pub(in crate::files) fn open_editor_context_menu(
        &mut self,
        editor: Entity<editor::FileEditorState>,
        availability: editor::EditorMenuAvailability,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        self.editor_context_menu.open(EditorContextMenu {
            editor,
            position,
            availability,
        });
        cx.notify();
    }

    fn close_editor_context_menu(&mut self, cx: &mut Context<Self>) {
        if self.editor_context_menu.begin_close() {
            crate::popover::reap_popup(cx, |surface: &mut Self| &mut surface.editor_context_menu);
            cx.notify();
        }
    }

    fn dispatch_editor_context_action(
        &mut self,
        editor: Entity<editor::FileEditorState>,
        action: editor::EditorContextAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_editor_context_menu(cx);
        editor::dispatch_context_action(&editor, action, window, cx);
    }

    fn editor_context_menu_row(
        theme: &crate::theme::Theme,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        editor: Entity<editor::FileEditorState>,
        action: editor::EditorContextAction,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        crate::popover::menu_row(theme, false, id)
            .id(id)
            .when(!enabled, |row| row.opacity(0.38).cursor_default())
            .when(enabled, |row| {
                row.on_click(cx.listener(move |this, _, window, cx| {
                    this.dispatch_editor_context_action(editor.clone(), action, window, cx)
                }))
            })
            .child(label)
            .into_any_element()
    }

    fn render_editor_context_menu(
        &mut self,
        theme: &crate::theme::Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let theme = &theme.for_popup();
        let menu = self.editor_context_menu.get()?;
        let editor = menu.editor.clone();
        let position = menu.position;
        let availability = menu.availability;
        let closing = self.editor_context_menu.closing_since();

        let card = crate::popover::popover_card(theme)
            .w(px(170.0))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_editor_context_menu(cx)))
            .flex()
            .flex_col()
            .child(Self::editor_context_menu_row(
                theme,
                "files-editor-context-cut",
                "Cut",
                availability.cut,
                editor.clone(),
                editor::EditorContextAction::Cut,
                cx,
            ))
            .child(Self::editor_context_menu_row(
                theme,
                "files-editor-context-copy",
                "Copy",
                availability.copy,
                editor.clone(),
                editor::EditorContextAction::Copy,
                cx,
            ))
            .child(Self::editor_context_menu_row(
                theme,
                "files-editor-context-paste",
                "Paste",
                availability.paste,
                editor.clone(),
                editor::EditorContextAction::Paste,
                cx,
            ))
            .child(crate::popover::menu_separator())
            .child(Self::editor_context_menu_row(
                theme,
                "files-editor-context-select-all",
                "Select All",
                true,
                editor,
                editor::EditorContextAction::SelectAll,
                cx,
            ))
            .into_any_element();

        Some(crate::popover::menu_at(
            "files-editor-context-menu",
            position,
            card,
            closing,
        ))
    }

    pub fn set_autosave_delay_ms(&mut self, delay_ms: u64, cx: &mut Context<Self>) {
        let pending = self.preview.set_autosave_delay_ms(delay_ms);
        for path in pending {
            self.schedule_autosave(path, cx);
        }
        cx.notify();
    }

    pub fn set_autosave_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        let pending = self.preview.set_autosave_enabled(enabled);
        for path in pending {
            self.schedule_autosave(path, cx);
        }
        cx.notify();
    }

    pub fn set_word_wrap(
        &mut self,
        word_wrap: bool,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_word_wrap(word_wrap, window, cx);
    }

    pub fn set_editor_font_size(&mut self, editor_font_size: f32, cx: &mut Context<Self>) {
        self.preview.set_editor_font_size(editor_font_size);
        cx.notify();
    }

    pub fn set_show_all_files(&mut self, show_all_files: bool, cx: &mut Context<Self>) {
        self.apply_show_all_files(show_all_files, cx);
    }

    pub fn ensure_loaded(&mut self, cx: &mut Context<Self>) {
        self.sync_target(cx);
        if self.request_context.is_none() {
            return;
        }
        self.ensure_watch(cx);
        if self.presentation.is_editor()
            && !self.preview.has_active()
            && let Some(path) = self.editor_path.clone()
        {
            self.open_file(path, cx);
        }
        if !self.presentation.is_editor() {
            self.ensure_tree_loaded(cx);
        }
    }

    fn ensure_tree_loaded(&mut self, cx: &mut Context<Self>) {
        if self.started {
            return;
        }
        self.started = true;
        self.load_directory(String::new(), None, cx);
    }

    pub fn retry_root(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        self.started = true;
        self.load_directory(String::new(), None, cx);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.presentation.is_editor() {
            return;
        }
        self.error = None;
        self.started = true;
        self.tree.invalidate_all_directories();
        let directories = std::iter::once(String::new())
            .chain(self.tree.expanded_directories())
            .collect::<Vec<_>>();
        for directory in directories {
            self.load_directory(directory, None, cx);
        }
    }

    pub fn tab_title(&self) -> SharedString {
        self.editor_path
            .as_deref()
            .and_then(|path| path.rsplit('/').next())
            .unwrap_or("Files")
            .into()
    }

    /// The file represented by this surface tab, when the browser has already
    /// promoted itself to an editor.
    pub fn attachment_path(&self) -> Option<&str> {
        self.editor_path.as_deref()
    }

    pub(super) fn open_tree_file(&mut self, path: String, cx: &mut Context<Self>) {
        cx.emit(FilesEvent::OpenFile(path));
    }

    pub(crate) fn focus_explorer(&self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = if self.explorer_page() == ExplorerPage::Agents {
            self.pane.focus.clone()
        } else if self.search_state.query.is_empty() {
            self.tree_focus.clone()
        } else {
            use gpui::Focusable;
            self.search.focus_handle(cx)
        };
        window.defer(cx, move |window, cx| focus.focus(window, cx));
    }

    /// Synchronize selection without replacing the user's current search.
    pub(crate) fn reveal_file(&mut self, path: String, cx: &mut Context<Self>) {
        if self.selected_editor_path.as_deref() == Some(&path) {
            return;
        }
        if self.request_context.is_none() || self.state.read(cx).engine().is_none() {
            return;
        }
        self.selected_editor_path = Some(path.clone());
        self.reveal_path(path, search::RevealIntent::SynchronizeSelection, cx);
    }

    pub(crate) fn reveal_file_explicit(&mut self, path: String, cx: &mut Context<Self>) {
        self.select_explorer_page(ExplorerPage::Files, cx);
        self.selected_editor_path = None;
        self.search.update(cx, |search, cx| search.set_text("", cx));
        self.reveal_file(path, cx);
    }

    pub(crate) fn is_current_target(&self, cx: &gpui::App) -> bool {
        self.request_context.is_some()
            && self.request_context
                == FilesRequestContext::for_chat(self.state.read(cx), &self.chat_id)
            && !self.target_change_pending
    }

    fn toggle_ignored(&mut self, cx: &mut Context<Self>) {
        cx.emit(FilesEvent::ShowAllFilesChanged(
            !self.tree.include_ignored(),
        ));
    }

    fn apply_show_all_files(&mut self, show_all_files: bool, cx: &mut Context<Self>) {
        if self.tree.set_include_ignored(show_all_files) {
            self.selected_editor_path = None;
            self.cancel_reveal();
            self.loads.clear();
            self.error = None;
            self.sync_tree_list();
            self.started = false;
            if !self.presentation.is_editor() {
                self.ensure_tree_loaded(cx);
            }
            if !self.search_state.query.is_empty() {
                self.search_state.query.clear();
                self.on_search_edited(cx);
            }
        }
    }

    pub fn load_directory(
        &mut self,
        directory: String,
        cursor: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(request_context) = self.request_context.clone() else {
            self.error = Some("No workspace available for this chat.".into());
            cx.notify();
            return;
        };
        let generation = self.tree.generation();
        let cached_paths = if cursor.is_none() {
            self.tree
                .node(&directory)
                .map(|node| node.children.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        if !self.tree.begin_load(&directory, cursor.clone(), generation) {
            return;
        }
        self.sync_tree_list();
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            self.tree.fail_load(
                &directory,
                cursor,
                "Workspace service is still starting.",
                generation,
            );
            self.sync_tree_list();
            cx.notify();
            return;
        };
        let key = (directory.clone(), cursor.clone());
        let request = ListWorkspaceDirectoryRequest {
            target: request_context.target.clone(),
            directory: directory.clone(),
            include_ignored: self.tree.include_ignored(),
            cursor: cursor.clone(),
        };
        let client = WorkspaceFilesClient::new(engine, request_context);
        let task = cx.spawn(async move |this, cx| {
            let mut result = client
                .list_directory_snapshot(request.clone(), &cached_paths)
                .await;
            if result.as_ref().is_err_and(|error| error.retryable()) {
                cx.background_executor()
                    .timer(Duration::from_millis(250))
                    .await;
                result = client.list_directory_snapshot(request, &cached_paths).await;
            }
            let _ = this.update(cx, |surface, cx| {
                if surface.tree.generation() != generation {
                    return;
                }
                let reload = surface.tree.node(&directory).is_some_and(|node| node.stale);
                match result {
                    Ok(page) => {
                        surface.error = None;
                        surface.tree.apply_page(page, generation);
                    }
                    Err(error) => {
                        let message = error.to_string();
                        if directory.is_empty() {
                            surface.error = Some(message.clone().into());
                        }
                        surface
                            .tree
                            .fail_load(&directory, cursor, message, generation);
                    }
                }
                surface.sync_tree_list();
                if reload {
                    surface.load_directory(directory, None, cx);
                }
                cx.notify();
            });
        });
        self.loads.insert(key, task);
        cx.notify();
    }

    pub fn tree(&self) -> &FileTreeModel {
        &self.tree
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    fn sync_target(&mut self, cx: &mut Context<Self>) -> bool {
        let next = FilesRequestContext::for_chat(self.state.read(cx), &self.chat_id);
        if self.request_context == next {
            self.target_change_pending = false;
            self.pending_request_context = None;
            return false;
        }
        if self.preview.has_unsaved_changes() {
            self.target_change_pending = true;
            self.pending_request_context = next;
            self.preview.cancel_autosaves();
            self.suspend_images(cx);
            cx.notify();
            return false;
        }
        self.apply_target(next, cx);
        true
    }

    pub(super) fn apply_pending_target(&mut self, cx: &mut Context<Self>) {
        if !self.target_change_pending {
            return;
        }
        let next = self.pending_request_context.take();
        self.target_change_pending = false;
        self.apply_target(next, cx);
        self.ensure_loaded(cx);
        cx.notify();
    }

    fn apply_target(&mut self, next: Option<FilesRequestContext>, cx: &mut Context<Self>) {
        self.release_git_status();
        self.suspend_images(cx);
        self.cancel_review_comment_flush(cx);
        self.loads.clear();
        self.watch_task = None;
        self.watch_sequence = None;
        self.watch_error = None;
        self.editor_context_menu = crate::popover::Popup::default();
        self.preview.reset();
        self.tree.reset();
        self.selected_editor_path = None;
        self.cancel_reveal();
        self.search_state.task = None;
        self.search_state.generation = self.search_state.generation.wrapping_add(1);
        self.search_state.query.clear();
        self.search_state.results.clear();
        self.search_state.loading = false;
        self.search_state.error = None;
        self.reset_search_results();
        self.sync_tree_list();
        self.error = if next.is_none() {
            Some("No workspace available for this chat.".into())
        } else {
            None
        };
        self.request_context = next;
        self.started = false;
        if !self.search.read(cx).text().trim().is_empty() {
            self.on_search_edited(cx);
        }
    }

    fn tree_has_content(&self) -> bool {
        self.tree.node("").is_some_and(|node| node.has_loaded)
    }

    fn sync_tree_list(&mut self) {
        if self.tree_list_generation != self.tree.generation() {
            self.tree_list.reset_with_uniform_height(
                self.tree.visible_rows().len(),
                px(tree::TREE_ROW_HEIGHT),
            );
            self.tree_list_generation = self.tree.generation();
        } else {
            tree::sync_list_rows(
                &self.tree_list,
                &self.tree_list_rows,
                self.tree.visible_rows(),
            );
        }
        self.tree_list_rows = self.tree.visible_rows().to_vec();
    }

    /// Escape or the mention-dismiss key: drop the filter and hand focus back
    /// to the tree.
    fn close_search(&mut self, cx: &mut Context<Self>) {
        self.clear_search(cx);
        self.search_restore_tree_focus = true;
        cx.notify();
    }

    /// The explorer's secondary header: the same toolbar band the editor
    /// carries directly under the titlebar (search field + visibility eye).
    fn render_explorer_header(
        &mut self,
        theme: &crate::theme::Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::Focusable;
        let include_ignored = self.tree.include_ignored();
        let search_focus = self.search.focus_handle(cx);
        toolbar(theme)
            .id("files-explorer-header")
            .debug_selector(|| "files-explorer-header".into())
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "escape" {
                    this.close_search(cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                crate::surface_chrome::input()
                    .id("files-search")
                    .debug_selector(|| "files-search".into())
                    .overflow_hidden()
                    .cursor_text()
                    .hover(|style| style.bg(crate::theme::ink(0.055)))
                    // Clicking the field's padding focuses the input too.
                    .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                        window.focus(&search_focus, cx);
                        cx.stop_propagation();
                    })
                    .child(
                        crate::icons::icon(crate::icons::MAGNIFER)
                            .size(px(12.0))
                            .flex_none()
                            .text_color(theme.text_faint),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .overflow_hidden()
                            .child(self.search.clone()),
                    ),
            )
            .child(
                toolbar_button(
                    "files-toggle-ignored",
                    if include_ignored {
                        "Hide hidden and ignored files"
                    } else {
                        "Show all files (even hidden)"
                    },
                )
                .debug_selector(|| "files-toggle-ignored".into())
                .when(include_ignored, |element| {
                    element.bg(crate::theme::wash(0.1))
                })
                .on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.toggle_ignored(cx);
                }))
                .child(
                    crate::icons::icon(if include_ignored {
                        crate::icons::EYE
                    } else {
                        crate::icons::EYE_CLOSED
                    })
                    .size(px(crate::surface_chrome::ICON_SIZE))
                    .text_color(if include_ignored {
                        theme.text
                    } else {
                        theme.text_muted
                    }),
                ),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod explorer_tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use std::{cell::RefCell, rc::Rc};

    #[gpui::test]
    fn explorer_header_search_and_visibility_toggle(cx: &mut TestAppContext) {
        use gpui::{Focusable, Modifiers};

        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(crate::theme::Theme::default());
        });
        let (files, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            FilesSurface::new_explorer(state, "chat".into(), false, cx)
        });
        cx.update(|window, _| window.activate_window());
        let show_all = Rc::new(RefCell::new(None));
        let emitted = show_all.clone();
        let _events = cx.update(|_, cx| {
            cx.subscribe(&files, move |_, event, _| {
                if let FilesEvent::ShowAllFilesChanged(value) = event {
                    *emitted.borrow_mut() = Some(*value);
                }
            })
        });
        cx.update(|window, cx| window.draw(cx).clear());
        // The explorer header is the same band as the editor header.
        let header = cx.debug_bounds("files-explorer-header").unwrap();
        assert_eq!(header.size.height, px(crate::surface_chrome::HEADER_HEIGHT));
        let bounds = cx.debug_bounds("files-search").unwrap();
        assert!(bounds.top() >= header.top() && bounds.bottom() <= header.bottom());
        // Click the field padding, not just the input's text hitbox.
        let padding = gpui::point(bounds.left() + px(2.0), bounds.center().y);
        cx.simulate_click(padding, Modifiers::default());
        cx.simulate_input("a very long filename\nwith another line.rs");
        files.read_with(cx, |files, cx| {
            assert_eq!(
                files.search.read(cx).text(),
                "a very long filename with another line.rs"
            );
        });
        cx.update(|window, cx| {
            assert!(files.read(cx).search.focus_handle(cx).is_focused(window));
        });
        let ignored = cx.debug_bounds("files-toggle-ignored").unwrap().center();
        cx.simulate_click(ignored, Modifiers::default());
        assert_eq!(*show_all.borrow(), Some(true));

        // Escape clears the filter and hands focus back to the tree.
        cx.update(|window, cx| window.focus(&files.read(cx).search.focus_handle(cx), cx));
        cx.simulate_keystrokes("escape");
        files.read_with(cx, |files, cx| {
            assert!(files.search.read(cx).text().is_empty());
        });
        // The keystroke's redraw consumes the restore flag and moves focus.
        cx.update(|window, cx| window.draw(cx).clear());
        cx.update(|window, cx| {
            assert!(files.read(cx).tree_focus.is_focused(window));
        });
    }

    #[gpui::test]
    fn explorer_open_delegates_without_becoming_an_editor(cx: &mut TestAppContext) {
        let surface = cx.new(|cx| {
            let state = cx.new(|_| AppState::new());
            FilesSurface::new_explorer(state, "chat".into(), false, cx)
        });
        let paths = Rc::new(RefCell::new(Vec::new()));
        let emitted = paths.clone();
        let _sub = cx.update(|cx| {
            cx.subscribe(&surface, move |_, event, _| {
                if let FilesEvent::OpenFile(path) = event {
                    emitted.borrow_mut().push(path.clone());
                }
            })
        });
        surface.update(cx, |surface, cx| {
            surface.open_tree_file("src/main.rs".into(), cx);
            surface.open_tree_file("README.md".into(), cx);
            assert_eq!(surface.presentation, FilesPresentation::Explorer);
            assert!(surface.editor_path.is_none());
            assert!(!surface.preview.has_active());
            assert!(!surface.preview.has_unsaved_changes());
        });
        assert_eq!(*paths.borrow(), ["src/main.rs", "README.md"]);
    }
}
