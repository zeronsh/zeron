//! Spaces sidebar: the space-filter dropdown (searchable, with "All projects"),
//! the filtered Sessions list, and the add-space palette (device
//! tabs + filtered folder browser).
//!
//! A space = a synced (device, folder) pair. Spaces stopped being a
//! navigation spine when tabs went device-local: the dropdown only FILTERS
//! the sidebar's session list (never the tab strip) and hosts space
//! management (add via the palette; rename/delete via row context menus).
//! Child module of `shell` so it renders straight off `Shell`'s private state.

use super::*;
use crate::pickers::{breadcrumbs, browser_rows, completion_prefix_len, parent_path};
use gpui::{FocusHandle, Window};
use std::collections::HashSet;
use zeron_proto::{ChatIndicator, Device, DriveEntry, DriveListing, FolderListing, Space};

/// Promote the user's ordered pins above the untouched activity projection.
/// Every unpinned id keeps exactly the relative order supplied by recency.
pub(super) fn project_pinned_first(recency_ids: &[String], pinned_ids: &[String]) -> Vec<String> {
    let active: HashSet<&str> = recency_ids.iter().map(String::as_str).collect();
    let pinned: HashSet<&str> = pinned_ids.iter().map(String::as_str).collect();
    let mut seen = HashSet::new();
    pinned_ids
        .iter()
        .filter(|id| active.contains(id.as_str()))
        .chain(
            recency_ids
                .iter()
                .filter(|id| !pinned.contains(id.as_str())),
        )
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect()
}

/// Move only the dragged pin. Every other pin, including hidden/archived pins,
/// keeps its relative order; no other position needs to be written.
pub(super) fn reorder_visible_pins(
    pinned_ids: &[String],
    visible_ids: &[String],
    from: usize,
    to: usize,
) -> Vec<String> {
    if from >= visible_ids.len() || to >= visible_ids.len() || from == to {
        return pinned_ids.to_vec();
    }

    let moved = &visible_ids[from];
    let anchor = &visible_ids[to];
    let mut result: Vec<_> = pinned_ids
        .iter()
        .filter(|id| *id != moved)
        .cloned()
        .collect();
    let Some(index) = result.iter().position(|id| id == anchor) else {
        return pinned_ids.to_vec();
    };
    result.insert(index + usize::from(from < to), moved.clone());
    result
}

/// Remove only ids absent from the workspace. Archived sessions remain known
/// so unarchiving restores their local pin and position.
pub(super) fn retain_known_pins(
    pinned_ids: &mut Vec<String>,
    known_chat_ids: &HashSet<String>,
) -> bool {
    let before = pinned_ids.len();
    let mut seen = HashSet::new();
    pinned_ids.retain(|id| known_chat_ids.contains(id) && seen.insert(id.clone()));
    pinned_ids.len() != before
}

/// Convert viewport coordinates to the first pinned row, below its disclosure.
fn pinned_session_pointer_y(pointer_y: f32, viewport_top: f32, scroll_top: f32) -> f32 {
    pointer_y - viewport_top + scroll_top
        - super::SIDEBAR_LIST_PAD_TOP
        - SIDEBAR_DISCLOSURE_HEADER_HEIGHT
        - SIDEBAR_DISCLOSURE_BODY_INSET
}

#[cfg(test)]
pub(super) fn pinned_session_drop_index(rel_y: f32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    let height = count as f32 * super::SIDEBAR_SESSION_SLOT - super::SIDEBAR_LIST_GAP;
    (rel_y >= 0.0 && rel_y <= height)
        .then(|| ((rel_y / super::SIDEBAR_SESSION_SLOT).floor() as usize).min(count - 1))
}

/// Hit testing shares the exact row metrics used by layout, including mixed PR rows.
pub(super) fn row_drop_index(y: f32, heights: &[f32], clamp: bool) -> Option<usize> {
    if heights.is_empty() {
        return None;
    }
    let total =
        heights.iter().sum::<f32>() + heights.len().saturating_sub(1) as f32 * SIDEBAR_LIST_GAP;
    if !clamp && !(0.0..=total).contains(&y) {
        return None;
    }
    let mut bottom = 0.0;
    for (index, height) in heights.iter().enumerate() {
        bottom += height + SIDEBAR_LIST_GAP;
        if y < bottom {
            return Some(index);
        }
    }
    Some(heights.len() - 1)
}

/// Keep a sidebar-wide drag physically bounded to the pinned section. The
/// strict drop helper above still identifies whether the pointer is actually
/// inside the section; this helper supplies the nearest valid pinned slot
/// while the pointer is over regular sessions.
#[cfg(test)]
pub(super) fn pinned_session_clamped_index(rel_y: f32, count: usize) -> Option<usize> {
    if count == 0 {
        return None;
    }
    Some(((rel_y.max(0.0) / super::SIDEBAR_SESSION_SLOT).floor() as usize).min(count - 1))
}

/// A drop can change pin membership or pinned order, never activity ordering.
fn sidebar_session_drop_pins(
    saved: &[String],
    visible: &[String],
    chat_id: &str,
    target: SidebarSessionDrop,
) -> Vec<String> {
    let mut next = saved.to_vec();
    match target {
        SidebarSessionDrop::Regular => next.retain(|id| id != chat_id),
        SidebarSessionDrop::Pinned(index) => {
            if let Some(from) = visible.iter().position(|id| id == chat_id) {
                return reorder_visible_pins(saved, visible, from, index.min(visible.len() - 1));
            }
            if saved.iter().any(|id| id == chat_id) {
                return next;
            }
            let insertion = visible
                .get(index)
                .and_then(|anchor| next.iter().position(|id| id == anchor))
                .or_else(|| {
                    visible
                        .last()
                        .and_then(|anchor| next.iter().position(|id| id == anchor))
                        .map(|ix| ix + 1)
                })
                .unwrap_or(next.len());
            next.insert(insertion, chat_id.to_owned());
        }
    }
    next
}

/// Preview geometry only: regular ordering is never persisted by a drag.
fn sidebar_gap_offset(row: usize, source: Option<usize>, boundary: usize, height: f32) -> f32 {
    match source {
        Some(source) if row == source => 0.0,
        Some(source) if row < source && row >= boundary => height,
        Some(source) if row > source && row < boundary => -height,
        None if row >= boundary => height,
        _ => 0.0,
    }
}

pub(super) fn pinned_drag_scroll_delta(
    pointer_y: f32,
    viewport_top: f32,
    viewport_bottom: f32,
) -> f32 {
    if viewport_bottom <= viewport_top {
        return 0.0;
    }
    if pointer_y < viewport_top + super::SIDEBAR_DRAG_SCROLL_BAND {
        let penetration = ((viewport_top + super::SIDEBAR_DRAG_SCROLL_BAND - pointer_y)
            / super::SIDEBAR_DRAG_SCROLL_BAND)
            .clamp(0.0, 1.0);
        -super::SIDEBAR_DRAG_SCROLL_MAX * penetration
    } else if pointer_y > viewport_bottom - super::SIDEBAR_DRAG_SCROLL_BAND {
        let penetration = ((pointer_y - (viewport_bottom - super::SIDEBAR_DRAG_SCROLL_BAND))
            / super::SIDEBAR_DRAG_SCROLL_BAND)
            .clamp(0.0, 1.0);
        super::SIDEBAR_DRAG_SCROLL_MAX * penetration
    } else {
        0.0
    }
}

