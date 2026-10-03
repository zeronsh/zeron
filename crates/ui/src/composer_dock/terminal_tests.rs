//! Exercise actual layout against an unmodified dock, frame by frame.
use super::*;
use crate::{terminal::dock, theme::Theme};
use gpui::{Context, Render, canvas, div, prelude::*};
use std::cell::Cell;

type Measurement = Rc<Cell<Option<Bounds<Pixels>>>>;

struct Fixture {
    states: [SharedDock; 2],
    now: Instant,
    docked: bool,
    reduced: bool,
    viewport: f32,
    composer_height: f32,
    terminal_height: f32,
    terminal_content_height: f32,
    right_width: f32,
    composers: [Measurement; 2],
    terminal: Measurement,
    underlay: Measurement,
    geometry: dock::SharedGeometry,
}

fn measured(height: f32, measurement: Measurement) -> gpui::Div {
    div().h(px(height)).w_full().relative().child(
        canvas(
            move |bounds, _, _| measurement.set(Some(bounds)),
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0(),
    )
}

impl Render for Fixture {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mut columns = Vec::new();
        for ix in 0..2 {
            self.states[ix].borrow_mut().observe_column(
                self.docked,
                (0.0, self.right_width),
                !self.reduced,
                self.now,
            );
            self.states[ix]
                .borrow_mut()
                .tick(self.docked, self.reduced, self.now);
            let available = (self.viewport
                - self.composer_height
                - Theme::TITLEBAR_HEIGHT
                - Theme::STATUS_STRIP_HEIGHT)
                .max(0.0);
            let geometry = Rc::new(Cell::new(dock::Geometry::new(
                self.terminal_height,
                self.terminal_content_height,
                (self.viewport * 0.55).min(available),
            )));
            let composer = docked_composer(
                measured(self.composer_height, self.composers[ix].clone())
                    .flex_none()
                    .w(px(350.0))
                    .mx_auto(),
                self.states[ix].clone(),
                self.viewport,
                self.reduced,
                self.now,
            );
            let composer = if ix == 0 {
                composer.reserve_terminal(geometry.clone())
            } else {
                composer
            };
            let terminal = self.terminal.clone();
            let column = div()
                .w(px(400.0))
                .h(px(self.viewport))
                .relative()
                .flex()
                .flex_col()
                .when(ix == 0, |el| {
                    el.child(div().absolute().inset_0().child(dock::above_terminal(
                        measured(0.0, self.underlay.clone()).h_full(),
                        geometry.clone(),
                    )))
                })
                .child(div().flex_1().min_h_0())
                .child(div().h(px(Theme::STATUS_STRIP_HEIGHT)).flex_none())
                .child(composer)
                .child(if ix == 0 {
                    dock::terminal(geometry, self.geometry.clone(), move |g| {
                        measured(g.height, terminal).into_any_element()
                    })
                    .into_any_element()
                } else {
                    div()
                        .h(px(geometry.get().reserved_height))
                        .flex_none()
                        .into_any_element()
                });
            columns.push(column);
        }
        div().size_full().flex().children(columns)
    }
}

