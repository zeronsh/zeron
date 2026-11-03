//! A stretch left behind: drop into crates/zgx/tests/ and `cargo test
//! --test stranded_stretch`. Fails on zgx aaa304eb2: the vertical view stays
//! translated by its stretch for good.
use std::time::{Duration, Instant};
use zgx::{
    compose::prelude::*,
    input::{InputEvent, ScrollGesture},
    widgets::Ui,
};

fn scroll_at(ui: &mut Ui, x: f32, y: f32, delta_x: f32, delta_y: f32) {
    ui.dispatch(InputEvent::Scroll { x, y, delta_x, delta_y });
    ui.prepare_frame();
}

fn phase(ui: &mut Ui, phase: ScrollGesture) {
    ui.dispatch(InputEvent::ScrollGesture(phase));
    ui.prepare_frame();
}

#[test]
fn a_stretch_springs_back_when_another_view_stretches_meanwhile() {
    let mut ui = Ui::new(400., 200.);
    ui.set_elastic_scrolling(true);
    // A list (100 tall over 400) beside a strip that scrolls across.
    let (list, strip) = (ui.signal(0.), ui.signal(0.));
    let view = ui.mount(
        row()
            .child(scroll(list.clone()).id("list").w(100.).h(100.).child(div().h(400.)))
            .child(scroll_x(strip).w(200.).h(100.).child(div().w(800.).h(100.))),
    );
    ui.prepare_frame();
    let content = {
        let scene = ui.scene.borrow();
        let root = view.find("list").unwrap();
        scene.children(scene.children(root)[0])[0]
    };
    let shift = |ui: &Ui| ui.scene.borrow().transform(content).y + list.get();

    // Pull the list past its end and let go: it springs back...
    list.set(300.);
    ui.prepare_frame();
    phase(&mut ui, ScrollGesture::Began);
    scroll_at(&mut ui, 50., 50., 0., 200.);
    phase(&mut ui, ScrollGesture::Ended);
    assert!(shift(&ui) < -10., "stretched");
    // ...but before it is back, a gesture pulls the strip past its start.
    phase(&mut ui, ScrollGesture::Began);
    scroll_at(&mut ui, 200., 50., -40., 0.);
    phase(&mut ui, ScrollGesture::Ended);
    let mut now = Instant::now();
    for _ in 0..200 {
        now += Duration::from_millis(16);
        ui.advance_interactions(now).unwrap();
        ui.prepare_frame();
    }
    // The strip springs back; the list stays stretched for good. Once it
    // fits (a taller window), nothing ever picks it again.
    assert_eq!(shift(&ui), 0., "the list is back (stranded at {})", shift(&ui));
}