#[cfg(test)]
mod pinned_session_tests {
    fn pin_change(id: &str) -> zeron_proto::SidebarPinChange {
        zeron_proto::SidebarPinChange::Pin {
            session_id: id.into(),
            after: None,
            before: None,
        }
    }
    use super::{
        pinned_drag_scroll_delta, pinned_drag_scroll_step, pinned_drag_snapshot_is_valid,
        pinned_session_clamped_index, pinned_session_drop_index, project_pinned_first,
        reorder_visible_pins, retain_known_pins,
    };
    use std::collections::HashSet;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    fn pin_test_shell(
        cx: &mut gpui::TestAppContext,
        path: &std::path::Path,
    ) -> gpui::WindowHandle<super::Shell> {
        use super::*;
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: path.into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        })
    }

    fn remote_pin_state(state: &mut super::AppState, synced: bool, initialized: bool) {
        state.workspace_scope = Some(zeron_proto::WorkspaceScope::Synced);
        state.auth = Some(zeron_proto::AuthState::SignedIn {
            user: zeron_proto::UserProfile {
                id: "user".into(),
                email: "test@example.com".into(),
                name: None,
            },
            org_id: Some("org".into()),
        });
        state.sidebar_preferences.synced = synced;
        state.sidebar_preferences.initialized = initialized;
    }

    fn pin_test_chat(id: &str) -> zeron_proto::Chat {
        serde_json::from_value(serde_json::json!({
            "id": id, "title": id, "deviceId": "local", "archived": false,
            "createdAt": chrono::Utc::now(),
        }))
        .unwrap()
    }

    fn pin_test_engine() -> (
        crate::state::EngineHandle,
        tokio::sync::mpsc::Receiver<String>,
        tokio::sync::mpsc::Sender<String>,
    ) {
        let (out, requests) = tokio::sync::mpsc::channel(16);
        let (replies, inbound) = tokio::sync::mpsc::channel(16);
        (
            crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound)),
            requests,
            replies,
        )
    }

    fn pin_snapshot(revision: u64, pins: &[&str]) -> zeron_proto::SidebarPreferencesState {
        zeron_proto::SidebarPreferencesState {
            revision,
            synced: true,
            initialized: true,
            pinned_session_ids: ids(pins),
        }
    }

    fn deliver_pin_rpc_reply(
        runtime: &tokio::runtime::Runtime,
        replies: &tokio::sync::mpsc::Sender<String>,
        reply: serde_json::Value,
    ) {
        // The current-thread reactor drains the actual RpcClient reader before
        // GPUI resumes. No wall-clock sleeps or hand-invoked completion handlers.
        runtime.block_on(async {
            replies.send(reply.to_string()).await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                while replies.capacity() < replies.max_capacity() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        });
    }

    #[gpui::test]
    fn sidebar_rpc_replies_dispatch_each_queued_drop_once_and_recover_rejection(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (engine, mut requests, replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, true, true);
                    state.set_test_engine(engine);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell.apply_sidebar_pin_change(key.clone(), pin_change("first"), cx);
                shell.apply_sidebar_pin_change(key, pin_change("second"), cx);
            })
            .unwrap();
        cx.run_until_parked();
        let first: serde_json::Value = serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        assert_eq!(
            first["params"]["change"],
            serde_json::to_value(pin_change("first")).unwrap()
        );
        assert!(requests.try_recv().is_err());
        deliver_pin_rpc_reply(
            &runtime,
            &replies,
            serde_json::json!({"id": first["id"], "ok": {"ok": true, "sidebarPreferences": pin_snapshot(1, &["first"])}}),
        );
        cx.run_until_parked();
        let second: serde_json::Value =
            serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        assert_eq!(
            second["params"]["change"],
            serde_json::to_value(pin_change("second")).unwrap()
        );
        assert!(requests.try_recv().is_err());
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["first", "second"]));
                assert_eq!(
                    shell.state.read(cx).sidebar_preferences.pinned_session_ids,
                    ids(&["first"])
                );
                shell.state.update(cx, |state, _| {
                    state.apply_sidebar_preferences(pin_snapshot(3, &["remote"]));
                });
            })
            .unwrap();
        deliver_pin_rpc_reply(
            &runtime,
            &replies,
            serde_json::json!({"id": second["id"], "err": "rejected by test registry"}),
        );
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["remote"]));
                assert!(shell.sidebar_pin_write.is_none());
                assert!(
                    shell
                        .sidebar_notice
                        .as_deref()
                        .unwrap()
                        .contains("rejected by test registry")
                );
            })
            .unwrap();
        assert!(requests.try_recv().is_err());
    }

    #[gpui::test]
    fn sidebar_rpc_timeout_blocks_overtaking_until_the_late_reply_arrives(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (engine, mut requests, replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, true, true);
                    state.set_test_engine(engine);
                    state.sidebar_preferences = pin_snapshot(1, &["confirmed"]);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell.apply_sidebar_pin_change(key.clone(), pin_change("slow"), cx);
                shell.apply_sidebar_pin_change(key, pin_change("queued"), cx);
            })
            .unwrap();
        cx.run_until_parked();
        let request: serde_json::Value =
            serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(21));
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.sidebar_pin_write.as_ref().unwrap().unconfirmed);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["confirmed"]));
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(!shell.apply_sidebar_pin_change(key, pin_change("overtaking"), cx));
            })
            .unwrap();
        assert!(
            requests.try_recv().is_err(),
            "queued and fresh drops must not overtake the slow write"
        );
        deliver_pin_rpc_reply(
            &runtime,
            &replies,
            serde_json::json!({"id": request["id"], "ok": {"ok": true, "sidebarPreferences": pin_snapshot(2, &["slow"])}}),
        );
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(shell.sidebar_pin_write.is_none());
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["slow"]));
                assert!(
                    shell.sidebar_notice.is_none(),
                    "late success must clear the waiting notice"
                );
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(shell.apply_sidebar_pin_change(key, pin_change("after-confirmation"), cx));
            })
            .unwrap();
        cx.run_until_parked();
        let next: serde_json::Value = serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        assert_eq!(
            next["params"]["change"],
            serde_json::to_value(pin_change("after-confirmation")).unwrap()
        );
        assert!(requests.try_recv().is_err());
    }

    #[gpui::test]
    fn sidebar_unconfirmed_write_stops_the_queue_without_overwriting_observed_pins(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let (engine, _requests, _replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, true, true);
                    state.set_test_engine(engine);
                    state.sidebar_preferences = pin_snapshot(2, &["confirmed"]);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell.apply_sidebar_pin_change(key.clone(), pin_change("first"), cx);
                shell.apply_sidebar_pin_change(key, pin_change("second"), cx);
                let id = shell.sidebar_pin_write.as_ref().unwrap().id;
                shell.mark_pin_write_unconfirmed(id, cx);
                assert!(shell.sidebar_pin_write.as_ref().unwrap().unconfirmed);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["confirmed"]));
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(!shell.apply_sidebar_pin_change(key, pin_change("third"), cx));
                assert_eq!(
                    shell.finish_sidebar_pin_write(id, Err("late error".into()), cx),
                    None
                );
                assert!(shell.sidebar_pin_write.is_none());
                shell.state.update(cx, |state, _| {
                    state.apply_sidebar_preferences(pin_snapshot(3, &["first"]));
                });
                assert_eq!(
                    shell.active_sidebar_pins(cx),
                    ids(&["first"]),
                    "a timed-out request may still commit and must be reconciled via its watch"
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_optimistic_writes_preserve_newer_edits_and_watch_state_on_failure(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let (engine, mut requests, _replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        let write_id = window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, false, true);
                    state.sidebar_preferences = pin_snapshot(1, &["original"]);
                    state.set_test_engine(engine);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(shell.apply_sidebar_pin_change(key.clone(), pin_change("first"), cx));
                assert!(shell.apply_sidebar_pin_change(key, pin_change("second"), cx));
                assert_eq!(
                    shell.active_sidebar_pins(cx),
                    ids(&["original", "first", "second"])
                );
                assert_eq!(
                    shell.state.read(cx).sidebar_preferences.pinned_session_ids,
                    ids(&["original"])
                );
                assert!(
                    shell.mutate_task.is_none(),
                    "pin writes cannot be cancelled by generic sidebar mutations"
                );
                shell.sidebar_pin_write.as_ref().unwrap().id
            })
            .unwrap();
        cx.run_until_parked();
        let request: serde_json::Value =
            serde_json::from_str(&requests.try_recv().unwrap()).unwrap();
        assert_eq!(
            request["params"]["change"],
            serde_json::to_value(pin_change("first")).unwrap()
        );
        assert!(
            requests.try_recv().is_err(),
            "only one write may be in flight"
        );
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.apply_sidebar_preferences(pin_snapshot(7, &["remote"]));
                });
                assert_eq!(
                    shell.finish_sidebar_pin_write(write_id, Err("rejected".into()), cx),
                    Some(pin_change("second"))
                );
                assert_eq!(
                    shell.active_sidebar_pins(cx),
                    ids(&["remote", "second"]),
                    "an older failure must not roll back a newer drop"
                );
                assert_eq!(
                    shell.finish_sidebar_pin_write(write_id, Err("rejected".into()), cx),
                    None
                );
                assert_eq!(
                    shell.active_sidebar_pins(cx),
                    ids(&["remote"]),
                    "failure restores latest observed state, not the old backup"
                );
                assert!(
                    shell
                        .sidebar_notice
                        .as_deref()
                        .unwrap()
                        .contains("Couldn't save pins")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_write_acknowledgements_ignore_older_watches_and_previous_operations(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let (engine, _requests, _replies) = pin_test_engine();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, true, true);
                    state.set_test_engine(engine);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell.apply_sidebar_pin_change(key.clone(), pin_change("saved"), cx);
                let first = shell.sidebar_pin_write.as_ref().unwrap().id;
                shell.finish_sidebar_pin_write(first, Ok(pin_snapshot(5, &["saved"])), cx);
                shell.state.update(cx, |state, _| {
                    assert!(!state.apply_sidebar_preferences(pin_snapshot(4, &["stale"])));
                });
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["saved"]));
                shell.apply_sidebar_pin_change(key, pin_change("new-drop"), cx);
                let second = shell.sidebar_pin_write.as_ref().unwrap().id;
                shell.finish_sidebar_pin_write(first, Err("late failure".into()), cx);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["saved", "new-drop"]));
                shell.state.update(cx, |state, _| {
                    state.apply_sidebar_preferences(pin_snapshot(8, &["newer-remote"]));
                });
                shell.sidebar_notice = Some("Unrelated archive error".into());
                shell.finish_sidebar_pin_write(second, Ok(pin_snapshot(6, &["new-drop"])), cx);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["newer-remote"]));
                assert_eq!(
                    shell.sidebar_notice.as_deref(),
                    Some("Unrelated archive error")
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_write_replies_cannot_cross_profile_or_engine_boundaries(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let _guard = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        for change_profile in [true, false] {
            let (engine, _requests, _replies) = pin_test_engine();
            let (replacement, _other_requests, _other_replies) = pin_test_engine();
            window
                .update(cx, |shell, _, cx| {
                    shell.state.update(cx, |state, _| {
                        remote_pin_state(state, true, true);
                        state.set_test_engine(engine);
                    });
                    let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                    shell.apply_sidebar_pin_change(key, pin_change("pending"), cx);
                    let id = shell.sidebar_pin_write.as_ref().unwrap().id;
                    shell.state.update(cx, |state, _| {
                        if change_profile {
                            state.workspace_scope = Some(zeron_proto::WorkspaceScope::Local);
                        } else {
                            state.set_test_engine(replacement);
                        }
                        state.sidebar_preferences = pin_snapshot(0, &["new-runtime"]);
                    });
                    shell.finish_sidebar_pin_write(id, Ok(pin_snapshot(99, &["old-runtime"])), cx);
                    assert_eq!(
                        shell.state.read(cx).sidebar_preferences.pinned_session_ids,
                        ids(&["new-runtime"])
                    );
                    assert!(shell.sidebar_pin_write.is_none());
                })
                .unwrap();
        }
    }

    #[gpui::test]
    fn sidebar_drop_without_engine_returns_without_an_optimistic_pin(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, window, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, false, true);
                    state.chats = vec![pin_test_chat("normal")];
                });
                shell.settings.space_filter = None;
                let payload = SidebarSessionDrag {
                    chat_id: "normal".into(),
                    visible_ids: std::sync::Arc::new(vec![]),
                    filter: None,
                    profile_key: shell.active_sidebar_pin_profile_key(cx).unwrap(),
                };
                shell.begin_sidebar_session_transfer(
                    &payload,
                    gpui::point(px(0.0), px(0.0)),
                    window,
                    cx,
                );
                shell.finish_sidebar_session_transfer(&payload, SidebarSessionDrop::Pinned(0), cx);
                assert!(shell.active_sidebar_pins(cx).is_empty());
                assert!(shell.sidebar_session_return.is_some());
                assert!(shell.sidebar_pin_write.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_menu_and_drop_both_reject_unknown_remote_preferences(cx: &mut gpui::TestAppContext) {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, window, cx| {
                shell.state.update(cx, |state, _| {
                    remote_pin_state(state, false, false);
                    state.chats = vec![pin_test_chat("normal")];
                });
                shell.settings.space_filter = None;
                shell.set_chat_pinned("normal".into(), true, cx);
                assert_eq!(
                    shell.sidebar_notice.as_deref(),
                    Some("Pins are still syncing")
                );
                let payload = SidebarSessionDrag {
                    chat_id: "normal".into(),
                    visible_ids: std::sync::Arc::new(vec![]),
                    filter: None,
                    profile_key: shell.active_sidebar_pin_profile_key(cx).unwrap(),
                };
                shell.begin_sidebar_session_transfer(
                    &payload,
                    gpui::point(px(0.0), px(0.0)),
                    window,
                    cx,
                );
                shell.finish_sidebar_session_transfer(&payload, SidebarSessionDrop::Pinned(0), cx);
                assert!(shell.active_sidebar_pins(cx).is_empty());
                assert!(!shell.state.read(cx).sidebar_preferences.initialized);
                assert_eq!(
                    shell.sidebar_notice.as_deref(),
                    Some("Pins are still syncing")
                );
                assert!(
                    shell.sidebar_session_return.is_some(),
                    "rejected drop returns to its origin"
                );
                assert!(shell.mutate_task.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_full_capacity_drop_returns_without_mutating_local_or_remote_pins(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, window, cx| {
                let saved: Vec<String> = (0..zeron_proto::MAX_SIDEBAR_PINS)
                    .map(|n| format!("hidden-{n}"))
                    .collect();
                for remote in [false, true] {
                    shell.state.update(cx, |state, _| {
                        if remote {
                            remote_pin_state(state, true, true);
                            state.sidebar_preferences.pinned_session_ids = saved.clone();
                        } else {
                            state.workspace_scope = Some(WorkspaceScope::Local);
                        }
                        state.chats = vec![pin_test_chat("normal")];
                    });
                    let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                    if !remote {
                        shell
                            .settings
                            .sidebar_pinned_session_ids_by_profile
                            .insert(key.clone(), saved.clone());
                    }
                    shell.settings.space_filter = None;
                    let payload = SidebarSessionDrag {
                        chat_id: "normal".into(),
                        visible_ids: std::sync::Arc::new(vec![]),
                        filter: None,
                        profile_key: key,
                    };
                    shell.begin_sidebar_session_transfer(
                        &payload,
                        gpui::point(px(0.0), px(0.0)),
                        window,
                        cx,
                    );
                    shell.finish_sidebar_session_transfer(
                        &payload,
                        SidebarSessionDrop::Pinned(0),
                        cx,
                    );
                    assert_eq!(shell.active_sidebar_pins(cx), saved);
                    assert_eq!(
                        shell.sidebar_notice.as_deref(),
                        Some("You can pin up to 200 sessions")
                    );
                    assert!(shell.sidebar_session_return.is_some());
                }
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_validation_preserves_offline_edits_and_rejects_stale_profiles(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell
                    .state
                    .update(cx, |state, _| remote_pin_state(state, false, true));
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                assert!(shell.validate_sidebar_pin_change(&key, &ids(&["cached"]), cx));
                assert!(!shell.validate_sidebar_pin_change(
                    "another-profile",
                    &ids(&["cached"]),
                    cx
                ));
                assert!(!shell.validate_sidebar_pin_change(
                    &key,
                    &ids(&["duplicate", "duplicate"]),
                    cx
                ));
                assert!(!shell.validate_sidebar_pin_change(&key, &ids(&[""]), cx));
                let saved: Vec<_> = (0..zeron_proto::MAX_SIDEBAR_PINS)
                    .map(|n| format!("pin-{n}"))
                    .collect();
                let reordered = super::sidebar_session_drop_pins(
                    &saved,
                    &saved,
                    &saved[0],
                    super::SidebarSessionDrop::Pinned(199),
                );
                assert!(shell.validate_sidebar_pin_change(&key, &reordered, cx));
                let unpinned = super::sidebar_session_drop_pins(
                    &saved,
                    &saved,
                    &saved[0],
                    super::SidebarSessionDrop::Regular,
                );
                assert!(shell.validate_sidebar_pin_change(&key, &unpinned, cx));
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_preferences_arriving_before_chats_never_prune_live_pins(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.apply_chats(vec![]);
                    remote_pin_state(state, true, true);
                    state.sidebar_preferences.pinned_session_ids = ids(&["live-remote"]);
                });
                shell.on_state_changed(&shell.state.clone(), cx);
                assert_eq!(shell.active_sidebar_pins(cx), ids(&["live-remote"]));
                assert!(shell.mutate_task.is_none());
            })
            .unwrap();
    }

    #[gpui::test]
    fn sidebar_project_groups_preserve_pin_order_and_keyboard_order(cx: &mut gpui::TestAppContext) {
        use super::*;
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.settings.sidebar_organization = SidebarOrganization::ByProject;
                shell
                    .settings
                    .sidebar_pins_mut("local".into())
                    .push("pin".into());
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state.spaces = ["a", "b"].into_iter().map(|id| serde_json::from_value(serde_json::json!({
                        "id": id, "deviceId": "local", "path": format!("/project/{id}"), "createdAt": Utc::now()
                    })).unwrap()).collect();
                    state.chats = [("pin", "a"), ("b-new", "b"), ("a-new", "a"), ("b-old", "b")]
                        .into_iter()
                        .enumerate()
                        .map(|(ix, (id, project))| {
                            let mut chat = pin_test_chat(id);
                            chat.space_id = Some(project.into());
                            chat.created_at = Utc::now() - chrono::Duration::minutes(ix as i64);
                            chat
                        })
                        .collect();
                });
                assert_eq!(
                    shell.sidebar_visible_order(cx),
                    ids(&["pin", "b-new", "b-old", "a-new"])
                );
                shell.settings.space_filter = Some("a".into());
                assert_eq!(shell.sidebar_visible_order(cx), ids(&["pin", "a-new"]));
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn sidebar_remote_pins_ignore_old_local_preferences(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = pin_test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                shell.state.update(cx, |state, _| {
                    state.apply_chats(vec![]);
                    remote_pin_state(state, true, false);
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell
                    .settings
                    .sidebar_pinned_session_ids_by_profile
                    .insert(key.clone(), ids(&["live-remote"]));
                shell.on_state_changed(&shell.state.clone(), cx);
                assert_eq!(shell.settings.sidebar_pins(&key), ids(&["live-remote"]));
                assert!(!shell.state.read(cx).sidebar_preferences.initialized);
            })
            .unwrap();
    }

    #[test]
    fn pins_lead_without_changing_unpinned_recency() {
        let recency = ids(&["newest", "p2", "middle", "p1", "oldest"]);
        assert_eq!(
            project_pinned_first(&recency, &ids(&["p1", "p2"])),
            ids(&["p1", "p2", "newest", "middle", "oldest"])
        );
    }

    #[test]
    fn inline_session_slide_retargets_and_returns_without_a_ghost() {
        use super::*;
        let mut slide = SidebarSessionSlide {
            from: 0.0,
            to: SIDEBAR_SESSION_SLOT,
            epoch: 1,
            started: std::time::Instant::now() - TAB_SLIDE.total(),
        };
        slide.retarget(2.0 * SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.from, SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.to, 2.0 * SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.epoch, 2);
        slide.retarget(2.0 * SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.epoch, 2);
        slide.started -= TAB_SLIDE.total();
        slide.retarget(0.0);
        assert_eq!(slide.from, 2.0 * SIDEBAR_SESSION_SLOT);
        assert_eq!(slide.to, 0.0);
    }

    #[test]
    fn session_layout_reverses_from_its_current_height() {
        use super::*;
        let mut slide = SidebarSessionSlide {
            from: 0.0,
            to: SIDEBAR_SESSION_SLOT,
            epoch: 0,
            started: std::time::Instant::now() - TAB_SLIDE.total() / 2,
        };
        let halfway = slide.current();
        assert!(halfway > 0.0 && halfway < SIDEBAR_SESSION_SLOT);
        slide.retarget(0.0);
        assert!((slide.from - halfway).abs() < 0.5);
        assert_eq!(slide.to, 0.0);
        slide.started -= TAB_SLIDE.total();
        assert_eq!(slide.current(), 0.0);
    }

    #[test]
    fn transfer_gaps_shift_neighbors_without_reordering_data() {
        use super::sidebar_gap_offset;
        // Entering from another section opens a full slot at the destination.
        assert_eq!(sidebar_gap_offset(0, None, 1, 63.0), 0.0);
        assert_eq!(sidebar_gap_offset(1, None, 1, 63.0), 63.0);
        assert_eq!(sidebar_gap_offset(2, None, 1, 63.0), 63.0);
        // Within a normal group, its original vacant slot is reused.
        assert_eq!(sidebar_gap_offset(0, Some(2), 0, 63.0), 63.0);
        assert_eq!(sidebar_gap_offset(1, Some(2), 0, 63.0), 63.0);
        assert_eq!(sidebar_gap_offset(2, Some(2), 0, 63.0), 0.0);
        assert_eq!(sidebar_gap_offset(1, Some(0), 3, 63.0), -63.0);
        assert_eq!(sidebar_gap_offset(3, Some(0), 3, 63.0), 0.0);
    }

    #[gpui::test]
    fn pinned_disclosure_click_collapses_and_restores_rows(cx: &mut gpui::TestAppContext) {
        use super::*;

        struct PinnedHost(Entity<Shell>);
        impl Render for PinnedHost {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |shell, cx| {
                    let items = (0..2)
                        .map(|_| div().h(px(61.0)).into_any_element())
                        .collect();
                    div()
                        .w(px(280.0))
                        .flex()
                        .flex_col()
                        .child(shell.render_pinned_section(items, 128.0, &Theme::default(), cx))
                })
            }
        }

        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            PinnedHost(cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                );
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Development);
                    state.sidebar_preferences.pinned_session_ids = vec!["pin".into()];
                    state.chats = ["pin", "regular"]
                        .into_iter()
                        .map(|id| {
                            serde_json::from_value(serde_json::json!({
                                "id": id, "deviceId": "local", "archived": false,
                                "createdAt": Utc::now(),
                            }))
                            .unwrap()
                        })
                        .collect();
                });
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());
        for open in [true, false, true] {
            if !open || !shell.read_with(cx, |shell, _| shell.pinned_open) {
                let toggle = cx.debug_bounds("pinned-toggle").unwrap().center();
                cx.simulate_mouse_down(toggle, MouseButton::Left, gpui::Modifiers::default());
                cx.simulate_mouse_up(toggle, MouseButton::Left, gpui::Modifiers::default());
            }
            shell.update(cx, |shell, cx| {
                assert_eq!(shell.pinned_open, open);
                let expected = if open {
                    vec!["pin", "regular"]
                } else {
                    vec!["regular"]
                };
                assert_eq!(shell.sidebar_visible_order(cx), expected);
                assert_eq!(shell.active_sidebar_pins(cx), ["pin"]);
                // Finish the tween to assert settled layout without wall-clock sleeps.
                shell.sidebar_disclosure_motion.clear();
                cx.notify();
            });
            cx.update(|window, cx| {
                window.refresh();
                window.draw(cx).clear();
            });
            let bounds = cx.debug_bounds("sidebar-pinned-section").unwrap();
            assert_eq!(
                f32::from(bounds.size.height),
                if open { 156.0 } else { 28.0 }
            );
            assert!(cx.debug_bounds("sidebar-pinned-divider").is_none());
        }
    }

    #[gpui::test]
    fn session_section_drops_preserve_live_activity_order(cx: &mut gpui::TestAppContext) {
        exercise_session_section_drops(cx, false, true);
    }

    #[gpui::test]
    fn compact_sidebar_section_drops_follow_pointer(cx: &mut gpui::TestAppContext) {
        exercise_session_section_drops(cx, true, false);
    }

    #[gpui::test]
    fn hidden_label_sidebar_section_drops_follow_pointer(cx: &mut gpui::TestAppContext) {
        exercise_session_section_drops(cx, false, false);
    }

    fn exercise_session_section_drops(
        cx: &mut gpui::TestAppContext,
        compact: bool,
        show_label: bool,
    ) {
        use super::*;

        struct SidebarHost(Entity<Shell>);
        impl Render for SidebarHost {
            fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
                self.0.update(cx, |shell, cx| {
                    div()
                        .w(px(280.0))
                        .h(px(800.0))
                        .on_drag_move::<SidebarSessionDrag>(
                            cx.listener(Shell::contain_pinned_session_drag),
                        )
                        .on_drop::<SidebarSessionDrag>(
                            cx.listener(|shell, _, _, cx| {
                                shell.cancel_sidebar_session_transfer(cx)
                            }),
                        )
                        .child(shell.render_chat_sidebar(&Theme::default(), cx))
                })
            }
        }
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        let (host, cx) = cx.add_window_view(|_, cx| {
            SidebarHost(cx.new(|cx| {
                let state = cx.new(|_| AppState::new());
                let mut shell = Shell::new(
                    state,
                    EngineBootConfig {
                        data_dir: dir.path().into(),
                        ipc_port: 0,
                        edge_url: "http://127.0.0.1:1".into(),
                        edge_token: None,
                        org_id: None,
                        workos_client_id: None,
                        default_harness: zeron_proto::HarnessId::Mock,
                    },
                    cx,
                );
                shell.settings.sidebar_organization = SidebarOrganization::InOneList;
                shell.settings.sidebar_show_branch = true;
                shell.settings.sidebar_compact = compact;
                shell.settings.sidebar_show_project_label = show_label;
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Local);
                    state.local_device_id = Some("local".into());
                    state.chats = ["older", "newer"]
                        .into_iter()
                        .enumerate()
                        .map(|(ix, id)| {
                            serde_json::from_value(serde_json::json!({
                                "id": id, "title": id, "deviceId": "local", "archived": false,
                                "sourceContext": {
                                    "checkoutId": "checkout", "repoRoot": "/project", "cwd": "/project",
                                    "branch": "feature/sidebar-drag", "observedAt": Utc::now(),
                                },
                                "createdAt": Utc::now() - chrono::Duration::minutes(10 - ix as i64),
                            }))
                            .unwrap()
                        })
                        .collect();
                    let mut archived = state.chats[0].clone();
                    archived.id = "archived".into();
                    archived.archived = true;
                    state.chats.push(archived);
                });
                shell
            }))
        });
        let shell = host.read_with(cx, |host, _| host.0.clone());

        // Archived rows share geometry and metadata in every sidebar layout.
        let active = cx.debug_bounds("chat-older").unwrap();
        let archived = cx.debug_bounds("chat-archived").unwrap();
        assert_eq!(active.size, archived.size);
        assert_eq!(cx.debug_bounds("chat-branch-archived").is_some(), !compact);
        assert_eq!(
            cx.debug_bounds("chat-device-archived").is_some(),
            !compact && show_label
        );
        if compact {
            assert!(cx.debug_bounds("chat-status-archived").is_some());
            let time = cx.debug_bounds("chat-time-archived").unwrap();
            cx.simulate_mouse_move(archived.center(), None, gpui::Modifiers::default());
            assert_eq!(cx.debug_bounds("chat-time-archived").unwrap(), time);
        }

        // Sessions owns all unpinned rows, including their keyboard traversal.
        for open in [false, true] {
            let toggle = cx.debug_bounds("sessions-toggle").unwrap().center();
            cx.simulate_mouse_down(toggle, MouseButton::Left, gpui::Modifiers::default());
            cx.simulate_mouse_up(toggle, MouseButton::Left, gpui::Modifiers::default());
            shell.update(cx, |shell, cx| {
                assert_eq!(shell.sessions_open, open);
                assert_eq!(
                    shell.sidebar_visible_order(cx),
                    if open {
                        ids(&["newer", "older"])
                    } else {
                        Vec::new()
                    }
                );
                shell.sidebar_disclosure_motion.clear();
                cx.notify();
            });
        }
        if compact {
            let status = cx.debug_bounds("chat-status-older").unwrap();
            let time = cx.debug_bounds("chat-time-older").unwrap();
            assert!(status.right() < time.left());
            let row = cx.debug_bounds("chat-older").unwrap();
            let title = cx.debug_bounds("chat-title-older").unwrap();
            cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
            assert!(cx.debug_bounds("chat-title-older").unwrap().size.width < title.size.width);
            assert_eq!(cx.debug_bounds("chat-status-older").unwrap(), status);
            assert_eq!(cx.debug_bounds("chat-time-older").unwrap(), time);
        }

        // Dragging a regular session over another regular session is a no-op.
        let source_bounds = cx.debug_bounds("chat-older").unwrap();
        let from = source_bounds.center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let dragged_card = cx.debug_bounds("chat-older").unwrap();
        assert!(cx.debug_bounds("drag-chat-older").is_none());
        assert_eq!(dragged_card.size, source_bounds.size);
        assert_eq!(dragged_card.origin.x, source_bounds.origin.x);
        assert_eq!(
            f32::from(dragged_card.size.height),
            super::super::sidebar_row_height(compact, show_label, true, false)
        );
        // Even a sub-row move follows the pointer immediately, before a drop slot changes.
        let pointer = from + gpui::point(px(8.0), px(7.0));
        cx.simulate_mouse_move(pointer, Some(MouseButton::Left), gpui::Modifiers::default());
        let moving = cx.debug_bounds("chat-older").unwrap();
        assert!((f32::from(moving.origin.y - source_bounds.origin.y) - 7.0).abs() < 1.0);
        let slot = cx.debug_bounds("session-slot-newer").unwrap();
        let target = gpui::point(slot.center().x, slot.top() + px(3.0));
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            let drag = shell.sidebar_session_transfer.as_ref().unwrap();
            assert_eq!(drag.preview.as_ref().unwrap().group, "regular");
            assert!(drag.siblings["newer"].to > 0.0);
            assert!(shell.active_sidebar_pins(cx).is_empty());
        });
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        let returning_card = cx.debug_bounds("chat-older").unwrap();
        assert_eq!(returning_card.size, source_bounds.size);
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), Vec::<String>::new());
            assert!(shell.sidebar_session_return.is_some());
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["newer", "older"]));
            shell.sidebar_session_return = None;
            cx.notify();
        });

        // The transient empty Pinned header accepts the first pin.
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let target = cx.debug_bounds("pinned-toggle").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            let drag = shell.sidebar_session_transfer.as_mut().unwrap();
            assert_eq!(drag.source_collapse.to, drag.row_height + SIDEBAR_LIST_GAP);
            drag.source_collapse.started -= TAB_SLIDE.total();
            for gap in drag.section_gaps.values_mut() {
                gap.started -= TAB_SLIDE.total();
            }
            cx.notify();
        });
        assert_eq!(
            cx.debug_bounds("session-slot-older").unwrap().size.height,
            px(0.0)
        );
        // Only the surviving row (and minimum list spacing) remains, not a
        // second card-sized vacancy at the source.
        assert!(
            cx.debug_bounds("sidebar-regular-sessions")
                .unwrap()
                .size
                .height
                <= cx.debug_bounds("session-slot-newer").unwrap().size.height
                    + px(SIDEBAR_LIST_GAP)
        );
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older"]));
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["older", "newer"]));
            assert!(
                shell.sidebar_resort.is_empty(),
                "successful pin must not replay the movement"
            );
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // A single pin can leave Pinned and returns to its activity position.
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let slot = cx.debug_bounds("session-slot-newer").unwrap();
        let target = gpui::point(slot.center().x, slot.top() + px(3.0));
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert!(shell.sidebar_transfer_extra_gap("regular") > 0.0);
            assert!(shell.sidebar_session_transfer.as_ref().unwrap().siblings["newer"].to > 0.0);
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older"]));
        });
        shell.update(cx, |shell, cx| {
            let drag = shell.sidebar_session_transfer.as_mut().unwrap();
            assert_eq!(drag.source_collapse.to, drag.row_height);
            drag.source_collapse.started -= TAB_SLIDE.total();
            for gap in drag.section_gaps.values_mut() {
                gap.started -= TAB_SLIDE.total();
            }
            cx.notify();
        });
        assert_eq!(
            cx.debug_bounds("session-slot-older").unwrap().size.height,
            px(0.0)
        );
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert!(shell.active_sidebar_pins(cx).is_empty());
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["newer", "older"]));
            assert!(
                shell.sidebar_resort.is_empty(),
                "successful unpin must not replay the movement"
            );
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // A live activity update still wins while the normal session is dragged.
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        shell.update(cx, |shell, cx| {
            shell.state.update(cx, |state, _| {
                state
                    .chats
                    .iter_mut()
                    .find(|chat| chat.id == "older")
                    .unwrap()
                    .last_message_at = Some(Utc::now());
            });
            cx.notify();
        });
        let target = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert!(shell.active_sidebar_pins(cx).is_empty());
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["older", "newer"]));
            assert!(shell.sidebar_session_return.is_some());
        });

        // A closed Pinned header remains a valid target and opens on success.
        shell.update(cx, |shell, cx| {
            shell.pinned_open = false;
            cx.notify();
        });
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let target = cx.debug_bounds("pinned-toggle").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert!(shell.pinned_open);
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["newer"]));
            assert!(shell.sidebar_resort.is_empty());
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // Dropping outside the sections cancels; it must not unpin the source.
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let regular = cx.debug_bounds("session-slot-older").unwrap().center();
        cx.simulate_mouse_move(regular, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            let drag = shell.sidebar_session_transfer.as_mut().unwrap();
            assert!(drag.source_collapse.to > 0.0);
            drag.source_collapse.started -= TAB_SLIDE.total() / 2;
            for gap in drag.section_gaps.values_mut() {
                gap.started -= TAB_SLIDE.total() / 2;
            }
            cx.notify();
        });
        let outside = gpui::point(px(275.0), px(790.0));
        cx.simulate_mouse_move(outside, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(outside, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["newer"]));
            assert!(shell.sidebar_session_transfer.is_none());
            let returning = shell.sidebar_session_return.as_ref().unwrap();
            assert!(returning.transfer.source_collapse.from > 0.0);
            assert_eq!(returning.transfer.source_collapse.to, 0.0);
            assert!(
                returning
                    .transfer
                    .section_gaps
                    .values()
                    .all(|gap| gap.to == 0.0)
            );
        });

        // Remote pin changes invalidate an in-flight snapshot instead of being overwritten.
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        shell.update(cx, |shell, cx| {
            let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
            shell
                .settings
                .sidebar_pinned_session_ids_by_profile
                .insert(key, ids(&["older", "newer"]));
            cx.notify();
        });
        let target = cx.debug_bounds("pinned-toggle").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older", "newer"]));
            shell.sidebar_resort.clear();
            shell.sidebar_new_keys.clear();
            shell.reduced_motion = true;
            cx.notify();
        });

        // Even with no regular rows, a temporary destination can unpin one.
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let target = cx
            .debug_bounds("sidebar-regular-sessions")
            .unwrap()
            .center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older"]));
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["older", "newer"]));
            assert!(shell.sidebar_resort.is_empty());
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // Dropping in the lower half of a pinned row inserts after that row.
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let row = cx.debug_bounds("chat-older").unwrap();
        let target = gpui::point(row.center().x, row.bottom() - px(3.0));
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert!(shell.sidebar_transfer_extra_gap("pinned") > 0.0);
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older"]));
        });
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["older", "newer"]));
            assert!(shell.sidebar_resort.is_empty());
            assert!(shell.sidebar_new_keys.is_empty());
            cx.notify();
        });

        // Existing pin-to-pin reordering still works with the shared payload.
        shell.update(cx, |shell, cx| {
            shell.reduced_motion = false;
            cx.notify();
        });
        let from = cx.debug_bounds("chat-newer").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let row = cx.debug_bounds("chat-older").unwrap();
        let target = gpui::point(row.center().x, row.top() + px(3.0));
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.active_sidebar_pins(cx), ids(&["newer", "older"]));
            assert!(
                shell.sidebar_resort.is_empty(),
                "successful reorder must not replay the movement"
            );
            assert!(shell.sidebar_new_keys.is_empty());
            assert!(shell.sidebar_session_transfer.is_none());
            assert!(shell.sidebar_session_return.is_none());
        });

        // Suppressing a completed drag must not disable later activity-driven glides.
        shell.update(cx, |shell, cx| {
            let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
            for id in shell.active_sidebar_pins(cx) {
                shell.apply_sidebar_pin_change(
                    key.clone(),
                    zeron_proto::SidebarPinChange::Unpin { session_id: id },
                    cx,
                );
            }
            cx.notify();
        });
        let epoch = shell.update(cx, |shell, cx| {
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["older", "newer"]));
            shell.resort_epoch
        });
        shell.update(cx, |shell, cx| {
            shell.state.update(cx, |state, _| {
                state
                    .chats
                    .iter_mut()
                    .find(|chat| chat.id == "newer")
                    .unwrap()
                    .last_message_at = Some(Utc::now() + chrono::Duration::seconds(1));
            });
            cx.notify();
        });
        shell.update(cx, |shell, cx| {
            assert_eq!(shell.sidebar_visible_order(cx), ids(&["newer", "older"]));
            assert!(shell.resort_epoch > epoch);
            assert!(!shell.sidebar_resort.is_empty());
        });
        // A collapsed Sessions header remains an unpin target and opens on drop.
        shell.update(cx, |shell, cx| {
            let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
            *shell.settings.sidebar_pins_mut(key) = ids(&["older"]);
            shell.sidebar_session_return = None;
            shell.sidebar_resort.clear();
            shell.sidebar_disclosure_motion.clear();
            cx.notify();
        });
        let toggle = cx.debug_bounds("sessions-toggle").unwrap().center();
        cx.simulate_mouse_down(toggle, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_up(toggle, MouseButton::Left, gpui::Modifiers::default());
        shell.update(cx, |shell, cx| {
            assert!(!shell.sessions_open);
            shell.sidebar_disclosure_motion.clear();
            cx.notify();
        });
        let from = cx.debug_bounds("chat-older").unwrap().center();
        cx.simulate_mouse_down(from, MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            from + gpui::point(px(8.0), px(0.0)),
            Some(MouseButton::Left),
            gpui::Modifiers::default(),
        );
        let target = cx.debug_bounds("sessions-toggle").unwrap().center();
        cx.simulate_mouse_move(target, Some(MouseButton::Left), gpui::Modifiers::default());
        cx.simulate_mouse_up(target, MouseButton::Left, gpui::Modifiers::default());
        shell.read_with(cx, |shell, cx| {
            assert!(shell.sessions_open);
            assert!(shell.active_sidebar_pins(cx).is_empty());
        });
    }

    #[test]
    fn missing_duplicate_and_archived_pins_do_not_disturb_regular_rows() {
        let recency = ids(&["b", "a", "c"]);
        assert_eq!(
            project_pinned_first(&recency, &ids(&["archived", "a", "a"])),
            ids(&["a", "b", "c"])
        );
    }

    #[test]
    fn filtered_pin_reorder_preserves_every_other_pins_relative_order() {
        let saved = ids(&["a1", "b1", "a2", "archived", "b2"]);
        let visible = ids(&["a1", "a2"]);
        assert_eq!(
            reorder_visible_pins(&saved, &visible, 0, 1),
            ids(&["b1", "a2", "a1", "archived", "b2"])
        );
    }

    #[test]
    fn pin_reorder_rejects_invalid_or_noop_moves() {
        let saved = ids(&["a", "b"]);
        assert_eq!(reorder_visible_pins(&saved, &saved, 0, 0), saved);
        assert_eq!(reorder_visible_pins(&saved, &saved, 8, 0), saved);
    }

    #[test]
    fn pin_cleanup_retains_archived_and_prunes_deleted() {
        let mut saved = ids(&["active", "archived", "deleted", "active"]);
        let known = HashSet::from(["active".to_string(), "archived".to_string()]);
        assert!(retain_known_pins(&mut saved, &known));
        assert_eq!(saved, ids(&["active", "archived"]));
        assert!(!retain_known_pins(&mut saved, &known));
    }

    #[test]
    fn pin_cleanup_for_one_profile_leaves_other_profiles_untouched() {
        let mut settings = crate::settings::UiSettings::default();
        settings
            .sidebar_pins_mut("local".to_string())
            .extend(ids(&["local-active", "local-deleted"]));
        settings
            .sidebar_pins_mut("synced:org-1:user-1".to_string())
            .push("synced-pin".to_string());

        let known = HashSet::from(["local-active".to_string()]);
        assert!(retain_known_pins(
            settings.sidebar_pins_mut("local".to_string()),
            &known,
        ));
        assert_eq!(settings.sidebar_pins("local"), ["local-active"]);
        assert_eq!(settings.sidebar_pins("synced:org-1:user-1"), ["synced-pin"]);
    }

    #[test]
    fn sidebar_hit_testing_uses_compact_and_mixed_row_heights() {
        assert_eq!(super::row_drop_index(31.0, &[29.0, 29.0], false), Some(1));
        assert_eq!(super::row_drop_index(61.0, &[29.0, 29.0], false), None);
        assert_eq!(super::row_drop_index(48.0, &[45.0, 63.0], false), Some(1));
        assert_eq!(super::row_drop_index(-1.0, &[29.0], false), None);
        assert_eq!(super::row_drop_index(-1.0, &[29.0], true), Some(0));
        assert_eq!(super::row_drop_index(500.0, &[29.0, 45.0], true), Some(1));
    }

    #[test]
    fn pinned_drop_index_quantizes_clamps_and_rejects_outside() {
        assert_eq!(pinned_session_drop_index(-1.0, 3), None);
        assert_eq!(pinned_session_drop_index(0.0, 3), Some(0));
        assert_eq!(
            pinned_session_drop_index(super::super::SIDEBAR_SESSION_SLOT, 3),
            Some(1)
        );
        assert_eq!(pinned_session_drop_index(500.0, 3), None);
        assert_eq!(pinned_session_drop_index(0.0, 0), None);
    }

    #[test]
    fn pinned_drag_accounts_for_disclosure_header_and_scroll() {
        // The first row starts 36px below the scroll viewport: 4px list
        // padding, 28px header, and 4px body inset.
        let viewport_top = 100.0;
        let row_top = 136.0;
        assert_eq!(
            super::pinned_session_pointer_y(row_top, viewport_top, 0.0),
            0.0
        );
        assert_eq!(
            pinned_session_drop_index(super::pinned_session_pointer_y(125.0, viewport_top, 0.0), 3),
            None,
        );
        assert_eq!(
            pinned_session_drop_index(
                super::pinned_session_pointer_y(row_top, viewport_top, 63.0),
                3
            ),
            Some(1),
        );
    }

    #[test]
    fn sidebar_wide_pin_drag_clamps_to_the_nearest_pinned_slot() {
        assert_eq!(pinned_session_clamped_index(-50.0, 3), Some(0));
        assert_eq!(
            pinned_session_clamped_index(super::super::SIDEBAR_SESSION_SLOT, 3),
            Some(1)
        );
        assert_eq!(pinned_session_clamped_index(500.0, 3), Some(2));
        assert_eq!(pinned_session_clamped_index(0.0, 0), None);
    }

    #[test]
    fn session_transfers_only_change_pin_membership_and_order() {
        use super::{SidebarSessionDrop, sidebar_session_drop_pins};
        let saved = ids(&["hidden", "a", "b", "hidden-tail"]);
        let visible = ids(&["a", "b"]);
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "normal", SidebarSessionDrop::Regular),
            saved
        );
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "normal", SidebarSessionDrop::Pinned(1)),
            ids(&["hidden", "a", "normal", "b", "hidden-tail"])
        );
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "normal", SidebarSessionDrop::Pinned(2)),
            ids(&["hidden", "a", "b", "normal", "hidden-tail"])
        );
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "a", SidebarSessionDrop::Regular),
            ids(&["hidden", "b", "hidden-tail"])
        );
        assert_eq!(
            sidebar_session_drop_pins(&saved, &visible, "a", SidebarSessionDrop::Pinned(1)),
            ids(&["hidden", "b", "a", "hidden-tail"])
        );
        assert_eq!(
            sidebar_session_drop_pins(&[], &[], "first", SidebarSessionDrop::Pinned(0)),
            ids(&["first"])
        );
        assert!(
            sidebar_session_drop_pins(
                &ids(&["only"]),
                &ids(&["only"]),
                "only",
                SidebarSessionDrop::Regular
            )
            .is_empty()
        );
    }

    #[test]
    fn pinned_edge_scroll_is_proportional_and_lifecycle_bound() {
        let top = 100.0;
        let bottom = 300.0;
        assert_eq!(pinned_drag_scroll_delta(200.0, top, bottom), 0.0);
        assert_eq!(pinned_drag_scroll_delta(124.0, top, bottom), -6.0);
        assert_eq!(pinned_drag_scroll_delta(276.0, top, bottom), 6.0);
        assert_eq!(
            pinned_drag_scroll_step(true, 4, 4, 20.0, 100.0, 6.0),
            Some(26.0)
        );
        assert_eq!(pinned_drag_scroll_step(false, 4, 4, 20.0, 100.0, 6.0), None);
        assert_eq!(pinned_drag_scroll_step(true, 3, 4, 20.0, 100.0, 6.0), None);
    }

    #[test]
    fn pinned_drag_snapshot_requires_the_same_remote_order() {
        let snapshot = ids(&["a", "b"]);
        assert!(pinned_drag_snapshot_is_valid("a", &snapshot, &snapshot));
        assert!(!pinned_drag_snapshot_is_valid(
            "a",
            &snapshot,
            &ids(&["b", "a"])
        ));
        assert!(!pinned_drag_snapshot_is_valid("a", &snapshot, &ids(&["a"])));
    }
}

