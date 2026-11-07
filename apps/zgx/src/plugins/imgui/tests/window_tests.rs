//! The rules a plugin window follows, without a native window: what a
//! request becomes, what the controls change, and what a closed window
//! refuses.
//!
//! The native half (a real OS window with the requested title and client
//! size, the five controls leaving its handle alone, `set_title`/`set_size`
//! changing it in place and the OS close button running `on_close` once) is
//! exercised in the app itself: it needs zgx's event loop to make windows,
//! which a headless test has none of. `apps/zgx/src/plugins/imgui/probe.rs`
//! drives exactly that and reports what the probe observed.

use zgx_desktop::Titlebar;
use zgx_script::Value;

use crate::plugins::imgui::window::{
    LastPlacement, MAX_HEIGHT, MAX_WIDTH, MIN_HEIGHT, MIN_WIDTH, WindowKey, WindowSpec,
    WindowState, check_quota, frame_inset, window_options,
};

/// Parses a request as a plugin's `open_window` would.
fn parse(request: &str) -> Result<WindowSpec, String> {
    let value = Value::from_json(request).expect("a JSON table");
    WindowSpec::parse("plug", &value)
}

fn parse_error(request: &str) -> String {
    parse(request).expect_err("the request should have been refused")
}

/// A state for one window, as made from `spec`.
fn state(spec: &WindowSpec) -> WindowState {
    WindowState::new(WindowKey::new("plug", &spec.id), spec)
}

