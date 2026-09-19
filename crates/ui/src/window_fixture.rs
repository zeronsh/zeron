//! Native-platform probe, compiled only by the multi-window fixture example.

use std::{path::PathBuf, time::Duration};

use gpui::App;

use crate::{app_menus, app_runtime::AppRuntime, state::ConnectionStatus, window_manager};

pub(crate) fn start(output: PathBuf, cx: &mut App) {
    cx.spawn(async move |cx| {
        let mut ready = false;
        for _ in 0..200 {
            ready = cx.update(|cx| matches!(cx.global::<AppRuntime>().state.read(cx).connection, ConnectionStatus::Ready));
            if ready { break; }
            cx.background_executor().timer(Duration::from_millis(100)).await;
        }
        assert!(ready, "fixture engine did not become ready");
        let first = cx.update(|cx| window_manager::recent(cx).unwrap());
        cx.update(|cx| {
            cx.dispatch_action(&app_menus::NewWindow);
            cx.dispatch_action(&app_menus::NewWindow);
        });
        cx.background_executor().timer(Duration::from_millis(500)).await;
        let handles = cx.update(|cx| {
            let handles = cx.windows().into_iter().filter_map(|w| w.downcast::<crate::shell::Shell>()).collect::<Vec<_>>();
            assert_eq!(handles.len(), 3);
            let keys = handles.iter().map(|w| w.read(cx).unwrap().state.read(cx).window_key.clone().unwrap()).collect::<std::collections::HashSet<_>>();
            assert_eq!(keys.len(), 3, "each native window needs its own identity");
            // The original window can still be finishing its boot fade.
            // Newly attached windows must expose their content immediately.
            for window in handles.iter().filter(|window| **window != first) {
                assert!(!window.read(cx).unwrap().startup_overlay_visible(), "a ready window must expose its content without another engine notification");
            }
            first.update(cx, |shell, window, cx| {
                assert!(shell.prepare_window_close(cx));
                window.remove_window();
            }).unwrap();
            handles
        });
        cx.background_executor().timer(Duration::from_millis(500)).await;
        cx.update(|cx| {
            assert_eq!(window_manager::views(cx).len(), 2);
            assert!(matches!(cx.global::<AppRuntime>().state.read(cx).connection, ConnectionStatus::Ready));
            assert!(first.read(cx).is_err());
            let replacement = window_manager::open(window_manager::Open::Blank, cx).unwrap();
            assert!(!handles.contains(&replacement));
            assert_eq!(window_manager::views(cx).len(), 3);
        });
        cx.background_executor().timer(Duration::from_millis(500)).await;
        cx.update(|cx| {
            std::fs::write(&output, "PASS: three native windows, independent identities, first-window close, live engine and replacement window\n").unwrap();
            app_menus::request_quit(cx);
        });
    }).detach();
}