pub(super) fn pinned_drag_scroll_step(
    drag_active: bool,
    loop_generation: u64,
    drag_generation: u64,
    current: f32,
    max: f32,
    delta: f32,
) -> Option<f32> {
    if !drag_active || loop_generation != drag_generation || delta == 0.0 {
        return None;
    }
    let next = (current + delta).clamp(0.0, max.max(0.0));
    (next != current).then_some(next)
}

pub(super) fn pinned_drag_snapshot_is_valid(
    dragged_id: &str,
    snapshot_ids: &[String],
    current_ids: &[String],
) -> bool {
    snapshot_ids.iter().any(|id| id == dragged_id) && snapshot_ids == current_ids
}

struct ActiveChatRow {
    status: ChatIndicator,
    chat: zeron_proto::Chat,
    folder: String,
    branch: Option<String>,
    change_request: Option<zeron_proto::ChangeRequestSummary>,
    group: Option<(String, String)>,
}

pub(super) fn compare_sidebar_chats(
    sort: SidebarSort,
    left: &zeron_proto::Chat,
    right: &zeron_proto::Chat,
) -> std::cmp::Ordering {
    let primary = match sort {
        SidebarSort::Created => right.created_at.cmp(&left.created_at),
        SidebarSort::LastUpdated => right
            .last_message_at
            .unwrap_or(right.created_at)
            .cmp(&left.last_message_at.unwrap_or(left.created_at)),
    };
    primary.then_with(|| left.id.cmp(&right.id))
}

/// The space-filter dropdown, `Some` while open. The same searchable-menu
/// recipe as the composer's ref picker: filter input on top
/// (`PaletteSearch` context so ↑↓/⏎ bubble to the card), ranked substring
/// rows, keyboard highlight.
pub(super) struct SpacesMenu {
    search: Entity<ComposerInput>,
    /// Keyboard highlight — an index into [`Shell::spaces_menu_rows`], or
    /// that list's length when the pinned "New project…" footer holds it.
    active: usize,
    /// Tracked on the card — puts it on the keyboard dispatch path while the
    /// search input holds focus (the structure every working picker uses).
    focus: FocusHandle,
    list_scroll: gpui::ScrollHandle,
    _search_events: Subscription,
}

pub(super) struct SidebarViewMenu {
    /// Keyboard cursor. Mouse-opened menus start without one so the persisted
    /// radio/check state is the only selection signal until an arrow key is
    /// pressed.
    active: Option<usize>,
    focus: FocusHandle,
}

struct SidebarViewOptionsTooltip;

impl Render for SidebarViewOptionsTooltip {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(6.0))
            .border_1()
            .border_color(theme.border_strong)
            .bg(theme.surface_raised)
            .shadow_md()
            .text_size(crate::typography::ui_rems(11.0))
            .text_color(theme.text)
            .child("Sidebar view options")
    }
}

#[derive(Clone, Copy)]
enum SidebarViewRow {
    ByProject,
    Compact,
    ShowProjectIcon,
    ShowProjectLabel,
    ByDevice,
    InOneList,
    LastUpdated,
    Created,
    ShowBranch,
    ShowPullRequest,
    ShowHarness,
}

impl SidebarViewRow {
    /// Radio-style presentation choices behave like the project selector and
    /// dismiss after selection. Show toggles stay open for batch changes.
    fn closes_menu(self) -> bool {
        matches!(
            self,
            Self::ByProject | Self::ByDevice | Self::InOneList | Self::LastUpdated | Self::Created
        )
    }
}

const SIDEBAR_VIEW_ROWS: [SidebarViewRow; 11] = [
    SidebarViewRow::ByDevice,
    SidebarViewRow::ByProject,
    SidebarViewRow::InOneList,
    SidebarViewRow::LastUpdated,
    SidebarViewRow::Created,
    SidebarViewRow::ShowBranch,
    SidebarViewRow::ShowPullRequest,
    SidebarViewRow::ShowHarness,
    SidebarViewRow::ShowProjectIcon,
    SidebarViewRow::ShowProjectLabel,
    SidebarViewRow::Compact,
];

// list items stay tightly related at 2px, while section boundaries use 12px
// (well over 2x the intra-list gap). Disclosure content gets a small 4px
// handoff from its header without leaving dead space while collapsed.
const SIDEBAR_SECTION_GAP: f32 = 12.0;
pub(super) const SIDEBAR_DISCLOSURE_HEADER_HEIGHT: f32 = 28.0;
pub(super) const SIDEBAR_DISCLOSURE_BODY_INSET: f32 = 4.0;
const SIDEBAR_DISCLOSURE_SECTION_HEIGHT: f32 =
    SIDEBAR_SECTION_GAP + SIDEBAR_DISCLOSURE_HEADER_HEIGHT;
pub(super) const SIDEBAR_DISCLOSURE_TWEEN_GRACE: std::time::Duration =
    std::time::Duration::from_millis(120);

/// Put this machine's device group first without disturbing the recency-based
/// order of any remote groups. A targeted promotion is more truthful than a
/// full name sort: local context leads, then the user's chosen chat sort wins.
fn promote_local_device_group<T>(
    groups: &mut Vec<(Option<(String, String)>, Vec<T>)>,
    local_device_id: Option<&str>,
) {
    let Some(local_device_id) = local_device_id else {
        return;
    };
    let Some(index) = groups.iter().position(|(group, _)| {
        group
            .as_ref()
            .is_some_and(|(device_id, _)| device_id == local_device_id)
    }) else {
        return;
    };
    if index > 0 {
        let local = groups.remove(index);
        groups.insert(0, local);
    }
}

/// Shared quiet rule for sidebar groups and palette sections.
pub(super) fn sidebar_separator(theme: &Theme) -> gpui::Div {
    div().h(px(1.0)).bg(theme.border.opacity(0.6))
}

fn sidebar_disclosure_header(theme: &Theme, label: SharedString, chevron: AnyElement) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .h(px(SIDEBAR_DISCLOSURE_HEADER_HEIGHT))
        .px(px(Theme::SPACE_SM))
        .cursor_pointer()
        .child(super::sidebar_faded_label(
            "sidebar-disclosure-label".into(),
            false,
            div()
                .text_size(crate::typography::ui_rems(12.0))
                .font_weight(gpui::FontWeight::MEDIUM)
                .text_color(theme.text_muted.opacity(0.5))
                .child(label),
        ))
        .child(div().flex_1())
        .child(chevron)
}

/// One activatable row of the open dropdown, in nav order. `AddSpace` names
/// the card's pinned "New project…" footer, not a list row — keyboard nav
/// maps the list-length index to it.
#[derive(Clone, PartialEq)]
pub(super) enum SpacesMenuRow {
    All,
    Space(String),
    AddSpace,
}

/// New project navigates devices, locations, then folders on a command-palette surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProjectStep {
    Devices,
    Locations,
    Folders,
}

pub(super) struct AddSpaceFlow {
    step: ProjectStep,
    location: Option<(String, Option<String>)>,
    /// The selected device.
    device: Option<Device>,
    /// Filter input; Enter descends into the highlighted folder. Carries the
    /// tab-completion ghost (the faint suffix ⇥ accepts), and a trailing `/`
    /// on a folder-naming query descends immediately.
    search: Entity<ComposerInput>,
    browser: Loadable<FolderListing>,
    /// The selected device's mounted drives/volumes.
    /// Best-effort: an error just leaves the section at Home only.
    drives: Loadable<Vec<DriveEntry>>,
    /// Requested browser path (`None` = the device's default, i.e. home).
    browser_path: Option<String>,
    /// The device's home (the path a `None` browse resolved to) — breadcrumbs
    /// fold everything up to here into the Home crumb.
    home: Option<String>,
    /// Best-effort git seed for the CURRENT browser path (known when we
    /// descended through an entry whose `is_repo` we saw; the owning device's
    /// SpacesSync re-verifies either way).
    browser_repo: bool,
    /// Keyboard highlight within the current step’s filtered rows.
    active: usize,
    submit_busy: bool,
    error: Option<SharedString>,
    /// Tracked on the card (`track_focus`) — puts the card on the keyboard
    /// dispatch path so ↑↓/⌫/esc reach `add_space_key` while the search input
    /// holds focus (the structure every working picker uses).
    focus: FocusHandle,
    /// Folder-list scroll — keyboard navigation keeps the highlighted row in
    /// view (`scroll_to_item`).
    list_scroll: gpui::ScrollHandle,
    focus_pending: bool,
    load_task: Option<Task<()>>,
    drives_task: Option<Task<()>>,
    submit_task: Option<Task<()>>,
    _search_events: Subscription,
}

/// Segment-aware "is `path` at or under `base`" (`/media/a` is not under
/// `/media/ab`); a root base covers everything.
fn path_under(path: &str, base: &str) -> bool {
    let base = base.trim_end_matches('/');
    base.is_empty() || path == base || path.starts_with(&format!("{base}/"))
}

/// The space-row Rename dialog (same shape as [`RenameChatDialog`]).
pub(super) struct RenameSpaceDialog {
    pub space_id: String,
    pub input: Entity<ComposerInput>,
    pub focus_pending: bool,
    pub _events: Subscription,
}

/// Dot color for a chat's display status (tab dots + Sessions rows).
pub(super) fn status_dot_color(status: ChatIndicator, theme: &Theme) -> gpui::Hsla {
    match status {
        // Preset activity tone, not warning amber: running is routine.
        // Non-done statuses sit well below full
        // strength: at full alpha the colored words shouted across the
        // whole sidebar (user request) — only Done keeps its pop.
        ChatIndicator::Working => theme.busy.opacity(0.55),
        // Blue: "asking you a question" must read differently from "busy
        // working" at a glance.
        ChatIndicator::AwaitingInput => theme.accent.opacity(0.6),
        ChatIndicator::Errored => theme.danger.opacity(0.65),
        // Green: finished-but-unseen reads as "ready for you".
        ChatIndicator::Completed => {
            theme.success.opacity(0.9) // emerald-400
        }
        ChatIndicator::Idle => crate::theme::ink(0.14),
    }
}

// Handle-based rail host for the spaces dropdown: its list is a plain
// tracked scroller, so the trait's default metrics/press/drag (off the live
// ScrollHandle) apply unchanged.
impl popover::ScrollRailHost for Shell {
    fn rail_bar(&mut self) -> &mut popover::MenuScrollbarState {
        &mut self.spaces_menu_bar
    }

    fn rail_scroll(&self) -> Option<gpui::ScrollHandle> {
        self.spaces_menu.get().map(|menu| menu.list_scroll.clone())
    }
}

impl Shell {
    fn begin_sidebar_disclosure_motion(
        &mut self,
        key: &str,
        resting_height: f32,
        target_height: f32,
    ) {
        let previous = self.sidebar_disclosure_motion.get(key).copied();
        let from = previous
            .filter(|motion| motion.animating())
            .map(SidebarDisclosureMotion::current)
            .unwrap_or(resting_height);
        let epoch = previous.map_or(1, |motion| motion.epoch + 1);
        self.sidebar_disclosure_motion.insert(
            key.to_owned(),
            SidebarDisclosureMotion::new(epoch, from, target_height),
        );
    }

    fn render_sidebar_disclosure_body(
        &self,
        key: &str,
        open: bool,
        full_height: f32,
        content: AnyElement,
    ) -> AnyElement {
        let target = if open { full_height } else { 0.0 };
        let frame = div().w_full().flex_none().overflow_hidden().child(content);
        let Some(tween) = self
            .sidebar_disclosure_motion
            .get(key)
            .copied()
            .filter(|motion| motion.animating())
        else {
            return frame.h(px(target)).into_any_element();
        };
        let denominator = full_height.max(1.0);
        frame
            .with_animation(
                SharedString::from(format!("sidebar-disclosure-{key}-{}", tween.epoch)),
                motion::COLLAPSE.animation(),
                move |el, t| {
                    let height = motion::lerp(tween.from, tween.to, t);
                    let reveal = (height / denominator).clamp(0.0, 1.0);
                    el.h(px(height))
                        .opacity(0.35 + 0.65 * reveal)
                        .relative()
                        .top(px(-3.0 * (1.0 - reveal)))
                },
            )
            .into_any_element()
    }

    fn sidebar_disclosure_chevron(&self, key: &str, open: bool, theme: &Theme) -> AnyElement {
        let resting_reveal = if open { 1.0 } else { 0.0 };
        let chevron = icon(icons::ALT_ARROW_RIGHT)
            .size(px(12.0))
            .text_color(theme.text_muted.opacity(0.5));
        if let Some(tween) = self
            .sidebar_disclosure_motion
            .get(key)
            .copied()
            .filter(|motion| motion.animating())
        {
            let denominator = tween.from.max(tween.to).max(1.0);
            let from = (tween.from / denominator).clamp(0.0, 1.0);
            let to = (tween.to / denominator).clamp(0.0, 1.0);
            div()
                .flex_none()
                .size(px(12.0))
                .child(chevron.with_animation(
                    SharedString::from(format!("sidebar-chevron-{key}-{}", tween.epoch)),
                    motion::COLLAPSE.animation(),
                    move |el, t| {
                        let reveal = motion::lerp(from, to, t);
                        el.with_transformation(gpui::Transformation::rotate(gpui::percentage(
                            reveal * 0.25,
                        )))
                    },
                ))
                .into_any_element()
        } else {
            div()
                .flex_none()
                .size(px(12.0))
                .child(
                    chevron.with_transformation(gpui::Transformation::rotate(gpui::percentage(
                        resting_reveal * 0.25,
                    ))),
                )
                .into_any_element()
        }
    }
    // ---- space filter ----

    /// Set the sidebar's session filter (`None` = All spaces). On the
    /// new-session canvas the space context follows the filter — the canvas
    /// default is "the space you're looking at".
    pub(super) fn set_space_filter(&mut self, filter: Option<String>, cx: &mut Context<Self>) {
        if self.settings.space_filter != filter {
            self.cancel_pinned_session_drag(cx);
        }
        self.settings.space_filter = filter.clone();
        if let Some(space_id) = filter
            && self.state.read(cx).selected_chat.is_none()
        {
            self.state
                .update(cx, |s, cx| s.select_space(Some(space_id), cx));
        }
        self.close_spaces_menu(cx);
        self.schedule_save(cx);
        cx.notify();
    }