#[test]
fn a_request_keeps_the_title_and_the_size_it_was_given() {
    let spec =
        parse(r#"{"id":"inspector","title":"Probe Inspector","size":{"width":720,"height":480}}"#)
            .unwrap();
    assert_eq!(spec.title, "Probe Inspector");
    assert_eq!(spec.size, (720.0, 480.0));
    assert_eq!(spec.titlebar, Titlebar::Standard);
    assert!(spec.resizable && spec.visible && spec.decorations);
    assert!(spec.position.is_none());
    assert!(spec.min_size.is_none() && spec.max_size.is_none());
}

#[test]
fn a_window_with_no_size_opens_at_the_default() {
    let spec = parse(r#"{"id":"inspector","title":"Denied"}"#).unwrap();
    assert_eq!(spec.size, (720.0, 480.0));
}

#[test]
fn a_size_whose_width_is_text_names_size_and_the_window() {
    let error =
        parse_error(r#"{"id":"inspector","title":"x","size":{"width":"big","height":480}}"#);
    assert!(error.contains("size"), "{error}");
    assert!(error.contains("inspector"), "{error}");
}

#[test]
fn a_negative_size_names_size_and_the_window() {
    let error = parse_error(r#"{"id":"inspector","title":"x","size":{"width":-10,"height":480}}"#);
    assert!(error.contains("size"), "{error}");
    assert!(error.contains("inspector"), "{error}");
}

#[test]
fn a_position_missing_y_names_position_and_the_window() {
    let error = parse_error(r#"{"id":"inspector","title":"x","position":{"x":100}}"#);
    assert!(error.contains("position"), "{error}");
    assert!(error.contains('y'), "{error}");
    assert!(error.contains("inspector"), "{error}");
}

#[test]
fn a_title_that_is_not_text_names_title() {
    let error = parse_error(r#"{"id":"inspector","title":42}"#);
    assert!(error.contains("title"), "{error}");
    assert!(error.contains("inspector"), "{error}");
}

#[test]
fn an_empty_id_names_id() {
    let error = parse_error(r#"{"id":"","title":"x"}"#);
    assert!(error.contains("id"), "{error}");
}

#[test]
fn a_missing_id_names_id() {
    let error = parse_error(r#"{"title":"x"}"#);
    assert!(error.contains("id"), "{error}");
}

#[test]
fn a_title_longer_than_200_bytes_is_refused() {
    let title = "a".repeat(201);
    let error = parse_error(&format!(r#"{{"id":"inspector","title":"{title}"}}"#));
    assert!(error.contains("title"), "{error}");
    assert!(error.contains("inspector"), "{error}");
}

#[test]
fn a_titlebar_must_be_standard_inset_or_hidden() {
    let error = parse_error(r#"{"id":"inspector","title":"x","titlebar":"frameless"}"#);
    assert!(error.contains("titlebar"), "{error}");
    assert!(error.contains("inspector"), "{error}");
    for style in ["standard", "inset", "hidden"] {
        let spec = parse(&format!(
            r#"{{"id":"inspector","title":"x","titlebar":"{style}"}}"#
        ))
        .unwrap();
        assert_eq!(
            spec.titlebar,
            match style {
                "inset" => Titlebar::Inset,
                "hidden" => Titlebar::Hidden,
                _ => Titlebar::Standard,
            }
        );
    }
}

#[test]
fn standard_without_decorations_is_refused_and_names_both_fields() {
    let error =
        parse_error(r#"{"id":"inspector","title":"x","titlebar":"standard","decorations":false}"#);
    assert!(error.contains("decorations"), "{error}");
    assert!(error.contains("titlebar"), "{error}");
    assert!(error.contains("inspector"), "{error}");
}

#[test]
fn hidden_chrome_drops_the_platform_frame() {
    let spec =
        parse(r#"{"id":"inspector","title":"x","titlebar":"hidden","decorations":false}"#).unwrap();
    assert_eq!(spec.titlebar, Titlebar::Hidden);
    assert!(!spec.decorations);
    let implicit = parse(r#"{"id":"inspector","title":"x","titlebar":"hidden"}"#).unwrap();
    assert_eq!(implicit.titlebar, Titlebar::Hidden);
    assert!(
        !implicit.decorations,
        "hidden chrome must never keep the platform frame"
    );
}

#[test]
fn a_minimum_larger_than_the_maximum_names_min_size() {
    let error = parse_error(
        r#"{"id":"inspector","title":"x","min_size":{"width":900,"height":600},"max_size":{"width":400,"height":300}}"#,
    );
    assert!(error.contains("min_size"), "{error}");
    assert!(error.contains("inspector"), "{error}");
}

#[test]
fn sizes_are_clamped_to_what_a_window_can_be() {
    let small =
        parse(r#"{"id":"inspector","title":"x","size":{"width":100,"height":60}}"#).unwrap();
    assert_eq!(small.size, (MIN_WIDTH, MIN_HEIGHT));
    let big =
        parse(r#"{"id":"inspector","title":"x","size":{"width":9000,"height":5000}}"#).unwrap();
    assert_eq!(big.size, (MAX_WIDTH, MAX_HEIGHT));
    let bounds = parse(
        r#"{"id":"inspector","title":"x","size":{"width":900,"height":600},"min_size":{"width":400,"height":300},"max_size":{"width":1280,"height":720}}"#,
    )
    .unwrap();
    assert_eq!(bounds.size, (900.0, 600.0));
    assert_eq!(bounds.min_size, Some((400.0, 300.0)));
    assert_eq!(bounds.max_size, Some((1280.0, 720.0)));
}

#[test]
fn the_options_zgx_opens_carry_the_exact_title_and_logical_size() {
    let spec = parse(
        r#"{"id":"inspector","title":"Probe Inspector","size":{"width":720,"height":480},"position":{"x":100,"y":120}}"#,
    )
    .unwrap();
    let key = WindowKey::new("plug", "inspector");
    let options = spec.to_options(&key, 2.0);
    assert_eq!(options.title, "Probe Inspector");
    assert_eq!((options.width, options.height), (720.0, 480.0));
    let bounds = options.bounds.unwrap();
    assert_eq!(bounds.size, (1440, 960));
    let inset = frame_inset(spec.decorations, spec.titlebar, spec.resizable, 2.0);
    assert_eq!(bounds.position, Some((200 + inset.0, 240 + inset.1)));
    assert!(options.resizable);
    assert_eq!(options.tabbing, zgx_desktop::Tabbing::Disallowed);
    // Persistence is opt-in: a requested size is the size a window opens at.
    assert!(options.frame_autosave.is_none());
    let remembered = parse(
        r#"{"id":"inspector","title":"Probe Inspector","size":{"width":720,"height":480},"remember":true}"#,
    )
    .unwrap();
    assert_eq!(
        remembered.to_options(&key, 1.0).frame_autosave.as_deref(),
        Some("plugin.plug.inspector")
    );
}

#[test]
fn hiding_is_not_closing() {
    let spec = parse(r#"{"id":"inspector","title":"x"}"#).unwrap();
    let state = state(&spec);
    assert!(state.is_open() && state.is_shown());
    state.hide().unwrap();
    assert!(state.is_open(), "hiding a window closed it");
    assert!(!state.is_shown());
    state.show().unwrap();
    assert!(state.is_open() && state.is_shown());
}

#[test]
fn minimizing_and_restoring_change_state_alone() {
    let spec = parse(r#"{"id":"inspector","title":"x"}"#).unwrap();
    let state = state(&spec);
    state.minimize().unwrap();
    assert!(state.is_open(), "minimizing a window closed it");
    assert!(state.is_minimized() && !state.is_shown());
    state.restore().unwrap();
    assert!(state.is_open() && state.is_shown() && !state.is_minimized());
}

#[test]
fn set_title_and_set_size_change_what_the_window_reports() {
    let spec =
        parse(r#"{"id":"inspector","title":"x","size":{"width":720,"height":480}}"#).unwrap();
    let state = state(&spec);
    state.set_title("Renamed Probe").unwrap();
    assert_eq!(state.title(), "Renamed Probe");
    state.set_size((1000.0, 640.0)).unwrap();
    assert_eq!(state.size(), (1000.0, 640.0));
    assert!(state.is_open());
}

#[test]
fn a_closed_window_reports_closed_and_refuses_every_control() {
    let spec = parse(r#"{"id":"inspector","title":"x"}"#).unwrap();
    let state = state(&spec);
    assert!(state.begin_close(), "the first close did not close it");
    assert!(!state.begin_close(), "it was closed twice");
    assert!(!state.is_open() && !state.is_shown());
    for error in [
        state.show().unwrap_err(),
        state.hide().unwrap_err(),
        state.minimize().unwrap_err(),
        state.restore().unwrap_err(),
        state.maximize().unwrap_err(),
        state.toggle_maximize().unwrap_err(),
        state.set_title("again").unwrap_err(),
        state.set_size((1000.0, 640.0)).unwrap_err(),
    ] {
        assert!(error.contains("inspector"), "{error}");
        assert!(error.contains("closed"), "{error}");
    }
}

#[test]
fn begin_resize_never_reports_success_for_a_no_op() {
    let spec = parse(r#"{"id":"inspector","title":"x"}"#).unwrap();
    let state = state(&spec);
    let error = state
        .begin_resize()
        .expect_err("begin_resize must not succeed");
    assert!(error.contains("begin_resize"), "{error}");
    assert!(error.contains("no platform"), "{error}");
    assert!(error.contains("inspector"), "{error}");
    state.begin_close();
    let closed = state.begin_resize().expect_err("a closed window refuses");
    assert!(closed.contains("closed"), "{closed}");
}

#[test]
fn maximizing_and_toggling_track_state_and_restore_clears_it() {
    let spec = parse(r#"{"id":"inspector","title":"x"}"#).unwrap();
    let state = state(&spec);
    state.maximize().unwrap();
    assert!(state.is_open() && state.is_shown() && state.is_maximized());
    state.toggle_maximize().unwrap();
    assert!(!state.is_maximized());
    state.toggle_maximize().unwrap();
    assert!(state.is_maximized());
    state.restore().unwrap();
    assert!(state.is_open() && state.is_shown());
    assert!(!state.is_minimized() && !state.is_maximized());
}

#[test]
fn the_same_window_id_in_two_plugins_is_two_windows() {
    let a = WindowKey::new("a", "inspector");
    let b = WindowKey::new("b", "inspector");
    assert_ne!(a.global(), b.global());
    let spec = parse(r#"{"id":"inspector","title":"x"}"#).unwrap();
    assert_eq!(
        state(&spec).key(),
        &WindowKey::new("plug", "inspector"),
        "a state lost which plugin it belongs to"
    );
}

#[test]
fn the_eighth_window_is_allowed_and_the_ninth_names_eight() {
    for mine in 0..8 {
        assert!(check_quota("plug", mine, mine).is_ok());
    }
    let error = check_quota("plug", 8, 8).expect_err("a ninth window was allowed");
    assert!(error.contains('8'), "{error}");
    assert!(error.contains("plug"), "{error}");
}

#[test]
fn the_whole_app_stops_at_32_windows() {
    let error = check_quota("plug", 0, 32).expect_err("a 33rd window was allowed");
    assert!(error.contains("32"), "{error}");
    assert!(check_quota("plug", 0, 31).is_ok());
}

#[test]
fn the_live_cap_is_a_count_not_a_latch() {
    assert!(check_quota("plug", 8, 8).is_err(), "nine open is refused");
    assert!(
        check_quota("plug", 7, 8).is_ok(),
        "a closed slot opens again"
    );
}

#[test]
fn a_remembered_window_restores_the_saved_placement_and_a_plain_one_does_not() {
    let remembered = parse(
        r#"{"id":"inspector","title":"x","size":{"width":720,"height":480},"position":{"x":120,"y":80},"remember":true}"#,
    )
    .unwrap();
    let plain = parse(
        r#"{"id":"inspector","title":"x","size":{"width":720,"height":480},"remember":false}"#,
    )
    .unwrap();
    let key = WindowKey::new("plug", "inspector");
    let saved = LastPlacement {
        bounds: zgx_desktop::WindowBounds {
            position: Some((400, 300)),
            size: (1800, 1120),
        },
        size: (900.0, 560.0),
    };

    let (options, size) = window_options(&remembered, &key, 2.0, Some(saved));
    assert_eq!(options.bounds, Some(saved.bounds));
    assert_eq!(size, (900.0, 560.0));

    let (options, size) = window_options(&plain, &key, 2.0, Some(saved));
    assert!(
        options.bounds.is_none(),
        "a plain window keeps no saved frame"
    );
    assert_eq!(size, (720.0, 480.0));
    assert!(options.frame_autosave.is_none());
}

#[test]
fn a_requested_position_is_compensated_to_the_client_origin() {
    let spec = parse(
        r#"{"id":"inspector","title":"x","size":{"width":400,"height":300},"position":{"x":100,"y":120}}"#,
    )
    .unwrap();
    let key = WindowKey::new("plug", "inspector");
    let inset = frame_inset(spec.decorations, spec.titlebar, spec.resizable, 1.0);
    if cfg!(windows) {
        assert!(inset.0 < 0 && inset.1 < 0, "{inset:?}");
    } else {
        assert_eq!(inset, (0, 0));
    }
    let options = spec.to_options(&key, 1.0);
    let position = options.bounds.unwrap().position.unwrap();
    assert_eq!(position, (100 + inset.0, 120 + inset.1));
}
