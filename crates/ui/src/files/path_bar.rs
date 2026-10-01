//! The editor header's editable path: click the breadcrumb to swap it for a
//! text field holding the file's full path. Enter or leaving the field opens
//! whatever the text names; Escape puts the breadcrumb back.

use gpui::{Context, Focusable, SharedString, Window, div, prelude::*, px};
use zeron_proto::{ListWorkspaceDirectoryRequest, WorkspaceEntryKind};

use super::{FilesEvent, FilesSurface, client::WorkspaceFilesClient, model::parent_path};
use crate::theme::Theme;

/// Directory pages walked while looking for the typed entry before giving up.
const MAX_LOOKUP_PAGES: usize = 40;

/// The path field's transient state. Present only while the field is showing.
pub(super) struct PathEdit {
    /// Why the last commit was refused; cleared by the next keystroke.
    pub error: Option<SharedString>,
    /// A lookup is in flight; blur must not commit a second time.
    pub checking: bool,
    /// The field has held focus at least once. Focus lands a frame after the
    /// edit begins, so blur is only meaningful after this flips.
    pub focused_once: bool,
}

/// Where a path is anchored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Anchor {
    Root,
    Home,
    Relative,
}

fn split(path: &str) -> (Anchor, Vec<&str>) {
    let (anchor, rest) = if let Some(rest) = path.strip_prefix('/') {
        (Anchor::Root, rest)
    } else if path == "~" {
        (Anchor::Home, "")
    } else if let Some(rest) = path.strip_prefix("~/") {
        (Anchor::Home, rest)
    } else {
        (Anchor::Relative, path)
    };
    let parts = rest
        .split('/')
        .filter(|part| !part.is_empty() && *part != ".")
        .collect();
    (anchor, parts)
}

/// Fold `..` segments; `None` when the path climbs above its anchor.
fn normalize(parts: Vec<&str>) -> Option<Vec<&str>> {
    let mut out: Vec<&str> = Vec::with_capacity(parts.len());
    for part in parts {
        if part == ".." {
            out.pop()?;
        } else {
            out.push(part);
        }
    }
    Some(out)
}

/// The full path shown in the field: workspace paths join the working
/// directory, outside paths are already absolute.
pub(super) fn display_path(cwd: &str, path: &str) -> String {
    if path.starts_with('/') {
        return path.to_string();
    }
    let cwd = cwd.trim_end_matches('/');
    format!("{cwd}/{path}")
}

/// What typed text names, as a wire path: workspace-relative when it lands
/// under `cwd`, absolute (leading `/`) when it lands elsewhere. `None` for
/// empty text, the workspace root itself, or anything that cannot be a file
/// path (climbing out of the filesystem, a `~` that `cwd` cannot resolve).
pub(super) fn resolve_input(cwd: &str, input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let (root_anchor, root_parts) = split(cwd);
    let root_parts = normalize(root_parts)?;
    let (anchor, parts) = split(input);
    let (anchor, parts) = match anchor {
        Anchor::Relative => (
            root_anchor,
            normalize(root_parts.iter().copied().chain(parts).collect())?,
        ),
        other => (other, normalize(parts)?),
    };
    if anchor == root_anchor && parts.len() > root_parts.len() && parts.starts_with(&root_parts) {
        return Some(parts[root_parts.len()..].join("/"));
    }
    if anchor == Anchor::Root && !parts.is_empty() && parts != root_parts {
        return Some(format!("/{}", parts.join("/")));
    }
    None
}

enum Lookup {
    File,
    Directory,
    Missing,
}

impl FilesSurface {
    /// Swap the breadcrumb for the field, prefilled with the full path and
    /// fully selected so it can be copied or overtyped.
    pub(super) fn begin_path_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.path_edit.is_some() {
            return;
        }
        let (Some(path), Some(context), Some(input)) = (
            self.preview.active.clone(),
            self.request_context.as_ref(),
            self.path_input.clone(),
        ) else {
            return;
        };
        let text = display_path(&context.cwd, &path);
        input.update(cx, |input, cx| {
            input.set_text(text, cx);
            input.select_all_text(cx);
        });
        self.path_edit = Some(PathEdit {
            error: None,
            checking: false,
            focused_once: false,
        });
        let focus = input.focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    /// Drop the field. `refocus` returns the keyboard to the editor (Escape);
    /// a blur or an open leaves focus where the user sent it.
    pub(super) fn end_path_edit(
        &mut self,
        refocus: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.path_edit.take().is_none() {
            return;
        }
        self.path_check = None;
        if refocus {
            self.focus_editor(window, cx);
        }
        cx.notify();
    }