    /// Close the space-filter dropdown through the exit animation (no-op when
    /// it isn't open). Every close path funnels here so the menu always
    /// animates out instead of vanishing.
    pub(super) fn close_spaces_menu(&mut self, cx: &mut Context<Self>) {
        if self.spaces_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.spaces_menu);
            cx.notify();
        }
    }

    fn on_spaces_menu_list_hover(
        &mut self,
        hovered: &bool,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.spaces_menu_bar.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    /// Open the new-session canvas in a just-added space, preserving the
    /// sidebar's current project filter.
    pub(super) fn land_in_space(&mut self, space_id: String, cx: &mut Context<Self>) {
        self.cancel_pinned_session_drag(cx);
        self.set_route(Route::Chat, cx);
        self.focus_composer(cx);
        self.nav.push(NavEntry::Chat(String::new()));
        // "All" stays as-is; an explicit project filter follows the new
        // project so the first send lands in a visible session.
        if self.settings.space_filter.is_some() {
            self.settings.space_filter = Some(space_id.clone());
        }
        self.settings.last_space_id = Some(space_id.clone());
        self.state.update(cx, |s, cx| {
            s.select_space(Some(space_id), cx);
            s.select_chat(None, cx);
        });
        self.schedule_save(cx);
        cx.notify();
    }

    // ---- sidebar sections ----

    pub(super) fn sidebar_transfer_extra_gap(&self, group: &str) -> f32 {
        self.sidebar_session_transfer
            .as_ref()
            .or_else(|| {
                self.sidebar_session_return
                    .as_ref()
                    .map(|state| &state.transfer)
            })
            .and_then(|drag| drag.section_gaps.get(group))
            .map_or(0.0, |gap| {
                if self.reduced_motion {
                    gap.to
                } else {
                    gap.current()
                }
            })
    }

    fn render_sidebar_gap_row(
        &mut self,
        row: AnyElement,
        id: &str,
        group: &str,
        index: usize,
    ) -> AnyElement {
        let returning = self.sidebar_session_transfer.is_none();
        let transfer = if let Some(drag) = self.sidebar_session_transfer.as_mut() {
            Some(drag)
        } else {
            self.sidebar_session_return
                .as_mut()
                .map(|returning| &mut returning.transfer)
        };
        let Some(drag) = transfer else {
            return row;
        };
        // Pin-to-pin keeps the existing sibling-slide implementation.
        let target = if returning || (group == "pinned" && drag.source_group == "pinned") {
            0.0
        } else {
            drag.preview
                .as_ref()
                .filter(|gap| gap.group == group)
                .map_or(0.0, |gap| {
                    sidebar_gap_offset(
                        index,
                        (drag.source_group == group).then_some(drag.source_index),
                        gap.index,
                        drag.row_height + SIDEBAR_LIST_GAP,
                    )
                })
        };
        let slide = drag
            .siblings
            .entry(id.to_owned())
            .or_insert_with(|| SidebarSessionSlide {
                from: 0.0,
                to: 0.0,
                epoch: drag.slide.epoch,
                started: std::time::Instant::now(),
            });
        slide.retarget(target);
        let (from, to, epoch) = (slide.from, slide.to, slide.epoch);
        let frame = div().relative().child(row);
        if !returning || self.reduced_motion {
            frame.top(px(to)).into_any_element()
        } else {
            frame
                .with_animation(
                    SharedString::from(format!("session-gap-{id}-{epoch}")),
                    TAB_SLIDE.animation(),
                    move |el, t| el.top(px(motion::lerp(from, to, t))),
                )
                .into_any_element()
        }
    }

    pub(super) fn begin_sidebar_session_transfer(
        &mut self,
        payload: &SidebarSessionDrag,
        cursor_offset: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        self.chat_status_hover = None;
        self.cancel_pinned_session_drag(cx);
        self.sidebar_session_return = None;
        self.pinned_session_drag_generation = self.pinned_session_drag_generation.wrapping_add(1);
        let top = f32::from(
            window.mouse_position().y - cursor_offset.y - self.sidebar_scroll.bounds().top(),
        ) - f32::from(self.sidebar_scroll.offset().y);
        self.sidebar_session_transfer = Some(SidebarSessionTransfer {
            payload: payload.clone(),
            origin: std::rc::Rc::new(std::cell::Cell::new(
                window.mouse_position() - cursor_offset,
            )),
            cursor_offset,
            pointer: window.mouse_position(),
            viewport: None,
            preview: None,
            source_group: String::new(),
            source_index: 0,
            row_height: 0.0,
            source_collapse: SidebarSessionSlide {
                from: 0.0,
                to: 0.0,
                epoch: 0,
                started: std::time::Instant::now(),
            },
            collapsed_height: 0.0,
            section_gaps: Default::default(),
            siblings: Default::default(),
            slide: SidebarSessionSlide {
                from: top,
                to: top,
                epoch: self.pinned_session_drag_generation << 32,
                started: std::time::Instant::now(),
            },
        });
        {
            let origin = self
                .sidebar_session_transfer
                .as_ref()
                .unwrap()
                .origin
                .clone();
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(
                            SIDEBAR_DRAG_SCROLL_FRAME_MS,
                        ))
                        .await;
                    let keep_running = this
                        .update(cx, |shell, cx| {
                            let Some(transfer) = shell.sidebar_session_transfer.as_ref() else {
                                return false;
                            };
                            if !cx.has_active_drag()
                                || !std::rc::Rc::ptr_eq(&origin, &transfer.origin)
                            {
                                return false;
                            }
                            // Layout also animates while the pointer is stationary.
                            cx.notify();
                            // Pinned drags already have their own edge-scroll loop.
                            if transfer
                                .payload
                                .visible_ids
                                .contains(&transfer.payload.chat_id)
                            {
                                return true;
                            }
                            let Some(viewport) = transfer.viewport else {
                                return true;
                            };
                            if !viewport.contains(&transfer.pointer) {
                                return true;
                            }
                            let delta = pinned_drag_scroll_delta(
                                f32::from(transfer.pointer.y),
                                f32::from(viewport.top()),
                                f32::from(viewport.bottom()),
                            );
                            let offset = shell.sidebar_scroll.offset();
                            let scroll_top = -f32::from(offset.y);
                            let max_scroll = f32::from(shell.sidebar_scroll.max_offset().y);
                            let next = (scroll_top + delta).clamp(0.0, max_scroll);
                            if next != scroll_top {
                                shell
                                    .sidebar_scroll
                                    .set_offset(gpui::point(offset.x, px(-next)));
                                cx.notify();
                            }
                            true
                        })
                        .unwrap_or(false);
                    if !keep_running {
                        break;
                    }
                }
            })
            .detach();
        }
        cx.notify();
    }

    pub(super) fn sidebar_session_transfer_is_valid(
        &self,
        payload: &SidebarSessionDrag,
        cx: &App,
    ) -> bool {
        if payload.filter != self.settings.space_filter
            || self.active_sidebar_pin_profile_key(cx).as_deref() != Some(&payload.profile_key)
        {
            return false;
        }
        let state = self.state.read(cx);
        let visible: HashSet<String> = state
            .sidebar_chats(Utc::now(), payload.filter.as_deref())
            .into_iter()
            .map(|(_, chat)| chat.id.clone())
            .collect();
        if !visible.contains(&payload.chat_id) {
            return false;
        }
        let pins = self.sidebar_pins_for_profile(&payload.profile_key, cx);
        let current: Vec<_> = pins.into_iter().filter(|id| visible.contains(id)).collect();
        current == *payload.visible_ids
    }

    pub(super) fn cancel_sidebar_session_transfer(&mut self, cx: &mut Context<Self>) {
        self.chat_status_hover = None;
        self.cancel_pinned_session_drag(cx);
        if let Some(mut transfer) = self.sidebar_session_transfer.take() {
            transfer.preview = None;
            if !self.reduced_motion && self.sidebar_session_transfer_is_valid(&transfer.payload, cx)
            {
                self.sidebar_session_return = Some(SidebarSessionReturn {
                    transfer,
                    epoch: self.pinned_session_drag_generation,
                    started: std::time::Instant::now(),
                });
                let epoch = self.pinned_session_drag_generation;
                cx.spawn(async move |this, cx| {
                    loop {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(
                                SIDEBAR_DRAG_SCROLL_FRAME_MS,
                            ))
                            .await;
                        let keep_running = this
                            .update(cx, |shell, cx| {
                                let Some(returning) = shell.sidebar_session_return.as_ref() else {
                                    return false;
                                };
                                if returning.epoch != epoch {
                                    return false;
                                }
                                if returning.started.elapsed() >= TAB_SLIDE.total()
                                    && returning.transfer.slide.started.elapsed()
                                        >= TAB_SLIDE.total()
                                    && returning.transfer.source_collapse.started.elapsed()
                                        >= TAB_SLIDE.total()
                                    && returning
                                        .transfer
                                        .section_gaps
                                        .values()
                                        .all(|gap| gap.started.elapsed() >= TAB_SLIDE.total())
                                {
                                    shell.sidebar_session_return = None;
                                    cx.notify();
                                    return false;
                                }
                                cx.notify();
                                true
                            })
                            .unwrap_or(false);
                        if !keep_running {
                            break;
                        }
                    }
                })
                .detach();
                self.pinned_session_drag_generation =
                    self.pinned_session_drag_generation.wrapping_add(1);
            }
            cx.notify();
        }
    }

    pub(super) fn render_moving_sidebar_session(
        &mut self,
        row: AnyElement,
        height: f32,
        theme: &Theme,
    ) -> AnyElement {
        let viewport = self.sidebar_scroll.bounds();
        let scroll_top = -f32::from(self.sidebar_scroll.offset().y);
        let returning = self.sidebar_session_transfer.is_none();
        let transfer = if let Some(transfer) = self.sidebar_session_transfer.as_mut() {
            transfer
        } else {
            &mut self.sidebar_session_return.as_mut().unwrap().transfer
        };
        let origin = f32::from(transfer.origin.get().y - viewport.top()) + scroll_top;
        let target = if returning {
            origin
        } else {
            f32::from(transfer.pointer.y - transfer.cursor_offset.y - viewport.top()) + scroll_top
        };
        if returning {
            transfer.slide.retarget(target);
        } else {
            transfer.slide.from = target;
            transfer.slide.to = target;
            transfer.slide.started = std::time::Instant::now();
        }
        let from = transfer.slide.from;
        let to = transfer.slide.to;
        let epoch = transfer.slide.epoch;
        let frame = div()
            .absolute()
            .left(px(Theme::SPACE_SM))
            .right(px(Theme::SPACE_SM))
            .h(px(height))
            .rounded(px(8.0))
            .when(!returning, |el| el.bg(theme.surface_raised).shadow_md())
            .child(row);
        if self.reduced_motion {
            frame.top(px(to)).into_any_element()
        } else {
            frame
                .with_animation(
                    ("sidebar-session-slide", epoch),
                    TAB_SLIDE.animation(),
                    move |el, t| el.top(px(motion::lerp(from, to, t))),
                )
                .into_any_element()
        }
    }

    pub(super) fn finish_sidebar_session_transfer(
        &mut self,
        payload: &SidebarSessionDrag,
        target: SidebarSessionDrop,
        cx: &mut Context<Self>,
    ) {
        self.chat_status_hover = None;
        let matches_drag = self.sidebar_session_transfer.as_ref().is_some_and(|drag| {
            drag.payload.chat_id == payload.chat_id
                && drag.payload.profile_key == payload.profile_key
                && drag.payload.filter == payload.filter
                && drag.payload.visible_ids == payload.visible_ids
        });
        if !matches_drag || !self.sidebar_session_transfer_is_valid(payload, cx) {
            self.cancel_sidebar_session_transfer(cx);
            return;
        }
        let saved = self.sidebar_pins_for_profile(&payload.profile_key, cx);
        let next =
            sidebar_session_drop_pins(&saved, &payload.visible_ids, &payload.chat_id, target);
        // Validate and accept before ending the preview. Rejected drops use
        // the same animated return path as dropping outside a destination.
        let change = if let Some(index) = next.iter().position(|id| id == &payload.chat_id) {
            let after = index.checked_sub(1).and_then(|i| next.get(i)).cloned();
            let before = next.get(index + 1).cloned();
            if saved.contains(&payload.chat_id) {
                zeron_proto::SidebarPinChange::Move {
                    session_id: payload.chat_id.clone(),
                    after,
                    before,
                }
            } else {
                zeron_proto::SidebarPinChange::Pin {
                    session_id: payload.chat_id.clone(),
                    after,
                    before,
                }
            }
        } else {
            zeron_proto::SidebarPinChange::Unpin {
                session_id: payload.chat_id.clone(),
            }
        };
        if !self.apply_sidebar_pin_change(payload.profile_key.clone(), change, cx) {
            self.cancel_sidebar_session_transfer(cx);
            return;
        }
        self.sidebar_session_transfer = None;
        self.cancel_pinned_session_drag(cx);
        if matches!(target, SidebarSessionDrop::Pinned(_)) {
            self.pinned_open = true;
        } else {
            self.sessions_open = true;
        }
        // Only pin preferences change. Regular rows keep their live activity sort.
        // Drag previews already animated this move. Establish a fresh layout
        // baseline so the automatic resort glide does not replay it on release.
        self.sidebar_prev_order.clear();
        self.sidebar_resort.clear();
        self.sidebar_new_keys.clear();
        cx.notify();
    }

    pub(super) fn update_pinned_session_drag(
        &mut self,
        payload: &SidebarSessionDrag,
        over: usize,
        cx: &mut Context<Self>,
    ) {
        if !self.pinned_open
            || self.active_sidebar_pin_profile_key(cx).as_deref() != Some(&payload.profile_key)
        {
            self.cancel_pinned_session_drag(cx);
            return;
        }
        let Some(from) = payload
            .visible_ids
            .iter()
            .position(|id| id == &payload.chat_id)
        else {
            self.cancel_pinned_session_drag(cx);
            return;
        };
        if over >= payload.visible_ids.len() {
            self.cancel_pinned_session_drag(cx);
            return;
        }
        match &mut self.pinned_session_drag {
            Some(drag)
                if drag.chat_id == payload.chat_id
                    && drag.filter == payload.filter
                    && drag.profile_key == payload.profile_key
                    && drag.visible_ids.as_ref() == payload.visible_ids.as_ref()
                    && drag.over != over =>
            {
                drag.prev_over = drag.over;
                drag.over = over;
                drag.epoch = drag.epoch.wrapping_add(1);
                cx.notify();
            }
            Some(drag)
                if drag.chat_id == payload.chat_id
                    && drag.filter == payload.filter
                    && drag.profile_key == payload.profile_key
                    && drag.visible_ids.as_ref() == payload.visible_ids.as_ref() => {}
            _ => {
                self.pinned_session_drag_generation =
                    self.pinned_session_drag_generation.wrapping_add(1);
                self.pinned_session_drag = Some(PinnedSessionDragState {
                    chat_id: payload.chat_id.clone(),
                    visible_ids: payload.visible_ids.clone(),
                    from,
                    over,
                    prev_over: from,
                    epoch: 0,
                    filter: payload.filter.clone(),
                    profile_key: payload.profile_key.clone(),
                    pointer_y: None,
                    viewport_top: 0.0,
                    viewport_bottom: 0.0,
                    generation: self.pinned_session_drag_generation,
                    autoscroll_active: false,
                });
                cx.notify();
            }
        }
    }

    pub(super) fn track_pinned_session_drag_pointer(
        &mut self,
        payload: SidebarSessionDrag,
        pointer_y: f32,
        viewport_top: f32,
        viewport_bottom: f32,
        cx: &mut Context<Self>,
    ) {
        if !payload.visible_ids.contains(&payload.chat_id) {
            return;
        }
        let scroll_top = -f32::from(self.sidebar_scroll.offset().y);
        let rel_y = pinned_session_pointer_y(pointer_y, viewport_top, scroll_top);
        let inside_pinned_section =
            row_drop_index(rel_y, &self.sidebar_pinned_heights, false).is_some();
        let Some(over) = row_drop_index(rel_y, &self.sidebar_pinned_heights, true) else {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.pointer_y = None;
                drag.autoscroll_active = false;
            }
            return;
        };

        self.update_pinned_session_drag(&payload, over, cx);
        if !inside_pinned_section {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.pointer_y = None;
                drag.autoscroll_active = false;
            }
            return;
        }
        let delta = pinned_drag_scroll_delta(pointer_y, viewport_top, viewport_bottom);
        let Some(drag) = self.pinned_session_drag.as_mut() else {
            return;
        };
        drag.pointer_y = Some(pointer_y);
        drag.viewport_top = viewport_top;
        drag.viewport_bottom = viewport_bottom;
        let should_start = delta != 0.0 && !drag.autoscroll_active;
        let generation = drag.generation;
        if should_start {
            drag.autoscroll_active = true;
            self.start_pinned_session_autoscroll(generation, cx);
        }
    }

    fn start_pinned_session_autoscroll(&mut self, generation: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(
                        super::SIDEBAR_DRAG_SCROLL_FRAME_MS,
                    ))
                    .await;
                let keep_running = this
                    .update(cx, |shell, cx| {
                        shell.pinned_session_autoscroll_tick(generation, cx)
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
    }

    fn pinned_session_autoscroll_tick(&mut self, generation: u64, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.pinned_session_drag.as_ref() else {
            return false;
        };
        if drag.generation != generation || !cx.has_active_drag() {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.autoscroll_active = false;
            }
            return false;
        }
        let Some(pointer_y) = drag.pointer_y else {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.autoscroll_active = false;
            }
            return false;
        };
        let viewport_top = drag.viewport_top;
        let viewport_bottom = drag.viewport_bottom;
        let delta = pinned_drag_scroll_delta(pointer_y, viewport_top, viewport_bottom);
        let scroll_top = -f32::from(self.sidebar_scroll.offset().y);
        let max_scroll = f32::from(self.sidebar_scroll.max_offset().y);
        let Some(next_scroll) = pinned_drag_scroll_step(
            true,
            generation,
            drag.generation,
            scroll_top,
            max_scroll,
            delta,
        ) else {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.autoscroll_active = false;
            }
            return false;
        };

        let rel_y = pinned_session_pointer_y(pointer_y, viewport_top, next_scroll);
        let Some(over) = row_drop_index(rel_y, &self.sidebar_pinned_heights, false) else {
            if let Some(drag) = self.pinned_session_drag.as_mut() {
                drag.autoscroll_active = false;
            }
            return false;
        };

        let offset = self.sidebar_scroll.offset();
        self.sidebar_scroll
            .set_offset(gpui::point(offset.x, px(-next_scroll)));
        if let Some(drag) = self.pinned_session_drag.as_mut()
            && drag.over != over
        {
            drag.prev_over = drag.over;
            drag.over = over;
            drag.epoch = drag.epoch.wrapping_add(1);
        }
        cx.notify();
        true
    }

    pub(super) fn pinned_session_drag_is_valid(&self, cx: &App) -> bool {
        let Some(drag) = self.pinned_session_drag.as_ref() else {
            return true;
        };
        if !self.pinned_open
            || drag.filter != self.settings.space_filter
            || self.active_sidebar_pin_profile_key(cx).as_deref() != Some(&drag.profile_key)
        {
            return false;
        }
        let current_pins = self.sidebar_pins_for_profile(&drag.profile_key, cx);
        let visible_ids: HashSet<String> = self
            .state
            .read(cx)
            .overview_chats(Utc::now())
            .into_iter()
            .filter(|(_, chat)| match &drag.filter {
                Some(space_id) => chat.space_id.as_deref() == Some(space_id.as_str()),
                None => true,
            })
            .map(|(_, chat)| chat.id.clone())
            .collect();
        let current_ids = current_pins
            .iter()
            .filter(|id| visible_ids.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        pinned_drag_snapshot_is_valid(&drag.chat_id, drag.visible_ids.as_ref(), &current_ids)
    }

    pub(super) fn cancel_pinned_session_drag(&mut self, cx: &mut Context<Self>) {
        if self.pinned_session_drag.take().is_some() {
            self.pinned_session_drag_generation =
                self.pinned_session_drag_generation.wrapping_add(1);
            cx.notify();
        }
    }

    /// The filter's scrollable rows: "All projects", then spaces matching
    /// the search (ranked — `popover::filter_indices`). "All" only shows on
    /// an empty query (searching means hunting a space). The "New project…"
    /// action is not a row here — the card renders it as a pinned footer.
    fn spaces_menu_rows(&self, cx: &App) -> Vec<SpacesMenuRow> {
        let query = self
            .spaces_menu
            .get()
            .map(|menu| menu.search.read(cx).text().to_string())
            .unwrap_or_default();
        let state = self.state.read(cx);
        let spaces = state.spaces_sorted();
        let names: Vec<String> = spaces
            .iter()
            .map(|s| s.display_name().to_string())
            .collect();
        let mut rows: Vec<SpacesMenuRow> = Vec::new();
        if query.trim().is_empty() {
            rows.push(SpacesMenuRow::All);
        }
        rows.extend(
            popover::filter_indices(&query, &names)
                .into_iter()
                .map(|ix| SpacesMenuRow::Space(spaces[ix].id.clone())),
        );
        rows
    }

    fn open_spaces_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_sidebar_view_menu(cx);
        // "PaletteSearch" context: ↑↓/⏎ stay unbound in the input and bubble
        // to the card's key handler.
        let search =
            cx.new(|cx| ComposerInput::with_context("Search projects…", "PaletteSearch", cx));
        let search_events = cx.subscribe(&search, |this: &mut Shell, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                if let Some(menu) = this.spaces_menu.open_mut() {
                    menu.active = 0;
                }
                cx.notify();
            }
        });
        // The highlight starts ON the current filter row.
        let current = self.settings.space_filter.clone();
        let handle = search.read(cx).focus_handle(cx);
        self.spaces_menu.open(SpacesMenu {
            search,
            active: 0,
            focus: cx.focus_handle(),
            list_scroll: gpui::ScrollHandle::new(),
            _search_events: search_events,
        });
        // Fresh handle at the top — don't let the stale rail baseline read
        // the reopen as scrolling.
        self.spaces_menu_bar.clear_scroll_baseline();
        let rows = self.spaces_menu_rows(cx);
        let start = match &current {
            None => 0,
            Some(id) => rows
                .iter()
                .position(|row| matches!(row, SpacesMenuRow::Space(s) if s == id))
                .unwrap_or(0),
        };
        if let Some(menu) = self.spaces_menu.open_mut() {
            menu.active = start;
        }
        // Focusable before first paint (the add-space palette's proven order).
        window.focus(&handle, cx);
        cx.notify();
    }

    fn activate_spaces_menu_row(&mut self, row: SpacesMenuRow, cx: &mut Context<Self>) {
        match row {
            SpacesMenuRow::All => self.set_space_filter(None, cx),
            SpacesMenuRow::Space(id) => self.set_space_filter(Some(id), cx),
            SpacesMenuRow::AddSpace => {
                self.close_spaces_menu(cx);
                self.open_add_space(cx);
            }
        }
    }

    /// Dropdown keys (bubbling from the focused search input): ↑↓ navigate,
    /// ⏎ activates the highlighted row, esc closes.
    fn spaces_menu_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        // The card stays mounted (and focused) through the exit animation —
        // keys must not drive a dying menu.
        if !self.spaces_menu.is_open() {
            return;
        }
        let key = popover::classify_key(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        );
        match key {
            popover::MenuKey::Escape => {
                self.close_spaces_menu(cx);
                cx.stop_propagation();
            }
            popover::MenuKey::Up | popover::MenuKey::Down => {
                let rows = self.spaces_menu_rows(cx);
                // +1: the pinned footer stays in the nav order, exactly as
                // when it was the list's last row.
                let count = rows.len() + 1;
                let delta = if key == popover::MenuKey::Up { -1 } else { 1 };
                if let Some(menu) = self.spaces_menu.open_mut() {
                    menu.active = popover::menu_step(Some(menu.active), count, delta).unwrap_or(0);
                    // The footer renders below the scroller — only in-list
                    // rows can be scrolled to (the footer index would leave
                    // a request pending against a row that never exists).
                    if menu.active < rows.len() {
                        menu.list_scroll.scroll_to_item(menu.active);
                    }
                    cx.notify();
                }
            }
            popover::MenuKey::Enter | popover::MenuKey::ModEnter => {
                let active = self.spaces_menu.get().map(|m| m.active).unwrap_or(0);
                let rows = self.spaces_menu_rows(cx);
                // One past the scrollable rows is the pinned footer.
                let row = if active < rows.len() {
                    rows[active].clone()
                } else {
                    SpacesMenuRow::AddSpace
                };
                self.activate_spaces_menu_row(row, cx);
            }
            popover::MenuKey::Backspace | popover::MenuKey::Other => {}
        }
    }

    fn close_sidebar_view_menu(&mut self, cx: &mut Context<Self>) {
        if self.sidebar_view_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.sidebar_view_menu);
            cx.notify();
        }
    }

    fn open_sidebar_view_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_spaces_menu(cx);
        let focus = cx.focus_handle();
        self.sidebar_view_menu.open(SidebarViewMenu {
            active: None,
            focus: focus.clone(),
        });
        window.focus(&focus, cx);
        cx.notify();
    }

    fn activate_sidebar_view_row(&mut self, row: SidebarViewRow, cx: &mut Context<Self>) {
        self.cancel_sidebar_session_transfer(cx);
        self.cancel_pinned_session_drag(cx);
        match row {
            SidebarViewRow::ByProject => {
                self.settings.sidebar_organization = SidebarOrganization::ByProject
            }
            SidebarViewRow::ShowProjectLabel => {
                self.settings.sidebar_show_project_label = !self.settings.sidebar_show_project_label
            }
            SidebarViewRow::Compact => {
                self.settings.sidebar_compact = !self.settings.sidebar_compact
            }
            SidebarViewRow::ShowProjectIcon => {
                self.settings.sidebar_show_project_icon = !self.settings.sidebar_show_project_icon
            }
            SidebarViewRow::ByDevice => {
                self.settings.sidebar_organization = SidebarOrganization::ByDevice
            }
            SidebarViewRow::InOneList => {
                self.settings.sidebar_organization = SidebarOrganization::InOneList
            }
            SidebarViewRow::LastUpdated => self.settings.sidebar_sort = SidebarSort::LastUpdated,
            SidebarViewRow::Created => self.settings.sidebar_sort = SidebarSort::Created,
            SidebarViewRow::ShowBranch => {
                self.settings.sidebar_show_branch = !self.settings.sidebar_show_branch
            }
            SidebarViewRow::ShowPullRequest => {
                self.settings.sidebar_show_pull_request = !self.settings.sidebar_show_pull_request;
                let visible = self.settings.sidebar_show_pull_request;
                self.state.update(cx, |state, cx| {
                    state.set_change_requests_visible(visible, cx)
                });
            }
            SidebarViewRow::ShowHarness => {
                self.settings.sidebar_show_harness = !self.settings.sidebar_show_harness
            }
        }
        self.schedule_save(cx);
        if row.closes_menu() {
            self.close_sidebar_view_menu(cx);
        }
        cx.notify();
    }

    fn sidebar_view_menu_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        if !self.sidebar_view_menu.is_open() {
            return;
        }
        match popover::classify_key(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        ) {
            popover::MenuKey::Escape => self.close_sidebar_view_menu(cx),
            popover::MenuKey::Up | popover::MenuKey::Down => {
                let up = event.keystroke.key.eq_ignore_ascii_case("arrowup");
                if let Some(menu) = self.sidebar_view_menu.open_mut() {
                    menu.active = popover::menu_step(
                        menu.active,
                        SIDEBAR_VIEW_ROWS.len(),
                        if up { -1 } else { 1 },
                    );
                    cx.notify();
                }
            }
            popover::MenuKey::Enter | popover::MenuKey::ModEnter => {
                let active = self.sidebar_view_menu.get().and_then(|m| m.active);
                if let Some(row) = active.and_then(|ix| SIDEBAR_VIEW_ROWS.get(ix)).copied() {
                    self.activate_sidebar_view_row(row, cx);
                }
            }
            popover::MenuKey::Backspace | popover::MenuKey::Other => {}
        }
    }

    fn render_sidebar_view_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let theme = &theme.for_popup();
        let Some(menu_state) = self.sidebar_view_menu.get() else {
            return div().into_any_element();
        };
        let active = menu_state.active;
        let focus = menu_state.focus.clone();
        let organization = self.settings.sidebar_organization;
        let sort = self.settings.sidebar_sort;
        let show_harness = self.settings.sidebar_show_harness;
        let show_branch = self.settings.sidebar_show_branch;
        let show_pr = self.settings.sidebar_show_pull_request;

        let labels = [
            "By device",
            "By project",
            "In one list",
            "Last updated",
            "Created",
            "Branch",
            "Pull request",
            "Harness",
            "Project icon",
            "Location",
            "Compact mode",
        ];
        let icons = [
            icons::LAPTOP,
            icons::FOLDER,
            icons::LIST,
            icons::CLOCK_CIRCLE,
            icons::CALENDAR,
            icons::GIT_BRANCH,
            icons::PULL_REQUEST,
            icons::BOT,
            icons::PROJECT_DEFAULT,
            icons::FOLDER,
            icons::LIST,
        ];
        let selected = [
            organization == SidebarOrganization::ByDevice,
            organization == SidebarOrganization::ByProject,
            organization == SidebarOrganization::InOneList,
            sort == SidebarSort::LastUpdated,
            sort == SidebarSort::Created,
            show_branch,
            show_pr,
            show_harness,
            self.settings.sidebar_show_project_icon,
            self.settings.sidebar_show_project_label,
            self.settings.sidebar_compact,
        ];
        let mut rows: Vec<AnyElement> = SIDEBAR_VIEW_ROWS
            .iter()
            .copied()
            .enumerate()
            .map(|(ix, row)| {
                popover::menu_row_nav(
                    theme,
                    selected[ix],
                    active == Some(ix),
                    format!("sidebar-view-row-{ix}"),
                )
                .id(("sidebar-view-row", ix))
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(menu) = this.sidebar_view_menu.open_mut() {
                        menu.active = None;
                    }
                    this.activate_sidebar_view_row(row, cx)
                }))
                .child(
                    icon(icons[ix])
                        .size(px(15.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(div().flex_1().child(SharedString::from(labels[ix])))
                .child(div().w(px(14.0)).flex_none().when(selected[ix], |el| {
                    el.child(
                        icon(icons::CHECK)
                            .size(px(14.0))
                            .text_color(theme.text_muted),
                    )
                }))
                .into_any_element()
            })
            .collect();
        // Compact mode is a layout choice, not a row-content toggle, so it
        // gets its own section under Show.
        let layout_rows = rows.split_off(10);
        let show_rows = rows.split_off(5);
        let sort_rows = rows.split_off(3);
        let organization_rows = rows;

        popover::popover_card(theme)
            .w(px(self.settings.sidebar_width - 2.0 * Theme::SPACE_SM))
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                this.sidebar_view_menu_key(event, cx)
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_sidebar_view_menu(cx)))
            .flex()
            .flex_col()
            .child(popover::menu_heading(theme, "Organize"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .children(organization_rows),
            )
            .child(popover::menu_separator())
            .child(popover::menu_heading(theme, "Sort"))
            .child(div().flex().flex_col().gap(px(2.0)).children(sort_rows))
            .child(popover::menu_separator())
            .child(popover::menu_heading(theme, "Show"))
            .child(div().flex().flex_col().gap(px(2.0)).children(show_rows))
            .child(popover::menu_separator())
            .child(popover::menu_heading(theme, "Layout"))
            .child(div().flex().flex_col().gap(px(2.0)).children(layout_rows))
            .into_any_element()
    }

    /// The sidebar's space-filter row: current filter ("All projects" or the
    /// space's name) + chevron, the dropdown floating beneath while open.
    /// Sits OUTSIDE the sidebar's scroll region so the float never clips.
    pub(super) fn render_spaces_filter(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let filter = self.settings.space_filter.clone();
        // Name + the dropdown rows' "@ device" tag on the trigger itself, so
        // the filtered space's host reads without opening the picker.
        let (label, device_tag): (SharedString, Option<(SharedString, bool)>) = {
            let state = self.state.read(cx);
            match filter.as_deref().and_then(|id| state.space_row(id)) {
                Some(space) => {
                    let (tag, offline) = state.space_device_tag(space, Utc::now());
                    (
                        space.display_name().to_string().into(),
                        Some((tag.into(), offline)),
                    )
                }
                None => (SharedString::from("All projects"), None),
            }
        };
        let open = self.spaces_menu.is_open();

        let trigger = div()
            .id("spaces-filter")
            .flex_1()
            .min_w_0()
            .h(px(29.0))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(Theme::SPACE_SM))
            .rounded(px(8.0))
            .px(px(Theme::SPACE_SM))
            .text_size(crate::typography::ui_rems(13.0))
            .font_weight(gpui::FontWeight::MEDIUM)
            .text_color(motion::hover_blend(
                "spaces-filter",
                theme.text.opacity(0.8),
                theme.text,
            ))
            .bg(if open {
                theme.glass_hover()
            } else {
                motion::hover_blend(
                    "spaces-filter",
                    theme.glass_hover().opacity(0.0),
                    theme.glass_hover(),
                )
            })
            .on_hover(motion::hover_listener("spaces-filter"))
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.spaces_menu.note_trigger_press()),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                // A press that found the menu open closes it (the card's
                // mouse-down-out already began the close) — never reopen.
                if this.spaces_menu.take_press_was_open() {
                    this.close_spaces_menu(cx);
                } else {
                    this.open_spaces_menu(window, cx);
                }
            }))
            .child(
                icon(icons::FOLDER)
                    .size(px(16.0))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            // flex_1 pushes the caret to the trigger's right edge and gives
            // long space names a bound to fade against; the "@ device"
            // tag hugs the name inside it rather than sitting by the caret.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(super::sidebar_faded_label(
                        "spaces-filter-label".into(),
                        false,
                        label,
                    ))
                    .when_some(device_tag, |el, (tag, offline)| {
                        el.child(super::sidebar_faded_label(
                            "spaces-filter-device".into(),
                            false,
                            div()
                                .text_size(crate::typography::ui_rems(10.0))
                                .font_weight(gpui::FontWeight::NORMAL)
                                .text_color(theme.text_muted.opacity(0.45))
                                .child(tag),
                        ))
                        // Disconnected glyph, not the word (user request).
                        .when(offline, |el| {
                            el.child(
                                icon(icons::WIFI_OFF)
                                    .size(px(12.0))
                                    .flex_none()
                                    .text_color(theme.warning.opacity(0.8)),
                            )
                        })
                    }),
            )
            .child(
                icon(icons::ALT_ARROW_DOWN)
                    .size(px(14.0))
                    .flex_none()
                    .text_color(theme.text_muted.opacity(0.6)),
            );
        let trigger = if self.spaces_menu.get().is_some() {
            let closing = self.spaces_menu.closing_since();
            let menu = self.render_spaces_menu(theme, cx);
            trigger.relative().child(popover::anchored_menu_below(
                "spaces-filter-menu",
                menu,
                closing,
            ))
        } else {
            trigger
        };

        let view_open = self.sidebar_view_menu.is_open();
        let view_focus = self.sidebar_view_trigger_focus.clone();
        let view_trigger = div()
            .id("sidebar-view-options")
            .role(gpui::Role::Button)
            .aria_label("Sidebar view options")
            .aria_expanded(view_open)
            .track_focus(&view_focus)
            .size(px(29.0))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.0))
            .border_1()
            .border_color(theme.border.opacity(0.0))
            .focus_visible(|el| el.border_color(theme.border_strong))
            .cursor_pointer()
            .text_color(theme.text_muted)
            .bg(if view_open {
                theme.glass_hover()
            } else {
                theme.glass_hover().opacity(0.0)
            })
            .hover(|el| el.bg(theme.glass_hover()))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, _| this.sidebar_view_menu.note_trigger_press()),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                if this.sidebar_view_menu.take_press_was_open() {
                    this.close_sidebar_view_menu(cx);
                } else {
                    this.open_sidebar_view_menu(window, cx);
                }
            }))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, window, cx| {
                if matches!(
                    event.keystroke.key.to_ascii_lowercase().as_str(),
                    "enter" | "space" | "arrowdown"
                ) {
                    cx.stop_propagation();
                    if this.sidebar_view_menu.is_open()
                        && !event.keystroke.key.eq_ignore_ascii_case("arrowdown")
                    {
                        this.close_sidebar_view_menu(cx);
                    } else if !this.sidebar_view_menu.is_open() {
                        this.open_sidebar_view_menu(window, cx);
                    }
                }
            }))
            .tooltip(|_, cx| cx.new(|_| SidebarViewOptionsTooltip).into())
            .tooltip_show_delay(std::time::Duration::from_millis(350))
            .child(
                icon(icons::SORT)
                    .size(px(16.0))
                    .text_color(theme.text_muted.opacity(0.6)),
            );
        let view_trigger = if self.sidebar_view_menu.get().is_some() {
            let closing = self.sidebar_view_menu.closing_since();
            let menu = self.render_sidebar_view_menu(theme, cx);
            view_trigger.relative().child(popover::anchored_menu_right(
                "sidebar-view-options-menu",
                menu,
                closing,
            ))
        } else {
            view_trigger
        };

        div()
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .px(px(Theme::SPACE_SM))
            .pt(px(8.0))
            .pb(px(4.0))
            .child(trigger)
            .child(view_trigger)
            .into_any_element()
    }

    /// The dropdown card: search on top, "All projects" + space rows (check on
    /// the active filter; right-click for rename/remove) + "New project…".
    fn render_spaces_menu(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let theme = &theme.for_popup();
        let (search, active, focus, list_scroll) = {
            let Some(menu) = self.spaces_menu.get() else {
                return div().into_any_element();
            };
            (
                menu.search.clone(),
                menu.active,
                menu.focus.clone(),
                menu.list_scroll.clone(),
            )
        };
        let rows = self.spaces_menu_rows(cx);
        let scrollbar = popover::rail(self, "spaces-menu-scrollbar", theme, cx);
        let filter = self.settings.space_filter.clone();
        // Keep the host tag so projects with the same name on different
        // devices remain distinguishable. Consume `rows` to avoid cloning
        // the list children per frame.
        let details: Vec<(
            SpacesMenuRow,
            SharedString,
            Option<SharedString>,
            bool,
            bool,
        )> = {
            let state = self.state.read(cx);
            rows.into_iter()
                .map(|row| match row {
                    SpacesMenuRow::All => (
                        SpacesMenuRow::All,
                        SharedString::from("All projects"),
                        None,
                        false,
                        filter.is_none(),
                    ),
                    SpacesMenuRow::Space(id) => {
                        let selected = filter.as_deref() == Some(id.as_str());
                        match state.space_row(&id) {
                            Some(space) => {
                                let (tag, offline) = state.space_device_tag(space, Utc::now());
                                (
                                    SpacesMenuRow::Space(id),
                                    space.display_name().to_string().into(),
                                    Some(tag.into()),
                                    offline,
                                    selected,
                                )
                            }
                            None => (
                                SpacesMenuRow::Space(id),
                                SharedString::from("?"),
                                None,
                                false,
                                selected,
                            ),
                        }
                    }
                    // spaces_menu_rows never yields this variant — the
                    // footer is rendered by the card, not the list.
                    SpacesMenuRow::AddSpace => unreachable!(),
                })
                .collect()
        };
        // The pinned footer's keyboard-nav index: one past the last
        // scrollable row, its permanent place at the end of the nav order.
        let add_index = details.len();

        let list = popover::menu_scroll_host("spaces-menu-list-host")
            .on_hover(cx.listener(Self::on_spaces_menu_list_hover))
            .child(
                popover::menu_scroll_list("spaces-menu-list", &list_scroll)
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    // Same scroll budget as the composer project menu.
                    .max_h(px(224.0))
                    .children(details.into_iter().enumerate().map(
                        |(ix, (row, label, tag, offline, selected))| {
                            let menu_space = match &row {
                                SpacesMenuRow::Space(id) => Some(id.clone()),
                                _ => None,
                            };
                            let activate = row;
                            popover::menu_row_nav(
                                theme,
                                selected,
                                ix == active,
                                format!("spaces-menu-row-{ix}"),
                            )
                            .id(("spaces-menu-row", ix))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.activate_spaces_menu_row(activate.clone(), cx);
                            }))
                            .when_some(menu_space, |el, space_id| {
                                el.on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        this.space_menu.open((space_id.clone(), event.position));
                                        cx.notify();
                                    }),
                                )
                            })
                            .child(div().flex_1().min_w_0().truncate().child(label))
                            .when_some(tag, |el, tag| {
                                el.child(
                                    div()
                                        .max_w(gpui::relative(0.5))
                                        .min_w_0()
                                        .truncate()
                                        .text_size(crate::typography::ui_rems(10.0))
                                        .font_weight(gpui::FontWeight::NORMAL)
                                        .text_color(theme.text_muted)
                                        .child(tag),
                                )
                            })
                            // Disconnected glyph, not the word (user request).
                            .when(offline, |el| {
                                el.child(
                                    icon(icons::WIFI_OFF)
                                        .size(px(12.0))
                                        .flex_none()
                                        .text_color(theme.warning.opacity(0.8)),
                                )
                            })
                            // No check glyph — the selected row's wash (menu_row's
                            // active styling) is the selection signal.
                        },
                    )),
            )
            .children(scrollbar);

        popover::popover_card(theme)
            // Match the trigger row as the sidebar is resized. Both live
            // inside the same SPACE_SM horizontal gutters.
            .w(px(self.settings.sidebar_width - 2.0 * Theme::SPACE_SM))
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                this.spaces_menu_key(event, cx)
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.close_spaces_menu(cx);
            }))
            .flex()
            .flex_col()
            // Same 2px rhythm as the composer project menu's root.
            .gap(px(2.0))
            .child(popover::search_input_frame(
                theme,
                search.into_any_element(),
            ))
            .child(list)
            // "New project…" is a pinned action row under the list (the
            // chat composer's project menu treatment) — scrolling must
            // never carry it away, and its nav index (`add_index`) keeps it
            // LAST.
            .child(
                // Full-bleed through the card's 4px inset — a divider
                // stopping short of the edges reads as a mistake (the
                // composer project menu's treatment).
                div()
                    .my(px(2.0))
                    .mx(px(-popover::CARD_INSET))
                    .h(px(1.0))
                    .flex_none()
                    .bg(theme.border.opacity(0.6)),
            )
            .child(
                popover::menu_row_nav(
                    theme,
                    false,
                    active == add_index,
                    "spaces-menu-add".to_string(),
                )
                .id("spaces-menu-add")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.activate_spaces_menu_row(SpacesMenuRow::AddSpace, cx);
                }))
                .child(
                    icon(icons::PLUS)
                        .size(px(12.0))
                        .flex_none()
                        .text_color(theme.text_muted),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from("New project…")),
                ),
            )
            .into_any_element()
    }

    /// Flat top-to-bottom chat ids exactly as [`Self::render_active_rows`]
    /// draws them: pins first, then the user's sort, device grouping,
    /// and local-device promotion. Jump shortcuts and session cycling read
    /// this projection so keyboard order never drifts from the screen.
    pub(super) fn sidebar_visible_order(&self, cx: &Context<Self>) -> Vec<String> {
        let filter = self.settings.space_filter.clone();
        let profile_key = self.active_sidebar_pin_profile_key(cx);
        let saved_pins = self.active_sidebar_pins(cx);
        let frozen_pinned = self
            .pinned_session_drag
            .as_ref()
            .filter(|drag| {
                drag.filter == filter && profile_key.as_deref() == Some(&drag.profile_key)
            })
            .map(|drag| drag.visible_ids.clone());
        let pinned_order = frozen_pinned
            .as_ref()
            .map_or(saved_pins.as_slice(), |ids| ids.as_slice());
        let state = self.state.read(cx);
        let mut chats: Vec<zeron_proto::Chat> = state
            .sidebar_chats(Utc::now(), filter.as_deref())
            .into_iter()
            .map(|(_, chat)| chat.clone())
            .collect();
        chats.sort_by(|left, right| compare_sidebar_chats(self.settings.sidebar_sort, left, right));
        let (pinned_chats, chats): (Vec<_>, Vec<_>) = chats
            .into_iter()
            .partition(|chat| pinned_order.contains(&chat.id));
        let ordered = if self.settings.sidebar_organization != SidebarOrganization::InOneList {
            let mut groups: Vec<(Option<(String, String)>, Vec<zeron_proto::Chat>)> = Vec::new();
            for chat in chats {
                let key = Some((
                    if self.settings.sidebar_organization == SidebarOrganization::ByProject {
                        chat.space_id
                            .clone()
                            .unwrap_or_else(|| format!("home:{}", chat.device_id))
                    } else {
                        chat.device_id.clone()
                    },
                    String::new(),
                ));
                if let Some((_, existing)) = groups.iter_mut().find(|(group, _)| group == &key) {
                    existing.push(chat);
                } else {
                    groups.push((key, vec![chat]));
                }
            }
            if self.settings.sidebar_organization == SidebarOrganization::ByDevice {
                promote_local_device_group(&mut groups, state.local_device_id.as_deref());
            }
            groups
                .into_iter()
                .flat_map(|(_, rows)| rows)
                .map(|chat| chat.id)
                .collect::<Vec<_>>()
        } else {
            chats.into_iter().map(|chat| chat.id).collect()
        };
        let ordered = pinned_chats
            .into_iter()
            .map(|chat| chat.id)
            .chain(ordered)
            .collect::<Vec<_>>();
        let mut visible = project_pinned_first(&ordered, pinned_order);
        if !self.sessions_open
            && self.settings.sidebar_organization == SidebarOrganization::InOneList
        {
            visible.retain(|id| pinned_order.contains(id));
        }
        if !self.pinned_open {
            let pins: HashSet<&str> = pinned_order.iter().map(String::as_str).collect();
            visible.retain(|id| !pins.contains(id.as_str()));
        }
        visible
    }

    /// Shared metadata and visibility settings for active and archived sessions.
    fn sidebar_chat_data(
        &self,
        status: ChatIndicator,
        chat: zeron_proto::Chat,
        state: &AppState,
    ) -> ActiveChatRow {
        // Line 1 is "project @ device" (t3code's project row);
        // project-less sessions read as their home-dir cwd `~`.
        let space = state.space_for_chat(&chat);
        let project = match (space, chat.space_id.as_deref()) {
            (Some(space), _) => space.display_name().to_string(),
            (None, None) => "~".to_string(),
            (None, Some(_)) => "?".to_string(),
        };
        let device = state
            .device_name(&chat.device_id)
            .unwrap_or("Unknown device")
            .to_string();
        let mut folder = project.clone();
        // Unknown device → no fragment, same as the archived list.
        if state.device_name(&chat.device_id).is_some() {
            folder = format!("{folder} @ {device}");
        }
        // The branch shows whenever the engine has stamped one —
        // main-checkout sessions included, not just worktrees.
        let branch = crate::change_requests::conversation_branch(&chat, &state.spaces)
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(str::to_string)
            .filter(|_| self.settings.sidebar_show_branch);
        let change_request = state
            .change_request_for_chat(&chat)
            .cloned()
            .filter(|_| self.settings.sidebar_show_pull_request);
        let group = match self.settings.sidebar_organization {
            SidebarOrganization::ByDevice => Some((chat.device_id.clone(), device)),
            SidebarOrganization::ByProject => Some((
                chat.space_id
                    .clone()
                    .unwrap_or_else(|| format!("home:{}", chat.device_id)),
                project,
            )),
            SidebarOrganization::InOneList => None,
        };
        ActiveChatRow {
            status,
            chat: chat.clone(),
            folder,
            branch,
            change_request,
            group,
        }
    }

    pub(super) fn render_active_rows(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> SidebarSessionRows {
        let now = Utc::now();
        let filter = self.settings.space_filter.clone();
        let profile_key = self.active_sidebar_pin_profile_key(cx);
        let saved_pins = self.active_sidebar_pins(cx);
        let frozen_pinned = self
            .pinned_session_drag
            .as_ref()
            .filter(|drag| {
                drag.filter == filter && profile_key.as_deref() == Some(&drag.profile_key)
            })
            .map(|drag| drag.visible_ids.clone());
        let mut rows: Vec<ActiveChatRow> = {
            let state = self.state.read(cx);
            let mut chats: Vec<_> = state
                .sidebar_chats(now, filter.as_deref())
                .into_iter()
                .map(|(status, chat)| (status, chat.clone()))
                .collect();
            chats.sort_by(|left, right| {
                compare_sidebar_chats(self.settings.sidebar_sort, &left.1, &right.1)
            });
            chats
                .into_iter()
                .map(|(status, chat)| self.sidebar_chat_data(status, chat, state))
                .collect()
        };
        let pinned_order = frozen_pinned
            .as_ref()
            .map_or(saved_pins.as_slice(), |ids| ids.as_slice());
        let base_ids: Vec<String> = rows.iter().map(|row| row.chat.id.clone()).collect();
        let ordered_ids = project_pinned_first(&base_ids, pinned_order);
        let rank: std::collections::HashMap<&str, usize> = ordered_ids
            .iter()
            .enumerate()
            .map(|(ix, id)| (id.as_str(), ix))
            .collect();
        rows.sort_by_key(|row| {
            rank.get(row.chat.id.as_str())
                .copied()
                .unwrap_or(usize::MAX)
        });
        let active: HashSet<&str> = base_ids.iter().map(String::as_str).collect();
        let pinned_count = pinned_order
            .iter()
            .filter(|id| active.contains(id.as_str()))
            .collect::<HashSet<_>>()
            .len();
        let regular_rows = rows.split_off(pinned_count);
        let pinned_rows = rows;
        self.sidebar_pinned_heights = pinned_rows
            .iter()
            .map(|row| {
                sidebar_row_height(
                    self.settings.sidebar_compact,
                    self.settings.sidebar_show_project_label,
                    row.branch.is_some(),
                    row.change_request.is_some(),
                )
            })
            .collect();
        let visible_pinned_ids = std::sync::Arc::new(
            pinned_rows
                .iter()
                .map(|row| row.chat.id.clone())
                .collect::<Vec<_>>(),
        );

        let mut regular_groups: Vec<(Option<(String, String)>, Vec<ActiveChatRow>)> = Vec::new();
        for row in regular_rows {
            if let Some((_, existing)) = regular_groups
                .iter_mut()
                .find(|(group, _)| group == &row.group)
            {
                existing.push(row);
            } else {
                regular_groups.push((row.group.clone(), vec![row]));
            }
        }
        if self.settings.sidebar_organization == SidebarOrganization::ByDevice {
            let local_device_id = self.state.read(cx).local_device_id.clone();
            promote_local_device_group(&mut regular_groups, local_device_id.as_deref());
        }
        let mut sections = Vec::with_capacity(regular_groups.len() + usize::from(pinned_count > 0));
        if !pinned_rows.is_empty() {
            sections.push((None, pinned_rows));
        }
        sections.extend(regular_groups);

        let returning = self.sidebar_session_transfer.is_none();
        let transfer = self.sidebar_session_transfer.as_mut().or_else(|| {
            self.sidebar_session_return
                .as_mut()
                .map(|state| &mut state.transfer)
        });
        if let Some(drag) = transfer {
            for (section_index, (group, rows)) in sections.iter().enumerate() {
                let Some(index) = rows
                    .iter()
                    .position(|row| row.chat.id == drag.payload.chat_id)
                else {
                    continue;
                };
                drag.source_group = if section_index == 0 && pinned_count > 0 {
                    "pinned".into()
                } else {
                    group
                        .as_ref()
                        .map_or_else(|| "regular".into(), |(key, _)| format!("regular:{key}"))
                };
                drag.source_index = index;
                drag.row_height = sidebar_row_height(
                    self.settings.sidebar_compact,
                    self.settings.sidebar_show_project_label,
                    rows[index].branch.is_some(),
                    rows[index].change_request.is_some(),
                );
                // Move the vacant slot to the destination instead of keeping two
                // holes. Sample layout and paint with the same reversible easing.
                let collapse = !returning
                    && drag
                        .preview
                        .as_ref()
                        .is_some_and(|gap| gap.group != drag.source_group);
                let target = if collapse {
                    drag.row_height
                        + if rows.len() > 1 {
                            SIDEBAR_LIST_GAP
                        } else {
                            0.0
                        }
                } else {
                    0.0
                };
                drag.source_collapse.retarget(target);
                let removed = if self.reduced_motion {
                    target
                } else {
                    drag.source_collapse.current()
                };
                if let Some(gap) = drag.preview.as_mut() {
                    let origin = f32::from(
                        drag.origin.get().y
                            - self.sidebar_scroll.bounds().top()
                            - self.sidebar_scroll.offset().y,
                    );
                    if gap.group != drag.source_group && gap.top > origin {
                        gap.top -= removed - drag.collapsed_height;
                    }
                }
                drag.collapsed_height = removed;
                let destination = drag
                    .preview
                    .as_ref()
                    .filter(|gap| !returning && gap.group != drag.source_group)
                    .map(|gap| gap.group.clone());
                if let Some(group) = &destination {
                    drag.section_gaps
                        .entry(group.clone())
                        .or_insert_with(|| SidebarSessionSlide {
                            from: 0.0,
                            to: 0.0,
                            epoch: 0,
                            started: std::time::Instant::now(),
                        });
                }
                for (group, gap) in &mut drag.section_gaps {
                    gap.retarget(if destination.as_ref() == Some(group) {
                        drag.row_height
                            + if group == "pinned" && pinned_count == 0 {
                                0.0
                            } else {
                                SIDEBAR_LIST_GAP
                            }
                    } else {
                        0.0
                    });
                }
                break;
            }
        }

        let selected = self.state.read(cx).selected_chat.clone();
        // Re-checked at render so the chips drop the FRAME a popover opens,
        // not on the next modifier event — the jumps are suppressed under it.
        let jump_hints = self.jump_hints && !self.overlay_owns_keyboard(cx);
        let keymap = self.settings.keymap.clone();
        // Flat top-to-bottom slot across groups: the same order
        // `sidebar_visible_order` hands the jump shortcuts and cycling, so a
        // chip always names the key that opens its row.
        let mut slot = 0usize;
        let mut rendered = Vec::new();
        let mut moving_row = None;
        for (group, rows) in sections {
            let pinned_group = slot < pinned_count;
            let drag_group = if pinned_group {
                "pinned".to_owned()
            } else {
                group
                    .as_ref()
                    .map_or_else(|| "regular".to_owned(), |(key, _)| format!("regular:{key}"))
            };
            let mut rendered_rows = Vec::with_capacity(rows.len());
            for (group_index, row) in rows.into_iter().enumerate() {
                let ActiveChatRow {
                    status,
                    chat,
                    folder,
                    branch,
                    change_request,
                    group: _,
                } = row;
                let time_ago: SharedString =
                    format_time_ago(chat.last_message_at.unwrap_or(chat.created_at), now).into();
                let is_selected = selected.as_deref() == Some(chat.id.as_str());
                let harness = self
                    .settings
                    .sidebar_show_harness
                    .then(|| chat.config.as_ref().map(|c| c.harness))
                    .flatten();
                let height = sidebar_row_height(
                    self.settings.sidebar_compact,
                    self.settings.sidebar_show_project_label,
                    branch.is_some(),
                    change_request.is_some(),
                );
                // Only rows a jump slot can reach wear a chip; row 10 onward
                // keeps its time-ago.
                let jump_slot = slot.checked_sub(if self.pinned_open { 0 } else { pinned_count });
                let jump_label: Option<SharedString> = if jump_hints && let Some(slot) = jump_slot {
                    let combo = keymap.get(ShortcutId::JumpSession(slot));
                    (slot < JUMP_SLOTS && !combo.is_empty()).then(|| badge_combo(combo).into())
                } else {
                    None
                };
                let drag = (self.pinned_open || slot >= pinned_count)
                    .then(|| {
                        profile_key.as_ref().map(|profile_key| SidebarSessionDrag {
                            chat_id: chat.id.clone(),
                            visible_ids: visible_pinned_ids.clone(),
                            filter: filter.clone(),
                            profile_key: profile_key.clone(),
                        })
                    })
                    .flatten();
                slot += 1;
                let origin = self
                    .sidebar_session_transfer
                    .as_ref()
                    .filter(|drag| drag.payload.chat_id == chat.id)
                    .map(|drag| drag.origin.clone())
                    .or_else(|| {
                        self.sidebar_session_return
                            .as_ref()
                            .filter(|returning| returning.transfer.payload.chat_id == chat.id)
                            .map(|returning| returning.transfer.origin.clone())
                    });
                let is_moving = origin.is_some();
                let removed = self
                    .sidebar_session_transfer
                    .as_ref()
                    .or_else(|| {
                        self.sidebar_session_return
                            .as_ref()
                            .map(|state| &state.transfer)
                    })
                    .filter(|drag| drag.payload.chat_id == chat.id)
                    .map_or(0.0, |drag| drag.collapsed_height);
                let slot_height = height - removed;
                let element = self.render_chat_row(
                    chat.id.clone(),
                    transcript::single_line(
                        &chat.title.clone().unwrap_or_else(|| "New session".into()),
                    )
                    .into(),
                    time_ago,
                    folder.into(),
                    branch.map(SharedString::from),
                    change_request,
                    harness,
                    status,
                    is_selected,
                    false,
                    is_moving,
                    if is_moving { None } else { drag },
                    jump_label,
                    None,
                    theme,
                    cx,
                );
                // The source slot shrinks when its vacancy moves across sections.
                // Its zero-height anchor still tracks the live activity position.
                let element = if let Some(origin) = origin {
                    moving_row = Some((element, height));
                    div()
                        .child(div().h(px(slot_height.max(0.0))))
                        .on_children_prepainted(move |bounds, _, _| {
                            if let Some(bounds) = bounds.first() {
                                origin.set(bounds.origin);
                            }
                        })
                        .into_any_element()
                } else {
                    self.render_sidebar_gap_row(element, &chat.id, &drag_group, group_index)
                };
                // The hit region stays at the natural slot while its content slides;
                // preview movement must not move its own insertion thresholds.
                let target_group = drag_group.clone();
                let element = div()
                    .id(SharedString::from(format!("session-slot-{}", chat.id)))
                    .debug_selector({
                        let id = chat.id.clone();
                        move || format!("session-slot-{id}")
                    })
                    .h(px(slot_height.max(0.0)))
                    .mb(px(slot_height.min(0.0)))
                    .flex_none()
                    .child(element)
                    .on_drag_move::<SidebarSessionDrag>(cx.listener(
                        move |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, cx| {
                            if !event.bounds.contains(&event.event.position)
                                || !this.sidebar_scroll.bounds().contains(&event.event.position)
                            {
                                return;
                            }
                            let scroll_top = -f32::from(this.sidebar_scroll.offset().y);
                            let viewport_top = this.sidebar_scroll.bounds().top();
                            let Some(drag) = this.sidebar_session_transfer.as_mut() else {
                                return;
                            };
                            let after = event.event.position.y >= event.bounds.center().y;
                            let index = group_index + usize::from(after);
                            let mut top = f32::from(
                                if after {
                                    event.bounds.bottom() + px(SIDEBAR_LIST_GAP)
                                } else {
                                    event.bounds.top()
                                } - viewport_top,
                            ) + scroll_top;
                            if drag.source_group == target_group && index > drag.source_index {
                                top -= drag.row_height + SIDEBAR_LIST_GAP - drag.collapsed_height;
                            }
                            drag.preview = Some(SidebarSessionGap {
                                group: target_group.clone(),
                                index,
                                pinned: pinned_group,
                                top,
                            });
                            cx.notify();
                        },
                    ))
                    .into_any_element();
                rendered_rows.push((format!("c:{}", chat.id), slot_height, element));
            }

            let Some((key, label)) = group else {
                rendered.extend(rendered_rows);
                if !pinned_group {
                    let extra = self.sidebar_transfer_extra_gap(&drag_group);
                    if extra > 0.0 {
                        rendered.push((
                            format!("gap:{drag_group}"),
                            extra - SIDEBAR_LIST_GAP,
                            div()
                                .flex_none()
                                .h(px((extra - SIDEBAR_LIST_GAP).max(0.0)))
                                .mb(px((extra - SIDEBAR_LIST_GAP).min(0.0)))
                                .into_any_element(),
                        ));
                    }
                }
                continue;
            };
            let organization = match self.settings.sidebar_organization {
                SidebarOrganization::ByDevice => "device",
                SidebarOrganization::ByProject => "project",
                SidebarOrganization::InOneList => "list",
            };
            let collapse_key = format!("{organization}:{key}");
            let motion_key = format!("group:{collapse_key}");
            let collapsed = self.sidebar_collapsed_groups.contains(&collapse_key);
            let row_count = rendered_rows.len();
            let extra_gap = self.sidebar_transfer_extra_gap(&drag_group);
            let body_height = SIDEBAR_DISCLOSURE_BODY_INSET
                + extra_gap
                + rendered_rows
                    .iter()
                    .map(|(_, height, _)| *height)
                    .sum::<f32>()
                + SIDEBAR_LIST_GAP * row_count.saturating_sub(1) as f32;
            let body = div()
                .w_full()
                .flex()
                .flex_col()
                .pt(px(SIDEBAR_DISCLOSURE_BODY_INSET))
                .gap(px(SIDEBAR_LIST_GAP))
                .children(rendered_rows.into_iter().map(|(_, _, row)| row))
                .when(extra_gap > 0.0, |el| {
                    el.child(
                        div()
                            .flex_none()
                            .h(px((extra_gap - SIDEBAR_LIST_GAP).max(0.0)))
                            .mb(px((extra_gap - SIDEBAR_LIST_GAP).min(0.0))),
                    )
                });
            let visible_label: SharedString = if collapsed {
                format!("{label} ({row_count})").into()
            } else {
                label.into()
            };
            let chevron = self.sidebar_disclosure_chevron(&motion_key, !collapsed, theme);
            let toggle_key = collapse_key.clone();
            let toggle_motion_key = motion_key.clone();
            let header = sidebar_disclosure_header(theme, visible_label, chevron)
                .id(SharedString::from(format!("sidebar-group-{collapse_key}")))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let was_open = !this.sidebar_collapsed_groups.contains(&toggle_key);
                    this.begin_sidebar_disclosure_motion(
                        &toggle_motion_key,
                        if was_open { body_height } else { 0.0 },
                        if was_open { 0.0 } else { body_height },
                    );
                    if was_open {
                        this.sidebar_collapsed_groups.insert(toggle_key.clone());
                    } else {
                        this.sidebar_collapsed_groups.remove(&toggle_key);
                    }
                    cx.notify();
                }));
            let body = self.render_sidebar_disclosure_body(
                &motion_key,
                !collapsed,
                body_height,
                body.into_any_element(),
            );
            let height =
                SIDEBAR_DISCLOSURE_SECTION_HEIGHT + if collapsed { 0.0 } else { body_height };
            let element = div()
                .w_full()
                .flex()
                .flex_col()
                .pt(px(SIDEBAR_SECTION_GAP))
                .child(header)
                .child(body)
                .into_any_element();
            rendered.push((format!("g:{collapse_key}"), height, element));
        }
        SidebarSessionRows {
            regular_count: base_ids.len().saturating_sub(pinned_count),
            rows: rendered,
            pinned_count,
            moving_row,
        }
    }

    pub(super) fn render_pinned_section(
        &mut self,
        items: Vec<AnyElement>,
        body_height: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.pinned_open;
        let label = if open {
            "Pinned".into()
        } else {
            format!("Pinned ({})", items.len()).into()
        };
        let chevron = self.sidebar_disclosure_chevron("pinned", open, theme);
        let header = sidebar_disclosure_header(theme, label, chevron)
            .id("pinned-toggle")
            .debug_selector(|| "pinned-toggle".into())
            .on_drag_move::<SidebarSessionDrag>(cx.listener(
                |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, _| {
                    if !event.bounds.contains(&event.event.position)
                        || !this.sidebar_scroll.bounds().contains(&event.event.position)
                    {
                        return;
                    }
                    let top = f32::from(event.bounds.bottom() - this.sidebar_scroll.bounds().top())
                        + SIDEBAR_DISCLOSURE_BODY_INSET
                        - f32::from(this.sidebar_scroll.offset().y);
                    if let Some(drag) = this.sidebar_session_transfer.as_mut() {
                        drag.preview = Some(SidebarSessionGap {
                            group: "pinned".into(),
                            index: 0,
                            pinned: true,
                            top,
                        });
                    }
                },
            ))
            .on_drop::<SidebarSessionDrag>(cx.listener(|this, payload, _, cx| {
                this.finish_sidebar_session_transfer(payload, SidebarSessionDrop::Pinned(0), cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                let was_open = this.pinned_open;
                this.cancel_pinned_session_drag(cx);
                this.begin_sidebar_disclosure_motion(
                    "pinned",
                    if was_open { body_height } else { 0.0 },
                    if was_open { 0.0 } else { body_height },
                );
                this.pinned_open = !was_open;
                // The disclosure owns this movement; avoid a second FLIP
                // animation on the regular sessions below it.
                this.sidebar_prev_order.clear();
                this.sidebar_resort.clear();
                this.sidebar_new_keys.clear();
                cx.notify();
            }));
        let content = div()
            .pt(px(SIDEBAR_DISCLOSURE_BODY_INSET))
            .child(Self::render_pinned_session_group(
                items,
                self.sidebar_transfer_extra_gap("pinned"),
                cx,
            ))
            .into_any_element();
        let body = self.render_sidebar_disclosure_body("pinned", open, body_height, content);
        div()
            .id("sidebar-pinned-section")
            .debug_selector(|| "sidebar-pinned-section".into())
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }

    pub(super) fn render_sessions_section(
        &mut self,
        content: AnyElement,
        body_height: f32,
        count: usize,
        follows_pinned: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.settings.sidebar_organization != SidebarOrganization::InOneList {
            return content;
        }
        let open = self.sessions_open;
        let label = if open {
            "Sessions".into()
        } else {
            format!("Sessions ({count})").into()
        };
        let chevron = self.sidebar_disclosure_chevron("sessions", open, theme);
        let header = sidebar_disclosure_header(theme, label, chevron)
            .id("sessions-toggle")
            .debug_selector(|| "sessions-toggle".into())
            .on_drag_move::<SidebarSessionDrag>(cx.listener(
                |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, _| {
                    if event.bounds.contains(&event.event.position) {
                        let top = f32::from(
                            event.bounds.bottom()
                                - this.sidebar_scroll.bounds().top()
                                - this.sidebar_scroll.offset().y,
                        ) + SIDEBAR_DISCLOSURE_BODY_INSET;
                        if let Some(drag) = this.sidebar_session_transfer.as_mut() {
                            drag.preview = Some(SidebarSessionGap {
                                group: "regular".into(),
                                index: 0,
                                pinned: false,
                                top,
                            });
                        }
                    }
                },
            ))
            .on_drop::<SidebarSessionDrag>(cx.listener(|this, payload, _, cx| {
                this.finish_sidebar_session_transfer(payload, SidebarSessionDrop::Regular, cx);
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.cancel_sidebar_session_transfer(cx);
                let was_open = this.sessions_open;
                this.begin_sidebar_disclosure_motion(
                    "sessions",
                    if was_open { body_height } else { 0.0 },
                    if was_open { 0.0 } else { body_height },
                );
                this.sessions_open = !was_open;
                this.sidebar_prev_order.clear();
                this.sidebar_resort.clear();
                this.sidebar_new_keys.clear();
                cx.notify();
            }));
        let content = div()
            .pt(px(SIDEBAR_DISCLOSURE_BODY_INSET))
            .child(content)
            .into_any_element();
        let body = self.render_sidebar_disclosure_body("sessions", open, body_height, content);
        div()
            .id("sidebar-sessions-section")
            .debug_selector(|| "sidebar-sessions-section".into())
            .flex()
            .flex_col()
            .pt(px(if follows_pinned {
                SIDEBAR_SECTION_GAP
            } else {
                0.0
            }))
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// Archived sessions share active-row data and layout, with a restore action.
    /// The shelf starts with ten sessions and pages by 25.
    pub(super) fn render_archived_section(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        const INITIAL: usize = 10;
        const PAGE: usize = 25;
        let now = Utc::now();
        let filter = self.settings.space_filter.clone();
        let mut rows: Vec<zeron_proto::Chat> = {
            let state = self.state.read(cx);
            state
                .chats
                .iter()
                .filter(|c| c.archived)
                .filter(|chat| match &filter {
                    Some(space_id) => chat.space_id.as_deref() == Some(space_id.as_str()),
                    None => true,
                })
                .cloned()
                .collect()
        };
        rows.sort_by(|left, right| compare_sidebar_chats(self.settings.sidebar_sort, left, right));
        if rows.is_empty() {
            return None;
        }
        let rows: Vec<_> = {
            let state = self.state.read(cx);
            rows.into_iter()
                .map(|chat| {
                    self.sidebar_chat_data(state.display_status_for(&chat, now), chat, state)
                })
                .collect()
        };
        let total = rows.len();
        let open = self.archived_open;
        let shown = self.archived_shown.max(INITIAL);
        let visible_count = total.min(shown);
        let has_more = total > shown;
        // "Show more" matches the row slot: compact rows are 29px, so the
        // button shrinks with them instead of towering over the list.
        let more_height = if self.settings.sidebar_compact {
            super::sidebar_row_height(true, true, false, false)
        } else {
            36.0
        };
        let body_height = SIDEBAR_DISCLOSURE_BODY_INSET
            + rows
                .iter()
                .take(shown)
                .map(|row| {
                    sidebar_row_height(
                        self.settings.sidebar_compact,
                        self.settings.sidebar_show_project_label,
                        row.branch.is_some(),
                        row.change_request.is_some(),
                    )
                })
                .sum::<f32>()
            + visible_count.saturating_sub(1) as f32 * SIDEBAR_LIST_GAP
            + if has_more {
                more_height + SIDEBAR_LIST_GAP
            } else {
                0.0
            };
        // Match Pinned: a muted label with a right-aligned disclosure chevron.
        // The count only shows while collapsed.
        let label: SharedString = if open {
            "Archived".into()
        } else {
            format!("Archived ({total})").into()
        };
        let chevron = self.sidebar_disclosure_chevron("archived", open, theme);
        let header = sidebar_disclosure_header(theme, label, chevron)
            .id("archived-toggle")
            .on_click(cx.listener(move |this, _, _, cx| {
                let was_open = this.archived_open;
                this.begin_sidebar_disclosure_motion(
                    "archived",
                    if was_open { body_height } else { 0.0 },
                    if was_open { 0.0 } else { body_height },
                );
                this.archived_open = !was_open;
                this.archived_shown = INITIAL;
                cx.notify();
            }));
        let section = div().flex().flex_col().child(header);
        let body = {
            let selected = self.state.read(cx).selected_chat.clone();
            let mut list = div()
                .flex()
                .flex_col()
                .pt(px(SIDEBAR_DISCLOSURE_BODY_INSET))
                .gap(px(SIDEBAR_LIST_GAP));
            for row in rows.into_iter().take(shown) {
                let chat = row.chat;
                let is_selected = selected.as_deref() == Some(chat.id.as_str());
                let harness = self
                    .settings
                    .sidebar_show_harness
                    .then(|| chat.config.as_ref().map(|c| c.harness))
                    .flatten();
                list = list.child(
                    self.render_chat_row(
                        chat.id.clone(),
                        transcript::single_line(
                            &chat.title.clone().unwrap_or_else(|| "New session".into()),
                        )
                        .into(),
                        format_time_ago(chat.last_message_at.unwrap_or(chat.created_at), now)
                            .into(),
                        row.folder.into(),
                        row.branch.map(SharedString::from),
                        row.change_request,
                        harness,
                        row.status,
                        is_selected,
                        true,
                        false,
                        None,
                        None,
                        None,
                        theme,
                        cx,
                    ),
                );
            }
            let mut body = div().w_full().flex().flex_col().child(list);
            if has_more {
                let remaining = (total - shown).min(PAGE);
                body = body.child(
                    div()
                        .id("archived-more")
                        // Sits outside the rows' gapped column — match the
                        // list's 2px row gap or it fuses with the last row.
                        .mt(px(2.0))
                        .h(px(more_height))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(10.0))
                        .px(px(Theme::SPACE_SM))
                        .rounded(px(6.0))
                        .text_size(crate::typography::ui_rems(13.0))
                        .text_color(theme.text_muted.opacity(0.55))
                        .cursor_pointer()
                        .hover(|s| s.bg(theme.glass_hover()).text_color(theme.text))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.archived_shown = this.archived_shown.max(INITIAL) + PAGE;
                            cx.notify();
                        }))
                        .child(
                            crate::icons::icon(crate::icons::PLUS)
                                .size(px(14.0))
                                .flex_none(),
                        )
                        .child(SharedString::from(format!("Show {remaining} more"))),
                );
            }
            body.into_any_element()
        };
        let body = self.render_sidebar_disclosure_body("archived", open, body_height, body);
        // The active list already provides the spacing above Archived.
        let section = section.child(body);
        Some(section.into_any_element())
    }

    // ---- add-space flow ----

    pub(super) fn open_add_space(&mut self, cx: &mut Context<Self>) {
        self.command_palette = None;
        // "PaletteSearch" context: navigation keys stay unbound so ↑↓/←/→/⏎
        // bubble to the palette frame (`add_space_key`) instead of moving the
        // text caret — Enter and ⌘Enter are both handled there.
        let search =
            cx.new(|cx| ComposerInput::with_context("Search devices…", "PaletteSearch", cx));
        let search_events = cx.subscribe(&search, |this: &mut Shell, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                // Typing `/` after a query that names a folder descends into
                // it — the query reads as a path segment, so the slash IS the
                // pick (shell-style). Otherwise the slash stays in the query
                // (it matches nothing, which is honest feedback).
                if this.add_space_slash_descend(cx) {
                    return;
                }
                if let Some(flow) = this.add_space.as_mut() {
                    flow.active = 0;
                    flow.list_scroll.set_offset(gpui::Point::default());
                }
                cx.notify();
            }
        });
        self.add_space = Some(AddSpaceFlow {
            step: ProjectStep::Devices,
            location: None,
            device: None,
            search,
            browser: Loadable::Idle,
            drives: Loadable::Idle,
            browser_path: None,
            home: None,
            browser_repo: false,
            active: 0,
            submit_busy: false,
            error: None,
            focus: cx.focus_handle(),
            list_scroll: gpui::ScrollHandle::new(),
            focus_pending: true,
            load_task: None,
            drives_task: None,
            submit_task: None,
            _search_events: search_events,
        });
        cx.notify();
    }

    /// Selecting a device advances to its locations.
    fn add_space_pick_device(&mut self, device: Device, cx: &mut Context<Self>) {
        let Some(flow) = self.add_space.as_mut() else {
            return;
        };
        flow.focus_pending = true;
        flow.step = ProjectStep::Locations;
        flow.location = None;
        flow.load_task = None;
        flow.drives_task = None;
        flow.list_scroll.set_offset(gpui::Point::default());
        flow.device = Some(device);
        flow.browser = Loadable::Idle;
        flow.drives = Loadable::Idle;
        flow.browser_path = None;
        flow.home = None;
        flow.browser_repo = false;
        flow.active = 0;
        flow.error = None;
        let search = flow.search.clone();
        search.update(cx, |input, cx| {
            input.set_placeholder("Search locations…", cx);
            input.set_text("", cx);
        });
        self.load_space_drives(cx);
        cx.notify();
    }

    fn add_space_goto_location(
        &mut self,
        name: String,
        path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(flow) = self.add_space.as_mut() else {
            return;
        };
        flow.focus_pending = true;
        flow.step = ProjectStep::Folders;
        flow.location = Some((name, path.clone()));
        flow.browser_repo = false;
        let search = flow.search.clone();
        search.update(cx, |input, cx| {
            input.set_placeholder("Search folders…", cx);
            input.set_text("", cx);
        });
        self.load_space_folders(path, cx);
    }

    fn add_space_back_to(&mut self, step: ProjectStep, cx: &mut Context<Self>) {
        let Some(flow) = self.add_space.as_mut() else {
            return;
        };
        flow.focus_pending = true;
        flow.step = step;
        flow.load_task = None;
        flow.browser = Loadable::Idle;
        flow.browser_path = None;
        flow.location = None;
        flow.browser_repo = false;
        flow.active = 0;
        flow.error = None;
        flow.list_scroll.set_offset(gpui::Point::default());
        if step == ProjectStep::Devices {
            flow.drives_task = None;
            flow.device = None;
            flow.drives = Loadable::Idle;
            flow.home = None;
        }
        let search = flow.search.clone();
        search.update(cx, |input, cx| {
            input.set_placeholder(
                if step == ProjectStep::Devices {
                    "Search devices…"
                } else {
                    "Search locations…"
                },
                cx,
            );
            input.set_text("", cx);
        });
        cx.notify();
    }

    fn add_space_devices(&self, cx: &App) -> Vec<Device> {
        let Some(flow) = &self.add_space else {
            return Vec::new();
        };
        let devices = &self.state.read(cx).devices;
        let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
        popover::filter_indices(flow.search.read(cx).text(), &names)
            .into_iter()
            .map(|ix| devices[ix].clone())
            .collect()
    }

    fn add_space_locations(&self, cx: &App) -> Vec<(String, Option<String>)> {
        let Some(flow) = &self.add_space else {
            return Vec::new();
        };
        let locations: Vec<_> = std::iter::once(("Home".to_string(), None))
            .chain(
                flow.drives
                    .ready()
                    .into_iter()
                    .flatten()
                    .map(|d| (d.name.clone(), Some(d.path.clone()))),
            )
            .collect();
        let names: Vec<_> = locations.iter().map(|(name, _)| name.as_str()).collect();
        popover::filter_indices(flow.search.read(cx).text(), &names)
            .into_iter()
            .map(|ix| locations[ix].clone())
            .collect()
    }

    /// ListDrives on the flow's device (relay-forwarded when remote).
    /// Failures stay silent — the section just shows Home; the folder
    /// browser's own error row already covers "device didn't respond".
    fn load_space_drives(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let local = self.state.read(cx).local_device_id.clone();
        let Some(flow) = self.add_space.as_mut() else {
            return;
        };
        let device_id = flow.device.as_ref().map(|d| d.id.clone());
        flow.drives = Loadable::Loading;
        flow.drives_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            // Only target remote devices — local calls skip the relay.
            if let (Some(target), local) = (&device_id, &local)
                && local.as_deref() != Some(target.as_str())
            {
                params.insert(
                    "targetDeviceId".into(),
                    serde_json::Value::String(target.clone()),
                );
            }
            let result = engine
                .client()
                .call(methods::LIST_DRIVES, serde_json::Value::Object(params))
                .await;
            this.update(cx, |shell, cx| {
                if let Some(flow) = shell.add_space.as_mut() {
                    flow.drives = match result {
                        Ok(value) => match serde_json::from_value::<DriveListing>(value) {
                            Ok(listing) => Loadable::Ready(listing.drives),
                            Err(err) => Loadable::Error(err.to_string()),
                        },
                        Err(err) => Loadable::Error(err.to_string()),
                    };
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The current listing's folder rows filtered by the search query
    /// (prefix matches first — `popover::filter_indices`).
    fn add_space_filtered(&self, cx: &App) -> Vec<zeron_proto::FolderEntry> {
        let Some(flow) = self.add_space.as_ref() else {
            return Vec::new();
        };
        if flow.step != ProjectStep::Folders {
            return Vec::new();
        }
        let Some(listing) = flow.browser.ready() else {
            return Vec::new();
        };
        let dirs = browser_rows(listing);
        let query = flow.search.read(cx).text().to_string();
        let names: Vec<&str> = dirs.iter().map(|e| e.name.as_str()).collect();
        popover::filter_indices(&query, &names)
            .into_iter()
            .map(|ix| dirs[ix].clone())
            .collect()
    }

    /// Descend into the highlighted (filtered) folder; clears the query.
    /// A path-shaped query with no matching rows browses the typed path
    /// instead — `/disk2⏎` must work, not sit on "No folders match" (an
    /// absolute query can never match a folder name anyway).
    fn add_space_open_active(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = self.add_space.as_ref() else {
            return;
        };
        match flow.step {
            ProjectStep::Devices => {
                if let Some(device) = self.add_space_devices(cx).get(flow.active).cloned() {
                    self.add_space_pick_device(device, cx);
                }
                return;
            }
            ProjectStep::Locations => {
                if let Some((name, path)) = self.add_space_locations(cx).get(flow.active).cloned() {
                    self.add_space_goto_location(name, path, cx);
                }
                return;
            }
            ProjectStep::Folders => {}
        }
        let rows = self.add_space_filtered(cx);
        let Some(flow) = self.add_space.as_ref() else {
            return;
        };
        if rows.is_empty() {
            let text = flow.search.read(cx).text().to_string();
            if text.starts_with('/') || text.starts_with('~') {
                if let Some(target) = crate::pickers::typed_path_target(&text, flow.home.as_deref())
                {
                    self.add_space_descend(target, false, cx);
                }
            }
            return;
        }
        let Some(listing) = flow.browser.ready() else {
            return;
        };
        let Some(entry) = rows.get(flow.active) else {
            return;
        };
        let full = crate::pickers::child_path(&listing.path, &entry.name);
        let is_repo = entry.is_repo;
        let search = flow.search.clone();
        if let Some(flow) = self.add_space.as_mut() {
            flow.browser_repo = is_repo;
        }
        search.update(cx, |input, cx| input.set_text("", cx));
        self.load_space_folders(Some(full), cx);
    }

    /// Slash-descend: when the query ends in `/` and the part before it names
    /// a folder of the current listing (exact name — matching casing wins
    /// over a case-colliding sibling — else a unique prefix), descend into it
    /// as though it were picked. Returns whether it fired —
    /// descending clears the query, so the caller must not keep acting on the
    /// old text.
    fn add_space_slash_descend(&mut self, cx: &mut Context<Self>) -> bool {
        if self
            .add_space
            .as_ref()
            .is_none_or(|f| f.step != ProjectStep::Folders)
        {
            return false;
        }
        // A typed PATH jump: an absolute (`/disk2/`) or home-relative (`~/x/`)
        // query browses that path directly — mounts at unconventional roots
        // (and anywhere else) are reachable without a Locations row. Same
        // trailing-`/` trigger as the folder-name descend below.
        {
            let Some(flow) = self.add_space.as_ref() else {
                return false;
            };
            let text = flow.search.read(cx).text().to_string();
            if text.ends_with('/') && (text.starts_with('/') || text.starts_with('~')) {
                let target = crate::pickers::typed_path_target(&text, flow.home.as_deref());
                let Some(target) = target else {
                    // Path-shaped but unresolvable (`~/…` before home is
                    // known) — leave the query alone.
                    return false;
                };
                self.add_space_descend(target, false, cx);
                return true;
            }
        }
        let target = {
            let Some(flow) = self.add_space.as_ref() else {
                return false;
            };
            let text = flow.search.read(cx).text().to_string();
            let Some(query) = text.strip_suffix('/') else {
                return false;
            };
            if query.is_empty() || query.contains('/') {
                return false;
            }
            let Some(listing) = flow.browser.ready() else {
                return false;
            };
            let dirs = browser_rows(listing);
            let names: Vec<&str> = dirs.iter().map(|e| e.name.as_str()).collect();
            crate::pickers::segment_target(&names, query).map(|ix| {
                (
                    crate::pickers::child_path(&listing.path, &dirs[ix].name),
                    dirs[ix].is_repo,
                )
            })
        };
        let Some((full, is_repo)) = target else {
            return false;
        };
        self.add_space_descend(full, is_repo, cx);
        true
    }

    /// The tab-completion target: the highlighted row when the query prefixes
    /// its name, else the first prefix match (filtering ranks those first).
    /// `(full name, remaining suffix)`; `None` on an empty query or when the
    /// match is already complete.
    fn add_space_completion(&self, cx: &App) -> Option<(String, String)> {
        let flow = self.add_space.as_ref()?;
        let query = flow.search.read(cx).text().to_string();
        if query.is_empty() {
            return None;
        }
        let rows = self.add_space_filtered(cx);
        let entry = rows
            .get(flow.active)
            .filter(|e| completion_prefix_len(&e.name, &query).is_some())
            .or_else(|| {
                rows.iter()
                    .find(|e| completion_prefix_len(&e.name, &query).is_some())
            })?;
        let len = completion_prefix_len(&entry.name, &query)?;
        if len >= entry.name.len() {
            return None;
        }
        Some((entry.name.clone(), entry.name[len..].to_string()))
    }

    /// ⇥: accept the completion — the query becomes the full folder name
    /// (the ghost the input was previewing). Descending stays on `/`/⏎.
    fn add_space_accept_completion(&mut self, cx: &mut Context<Self>) {
        let Some((name, _)) = self.add_space_completion(cx) else {
            return;
        };
        if let Some(flow) = self.add_space.as_ref() {
            let search = flow.search.clone();
            search.update(cx, |input, cx| input.set_text(name, cx));
        }
    }

    /// Descend into a specific folder row (mouse path); clears the query.
    fn add_space_descend(&mut self, full: String, is_repo: bool, cx: &mut Context<Self>) {
        let Some(flow) = self.add_space.as_mut() else {
            return;
        };
        flow.browser_repo = is_repo;
        let search = flow.search.clone();
        search.update(cx, |input, cx| input.set_text("", cx));
        self.load_space_folders(Some(full), cx);
    }

    /// ListFolders on the flow's device (relay-forwarded when remote).
    pub(super) fn load_space_folders(&mut self, path: Option<String>, cx: &mut Context<Self>) {
        let engine = self.state.read(cx).engine().cloned();
        let local = self.state.read(cx).local_device_id.clone();
        let Some(flow) = self.add_space.as_mut() else {
            return;
        };
        flow.focus_pending = true;
        let device_id = flow.device.as_ref().map(|d| d.id.clone());
        let went_home = path.is_none();
        flow.browser_path = path.clone();
        flow.browser = Loadable::Loading;
        flow.active = 0;
        flow.list_scroll.set_offset(gpui::Point::default());
        let Some(engine) = engine else {
            flow.browser = Loadable::Error("Device is not connected".into());
            cx.notify();
            return;
        };
        flow.load_task = Some(cx.spawn(async move |this, cx| {
            let mut params = serde_json::Map::new();
            if let Some(p) = &path {
                params.insert("path".into(), serde_json::Value::String(p.clone()));
            }
            // Only target remote devices — local calls skip the relay.
            if let (Some(target), local) = (&device_id, &local)
                && local.as_deref() != Some(target.as_str())
            {
                params.insert(
                    "targetDeviceId".into(),
                    serde_json::Value::String(target.clone()),
                );
            }
            let result = engine
                .client()
                .call(methods::LIST_FOLDERS, serde_json::Value::Object(params))
                .await;
            this.update(cx, |shell, cx| {
                if let Some(flow) = shell.add_space.as_mut() {
                    flow.browser = match result {
                        Ok(value) => match serde_json::from_value::<FolderListing>(value) {
                            Ok(listing) => {
                                // A pathless browse resolved home — remember it
                                // so the breadcrumbs can fold it into the
                                // device crumb.
                                if went_home {
                                    flow.home = Some(listing.path.clone());
                                }
                                Loadable::Ready(listing)
                            }
                            Err(err) => Loadable::Error(err.to_string()),
                        },
                        Err(err) => Loadable::Error(err.to_string()),
                    };
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Create the space for the browser's current folder.
    fn submit_add_space(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let Some(flow) = self.add_space.as_ref() else {
            return;
        };
        if flow.submit_busy || flow.step != ProjectStep::Folders {
            return;
        }
        let Some(device) = flow.device.clone() else {
            return;
        };
        let Some(listing) = flow.browser.ready() else {
            return;
        };
        let path = listing.path.clone();
        let git_detected = flow.browser_repo;
        // Same (device, folder) already has a space → just switch to it. The
        // engine dedupes this case too (a createSpace for a duplicate pair
        // no-ops), so creating would leave the minted id dangling.
        if let Some(existing) = self
            .state
            .read(cx)
            .spaces
            .iter()
            .find(|s| s.device_id == device.id && s.path == path)
            .map(|s| s.id.clone())
        {
            self.add_space = None;
            self.land_in_space(existing, cx);
            return;
        }
        let Some(flow) = self.add_space.as_mut() else {
            return;
        };
        flow.submit_busy = true;
        flow.error = None;
        let space_id = uuid::Uuid::new_v4().to_string();
        // Optimistic echo: the watch frame carrying the real row replaces it
        // by id (apply_spaces re-sorts; same-id upsert is idempotent).
        let space = Space {
            id: space_id.clone(),
            device_id: device.id.clone(),
            path: path.clone(),
            name: None,
            git_detected,
            git_checked_at: None,
            checkout_id: None,
            created_at: Utc::now(),
        };
        self.state.update(cx, |s, cx| {
            if !s.spaces.iter().any(|existing| existing.id == space.id) {
                s.spaces.push(space);
            }
            cx.notify();
        });
        let params = serde_json::json!({
            "op": "createSpace",
            "spaceId": space_id,
            "deviceId": device.id,
            "path": path,
            "gitDetected": git_detected,
        });
        let submit_id = space_id.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::MUTATE, params).await;
            this.update(cx, |shell, cx| {
                match result {
                    Ok(_) => {
                        shell.add_space = None;
                        shell.land_in_space(submit_id.clone(), cx);
                    }
                    Err(err) => {
                        // Roll the optimistic row back; surface the error inline.
                        shell.state.update(cx, |s, cx| {
                            s.spaces.retain(|space| space.id != submit_id);
                            cx.notify();
                        });
                        if let Some(flow) = shell.add_space.as_mut() {
                            flow.submit_busy = false;
                            flow.error = Some(format!("{err}").into());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        });
        if let Some(flow) = self.add_space.as_mut() {
            flow.submit_task = Some(task);
        }
        cx.notify();
    }

    /// Back traverses folders, then locations, then devices.
    fn add_space_go_up(&mut self, cx: &mut Context<Self>) {
        let Some(flow) = &self.add_space else {
            return;
        };
        match flow.step {
            ProjectStep::Devices => return,
            ProjectStep::Locations => self.add_space_back_to(ProjectStep::Devices, cx),
            ProjectStep::Folders => {
                let listing = flow.browser.ready();
                let root = flow
                    .location
                    .as_ref()
                    .and_then(|(_, path)| path.as_deref())
                    .or(flow.home.as_deref());
                let parent = listing
                    .filter(|l| Some(l.path.as_str()) != root)
                    .and_then(|l| parent_path(&l.path));
                if let Some(parent) = parent {
                    self.add_space_descend(parent, false, cx);
                } else {
                    self.add_space_back_to(ProjectStep::Locations, cx);
                }
            }
        }
    }

    /// Palette keys (bubbling from the focused search input) — every legend
    /// maps to a REAL key: ↑↓ (or ctrl-n/p) navigate, →/⏎ open the
    /// highlighted folder, ← up a level, ⇥ completes the query to the
    /// previewed folder name, ⌘⏎ add the OPEN folder, ⌫ (empty query) also
    /// goes up, esc closes. (Typing `/` also descends — see the Edited
    /// subscription.)
    fn add_space_key(&mut self, event: &gpui::KeyDownEvent, cx: &mut Context<Self>) {
        // ←/→ act on the FOLDERS, not the text cursor — the palette is a
        // navigator first; queries are short and edited with ⌫.
        match event.keystroke.key.as_str() {
            "right" => {
                self.add_space_open_active(cx);
                return;
            }
            "left" => {
                self.add_space_go_up(cx);
                return;
            }
            // Unbound in "PaletteSearch" (like enter), so it bubbles here
            // instead of editing text or moving focus.
            "tab" => {
                self.add_space_accept_completion(cx);
                return;
            }
            _ => {}
        }
        let key = popover::classify_key(
            event.keystroke.key.as_str(),
            event.keystroke.modifiers.platform,
            event.keystroke.modifiers.control,
        );
        match key {
            popover::MenuKey::Escape => {
                self.add_space = None;
                cx.notify();
                cx.stop_propagation();
            }
            popover::MenuKey::Up | popover::MenuKey::Down => {
                let count = match self.add_space.as_ref().map(|f| f.step) {
                    Some(ProjectStep::Devices) => self.add_space_devices(cx).len(),
                    Some(ProjectStep::Locations) => self.add_space_locations(cx).len(),
                    _ => self.add_space_filtered(cx).len(),
                };
                let delta = if key == popover::MenuKey::Up { -1 } else { 1 };
                if let Some(flow) = self.add_space.as_mut() {
                    flow.active = popover::menu_step(Some(flow.active), count, delta).unwrap_or(0);
                    // Keep the highlighted row in view as the cursor walks
                    // past the viewport (user-reported: the list didn't
                    // follow the keyboard).
                    flow.list_scroll.scroll_to_item(flow.active);
                    cx.notify();
                }
            }
            // ⏎ opens the highlighted folder (an alias for →); the space is
            // added with ⌘⏎ — and the chord acts on the folder OPEN in the
            // breadcrumbs, not the highlight. The highlight auto-rests on the
            // first row, so a chord that took it would add arbitrary
            // subfolders; the usual target (a repo root full of subfolders)
            // is only ever "the folder you're standing in".
            popover::MenuKey::Enter => self.add_space_open_active(cx),
            popover::MenuKey::ModEnter => self.submit_add_space(cx),
            popover::MenuKey::Backspace => {
                let empty = self
                    .add_space
                    .as_ref()
                    .is_some_and(|f| f.search.read(cx).is_empty());
                if empty {
                    self.add_space_go_up(cx);
                }
            }
            popover::MenuKey::Other => {}
        }
    }

    /// The same glass, header, row rhythm, scroll gutters and footer as Cmd+K.
    pub(super) fn render_add_space_overlay(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let flow = self.add_space.as_mut()?;
        if std::mem::take(&mut flow.focus_pending) {
            window.focus(&flow.search.focus_handle(cx), cx);
        }
        let step = flow.step;
        let search = flow.search.clone();
        let focus = flow.focus.clone();
        let scroll = flow.list_scroll.clone();
        let device = flow.device.clone();
        let listing = flow.browser.ready().cloned();
        let location = flow.location.clone();
        let home = flow.home.clone();
        let load_error = flow.browser.error().map(str::to_string);
        let error = flow.error.clone();
        let busy = flow.submit_busy;
        let active = flow.active;
        let loading = matches!(flow.browser, Loadable::Idle | Loadable::Loading);
        let drives_loading = matches!(flow.drives, Loadable::Loading);
        let ghost = self
            .add_space_completion(cx)
            .map(|(_, suffix)| SharedString::from(suffix));
        search.update(cx, |input, cx| {
            input.set_ghost(ghost, cx);
        });
        let query = search.read(cx).text().to_string();
        let row = |ix: usize| {
            popover::menu_row(&theme, ix == active, format!("project-result-{ix}"))
                .id(("project-result", ix))
                .rounded(px(popover::PALETTE_ITEM_RADIUS))
                .h(px(32.0))
                .flex_none()
        };
        let mut rows = Vec::new();
        match step {
            ProjectStep::Devices => {
                for (ix, device) in self.add_space_devices(cx).into_iter().enumerate() {
                    let glyph = match device.platform.as_str() {
                        "macos" | "darwin" => icons::LAPTOP,
                        "web" => icons::GLOBAL,
                        "ios" | "android" => icons::SMARTPHONE,
                        _ => icons::MONITOR,
                    };
                    let online = self.state.read(cx).device_online(&device.id, Utc::now());
                    let name = device.name.clone();
                    rows.push(
                        row(ix)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.add_space_pick_device(device.clone(), cx)
                            }))
                            .child(icon(glyph).size(px(17.0)).text_color(theme.text_muted))
                            .child(popover::search_highlight(name.into(), Some(&query), &theme))
                            .child(div().flex_1())
                            .child(div().size(px(5.0)).rounded_full().bg(if online {
                                theme.success
                            } else {
                                theme.text_faint
                            }))
                            .into_any_element(),
                    );
                }
            }
            ProjectStep::Locations => {
                for (ix, (name, path)) in self.add_space_locations(cx).into_iter().enumerate() {
                    let glyph = if path.is_none() {
                        icons::HOME
                    } else {
                        icons::HARD_DRIVE
                    };
                    let label = name.clone();
                    rows.push(
                        row(ix)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.add_space_goto_location(name.clone(), path.clone(), cx)
                            }))
                            .child(icon(glyph).size(px(17.0)).text_color(theme.text_muted))
                            .child(popover::search_highlight(
                                label.into(),
                                Some(&query),
                                &theme,
                            ))
                            .into_any_element(),
                    );
                }
            }
            ProjectStep::Folders => {
                if !loading && load_error.is_none() {
                    for (ix, entry) in self.add_space_filtered(cx).into_iter().enumerate() {
                        let base = listing.as_ref().map(|l| l.path.as_str()).unwrap_or("");
                        let full = crate::pickers::child_path(base, &entry.name);
                        let is_repo = entry.is_repo;
                        rows.push(
                            row(ix)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.add_space_descend(full.clone(), is_repo, cx)
                                }))
                                .child(
                                    icon(icons::FOLDER)
                                        .size(px(17.0))
                                        .text_color(theme.text_muted),
                                )
                                .child(popover::search_highlight(
                                    entry.name.into(),
                                    Some(&query),
                                    &theme,
                                ))
                                .child(div().flex_1())
                                .when(is_repo, |el| {
                                    el.child(
                                        icon(icons::GIT_BRANCH)
                                            .size(px(14.0))
                                            .text_color(theme.text_muted),
                                    )
                                })
                                .into_any_element(),
                        );
                    }
                }
            }
        }
        if let Some(flow) = self.add_space.as_mut() {
            flow.active = flow.active.min(rows.len().saturating_sub(1));
        }
        let empty = rows.is_empty();
        let mut results = div()
            .id("project-results")
            .max_h(px((f32::from(viewport.height) - 220.0).clamp(100.0, 424.0)))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .px(px(popover::CARD_INSET))
            .flex()
            .flex_col()
            .gap(px(SIDEBAR_LIST_GAP))
            .children(rows);
        if step == ProjectStep::Folders && loading {
            results = results.child(popover::skeleton_rows(
                "project-loading",
                &theme,
                5,
                cx.entity_id(),
                cx,
            ));
        } else if let Some(message) = load_error.filter(|_| step == ProjectStep::Folders) {
            results = results.child(
                popover::error_row(&theme, &message).p(px(14.0)).child(
                    popover::btn_ghost(&theme, "Retry", "project-retry")
                        .id("project-retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let path = this.add_space.as_ref().and_then(|f| f.browser_path.clone());
                            this.load_space_folders(path, cx);
                        })),
                ),
            );
        } else if empty {
            results = results.child(div().p(px(24.0)).text_color(theme.text_muted).child(
                match step {
                    ProjectStep::Devices => "No devices found",
                    ProjectStep::Locations => "No locations found",
                    ProjectStep::Folders if query.is_empty() => "No folders here",
                    ProjectStep::Folders => "No folders match",
                },
            ));
        }
        if step == ProjectStep::Locations && drives_loading {
            results = results.child(
                div()
                    .px(px(8.0))
                    .py(px(6.0))
                    .text_color(theme.text_muted)
                    .text_size(crate::typography::ui_rems(11.0))
                    .child("Loading locations…"),
            );
        }
        let crumb =
            |id: SharedString, name: SharedString, glyph: Option<&'static str>, current: bool| {
                div()
                    .id(id)
                    .h(px(26.0))
                    .px(px(7.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .text_color(if current {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .when(current, |el| el.bg(theme.element_hover))
                    .hover(|s| s.bg(theme.element_hover).text_color(theme.text))
                    .when_some(glyph, |el, glyph| {
                        el.child(
                            icon(glyph)
                                .size(px(14.0))
                                .flex_none()
                                .text_color(if current {
                                    theme.text
                                } else {
                                    theme.text_muted
                                }),
                        )
                    })
                    .child(div().max_w(px(140.0)).truncate().child(name))
            };
        // Keep each chevron with its destination when a long path wraps.
        let segment = |item: gpui::Stateful<gpui::Div>| {
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(
                    icon(icons::ALT_ARROW_RIGHT)
                        .size(px(12.0))
                        .text_color(theme.text_faint),
                )
                .child(item)
        };
        let mut trail = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_wrap()
            .items_center()
            .gap(px(2.0))
            .child(
                crumb(
                    "project-crumb-root".into(),
                    "New project".into(),
                    None,
                    step == ProjectStep::Devices,
                )
                .on_click(
                    cx.listener(|this, _, _, cx| this.add_space_back_to(ProjectStep::Devices, cx)),
                ),
            );
        if let Some(device) = device {
            let glyph = match device.platform.as_str() {
                "macos" | "darwin" => icons::LAPTOP,
                "ios" | "android" => icons::SMARTPHONE,
                _ => icons::MONITOR,
            };
            trail =
                trail.child(segment(
                    crumb(
                        "project-crumb-device".into(),
                        device.name.into(),
                        Some(glyph),
                        step == ProjectStep::Locations,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.add_space_back_to(ProjectStep::Locations, cx)
                    })),
                ));
        }
        if let Some((name, path)) = location {
            let glyph = if path.is_none() {
                icons::HOME
            } else {
                icons::HARD_DRIVE
            };
            let root = path.clone().or(home);
            let at_root = listing
                .as_ref()
                .is_none_or(|l| root.as_deref() == Some(l.path.as_str()));
            trail = trail.child(segment(
                crumb(
                    "project-crumb-location".into(),
                    name.clone().into(),
                    Some(glyph),
                    at_root,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.add_space_goto_location(name.clone(), path.clone(), cx)
                })),
            ));
            if let Some(listing) = listing.as_ref() {
                for (ix, (name, full)) in breadcrumbs(&listing.path).into_iter().enumerate() {
                    if root.as_deref().is_some_and(|root| path_under(root, &full)) {
                        continue;
                    }
                    trail = trail.child(segment(
                        crumb(
                            format!("project-crumb-folder-{ix}").into(),
                            name.into(),
                            Some(icons::FOLDER),
                            full == listing.path,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.add_space_descend(full.clone(), false, cx)
                        })),
                    ));
                }
            }
        }
        let crumbs = div()
            .px(px(14.0))
            .py(px(8.0))
            .flex()
            .items_start()
            .gap(px(8.0))
            .text_size(crate::typography::ui_rems(12.0))
            .child(
                div()
                    .id("project-crumb-back")
                    .size(px(26.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .text_color(theme.text_muted)
                    .hover(|s| s.bg(theme.element_hover).text_color(theme.text))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.add_space = None;
                        this.toggle_command_palette(window, cx);
                    }))
                    .child(
                        icon(icons::ARROW_LEFT)
                            .size(px(16.0))
                            .text_color(theme.text_muted),
                    ),
            )
            .child(
                div()
                    .w(px(1.0))
                    .h(px(16.0))
                    .mt(px(5.0))
                    .flex_none()
                    .bg(theme.border),
            )
            .child(trail);
        let header = div()
            .h(px(58.0))
            .flex_none()
            .px(px(18.0))
            .flex()
            .items_center()
            .gap(px(12.0))
            .border_b_1()
            .border_color(theme.border)
            .child(popover::palette_search_icon(&theme))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(crate::typography::ui_rems(15.0))
                    .child(search),
            )
            .child(popover::key_hint_text(&theme, "esc", ""));
        let footer = div()
            .flex_none()
            .px(px(18.0))
            .py(px(12.0))
            .border_t_1()
            .border_color(theme.border)
            .flex()
            .items_center()
            .gap(px(18.0))
            .child(popover::key_hint_pair(
                &theme,
                icons::ARROW_UP,
                icons::ARROW_DOWN,
                "Navigate",
            ))
            .child(popover::key_hint_text(&theme, "↵", "Open"))
            .child(popover::key_hint_text(&theme, "esc", "Close"))
            .child(div().flex_1())
            .when(step == ProjectStep::Folders, |el| {
                el.child(
                    popover::btn_ghost(
                        &theme,
                        if busy { "Adding…" } else { "Add project" },
                        "project-add",
                    )
                    .id("project-add")
                    .h(px(22.0))
                    .py(px(0.0))
                    .flex_none()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .when(busy || listing.is_none(), |el| el.opacity(0.5))
                    .on_click(cx.listener(|this, _, _, cx| this.submit_add_space(cx)))
                    .child(
                        popover::key_cap(&theme)
                            .text_size(crate::typography::ui_rems(11.0))
                            .child(crate::settings::badge_combo("mod-enter")),
                    ),
                )
            });
        let card =
            div()
                .id("add-space-palette")
                .track_focus(&focus)
                .w(px(600.0_f32.min(f32::from(viewport.width) - 32.0)))
                .flex()
                .flex_col()
                .rounded(px(14.0))
                .border_1()
                .border_color(theme.border)
                .when(!theme.is_frost(), |el| el.shadow_lg())
                .bg(popover::surface_bg(&theme))
                .text_color(theme.text)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    this.add_space_key(event, cx)
                }))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.add_space = None;
                    cx.notify();
                }))
                .child(header)
                .child(crumbs)
                .child(div().min_h_0().py(px(popover::CARD_INSET)).child(results))
                .when_some(error, |el, error| {
                    el.child(
                        div()
                            .px(px(18.0))
                            .pb(px(8.0))
                            .text_color(theme.danger)
                            .child(error),
                    )
                })
                .child(footer);
        Some(
            gpui::deferred(
                gpui::anchored()
                    .position(gpui::point(px(0.0), px(0.0)))
                    .child(
                        div()
                            .occlude()
                            .w(viewport.width)
                            .h(viewport.height)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(crate::frost::frosted(14.0, crate::frost::MENU_BLUR, card)),
                    ),
            )
            .priority(2)
            .into_any_element(),
        )
    }

    // ---- space context menu / rename / delete overlays ----

    fn close_space_menu(&mut self, cx: &mut Context<Self>) {
        if self.space_menu.begin_close() {
            popover::reap_popup(cx, |shell: &mut Self| &mut shell.space_menu);
            cx.notify();
        }
    }

    pub(super) fn open_rename_space(&mut self, space_id: String, cx: &mut Context<Self>) {
        self.close_space_menu(cx);
        let current = self
            .state
            .read(cx)
            .space_row(&space_id)
            .map(|s| s.display_name().to_string())
            .unwrap_or_default();
        let input = cx.new(|cx| ComposerInput::new("Project name", cx));
        input.update(cx, |input, cx| input.set_text(current, cx));
        let events = cx.subscribe(&input, |this: &mut Shell, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_rename_space(cx);
            }
        });
        self.rename_space_dialog = Some(RenameSpaceDialog {
            space_id,
            input,
            focus_pending: true,
            _events: events,
        });
        cx.notify();
    }

    pub(super) fn submit_rename_space(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.rename_space_dialog.take() else {
            return;
        };
        let name = dialog.input.read(cx).text().trim().to_string();
        if !name.is_empty() {
            self.mutate(
                serde_json::json!({ "op": "renameSpace", "spaceId": dialog.space_id, "name": name }),
                cx,
            );
        }
        cx.notify();
    }

    pub(super) fn delete_space(&mut self, space_id: String, cx: &mut Context<Self>) {
        self.delete_space_confirm = None;
        self.mutate(
            serde_json::json!({ "op": "deleteSpace", "spaceId": space_id }),
            cx,
        );
        cx.notify();
    }

    /// Space context menu + rename dialog + delete confirm (appended to the
    /// shell's overlay list).
    pub(super) fn render_space_overlays(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let mut overlays: Vec<AnyElement> = Vec::new();

        if let Some((space_id, position)) = self.space_menu.get().cloned() {
            let closing = self.space_menu.closing_since();
            let rename_id = space_id.clone();
            let delete_id = space_id.clone();
            let menu = popover::popover_card(&theme)
                .w(px(170.0))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.close_space_menu(cx);
                }))
                .flex()
                .flex_col()
                .child(
                    popover::menu_row(&theme, false, format!("space-menu-rename-{space_id}"))
                        .id("space-menu-rename")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_rename_space(rename_id.clone(), cx)
                        }))
                        .child(icon(icons::PEN).size(px(16.0)).text_color(theme.text_muted))
                        .child(SharedString::from("Rename…")),
                )
                .child(popover::menu_separator())
                .child(
                    popover::menu_row(&theme, false, format!("space-menu-delete-{space_id}"))
                        .id("space-menu-delete")
                        .text_color(theme.danger)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.close_space_menu(cx);
                            this.delete_space_confirm = Some(delete_id.clone());
                            cx.notify();
                        }))
                        .child(
                            icon(icons::TRASH_BIN_MINIMALISTIC)
                                .size(px(16.0))
                                .text_color(theme.danger),
                        )
                        .child(SharedString::from("Remove…")),
                )
                .into_any_element();
            overlays.push(popover::menu_at(
                "space-context-menu",
                position,
                menu,
                closing,
            ));
        }

        if let Some(dialog) = &mut self.rename_space_dialog {
            if std::mem::take(&mut dialog.focus_pending) {
                window.focus(&dialog.input.focus_handle(cx), cx);
            }
            let input = dialog.input.clone();
            let card = popover::dialog_card(&theme)
                .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                    if ev.keystroke.key == "escape" {
                        this.rename_space_dialog = None;
                        cx.notify();
                        cx.stop_propagation();
                    }
                }))
                .child(popover::dialog_title(&theme, "Rename project"))
                .child(
                    div()
                        .mt(px(12.0))
                        .child(popover::dialog_field(input.into_any_element())),
                )
                .child(
                    div()
                        .mt(px(16.0))
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            popover::btn_ghost(&theme, "Cancel", "rename-space-cancel")
                                .id("rename-space-cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.rename_space_dialog = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            popover::btn_primary(&theme, "Rename")
                                .id("rename-space-save")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.submit_rename_space(cx)),
                                ),
                        ),
                )
                .into_any_element();
            overlays.push(popover::modal("rename-space-dialog", viewport, card));
        }

        if let Some(space_id) = self.delete_space_confirm.clone() {
            let (name, device, count) = {
                let state = self.state.read(cx);
                let space = state.space_row(&space_id);
                (
                    space
                        .map(|s| s.display_name().to_string())
                        .unwrap_or_else(|| "this project".into()),
                    space
                        .and_then(|s| state.device_name(&s.device_id))
                        .unwrap_or("its device")
                        .to_string(),
                    state.chats_in_space(&space_id).len(),
                )
            };
            let copy = if count == 1 {
                format!(
                    "Removing “{name}” permanently deletes its 1 session on {device}. This can’t be undone."
                )
            } else {
                format!(
                    "Removing “{name}” permanently deletes its {count} sessions on {device}. This can’t be undone."
                )
            };
            let card = popover::dialog_card(&theme)
                .child(popover::dialog_title(&theme, "Remove project?"))
                .child(div().mt(px(6.0)).child(popover::dialog_body(&theme, copy)))
                .child(
                    div()
                        .mt(px(16.0))
                        .flex()
                        .flex_row()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            popover::btn_ghost(&theme, "Cancel", "delete-space-cancel")
                                .id("delete-space-cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.delete_space_confirm = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            popover::btn_danger(&theme, "Remove")
                                .id("delete-space-confirm")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.delete_space(space_id.clone(), cx)
                                })),
                        ),
                )
                .into_any_element();
            overlays.push(popover::modal("delete-space-dialog", viewport, card));
        }

        overlays
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};

    use super::{compare_sidebar_chats, promote_local_device_group};
    use crate::settings::SidebarSort;

    fn group(device: &str, value: u8) -> (Option<(String, String)>, Vec<u8>) {
        (Some((device.into(), device.into())), vec![value])
    }

    fn chat(id: &str) -> zeron_proto::Chat {
        zeron_proto::Chat {
            id: id.into(),
            device_id: "device".into(),
            title: None,
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: Some(Utc.timestamp_opt(10, 0).unwrap()),
            created_at: Utc.timestamp_opt(5, 0).unwrap(),
            harness_session_id: None,
            harness_session_cwd: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
        }
    }

    #[test]
    fn equal_sidebar_timestamps_sort_by_stable_chat_id() {
        let alpha = chat("alpha");
        let beta = chat("beta");
        assert!(compare_sidebar_chats(SidebarSort::Created, &alpha, &beta).is_lt());
        assert!(compare_sidebar_chats(SidebarSort::LastUpdated, &alpha, &beta).is_lt());
    }

    #[test]
    fn current_device_is_promoted_without_resorting_remote_groups() {
        let mut groups = vec![
            group("recent-remote", 1),
            group("local", 2),
            group("older-remote", 3),
        ];

        promote_local_device_group(&mut groups, Some("local"));

        let order: Vec<_> = groups
            .iter()
            .map(|(group, _)| group.as_ref().unwrap().0.as_str())
            .collect();
        assert_eq!(order, ["local", "recent-remote", "older-remote"]);
    }

    #[test]
    fn missing_current_device_leaves_group_order_untouched() {
        let mut groups = vec![group("first", 1), group("second", 2)];
        let before = groups.clone();

        promote_local_device_group(&mut groups, Some("not-present"));

        assert_eq!(groups, before);
    }
}