#[gpui::test]
fn terminal_clearance_preserves_composer_motion_and_same_frame_layout(
    cx: &mut gpui::TestAppContext,
) {
    let composers: [Measurement; 2] = Default::default();
    let terminal: Measurement = Default::default();
    let underlay: Measurement = Default::default();
    let geometry: dock::SharedGeometry = Default::default();
    let handle = cx.open_window(size(px(800.0), px(900.0)), |_, _| Fixture {
        states: Default::default(),
        now: Instant::now(),
        docked: false,
        reduced: false,
        viewport: 900.0,
        composer_height: 180.0,
        terminal_height: 495.0,
        terminal_content_height: 495.0,
        right_width: 0.0,
        composers: composers.clone(),
        terminal: terminal.clone(),
        underlay: underlay.clone(),
        geometry: geometry.clone(),
    });
    let draw = |cx: &mut gpui::TestAppContext| {
        cx.update_window(handle.into(), |_, window, cx| {
            window.draw(cx).clear();
        })
        .unwrap();
        let [actual, baseline] = composers.each_ref().map(|m| m.get().unwrap());
        let terminal = terminal.get().unwrap();
        let underlay = underlay.get().unwrap();
        assert!(
            (f32::from(actual.top() - baseline.top())).abs() < 0.1,
            "terminal budgeting must not alter the composer's trajectory: {actual:?} vs {baseline:?}"
        );
        if geometry.get().height > 0.1 {
            assert!(
                actual.bottom() <= terminal.top() + px(0.1),
                "composer {actual:?} overlaps terminal {terminal:?}"
            );
            assert!(
                (f32::from(underlay.bottom() - terminal.top())).abs() < 0.1,
                "transcript and terminal must use the same height in this frame"
            );
        }
        assert!(
            (f32::from(terminal.size.height) - geometry.get().height).abs() < 0.1,
            "painted terminal {terminal:?}, geometry {:?}",
            geometry.get()
        );
    };
    draw(cx);
    assert!((geometry.get().height - 352.0).abs() < 0.1);
    assert_eq!(geometry.get().reserved_height, 495.0);

    // Start, reverse, resize, and grow attachments while the route is moving.
    // Every frame compares with the original unconstrained composer path.
    for step in 0..180 {
        handle
            .update(cx, |fixture, _, cx| {
                fixture.now += std::time::Duration::from_millis(16);
                match step {
                    0 => fixture.docked = true,
                    8 => fixture.docked = false,
                    12 => {
                        fixture.docked = true;
                        fixture.composer_height = 260.0;
                    }
                    18 => {
                        fixture.viewport = 620.0;
                        fixture.composer_height = 320.0;
                    }
                    24 => {
                        fixture.viewport = 900.0;
                        fixture.composer_height = 100.0;
                    }
                    80 => fixture.docked = false,
                    88 => fixture.composer_height = 380.0,
                    96 => {
                        fixture.docked = true;
                        fixture.composer_height = 100.0;
                        fixture.right_width = 400.0;
                    }
                    120 => {
                        fixture.docked = false;
                        fixture.right_width = 0.0;
                    }
                    140 => {
                        fixture.docked = true;
                        fixture.right_width = 400.0;
                    }
                    _ => {}
                }
                cx.notify();
            })
            .unwrap();
        draw(cx);
    }
    // Reaching the full requested height also catches a constraint/spring
    // feedback loop that would otherwise leave the composer stuck mid-route.
    assert!((geometry.get().height - 495.0).abs() < 0.1);

    // Idle terminal toggles retain a fixed inner grid while only its viewport
    // opens/closes; the resize limit must not trap a docked terminal at its
    // current height. That would prevent the next upward drag.
    handle
        .update(cx, |fixture, _, cx| {
            fixture.right_width = 0.0;
            fixture.reduced = true;
            cx.notify();
        })
        .unwrap();
    for height in [0.0, 20.0, 100.0, 280.0, 495.0, 280.0, 20.0, 0.0] {
        handle
            .update(cx, |fixture, _, cx| {
                fixture.terminal_height = height;
                cx.notify();
            })
            .unwrap();
        draw(cx);
        assert!((geometry.get().height - height).abs() < 0.1);
        assert!((geometry.get().limit - 495.0).abs() < 0.1);
        assert!((geometry.get().content_height - 495.0).abs() < 0.1);
    }

    // Saved height larger than the resized viewport, reduced motion, and a
    // terminal below its ordinary 160px minimum must all remain bounded.
    for (viewport, height, requested, docked) in [
        (420.0, 260.0, 1200.0, false),
        (420.0, 260.0, 1200.0, true),
        (200.0, 380.0, 1200.0, false),
        (900.0, 180.0, 0.0, false),
        (900.0, 180.0, 280.0, false),
    ] {
        handle
            .update(cx, |fixture, _, cx| {
                fixture.viewport = viewport;
                fixture.composer_height = height;
                fixture.terminal_height = requested;
                fixture.terminal_content_height = requested.max(280.0);
                fixture.docked = docked;
                fixture.reduced = true;
                cx.notify();
            })
            .unwrap();
        draw(cx);
        assert!(geometry.get().height <= viewport * 0.55);
    }
}