    pub(super) fn on_path_input_edited(&mut self, cx: &mut Context<Self>) {
        if let Some(edit) = self.path_edit.as_mut()
            && edit.error.take().is_some()
        {
            cx.notify();
        }
    }

    /// Render-time blur watch: the field is a plain input with no blur
    /// event, so the header notes when focus has left it.
    pub(super) fn poll_path_blur(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(edit), Some(input)) = (self.path_edit.as_mut(), self.path_input.as_ref()) else {
            return;
        };
        if input.focus_handle(cx).is_focused(window) {
            edit.focused_once = true;
            return;
        }
        if !edit.focused_once || edit.checking {
            return;
        }
        let refused = edit.error.is_some();
        cx.defer_in(window, move |this, window, cx| {
            // A refused path is not retried on blur: leaving discards it.
            if refused {
                this.end_path_edit(false, window, cx);
            } else {
                this.commit_path_edit(window, cx);
            }
        });
    }

    pub(super) fn commit_path_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(edit), Some(input), Some(context), Some(active)) = (
            self.path_edit.as_mut(),
            self.path_input.as_ref(),
            self.request_context.clone(),
            self.preview.active.clone(),
        ) else {
            return;
        };
        if edit.checking {
            return;
        }
        let text = input.read(cx).text().to_string();
        let unchanged = text.trim() == display_path(&context.cwd, &active);
        if unchanged || text.trim().is_empty() {
            self.end_path_edit(true, window, cx);
            return;
        }
        let Some(target) = resolve_input(&context.cwd, &text) else {
            edit.error = Some("Not a file path in this workspace".into());
            cx.notify();
            return;
        };
        if target == active {
            self.end_path_edit(true, window, cx);
            return;
        }
        // Absolute paths beyond the workspace have no directory listing to
        // consult; the file surface reports a missing one itself.
        if super::path_is_outside(&target) {
            self.finish_path_edit(target, Lookup::File, window, cx);
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            edit.error = Some("Workspace service is still starting".into());
            cx.notify();
            return;
        };
        edit.checking = true;
        cx.notify();
        let client = WorkspaceFilesClient::new(engine, context.clone());
        let include_ignored = self.tree.include_ignored();
        self.path_check = Some(cx.spawn_in(window, async move |this, cx| {
            let outcome = lookup_entry(&client, &context, &target, include_ignored).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.request_context.as_ref() != Some(&context) {
                    return;
                }
                match outcome {
                    Ok(found) => this.finish_path_edit(target, found, window, cx),
                    Err(message) => this.refuse_path_edit(message, cx),
                }
            });
        }));
    }

    fn refuse_path_edit(&mut self, message: SharedString, cx: &mut Context<Self>) {
        self.path_check = None;
        if let Some(edit) = self.path_edit.as_mut() {
            edit.checking = false;
            edit.error = Some(message);
        }
        cx.notify();
    }

    fn finish_path_edit(
        &mut self,
        target: String,
        found: Lookup,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match found {
            Lookup::Missing => self.refuse_path_edit("No such file".into(), cx),
            Lookup::Directory => {
                self.end_path_edit(false, window, cx);
                cx.emit(FilesEvent::RevealFile(target));
            }
            Lookup::File => {
                self.end_path_edit(false, window, cx);
                cx.emit(FilesEvent::OpenFile(target));
            }
        }
    }

    /// The field that replaces the breadcrumb while editing.
    pub(super) fn render_path_field(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let input = self.path_input.clone()?;
        let error = self.path_edit.as_ref()?.error.clone();
        let focus = input.focus_handle(cx);
        Some(
            crate::surface_chrome::input()
                .id("files-path-field")
                .debug_selector(|| "files-path-field".into())
                .min_w_0()
                .flex_1()
                .overflow_hidden()
                .cursor_text()
                .when(error.is_some(), |field| {
                    field.border_color(theme.danger.opacity(0.6))
                })
                .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                    window.focus(&focus, cx);
                    cx.stop_propagation();
                })
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.end_path_edit(true, window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(div().min_w_0().flex_1().overflow_hidden().child(input))
                .when_some(error, |field, message| {
                    field.child(
                        div()
                            .flex_none()
                            .max_w(gpui::relative(0.45))
                            .truncate()
                            .font_family(theme.font_sans.clone())
                            .text_size(px(10.5))
                            .text_color(theme.danger)
                            .child(message),
                    )
                })
                .into_any_element(),
        )
    }
}