/// Synthetic responses for the isolated native screenshot fixture only.
#[cfg(feature = "project-palette-fixture")]
impl Shell {
    pub fn fixture_project_responses(&mut self, cx: &mut Context<Self>) {
        if std::env::var_os("ZERON_FIXTURE_BACKGROUND").is_some() {
            self.composer
                .read(cx)
                .pickers()
                .clone()
                .update(cx, |pickers, cx| pickers.fixture_model_catalog(cx));
        }
        let Some(flow) = self.add_space.as_mut() else {
            return;
        };
        if flow.device.is_some() && !matches!(flow.drives, Loadable::Ready(_)) {
            flow.drives = Loadable::Ready(
                serde_json::from_value(serde_json::json!([
                    {"name":"Projects", "path":"/projects"},
                    {"name":"System", "path":"/"}
                ]))
                .unwrap(),
            );
            cx.notify();
        }
        if flow.step == ProjectStep::Folders && !matches!(flow.browser, Loadable::Ready(_)) {
            let path = flow
                .browser_path
                .clone()
                .unwrap_or_else(|| "/home/alex".into());
            if flow.browser_path.is_none() {
                flow.home = Some(path.clone());
            }
            let names = match path.as_str() {
                "/home/alex" => vec!["Desktop", "Documents", "Downloads", "Projects"],
                "/projects" | "/home/alex/Projects" => vec!["fieldnotes", "mobile-app", "website"],
                _ => vec!["assets", "docs", "src", "tests"],
            };
            flow.browser = Loadable::Ready(
                serde_json::from_value(serde_json::json!({
                    "path":path,
                    "entries":names.into_iter().map(|name| serde_json::json!({
                        "name":name,"isDir":true,"isRepo":name=="fieldnotes"
                    })).collect::<Vec<_>>()
                }))
                .unwrap(),
            );
            cx.notify();
        }
    }
}

