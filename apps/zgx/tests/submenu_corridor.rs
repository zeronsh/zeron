//! Hover-intent awareness cones and spacing for nested dropdown submenus
//! (`ds/menu` `Menu.corridor` + `ds/composer` `Work in` submenus).
//!
//! Moving diagonally from an open submenu's trigger across sibling triggers
//! (`Project`, `Checkout`) and parent rows (`Search branches…`) toward the
//! open submenu panel keeps the submenu open; moving straight down, pausing
//! past the grace window, or reversing away hands over to the hovered sibling.

use std::path::PathBuf;
use std::time::Duration;

use zgx::input::InputEvent;
use zgx::scene::Rect;
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

fn open_work_in() -> TestApp {
    let mut app = TestApp::new(move |runtime| {
        ScriptHost::new(runtime, ui_dir().join("composer_place_fixture.luau"))
    });
    app.ui.scene.borrow_mut().resize(1000., 600.);
    app.frames(10);
    app.host
        .set_state(
            "test_place_devices",
            Value::from_json(
                r#"[
                {"value":"work-laptop","label":"work-laptop","you":true},
                {"value":"home-laptop","label":"home-laptop"},
                {"value":"work","label":"work"},
                {"value":"dev","label":"dev"},
                {"value":"m1-laptop","label":"m1-laptop"},
                {"value":"cloud","label":"Cloud","cloud":true}
            ]"#,
            )
            .expect("valid devices json"),
        )
        .expect("set test_place_devices");
    app.host
        .set_state(
            "test_place_device",
            Value::from_json(r#""work""#).expect("valid device json"),
        )
        .expect("set test_place_device");
    app.frames(20);
    app.click("role:button \"Branch\"");
    app.frames(20);
    assert_eq!(app.error(), None);
    app
}

fn bounds_of(app: &mut TestApp, selector: &str) -> Rect {
    let id = app
        .find(selector)
        .unwrap_or_else(|| panic!("missing `{selector}`:\n{}", app.tree()));
    app.ui.scene.borrow().bounds(id)
}

fn showing(app: &mut TestApp, selector: &str) -> bool {
    let Some(id) = app.find(selector) else {
        return false;
    };
    let scene = app.ui.scene.borrow();
    if !scene.contains(id) {
        return false;
    }
    let b = scene.bounds(id);
    b.width > 0. && b.height > 0.
}

fn move_to(app: &mut TestApp, x: f32, y: f32) {
    app.ui.dispatch(InputEvent::PointerMove { x, y });
    app.frames(3);
}

fn sleep_frames(app: &mut TestApp, total_ms: u64) {
    let step = 20u64;
    let mut elapsed = 0u64;
    while elapsed < total_ms {
        std::thread::sleep(Duration::from_millis(step));
        app.frames(2);
        elapsed += step;
    }
}

