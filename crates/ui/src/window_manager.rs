//! Native windows are independent views of the application runtime.

use gpui::{App, AppContext, Entity, Global, WindowHandle, WindowId};

use crate::{app_runtime::AppRuntime, shell::Shell, state::AppState};

#[derive(Default)]
pub(crate) struct Windows {
    /// Oldest first; handles do not keep closed roots alive.
    recent: Vec<WindowHandle<Shell>>,
    keys: std::collections::HashMap<WindowId, String>,
}

impl Global for Windows {}

pub(crate) enum Open {
    Restore,
    Blank,
    Chat(String),
}

pub(crate) fn init(cx: &mut App) {
    cx.set_global(Windows::default());
    // Secondary window identities are session-local; only the main window is
    // restored on a cold launch. Do not accumulate layouts from closed views.
    crate::settings::update(crate::settings::SavePolicy::Debounced, cx, |settings| {
        settings.windows.retain(|key, _| key == "main");
    });
    cx.on_window_closed(|cx, id| {
        cx.global_mut::<Windows>()
            .recent
            .retain(|w| w.window_id() != id);
        if let Some(key) = cx.global_mut::<Windows>().keys.remove(&id)
            && key != "main"
        {
            crate::settings::update(crate::settings::SavePolicy::Debounced, cx, |settings| {
                settings.windows.remove(&key);
            });
        }
        crate::lifecycle::resume(cx);
    })
    .detach();
}

pub(crate) fn activated(id: WindowId, cx: &mut App) {
    if !cx.has_global::<Windows>() {
        return;
    }
    let windows = cx.global_mut::<Windows>();
    if let Some(index) = windows.recent.iter().position(|w| w.window_id() == id) {
        let window = windows.recent.remove(index);
        windows.recent.push(window);
    }
}

pub(crate) fn recent(cx: &App) -> Option<WindowHandle<Shell>> {
    cx.try_global::<Windows>()?.recent.last().copied()
}

pub(crate) fn open(kind: Open, cx: &mut App) -> Option<WindowHandle<Shell>> {
    let runtime = cx.try_global::<AppRuntime>()?;
    let owner = runtime.state.clone();
    let boot = runtime.boot.clone();
    let context = recent(cx).and_then(|window| {
        window
            .update(cx, |shell, window, cx| {
                let state = shell.state.read(cx);
                let settings = crate::settings::windows::current(state.window_key.as_deref(), cx);
                let mut layout = crate::settings::windows::WindowSettings::from_ui(&settings);
                layout.last_space_id = state.selected_space.clone();
                let mut geometry =
                    crate::settings::windows::WindowGeometry::capture(window.window_bounds());
                geometry.x += 28.;
                geometry.y += 28.;
                geometry.maximized = false;
                layout.geometry = Some(geometry);
                (
                    state.selected_space.clone(),
                    state.no_project,
                    state.selected_device.clone(),
                    layout,
                )
            })
            .ok()
    });
    let state = cx.new(|cx| {
        let mut state = AppState::for_window(owner, cx);
        if matches!(kind, Open::Restore) {
            state.window_key = Some("main".into());
        } else {
            state.auto_selected = true;
            if let Some((space, no_project, device, _)) = &context {
                state.selected_space = space.clone();
                state.no_project = *no_project;
                state.selected_device = device.clone();
            }
        }
        state
    });
    let key = state.read(cx).window_key.clone().expect("window identity");
    if !matches!(kind, Open::Restore)
        && let Some((_, _, _, layout)) = context
    {
        crate::settings::update(crate::settings::SavePolicy::Debounced, cx, |settings| {
            settings.windows.insert(key.clone(), layout);
        });
    }
    let window = match crate::open_main_window(state, boot, cx) {
        Ok(window) => window,
        Err(error) => {
            tracing::error!(%error, "could not open a Zeron window");
            return None;
        }
    };
    cx.global_mut::<Windows>().recent.push(window);
    cx.global_mut::<Windows>()
        .keys
        .insert(window.window_id(), key);
    if let Open::Chat(chat) = kind {
        let _ = window.update(cx, |shell, _, cx| shell.open_chat(chat, cx));
    }
    cx.activate(true);
    let _ = window.update(cx, |_, window, _| window.activate_window());
    Some(window)
}

pub(crate) fn activate(cx: &mut App) -> Option<WindowHandle<Shell>> {
    let window = recent(cx).or_else(|| open(Open::Restore, cx))?;
    cx.activate(true);
    let _ = window.update(cx, |_, window, _| window.activate_window());
    Some(window)
}

pub(crate) fn chat_window(chat: &str, cx: &App) -> Option<WindowHandle<Shell>> {
    cx.try_global::<Windows>()?
        .recent
        .iter()
        .rev()
        .copied()
        .find(|window| {
            window
                .read(cx)
                .is_ok_and(|shell| shell.state.read(cx).selected_chat.as_deref() == Some(chat))
        })
}

pub(crate) fn notified_chat(chat: String, cx: &mut App) {
    let window = chat_window(&chat, cx).or_else(|| activate(cx));
    if let Some(window) = window {
        cx.activate(true);
        let _ = window.update(cx, |shell, window, cx| {
            window.activate_window();
            shell.open_chat(chat, cx);
        });
    }
}

pub(crate) fn deep_link(url: String, cx: &mut App) {
    let existing = crate::links::parse_zeron_conversation_link(&url)
        .ok()
        .and_then(|link| chat_window(&link.chat_id, cx));
    if let Some(window) = existing.or_else(|| activate(cx)) {
        let _ = window.update(cx, |shell, window, cx| {
            window.activate_window();
            shell.open_conversation_link(&url, cx);
        });
    }
}

pub(crate) fn views(cx: &App) -> Vec<Entity<Shell>> {
    cx.windows()
        .into_iter()
        .filter_map(|w| w.downcast::<Shell>())
        .filter_map(|w| w.entity(cx).ok())
        .collect()
}
