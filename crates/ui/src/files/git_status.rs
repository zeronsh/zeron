//! Shared metadata-only Git subscription. Views hold leases; the last lease
//! dropping cancels the RPC. Cached sources never own their consuming views.
use std::{collections::HashMap, time::Duration};

use gpui::{App, Context, Entity, Global, Task, WeakEntity, prelude::*};
use zeron_proto::{CheckoutGitStatus, GitFileState, GitFileStatus, WatchWorkspaceFilesRequest};
use zeron_rpc::{RpcError, methods};

use super::{
    FilesSurface,
    client::{FilesRequestContext, request_params},
    model::parent_path,
};
use crate::{state::EngineHandle, theme::Theme};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum DecorationKind {
    Untracked,
    Added,
    Modified,
    Renamed,
    Deleted,
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Decoration {
    kind: DecorationKind,
}

impl Decoration {
    pub(super) fn color(self, theme: &Theme) -> gpui::Hsla {
        match self.kind {
            DecorationKind::Untracked | DecorationKind::Added => theme.success,
            DecorationKind::Modified => theme.warning,
            DecorationKind::Renamed => theme.accent,
            DecorationKind::Deleted | DecorationKind::Conflict => theme.danger,
        }
    }
}

fn decoration(file: &GitFileStatus) -> Decoration {
    use GitFileState::*;
    let states = [file.index, file.worktree];
    let conflict = states.contains(&Unmerged)
        || matches!(
            (file.index, file.worktree),
            (Added, Added) | (Deleted, Deleted)
        );
    let kind = if conflict {
        DecorationKind::Conflict
    } else if states.contains(&Deleted) && states.contains(&Untracked) {
        DecorationKind::Modified
    } else if states.contains(&Deleted) {
        DecorationKind::Deleted
    } else if states.iter().any(|s| matches!(s, Renamed | Copied)) {
        DecorationKind::Renamed
    } else if states.iter().any(|s| matches!(s, Modified | TypeChanged)) {
        DecorationKind::Modified
    } else if states.contains(&Untracked) {
        DecorationKind::Untracked
    } else {
        DecorationKind::Added
    };
    Decoration { kind }
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && path
            .split('/')
            .all(|p| !p.is_empty() && p != "." && p != "..")
}

#[derive(Default, PartialEq, Eq)]
struct Decorations {
    files: HashMap<String, Decoration>,
    directories: HashMap<String, Decoration>,
}

impl Decorations {
    fn from_snapshot(snapshot: &CheckoutGitStatus) -> Self {
        let mut result = Self::default();
        if !snapshot.complete {
            return result;
        }
        for file in &snapshot.files {
            if !valid_path(&file.path) {
                continue;
            }
            let value = decoration(file);
            result.files.insert(file.path.clone(), value);
            for path in std::iter::once(file.path.as_str()).chain(file.old_path.as_deref()) {
                if !valid_path(path) {
                    continue;
                }
                let mut parent = parent_path(path);
                while let Some(path) = parent {
                    let entry = result.directories.entry(path.clone()).or_insert(value);
                    entry.kind = entry.kind.max(value.kind);
                    parent = parent_path(&path);
                }
            }
        }
        result
    }
}

#[derive(Default)]
struct StatusCache(Vec<WeakEntity<GitStatusSource>>);
impl Global for StatusCache {}

pub(super) struct GitStatusSource {
    engine: EngineHandle,
    context: FilesRequestContext,
    device_id: String,
    decorations: Decorations,
    revision: Option<String>,
    notice: Option<&'static str>,
    _task: Task<()>,
}

impl GitStatusSource {
    fn matches(&self, engine: &EngineHandle, context: &FilesRequestContext, device: &str) -> bool {
        self.engine.same_connection(engine)
            && self.device_id == device
            && self.context.target_device_id == context.target_device_id
            && match (&self.context.checkout_id, &context.checkout_id) {
                (Some(a), Some(b)) => a == b,
                (None, None) => self.context.cwd == context.cwd,
                _ => false,
            }
    }

    fn apply(
        &mut self,
        snapshot: Option<CheckoutGitStatus>,
        mut decorations: Decorations,
        cx: &mut Context<Self>,
    ) {
        let snapshot = snapshot.filter(|s| accepts(&self.context, &self.device_id, s));
        if snapshot.is_none() {
            decorations = Decorations::default();
        }
        let revision = snapshot.as_ref().map(|s| s.revision.clone());
        let notice = match &snapshot {
            Some(snapshot) if !snapshot.complete => Some("Git status unavailable or incomplete"),
            Some(_) => None,
            None if self.revision.is_some() || self.notice.is_some() => {
                Some("Git status unavailable")
            }
            None => None,
        };
        if revision == self.revision && notice == self.notice {
            return;
        }
        self.revision = revision;
        if self.decorations != decorations || self.notice != notice {
            self.decorations = decorations;
            self.notice = notice;
            cx.notify();
        }
    }

    fn acquire(
        engine: EngineHandle,
        context: FilesRequestContext,
        device: String,
        cx: &mut App,
    ) -> Entity<Self> {
        if !cx.has_global::<StatusCache>() {
            cx.set_global(StatusCache::default());
        }
        let cache = cx.global_mut::<StatusCache>();
        cache.0.retain(|source| source.upgrade().is_some());
        let candidates = cache.0.clone();
        for source in candidates.into_iter().filter_map(|source| source.upgrade()) {
            if source.read(cx).matches(&engine, &context, &device) {
                return source;
            }
        }
        let source = cx.new(|cx: &mut Context<Self>| {
            let connection = engine.clone();
            let params = request_params(
                &WatchWorkspaceFilesRequest {
                    target: context.target.clone(),
                },
                context.target_device_id.as_deref(),
            )
            .expect("Git status request is serializable");
            let task = cx.spawn(async move |this, cx| {
                loop {
                    match connection
                        .client()
                        .subscribe_checked(methods::WATCH_WORKSPACE_GIT_STATUS, params.clone())
                        .await
                    {
                        Ok(mut receiver) => {
                            while let Some(value) = receiver.recv().await {
                                let (snapshot, decorations) = cx
                                    .background_executor()
                                    .spawn(async move {
                                        let snapshot =
                                            serde_json::from_value::<
                                                zeron_proto::WorkspaceGitStatusFrame,
                                            >(value)
                                            .ok()
                                            .and_then(|frame| frame.status);
                                        let decorations = snapshot
                                            .as_ref()
                                            .map(Decorations::from_snapshot)
                                            .unwrap_or_default();
                                        (snapshot, decorations)
                                    })
                                    .await;
                                if this
                                    .update(cx, |source, cx| {
                                        source.apply(snapshot, decorations, cx)
                                    })
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                        // Older peers still browse normally, without decorations.
                        Err(error)
                            if matches!(error, RpcError::UnknownMethod(_))
                                || error.to_string().starts_with("unknown method:") =>
                        {
                            return;
                        }
                        Err(_) => {}
                    }
                    if this
                        .update(cx, |source, cx| {
                            source.apply(None, Decorations::default(), cx);
                            if source.notice.is_none() {
                                source.notice = Some("Git status unavailable");
                                cx.notify();
                            }
                        })
                        .is_err()
                    {
                        return;
                    }
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                }
            });
            Self {
                engine,
                context,
                device_id: device,
                decorations: Decorations::default(),
                revision: None,
                notice: None,
                _task: task,
            }
        });
        cx.global_mut::<StatusCache>().0.push(source.downgrade());
        source
    }
}

impl FilesSurface {
    #[cfg(test)]
    pub(crate) fn test_git_source(&self) -> Option<gpui::EntityId> {
        self.git_status.as_ref().map(Entity::entity_id)
    }
    pub(super) fn git_status_notice(&self, cx: &App) -> Option<&'static str> {
        self.git_status.as_ref()?.read(cx).notice
    }

    #[cfg(test)]
    pub(crate) fn test_git_color(
        &self,
        path: &str,
        directory: bool,
        cx: &App,
    ) -> Option<gpui::Hsla> {
        self.git_decoration(path, directory, cx)
            .map(|decoration| decoration.color(Theme::of(cx)))
    }
    pub(crate) fn release_git_status(&mut self) {
        self.git_status_subscription = None;
        self.git_status = None;
    }

    pub(crate) fn ensure_git_status(&mut self, cx: &mut Context<Self>) {
        if self.presentation.is_editor() {
            return;
        }
        let Some(mut context) = self.request_context.clone() else {
            return;
        };
        let state = self.state.read(cx);
        let Some(engine) = state.engine().cloned() else {
            let had_source = self.git_status.is_some();
            self.release_git_status();
            if had_source {
                cx.notify();
            }
            return;
        };
        let Some(chat) = state.chats.iter().find(|chat| chat.id == self.chat_id) else {
            return;
        };
        let device = chat.device_id.clone();
        // A shared source must survive the first consuming chat disappearing.
        if let Some(space) = &chat.space_id {
            context.target = zeron_proto::WorkspaceTarget {
                chat_id: None,
                space_id: Some(space.clone()),
                checkout_path: Some(context.cwd.clone()),
            };
        }
        if self
            .git_status
            .as_ref()
            .is_some_and(|source| source.read(cx).matches(&engine, &context, &device))
        {
            return;
        }
        self.release_git_status();
        let source = GitStatusSource::acquire(engine, context, device, cx);
        self.git_status_subscription = Some(cx.observe(&source, |_, _, cx| cx.notify()));
        self.git_status = Some(source);
        cx.notify();
    }

    pub(super) fn git_decoration(
        &self,
        path: &str,
        directory: bool,
        cx: &App,
    ) -> Option<Decoration> {
        let source = self.git_status.as_ref()?.read(cx);
        if directory {
            source.decorations.directories.get(path).copied()
        } else {
            source.decorations.files.get(path).copied()
        }
    }
}

fn accepts(context: &FilesRequestContext, device: &str, snapshot: &CheckoutGitStatus) -> bool {
    snapshot.device_id == device
        && context
            .checkout_id
            .as_ref()
            .is_none_or(|id| *id == snapshot.checkout_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::GitFileState::*;

    fn file(path: &str, index: GitFileState, worktree: GitFileState) -> GitFileStatus {
        GitFileStatus {
            path: path.into(),
            old_path: None,
            index,
            worktree,
        }
    }
    fn snapshot(files: Vec<GitFileStatus>) -> CheckoutGitStatus {
        CheckoutGitStatus {
            device_id: "remote".into(),
            checkout_id: "checkout".into(),
            revision: "1".into(),
            complete: true,
            files,
        }
    }

    #[test]
    fn ancestors_include_deleted_and_renamed_paths_without_creating_tree_rows() {
        let mut renamed = file("new/file.rs", Renamed, Unchanged);
        renamed.old_path = Some("old/file.rs".into());
        let data = Decorations::from_snapshot(&snapshot(vec![
            file("src/nested/a.rs", Unchanged, Modified),
            file("src/deleted.rs", Deleted, Unchanged),
            file("src/conflict.rs", Added, Added),
            renamed,
        ]));
        assert_eq!(data.directories["src"].kind, DecorationKind::Conflict);
        assert_eq!(
            data.directories["src/nested"].kind,
            DecorationKind::Modified
        );
        assert!(data.directories.contains_key("old"));
        assert!(data.directories.contains_key("new"));
        assert!(!data.files.contains_key("old/file.rs"));
        assert!(!data.directories.contains_key("sr"));
    }

    #[test]
    fn partial_results_are_unknown_and_paths_are_literal() {
        let mut status = snapshot(vec![
            file("new/ leading\nname", Untracked, Untracked),
            file("../outside", Modified, Unchanged),
        ]);
        let data = Decorations::from_snapshot(&status);
        assert_eq!(data.files.len(), 1);
        assert_eq!(
            data.files["new/ leading\nname"].kind,
            DecorationKind::Untracked
        );
        status.complete = false;
        assert!(Decorations::from_snapshot(&status).files.is_empty());
    }

    #[test]
    fn remote_identity_and_checkout_must_both_match() {
        let context = FilesRequestContext {
            target: zeron_proto::WorkspaceTarget {
                chat_id: Some("chat".into()),
                space_id: None,
                checkout_path: None,
            },
            target_device_id: Some("remote".into()),
            cwd: "/same/path".into(),
            checkout_id: Some("checkout".into()),
        };
        let mut status = snapshot(vec![]);
        assert!(accepts(&context, "remote", &status));
        status.device_id = "local".into();
        assert!(!accepts(&context, "remote", &status));
        status.device_id = "remote".into();
        status.checkout_id = "old-checkout".into();
        assert!(!accepts(&context, "remote", &status));
    }

    #[test]
    fn decorations_color_staged_and_unstaged_using_theme_tokens() {
        let value = decoration(&file("a", Modified, Modified));
        let mut theme = Theme::default();
        assert_eq!(value.color(&theme), theme.warning);
        assert_eq!(
            decoration(&file("a", Modified, Unchanged)).color(&theme),
            theme.warning
        );
        assert_eq!(
            decoration(&file("a", Unchanged, Modified)).color(&theme),
            theme.warning
        );
        theme.warning = gpui::hsla(0.6, 0.8, 0.4, 1.0);
        assert_eq!(value.color(&theme), theme.warning);
        assert_eq!(
            decoration(&file("new", Added, Unchanged)).color(&theme),
            theme.success
        );
        assert_eq!(
            decoration(&file("conflict", Deleted, Deleted)).color(&theme),
            theme.danger
        );
    }

    #[test]
    fn large_snapshot_aggregates_only_real_ancestors() {
        let status = snapshot(
            (0..10_000)
                .map(|i| {
                    file(
                        &format!("src/group{}/file{i}.rs", i % 100),
                        Unchanged,
                        Modified,
                    )
                })
                .collect(),
        );
        let wire_bytes = serde_json::to_vec(&status).unwrap().len();
        let start = std::time::Instant::now();
        let data = Decorations::from_snapshot(&status);
        eprintln!(
            "Git status: 10000 files, {wire_bytes} wire bytes, aggregation {:?}",
            start.elapsed()
        );
        assert_eq!(data.files.len(), 10_000);
        assert_eq!(data.directories.len(), 102); // root + src + 100 groups
        assert!(
            data.directories
                .values()
                .all(|d| d.kind == DecorationKind::Modified)
        );
        assert!(!data.directories.contains_key("src/group"));
    }
}
