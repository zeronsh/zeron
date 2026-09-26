//! Serializes application operations and keeps their task owner alive even
//! when its native window closes. Dirty-file decisions remain in each view.

use gpui::{App, Entity, EntityId, Global, SharedString};

use crate::shell::{Shell, SyncFlow, UpdateFlow};

#[derive(Clone, PartialEq)]
pub(crate) struct Progress {
    pub sync: SyncFlow,
    pub update: UpdateFlow,
    pub error: Option<SharedString>,
    pub current: Option<SharedString>,
    pub busy: bool,
    pub replacing: bool,
}

pub(crate) enum Action {
    Quit,
    Local(bool),
    Synced(bool),
    RuntimeQuit,
    Install(std::path::PathBuf),
}

#[derive(Default)]
struct Operations {
    owner: Option<EntityId>,
    lease: Option<Entity<Shell>>,
    progress: Option<Progress>,
    pending: Option<(Action, Option<Entity<Shell>>)>,
}

impl Global for Operations {}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(Operations::default());
}
pub(crate) fn installed(cx: &App) -> bool {
    cx.has_global::<Operations>()
}

pub(crate) fn claim(owner: Entity<Shell>, progress: Progress, cx: &mut App) -> bool {
    if !installed(cx) {
        return true;
    }
    let operations = cx.global_mut::<Operations>();
    if operations.owner != Some(owner.entity_id())
        && operations.progress.as_ref().is_some_and(|p| p.busy)
    {
        return false;
    }
    operations.owner = Some(owner.entity_id());
    operations.lease = progress.busy.then_some(owner);
    operations.progress = Some(progress);
    true
}

pub(crate) fn active_owner(except: EntityId, cx: &App) -> Option<Entity<Shell>> {
    let owner = cx.try_global::<Operations>()?.lease.as_ref()?;
    (owner.entity_id() != except).then(|| owner.clone())
}

pub(crate) fn is_owner(id: EntityId, cx: &App) -> bool {
    cx.try_global::<Operations>()
        .is_none_or(|operations| operations.owner.is_none_or(|owner| owner == id))
}

pub(crate) fn progress(id: EntityId, cx: &App) -> Option<Progress> {
    let operations = cx.try_global::<Operations>()?;
    (operations.owner != Some(id))
        .then(|| operations.progress.clone())
        .flatten()
}

pub(crate) fn publish(owner: Entity<Shell>, progress: Progress, cx: &mut App) {
    if !installed(cx) {
        return;
    }
    let operations = cx.global_mut::<Operations>();
    if operations.progress.as_ref() == Some(&progress) {
        return;
    }
    if operations.owner != Some(owner.entity_id()) {
        if operations.progress.as_ref().is_some_and(|p| p.busy) {
            return;
        }
        operations.owner = Some(owner.entity_id());
    }
    operations.lease = progress.busy.then_some(owner);
    let replacing = progress.replacing;
    operations.progress = Some(progress);
    cx.defer(move |cx| lock_views(replacing, cx));
    cx.refresh_windows();
}

pub(crate) fn replacing(cx: &App) -> bool {
    cx.try_global::<Operations>()
        .is_some_and(|operations| operations.progress.as_ref().is_some_and(|p| p.replacing))
}

pub(crate) fn blocks_commands(cx: &App) -> bool {
    cx.try_global::<Operations>().is_some_and(|operations| {
        operations.pending.is_some() || operations.progress.as_ref().is_some_and(|p| p.replacing)
    })
}

/// Called from actions while a Shell may be borrowed; inspection is deferred.
pub(crate) fn request(action: Action, owner: Option<Entity<Shell>>, cx: &mut App) {
    if !installed(cx) {
        init(cx);
    }
    cx.defer(move |cx| {
        let operations = cx.global_mut::<Operations>();
        if operations.pending.is_some() {
            return;
        }
        if !matches!(action, Action::Quit) && operations.progress.as_ref().is_some_and(|p| p.busy) {
            return;
        }
        operations.pending = Some((action, owner));
        check(cx);
    });
}

pub(crate) fn cancel(cx: &mut App) {
    if installed(cx) {
        cx.global_mut::<Operations>().pending = None;
    }
}

pub(crate) fn resume(cx: &mut App) {
    cx.defer(check);
}

fn check(cx: &mut App) {
    if cx
        .try_global::<Operations>()
        .is_none_or(|o| o.pending.is_none())
    {
        return;
    }
    let mut views = crate::window_manager::views(cx);
    // The initiating native window may have closed while another view saved.
    if let Some(owner) = cx
        .global::<Operations>()
        .pending
        .as_ref()
        .and_then(|(_, owner)| owner.clone())
        && !views.contains(&owner)
    {
        views.push(owner);
    }
    let mut ready = true;
    for view in views {
        ready &= view.update(cx, |shell, cx| shell.prepare_application_operation(cx));
    }
    if !ready {
        return;
    }
    let Some((action, owner)) = cx.global_mut::<Operations>().pending.take() else {
        return;
    };
    lock_views(true, cx);
    match action {
        Action::Quit => crate::app_menus::finish_quit(cx),
        action => {
            if let Some(owner) = owner {
                owner.update(cx, |shell, cx| {
                    shell.execute_application_operation(action, cx)
                });
            }
        }
    }
}

fn lock_views(locked: bool, cx: &mut App) {
    for view in crate::window_manager::views(cx) {
        view.update(cx, |shell, cx| shell.lock_application_editors(locked, cx));
    }
}