#[test]
fn diagonal_pointer_path_keeps_open_submenu_alive_and_keeps_gap() {
    let mut app = open_work_in();
    let device = bounds_of(&mut app, "role:menuitem \"Device\"");
    let project = bounds_of(&mut app, "role:menuitem \"Project\"");
    let checkout = bounds_of(&mut app, "role:menuitem \"Checkout\"");
    let search_branches = bounds_of(&mut app, "\"Search branches…\"");
    let parent_panel = bounds_of(&mut app, ".dropdown-panel[1]");

    // Enter Device trigger near its middle-left; Device submenu opens.
    let ox = device.x + device.width * 0.4;
    let oy = device.y + device.height * 0.5;
    move_to(&mut app, ox, oy);
    assert!(showing(&mut app, "\"Search devices…\""), "Device submenu should open on hover");

    // Verify slight gap (~2px) between parent dropdown and submenu panel.
    let sub_panel = bounds_of(&mut app, ".dropdown-panel[2]");
    let gap = sub_panel.x - (parent_panel.x + parent_panel.width);
    assert!(
        (1.0..=3.5).contains(&gap),
        "expected ~2px gap between parent ({parent_panel:?}) and submenu ({sub_panel:?}), got {gap}"
    );

    // Move diagonally down-right toward the bottom of the Device submenu,
    // crossing Project, Checkout, Search branches…, and the 2px gap over >240ms.
    let target_x = sub_panel.x + 24.;
    let target_y = sub_panel.y + sub_panel.height - 24.;
    let steps = [
        // Over Project row, moving diagonally toward the submenu.
        (
            ox + (target_x - ox) * 0.18,
            project.y + project.height * 0.5,
        ),
        // Over Checkout row, continuing diagonally.
        (
            ox + (target_x - ox) * 0.36,
            checkout.y + checkout.height * 0.5,
        ),
        // Over Search branches… row (non-submenu parent content).
        (
            ox + (target_x - ox) * 0.58,
            search_branches.y + search_branches.height * 0.5,
        ),
        // Near parent panel's right edge.
        (parent_panel.x + parent_panel.width - 4., target_y - 12.),
        // Inside the gap between panels.
        (parent_panel.x + parent_panel.width + 1., target_y - 6.),
    ];
    for (x, y) in steps {
        move_to(&mut app, x, y);
        sleep_frames(&mut app, 60);
        assert!(
            showing(&mut app, "\"Search devices…\""),
            "Device submenu closed prematurely at ({x:.1}, {y:.1})"
        );
        assert!(
            !showing(&mut app, "\"Search projects…\""),
            "Project submenu opened prematurely at ({x:.1}, {y:.1})"
        );
    }

    // Enter the Device submenu panel; it stays open indefinitely.
    move_to(&mut app, target_x, target_y);
    sleep_frames(&mut app, 400);
    assert!(showing(&mut app, "\"Search devices…\""));
    assert_eq!(app.error(), None);
}

#[test]
fn pausing_or_reversing_in_corridor_hands_over_to_sibling_submenu() {
    let mut app = open_work_in();
    let device = bounds_of(&mut app, "role:menuitem \"Device\"");
    let project = bounds_of(&mut app, "role:menuitem \"Project\"");

    // Open Device at (ox, oy).
    let ox = device.x + device.width * 0.35;
    let oy = device.y + device.height * 0.5;
    move_to(&mut app, ox, oy);
    assert!(showing(&mut app, "\"Search devices…\""));

    // Move diagonally onto Project: Device stays open at first.
    let px = ox + 36.;
    let py = project.y + project.height * 0.5;
    move_to(&mut app, px, py);
    assert!(showing(&mut app, "\"Search devices…\""));
    assert!(!showing(&mut app, "\"Search projects…\""));

    // Pause longer than GRACE (300ms): Project takes over.
    sleep_frames(&mut app, 380);
    assert!(!showing(&mut app, "\"Search devices…\""));
    assert!(showing(&mut app, "\"Search projects…\""));

    // Now test reversing away from the open submenu:
    // Go back to Device, move diagonally onto Project, then reverse left.
    move_to(&mut app, ox, oy);
    assert!(showing(&mut app, "\"Search devices…\""));
    move_to(&mut app, px, py);
    assert!(showing(&mut app, "\"Search devices…\""));
    assert!(!showing(&mut app, "\"Search projects…\""));

    move_to(&mut app, px - 10., py);
    assert!(
        !showing(&mut app, "\"Search devices…\""),
        "reversing away should hand over immediately"
    );
    assert!(showing(&mut app, "\"Search projects…\""));
    assert_eq!(app.error(), None);
}

#[test]
fn straight_down_move_switches_sibling_submenus_immediately() {
    let mut app = open_work_in();
    let device = bounds_of(&mut app, "role:menuitem \"Device\"");
    let project = bounds_of(&mut app, "role:menuitem \"Project\"");

    let x = device.x + device.width * 0.5;
    move_to(&mut app, x, device.y + device.height * 0.5);
    assert!(showing(&mut app, "\"Search devices…\""));

    // Straight vertical move (same x): switches immediately with no grace wait.
    move_to(&mut app, x, project.y + project.height * 0.5);
    assert!(!showing(&mut app, "\"Search devices…\""));
    assert!(showing(&mut app, "\"Search projects…\""));
    assert_eq!(app.error(), None);
}
