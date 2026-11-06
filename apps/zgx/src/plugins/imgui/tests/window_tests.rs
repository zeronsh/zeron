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
    MAX_HEIGHT, MAX_WIDTH, MIN_HEIGHT, MIN_WIDTH, WindowKey, WindowSpec, WindowState, check_quota,
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
    assert_eq!(bounds.position, Some((200, 240)));
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
        state.set_title("again").unwrap_err(),
        state.set_size((1000.0, 640.0)).unwrap_err(),
    ] {
        assert!(error.contains("inspector"), "{error}");
        assert!(error.contains("closed"), "{error}");
    }
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