#[cfg(test)]
mod project_flow_tests {
    use super::*;

    #[gpui::test]
    fn devices_locations_folders_and_back_clear_stale_state(cx: &mut gpui::TestAppContext) {
        let data = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
        });
        let shell = cx.new(|cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.devices = serde_json::from_value(serde_json::json!([
                    {"id":"local","name":"Studio","platform":"macos","lastSeenAt":null},
                    {"id":"remote","name":"Server","platform":"linux","lastSeenAt":null}
                ]))
                .unwrap();
                state
            });
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: data.path().into(),
                    ipc_port: 0,
                    edge_url: String::new(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        });
        shell.update(cx, |shell, cx| {
            shell.open_add_space(cx);
            assert_eq!(shell.add_space.as_ref().unwrap().step, ProjectStep::Devices);
            assert!(shell.add_space.as_ref().unwrap().device.is_none());
            let search = shell.add_space.as_ref().unwrap().search.clone();
            search.update(cx, |input, cx| input.set_text("server", cx));
            assert_eq!(shell.add_space_devices(cx).len(), 1);
            shell.add_space_open_active(cx);
            let flow = shell.add_space.as_mut().unwrap();
            assert_eq!(flow.step, ProjectStep::Locations);
            assert_eq!(flow.device.as_ref().unwrap().id, "remote");
            assert!(flow.search.read(cx).is_empty());
            flow.drives = Loadable::Ready(vec![DriveEntry {
                name: "Projects".into(),
                path: "/projects".into(),
            }]);
            search.update(cx, |input, cx| input.set_text("projects", cx));
            shell.add_space_open_active(cx);
            let flow = shell.add_space.as_mut().unwrap();
            assert_eq!(flow.step, ProjectStep::Folders);
            assert_eq!(flow.browser_path.as_deref(), Some("/projects"));
            flow.browser = Loadable::Ready(FolderListing {
                path: "/projects".into(),
                entries: Vec::new(),
                truncated: false,
            });
            shell.add_space_go_up(cx);
            assert_eq!(
                shell.add_space.as_ref().unwrap().step,
                ProjectStep::Locations
            );
            assert!(shell.add_space.as_ref().unwrap().browser.ready().is_none());
            shell.add_space_go_up(cx);
            let flow = shell.add_space.as_ref().unwrap();
            assert_eq!(flow.step, ProjectStep::Devices);
            assert!(flow.device.is_none());
            assert!(flow.drives.ready().is_none());
            assert!(flow.search.read(cx).is_empty());
            // Slash navigation only applies to folders, never device search.
            search.update(cx, |input, cx| input.set_text("/projects/", cx));
            assert!(!shell.add_space_slash_descend(cx));
        });
    }
}
