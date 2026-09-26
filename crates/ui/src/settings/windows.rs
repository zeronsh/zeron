//! Device-local window layouts, independent of process-wide preferences.

use gpui::{App, Bounds, WindowBounds, point, px, size};
use serde::{Deserialize, Serialize};

use super::{SavePolicy, UiSettings};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct WindowSettings {
    pub selected_chat: Option<String>,
    pub last_space_id: Option<String>,
    pub space_filter: Option<String>,
    pub sidebar_width: f32,
    pub sidebar_collapsed: bool,
    pub right_pane_width: f32,
    pub terminal_height: f32,
    pub geometry: Option<WindowGeometry>,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self::from_ui(&UiSettings::default())
    }
}

impl WindowSettings {
    pub fn from_ui(settings: &UiSettings) -> Self {
        Self {
            selected_chat: None,
            last_space_id: settings.last_space_id.clone(),
            space_filter: settings.space_filter.clone(),
            sidebar_width: settings.sidebar_width,
            sidebar_collapsed: settings.sidebar_collapsed,
            right_pane_width: settings.right_pane_width,
            terminal_height: settings.terminal_height,
            geometry: None,
        }
    }

    pub fn apply(&self, settings: &mut UiSettings) {
        settings.last_space_id = self.last_space_id.clone();
        settings.space_filter = self.space_filter.clone();
        settings.sidebar_width = self.sidebar_width;
        settings.sidebar_collapsed = self.sidebar_collapsed;
        settings.right_pane_width = self.right_pane_width;
        settings.terminal_height = self.terminal_height;
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
}

impl WindowGeometry {
    pub fn capture(bounds: WindowBounds) -> Self {
        let maximized = matches!(bounds, WindowBounds::Maximized(_));
        let bounds = bounds.get_bounds();
        Self {
            x: bounds.origin.x.into(),
            y: bounds.origin.y.into(),
            width: bounds.size.width.into(),
            height: bounds.size.height.into(),
            maximized,
        }
    }

    pub fn restore(&self, cx: &App) -> Option<WindowBounds> {
        if ![self.x, self.y, self.width, self.height]
            .into_iter()
            .all(f32::is_finite)
        {
            return None;
        }
        let bounds = Bounds::new(
            point(px(self.x), px(self.y)),
            size(px(self.width.max(900.)), px(self.height.max(600.))),
        );
        // Keep the titlebar reachable when a previously used monitor is gone.
        let titlebar = Bounds::new(bounds.origin, size(bounds.size.width, px(40.)));
        if !cx
            .displays()
            .iter()
            .any(|display| display.bounds().intersects(&titlebar))
        {
            return None;
        }
        Some(if self.maximized {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        })
    }
}

pub fn current(key: Option<&str>, cx: &App) -> UiSettings {
    let mut settings = super::current(cx);
    if let Some(window) = key.and_then(|key| settings.windows.get(key)).cloned() {
        window.apply(&mut settings);
    }
    settings.clamped()
}

const LOCAL_KEYS: &[&str] = &[
    "windows",
    "sidebarWidth",
    "sidebarCollapsed",
    "rightPaneWidth",
    "terminalHeight",
    "lastSpaceId",
    "spaceFilter",
    "openTabs",
    "tabOrder",
    "spaceOrder",
];

/// Apply only the fields this editor changed, including individual nested
/// keybindings. An older window cannot publish its stale snapshot over another.
pub(crate) fn merge_delta(
    current: &mut serde_json::Value,
    before: &serde_json::Value,
    after: &serde_json::Value,
) {
    if before == after {
        return;
    }
    if let (Some(current), Some(before), Some(after)) = (
        current.as_object_mut(),
        before.as_object(),
        after.as_object(),
    ) {
        for key in before.keys().chain(after.keys()) {
            if before.get(key) == after.get(key) {
                continue;
            }
            match after.get(key) {
                Some(next) => {
                    let entry = current
                        .entry(key.clone())
                        .or_insert(serde_json::Value::Null);
                    merge_delta(
                        entry,
                        before.get(key).unwrap_or(&serde_json::Value::Null),
                        next,
                    );
                }
                None => {
                    current.remove(key);
                }
            }
        }
    } else {
        *current = after.clone();
    }
}

pub fn publish(
    key: Option<&str>,
    before: &UiSettings,
    after: &UiSettings,
    selected_chat: Option<String>,
    cx: &mut App,
) {
    let mut previous = serde_json::to_value(before).expect("settings serialize");
    let mut next = serde_json::to_value(after).expect("settings serialize");
    if key.is_some() {
        for field in LOCAL_KEYS {
            previous.as_object_mut().unwrap().remove(*field);
            next.as_object_mut().unwrap().remove(*field);
        }
    }
    let changed = super::update(SavePolicy::Debounced, cx, |settings| {
        let mut value = serde_json::to_value(&*settings).expect("settings serialize");
        merge_delta(&mut value, &previous, &next);
        if let Ok(merged) = serde_json::from_value(value) {
            *settings = merged;
        }
    });
    if changed {
        cx.refresh_windows();
    }
    if let Some(key) = key {
        let mut window = WindowSettings::from_ui(after);
        window.selected_chat = selected_chat;
        super::update(SavePolicy::Debounced, cx, |settings| {
            window.geometry = settings
                .windows
                .get(key)
                .and_then(|old| old.geometry.clone());
            settings.windows.insert(key.to_string(), window);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn stale_windows_preserve_other_preferences_and_layouts(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            let baseline = UiSettings::default();
            super::super::init(baseline.clone(), dir.path(), cx);
            let mut a = baseline.clone();
            a.sound_enabled = false;
            a.sidebar_width = 260.;
            publish(Some("a"), &baseline, &a, Some("chat-a".into()), cx);
            let mut b = baseline.clone();
            b.notifications_enabled = false;
            b.sidebar_width = 310.;
            publish(Some("b"), &baseline, &b, Some("chat-b".into()), cx);
            let current = super::super::current(cx);
            assert!(!current.sound_enabled);
            assert!(!current.notifications_enabled);
            assert_eq!(current.windows["a"].sidebar_width, 260.);
            assert_eq!(current.windows["b"].sidebar_width, 310.);
            super::super::flush(cx);
            assert_eq!(UiSettings::load(dir.path()), current);
        });
    }

    #[test]
    fn independent_nested_shortcuts_merge_without_reverting_each_other() {
        let old = serde_json::json!({"newSession":"mod-n", "newProject":"mod-shift-n"});
        let mut current = serde_json::json!({"newSession":"mod-t", "newProject":"mod-shift-n"});
        let next = serde_json::json!({"newSession":"mod-n", "newProject":"mod-alt-p"});
        merge_delta(&mut current, &old, &next);
        assert_eq!(current["newSession"], "mod-t");
        assert_eq!(current["newProject"], "mod-alt-p");
    }
}