/// Find `target` by paging its parent's listing; cheaper than reading the
/// file and tells files from folders.
async fn lookup_entry(
    client: &WorkspaceFilesClient,
    context: &super::client::FilesRequestContext,
    target: &str,
    include_ignored: bool,
) -> Result<Lookup, SharedString> {
    let directory = parent_path(target).unwrap_or_default();
    let mut cursor = None;
    for _ in 0..MAX_LOOKUP_PAGES {
        let page = client
            .list_directory(ListWorkspaceDirectoryRequest {
                target: context.target.clone(),
                directory: directory.clone(),
                include_ignored,
                cursor: cursor.take(),
            })
            .await
            .map_err(|error| SharedString::from(error.to_string()))?;
        if let Some(entry) = page.entries.iter().find(|entry| entry.path == target) {
            return Ok(match entry.kind {
                WorkspaceEntryKind::Directory => Lookup::Directory,
                WorkspaceEntryKind::File | WorkspaceEntryKind::Symlink => Lookup::File,
            });
        }
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(Lookup::Missing),
        }
    }
    Ok(Lookup::Missing)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_joins_the_working_directory() {
        assert_eq!(display_path("/w/app", "src/main.rs"), "/w/app/src/main.rs");
        assert_eq!(display_path("/w/app/", "a.rs"), "/w/app/a.rs");
        assert_eq!(display_path("~", "notes.md"), "~/notes.md");
        assert_eq!(display_path("/w/app", "/etc/hosts"), "/etc/hosts");
    }

    #[test]
    fn display_and_resolve_round_trip() {
        for (cwd, path) in [
            ("/w/app", "src/main.rs"),
            ("~", "notes.md"),
            ("/w/app", "/etc/hosts"),
        ] {
            assert_eq!(
                resolve_input(cwd, &display_path(cwd, path)).as_deref(),
                Some(path)
            );
        }
    }

    #[test]
    fn relative_input_resolves_against_the_working_directory() {
        assert_eq!(
            resolve_input("/w/app", "src/lib.rs").as_deref(),
            Some("src/lib.rs")
        );
        assert_eq!(
            resolve_input("/w/app", "./src//lib.rs").as_deref(),
            Some("src/lib.rs")
        );
        assert_eq!(
            resolve_input("/w/app", "src/../Cargo.toml").as_deref(),
            Some("Cargo.toml")
        );
    }

    #[test]
    fn absolute_input_inside_the_workspace_becomes_relative() {
        assert_eq!(
            resolve_input("/w/app", "/w/app/src/lib.rs").as_deref(),
            Some("src/lib.rs")
        );
        assert_eq!(
            resolve_input("/w/app", "  /w/app/x/../y.rs \n").as_deref(),
            Some("y.rs")
        );
        assert_eq!(
            resolve_input("~", "~/docs/a.md").as_deref(),
            Some("docs/a.md")
        );
    }

    #[test]
    fn paths_beyond_the_workspace_stay_absolute() {
        assert_eq!(
            resolve_input("/w/app", "/etc/hosts").as_deref(),
            Some("/etc/hosts")
        );
        assert_eq!(
            resolve_input("/w/app", "../other/a.rs").as_deref(),
            Some("/w/other/a.rs")
        );
        // A sibling that merely shares a name prefix is not inside.
        assert_eq!(
            resolve_input("/w/app", "/w/app2/a.rs").as_deref(),
            Some("/w/app2/a.rs")
        );
    }

    #[test]
    fn unresolvable_input_is_refused() {
        assert_eq!(resolve_input("/w/app", ""), None);
        assert_eq!(resolve_input("/w/app", "   "), None);
        assert_eq!(resolve_input("/w/app", "/w/app"), None);
        assert_eq!(resolve_input("/w/app", "/.."), None);
        assert_eq!(resolve_input("/w/app", "/../.."), None);
        // `~` means nothing when the workspace is not under it.
        assert_eq!(resolve_input("/w/app", "~/a.rs"), None);
    }
}
