//! The frame pump, headless: one script-driven frame per shown window, the
//! widgets inside it, the per-callback budget, and the errors that must not
//! take the plugin down.
//!
//! A real Dear ImGui context and a real wgpu device are used, but no native
//! window: [`Probe`] is a [`FrameWindow`] with a context, a size and a place
//! to keep the frames it is handed, so the whole protocol runs here and its
//! pixels can be read back. The protocol is the one the SDK and the host use:
//! `frame::run` pushes a `imgui.frame` call, the push closure plays the
//! script (calling the widget ops and `frame_end`), and the frame is painted.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use dear_imgui_rs::{Io, Key, MouseButton};
use zgx::input::{InputEvent, PointerButton};
use zgx_gpu::wgpu;
use zgx_script::Value;

use super::support::{TestGpu, headless_gpu, imgui_guard};
use crate::plugins::imgui::frame::{self, FrameCalls, FrameEnd, FrameState, FrameWindow};
use crate::plugins::imgui::input;
use crate::plugins::imgui::render::{PreparedFrame, WgpuRenderer};
use crate::plugins::imgui::runtime::{FrameSpec, WindowContext};
use crate::plugins::imgui::snapshot::{self, OffscreenTarget, Snapshot};

/// A window with no desktop: a context, its size, and what it was handed.
struct Probe {
    id: String,
    open: Cell<bool>,
    shown: Cell<bool>,
    renderable: Cell<bool>,
    size: (f64, f64),
    context: RefCell<Option<WindowContext>>,
    frames: FrameState,
    delivered: RefCell<Vec<PreparedFrame>>,
    closed: Cell<bool>,
    target: Cell<(u32, u32)>,
    /// Mirrors the live renderer: the next frame must carry the atlas until
    /// one that does has been delivered, and again after a device change.
    needs_atlas: Cell<bool>,
}

impl Probe {
    fn new(id: &str) -> Rc<Self> {
        Rc::new(Self {
            id: id.to_string(),
            open: Cell::new(true),
            shown: Cell::new(true),
            renderable: Cell::new(true),
            size: (720.0, 480.0),
            context: RefCell::new(Some(
                WindowContext::try_create(format!("probe {id}")).expect("a context"),
            )),
            frames: FrameState::new(),
            delivered: RefCell::default(),
            closed: Cell::new(false),
            target: Cell::new((720, 480)),
            needs_atlas: Cell::new(true),
        })
    }

    /// The GPU view dropped every object, as `Viewport::device_changed` does:
    /// the next frame must upload the atlas again.
    fn device_changed(&self) {
        self.needs_atlas.set(true);
    }

    /// One ImGui input event, before the next frame starts (as the host's
    /// input layer does).
    fn edit_io(&self, edit: impl FnOnce(&mut Io)) {
        let mut context = self.context.borrow_mut();
        context
            .as_mut()
            .expect("the probe's context")
            .with_active(|context| edit(context.io_mut()))
            .expect("an active context");
    }

    fn text(&self, seq: u64, value: &str) -> Value {
        let arg = Value::record([("text", Value::from(value))]);
        self.frame_widget("text", seq, &arg).expect("a text call")
    }

    fn button(&self, seq: u64, label: &str) -> bool {
        let arg = Value::record([("label", Value::from(label))]);
        let answer = self
            .frame_widget("button", seq, &arg)
            .expect("a button call");
        matches!(answer.field("value"), Some(Value::Bool(true)))
    }

    fn invisible_button(&self, seq: u64, id: &str, width: f64, height: f64) -> Value {
        let arg = Value::record([
            ("id", Value::from(id)),
            (
                "size",
                Value::record([
                    ("width", Value::from(width)),
                    ("height", Value::from(height)),
                ]),
            ),
        ]);
        self.frame_widget("invisible_button", seq, &arg)
            .expect("an invisible_button call")
    }

    fn is_item_hovered(&self, seq: u64) -> bool {
        let answer = self
            .frame_widget(
                "is_item_hovered",
                seq,
                &Value::record(std::iter::empty::<(&str, Value)>()),
            )
            .expect("an is_item_hovered call");
        matches!(answer.field("value"), Some(Value::Bool(true)))
    }

    fn is_item_active(&self, seq: u64) -> bool {
        let answer = self
            .frame_widget(
                "is_item_active",
                seq,
                &Value::record(std::iter::empty::<(&str, Value)>()),
            )
            .expect("an is_item_active call");
        matches!(answer.field("value"), Some(Value::Bool(true)))
    }

    fn mouse_delta(&self, seq: u64) -> (f32, f32) {
        let answer = self
            .frame_widget(
                "mouse_delta",
                seq,
                &Value::record(std::iter::empty::<(&str, Value)>()),
            )
            .expect("a mouse_delta call");
        let axis = |name: &str| match answer.field(name) {
            Some(Value::Num(number)) => *number as f32,
            _ => panic!("mouse_delta.{name} is not a number"),
        };
        (axis("x"), axis("y"))
    }

    fn is_mouse_dragging(&self, seq: u64, button: Option<&str>) -> bool {
        let arg = match button {
            Some(button) => Value::record([("button", Value::from(button))]),
            None => Value::record(std::iter::empty::<(&str, Value)>()),
        };
        let answer = self
            .frame_widget("is_mouse_dragging", seq, &arg)
            .expect("an is_mouse_dragging call");
        matches!(answer.field("value"), Some(Value::Bool(true)))
    }

    fn style(&self) -> dear_imgui_rs::Style {
        let mut context = self.context.borrow_mut();
        context
            .as_mut()
            .expect("the probe's context")
            .with_active(|context| context.style().clone())
            .expect("an active context")
    }
}

impl FrameWindow for Probe {
    fn frame_plugin(&self) -> &str {
        "probe"
    }

    fn frame_id(&self) -> &str {
        &self.id
    }

    fn frame_open(&self) -> bool {
        self.open.get()
    }

    fn frame_shown(&self) -> bool {
        self.shown.get()
    }

    fn frame_renderable(&self) -> bool {
        self.renderable.get()
    }

    fn frame_size(&self) -> (f64, f64) {
        self.size
    }

    fn frame_scale(&self) -> f64 {
        1.0
    }

    fn frame_target(&self) -> Option<(u32, u32)> {
        Some(self.target.get())
    }

    fn frame_needs_atlas(&self) -> bool {
        self.needs_atlas.get()
    }

    fn frame_build(
        &self,
        spec: FrameSpec,
        draw: impl FnOnce(*const dear_imgui_rs::Ui),
    ) -> Result<PreparedFrame, String> {
        self.target.set(spec.target_size);
        let want_atlas = self.frame_needs_atlas();
        let mut guard = self.context.borrow_mut();
        let context = guard.as_mut().expect("the probe's context");
        context
            .build_frame(spec, want_atlas, |ui| {
                // SAFETY: `build_frame` keeps the frame open for this closure.
                let root = unsafe { &*ui };
                frame::root_window(root, &spec, || draw(ui));
            })
            .map_err(|error| error.to_string())
    }

    fn frame_deliver(&self, frame: PreparedFrame) {
        if frame.atlas().is_some() {
            self.needs_atlas.set(false);
        }
        self.delivered.borrow_mut().push(frame);
    }

    fn frame_close(&self) {
        self.closed.set(true);
        self.open.set(false);
        self.shown.set(false);
    }

    fn frame_state(&self) -> &FrameState {
        &self.frames
    }
}

/// Runs one frame through the real pump with `draw` playing the script: it
/// receives the frame's sequence number and calls the widget ops.
fn run_frame(probe: &Rc<Probe>, draw: impl Fn(&Probe, u64) + 'static) -> Option<FrameEnd> {
    let window = probe.clone();
    let push = move |_call: String, _kind: &'static str, fields: Vec<(&'static str, Value)>| {
        let seq = fields
            .iter()
            .find(|(key, _)| *key == "seq")
            .and_then(|(_, value)| match value {
                Value::Num(seq) => Some(*seq as u64),
                _ => None,
            })
            .expect("the frame request carries its seq");
        draw(&window, seq);
    };
    let calls = FrameCalls::new(push, |_| {}, |_| {});
    frame::run(probe.as_ref(), &calls)
}

/// A pump run whose failure reporter is observed.
fn run_frame_reporting(
    probe: &Rc<Probe>,
    failed: Rc<RefCell<Option<String>>>,
    draw: impl Fn(&Probe, u64) + 'static,
) -> Option<FrameEnd> {
    let window = probe.clone();
    let push = move |_call: String, _kind: &'static str, fields: Vec<(&'static str, Value)>| {
        let seq = fields
            .iter()
            .find(|(key, _)| *key == "seq")
            .and_then(|(_, value)| match value {
                Value::Num(seq) => Some(*seq as u64),
                _ => None,
            })
            .expect("the frame request carries its seq");
        draw(&window, seq);
    };
    let failed = failed.clone();
    let calls = FrameCalls::new(
        push,
        |_| {},
        move |error| {
            *failed.borrow_mut() = Some(error.to_string());
        },
    );
    frame::run(probe.as_ref(), &calls)
}

/// Spends real time inside a frame, standing in for the host's font-atlas
/// build and upload, which happen on the UI thread and are not the plugin's
/// callback work.
fn spend(ms: u64) {
    let until = Instant::now() + Duration::from_millis(ms);
    let mut spin = 0_u64;
    while Instant::now() < until {
        spin = spin.wrapping_add(1);
        std::hint::black_box(spin);
    }
}

/// The pixels of the frames `probe` delivered.
///
/// One renderer replays them in order, as the live view does: the atlas
/// arrives with the frame that carried it, and later frames reuse what the
/// renderer already holds.
fn raster(gpu: &TestGpu, probe: &Probe) -> Snapshot {
    raster_upto(gpu, probe, usize::MAX)
}

fn raster_upto(gpu: &TestGpu, probe: &Probe, count: usize) -> Snapshot {
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let target = OffscreenTarget::new(&gpu.device, (720, 480), format);
    let delivered = probe.delivered.borrow();
    assert!(!delivered.is_empty(), "a delivered frame");
    let mut painted = None;
    for prepared in delivered.iter().take(count) {
        let (_, snapshot) =
            snapshot::render(&mut renderer, &gpu.device, &gpu.queue, &target, prepared)
                .expect("an offscreen frame");
        painted = Some(snapshot);
    }
    painted.expect("a snapshot")
}

/// Pixels in `area` (left, top, right, bottom) that differ from `background`
/// by 16 or more in any channel.
fn differing(image: &Snapshot, background: &Snapshot, area: (u32, u32, u32, u32)) -> usize {
    differing_above(image, background, area, 16)
}

fn differing_above(
    image: &Snapshot,
    background: &Snapshot,
    area: (u32, u32, u32, u32),
    threshold: u8,
) -> usize {
    let (left, top, right, bottom) = area;
    let mut count = 0;
    for y in top..bottom.min(image.height()) {
        for x in left..right.min(image.width()) {
            let (Some(here), Some(was)) = (image.pixel(x, y), background.pixel(x, y)) else {
                continue;
            };
            if here
                .iter()
                .zip(was.iter())
                .take(3)
                .any(|(a, b)| a.abs_diff(*b) >= threshold)
            {
                count += 1;
            }
        }
    }
    count
}

fn rect_of(value: &Value) -> (f32, f32, f32, f32) {
    let number =
        |point: &str, axis: &str| match value.field(point).and_then(|point| point.field(axis)) {
            Some(Value::Num(number)) => *number as f32,
            _ => panic!("{point}.{axis} is not a number"),
        };
    (
        number("min", "x"),
        number("min", "y"),
        number("max", "x"),
        number("max", "y"),
    )
}

fn window_rect(rect: (f32, f32, f32, f32)) -> (u32, u32, u32, u32) {
    (
        rect.0.floor().max(0.0) as u32,
        rect.1.floor().max(0.0) as u32,
        rect.2.ceil().max(0.0) as u32,
        rect.3.ceil().max(0.0) as u32,
    )
}

fn width_of(rect: (f32, f32, f32, f32)) -> f32 {
    rect.2 - rect.0
}

fn height_of(rect: (f32, f32, f32, f32)) -> f32 {
    rect.3 - rect.1
}

#[test]
fn only_a_shown_window_asks_for_a_frame() {
    let _guard = imgui_guard();
    let probe = Probe::new("wants");
    assert!(probe.frame_wants(), "a shown window wants a frame");

    probe.shown.set(false);
    assert!(
        !probe.frame_wants(),
        "a hidden window must ask for no frame (and run no callback)"
    );
    probe.shown.set(true);

    probe.renderable.set(false);
    assert!(
        !probe.frame_wants(),
        "a window with no on_render has nothing to draw"
    );
    probe.renderable.set(true);

    let ran = run_frame(&probe, |probe, seq| {
        probe.text(seq, "once");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("a frame ran");
    assert!(ran.painted, "{ran:?}");
    let _ = ran;

    // (After a completed frame the next one is welcome again.)
    assert!(probe.frame_wants());
}

#[test]
fn a_hidden_window_is_never_run() {
    let _guard = imgui_guard();
    let probe = Probe::new("hidden");
    probe.shown.set(false);
    let drew = Rc::new(Cell::new(false));
    let flag = drew.clone();
    let window = probe.clone();
    let push = move |_call: String, _kind: &'static str, _fields: Vec<(&'static str, Value)>| {
        flag.set(true);
    };
    let end = frame::run(probe.as_ref(), &FrameCalls::new(push, |_| {}, |_| {}));
    assert!(end.is_none(), "a hidden window's frame must not run");
    assert!(!drew.get(), "no call may reach a hidden window");
    let _ = window;
}

// VAL-WIDGET-001: `text` draws the string inside its own item rect.
#[test]
fn text_draws_inside_its_own_item_rect() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();

    // The control run: the same window, nothing drawn.
    let control = Probe::new("text-control");
    run_frame(&control, |probe, seq| {
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the control frame");
    let background = raster(&gpu, &control);

    // The run under test: one text call, and its item rect from the binding.
    let probe = Probe::new("text");
    let rect = Rc::new(RefCell::new((0.0, 0.0, 0.0, 0.0)));
    let captured = rect.clone();
    run_frame(&probe, move |probe, seq| {
        let answer = probe.text(seq, "Hello, ImGui");
        *captured.borrow_mut() = rect_of(&answer);
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the text frame");
    let painted = raster(&gpu, &probe);

    let rect = *rect.borrow();
    let area = window_rect(rect);
    assert!(
        area.2 > area.0 && area.3 > area.1,
        "the item rect is {rect:?}"
    );
    let inside = differing(&painted, &background, area);
    assert!(
        inside >= 100,
        "expected the string's glyphs inside {rect:?}, got {inside} differing pixels"
    );
    // Nothing outside the item's own rect, horizontally.
    let outside = differing(
        &painted,
        &background,
        (0, area.1, area.0.saturating_sub(1), area.3),
    ) + differing(
        &painted,
        &background,
        (area.2 + 1, area.1, painted.width(), area.3),
    );
    assert_eq!(outside, 0, "text drew outside its item rect: {rect:?}");

    // A different final glyph differs inside that glyph's own column: the
    // glyphs are the string's, not a placeholder.
    let other = Probe::new("text-other");
    run_frame(&other, |probe, seq| {
        probe.text(seq, "Hello, ImGuX");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the second text frame");
    let different = raster(&gpu, &other);
    let column = (area.2.saturating_sub(12), area.1, area.2, area.3);
    let changed = differing(&different, &painted, column);
    assert!(
        changed >= 8,
        "the final glyph's column should differ, got {changed} pixels"
    );

    let path = std::env::temp_dir().join("zeron-zgx-imgui-tests/widget-text.png");
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    painted.save_png(&path).expect("the evidence PNG");
    println!(
        "imgui widget text: {} pixels inside {area:?}, {changed} changed in the last glyph -> {}",
        inside,
        path.display()
    );
}

// VAL-WIDGET-004: `button` fires exactly on release, never on press.
#[test]
fn button_fires_on_release_and_not_on_press() {
    let _guard = imgui_guard();
    let probe = Probe::new("button");

    // Where the button lands, and whether it was clicked, frame by frame.
    let rect = Rc::new(RefCell::new((0.0, 0.0, 0.0, 0.0)));
    let clicks = Rc::new(Cell::new(0));
    let seen = Rc::new(RefCell::new(Vec::<bool>::new()));

    let draw = {
        let (rect, clicks, seen) = (rect.clone(), clicks.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frames
                .widget(
                    probe,
                    "button",
                    seq,
                    &Value::record([("label", Value::from("Refresh"))]),
                )
                .expect("a button call");
            *rect.borrow_mut() = rect_of(&answer);
            let clicked = matches!(answer.field("value"), Some(Value::Bool(true)));
            if clicked {
                clicks.set(clicks.get() + 1);
            }
            seen.borrow_mut().push(clicked);
            let _ = probe.frame_script_end(seq, None, false);
        }
    };

    // Frame 0: where the button lands (no input yet).
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let centre = [(left + right) / 2.0, (top + bottom) / 2.0];
    seen.borrow_mut().clear();

    // Frame 1: the pointer moves to the button's centre — hover, not a click.
    probe.edit_io(|io| io.add_mouse_pos_event(centre));
    run_frame(&probe, draw.clone()).expect("frame 1");
    assert_eq!(clicks.get(), 0, "moving onto a button is not a click");
    assert!(!seen.borrow()[0]);

    // Frame 2: the left button goes down there — not a click.
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("frame 2");
    assert_eq!(clicks.get(), 0, "a press alone is not a click");
    assert!(!seen.borrow()[1]);

    // Frame 3: it comes up there — the click.
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("frame 3");
    assert_eq!(clicks.get(), 1, "the release is the click");
    assert!(seen.borrow()[2]);

    // Five frames with no input: still exactly one.
    for frame in 4..=8 {
        run_frame(&probe, draw.clone()).expect("a quiet frame");
        assert!(!seen.borrow()[frame - 1], "frame {frame} is not a click");
    }
    assert_eq!(clicks.get(), 1, "one press and release is one click");
}

// The same click, driven by the window's own document events through
// `input::apply`: the chain a real pointer takes (move, down, up) — no
// desktop needed.
#[test]
fn the_window_s_pointer_events_click_a_button() {
    let _guard = imgui_guard();
    let probe = Probe::new("mapped");
    let rect = Rc::new(RefCell::new((0.0, 0.0, 0.0, 0.0)));
    let clicks = Rc::new(Cell::new(0));

    let draw = {
        let (rect, clicks) = (rect.clone(), clicks.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frames
                .widget(
                    probe,
                    "button",
                    seq,
                    &Value::record([("label", Value::from("Click me"))]),
                )
                .expect("a button call");
            *rect.borrow_mut() = rect_of(&answer);
            if matches!(answer.field("value"), Some(Value::Bool(true))) {
                clicks.set(clicks.get() + 1);
            }
            let _ = probe.frame_script_end(seq, None, false);
        }
    };

    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);

    // The pointer moves over the button: hover, not a click.
    probe.edit_io(|io| {
        input::apply(io, &InputEvent::PointerMove { x, y });
    });
    run_frame(&probe, draw.clone()).expect("frame 1");
    assert_eq!(clicks.get(), 0, "moving onto a button is not a click");

    // The press alone: still not a click.
    probe.edit_io(|io| {
        input::apply(
            io,
            &InputEvent::PointerDown {
                x,
                y,
                button: PointerButton::Primary,
            },
        );
    });
    run_frame(&probe, draw.clone()).expect("frame 2");
    assert_eq!(clicks.get(), 0, "a press alone is not a click");

    // The release: the click.
    probe.edit_io(|io| {
        input::apply(
            io,
            &InputEvent::PointerUp {
                x,
                y,
                button: PointerButton::Primary,
            },
        );
    });
    run_frame(&probe, draw.clone()).expect("frame 3");
    assert_eq!(clicks.get(), 1, "the release is the click");

    // A pointer taken out of the window mid-press does not click either.
    probe.edit_io(|io| {
        input::apply(
            io,
            &InputEvent::PointerDown {
                x,
                y,
                button: PointerButton::Primary,
            },
        );
    });
    run_frame(&probe, draw.clone()).expect("frame 4");
    probe.edit_io(|io| {
        input::apply(io, &InputEvent::PointerCancel);
    });
    run_frame(&probe, draw.clone()).expect("frame 5");
    assert_eq!(clicks.get(), 1, "a cancelled press is not a click");
}

fn truth_of(value: &Value, field: &str) -> bool {
    matches!(value.field(field), Some(Value::Bool(true)))
}

#[test]
fn invisible_buttons_answer_hover_active_and_clicked() {
    let _guard = imgui_guard();
    let probe = Probe::new("chrome");

    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let seen = Rc::new(RefCell::new(Vec::<(bool, bool, bool, bool, bool)>::new()));
    let draw = {
        let (rect, seen) = (rect.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe.invisible_button(seq, "drag", 300.0, 32.0);
            *rect.borrow_mut() = rect_of(&answer);
            let hovered = truth_of(&answer, "hovered");
            let active = truth_of(&answer, "active");
            let clicked = truth_of(&answer, "clicked");
            let hovered_again = probe.is_item_hovered(seq);
            let active_again = probe.is_item_active(seq);
            seen.borrow_mut()
                .push((hovered, active, clicked, hovered_again, active_again));
            let _ = probe.frame_script_end(seq, None, false);
        }
    };

    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    assert!(right > left && bottom > top, "the rect exists: {rect:?}");
    assert_eq!(
        seen.borrow()[0],
        (false, false, false, false, false),
        "no input"
    );
    let centre = [(left + right) / 2.0, (top + bottom) / 2.0];

    probe.edit_io(|io| io.add_mouse_pos_event(centre));
    run_frame(&probe, draw.clone()).expect("hover");
    assert_eq!(
        seen.borrow()[1],
        (true, false, false, true, false),
        "hovering reports hovered only"
    );

    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("press");
    assert_eq!(
        seen.borrow()[2],
        (true, true, false, true, true),
        "pressing inside reports active while held"
    );

    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("release inside");
    assert_eq!(
        seen.borrow()[3],
        (true, false, true, true, false),
        "releasing inside reports the click once"
    );

    run_frame(&probe, draw.clone()).expect("idle after click");
    assert_eq!(
        seen.borrow()[4],
        (true, false, false, true, false),
        "the click is the release frame alone"
    );

    probe.edit_io(|io| io.add_mouse_pos_event(centre));
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("press again");
    probe.edit_io(|io| io.add_mouse_pos_event([right + 40.0, bottom + 40.0]));
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("release outside");
    let last = *seen.borrow().last().expect("a release outside frame");
    assert!(
        !last.2,
        "release outside the rect reports no click: {last:?}"
    );
}

#[test]
fn invisible_buttons_draw_nothing_and_reserve_their_rect() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();

    let empty = Probe::new("empty");
    run_frame(&empty, |probe, seq| {
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the empty frame");
    let background = raster(&gpu, &empty);

    let control = Probe::new("control");
    let control_rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let capture = control_rect.clone();
    run_frame(&control, move |probe, seq| {
        let answer = probe.text(seq, "after");
        *capture.borrow_mut() = rect_of(&answer);
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the control frame");
    let control_rect = *control_rect.borrow();

    let probe = Probe::new("with");
    let button_rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let after_rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let (button, after) = (button_rect.clone(), after_rect.clone());
    run_frame(&probe, move |probe, seq| {
        let answer = probe.invisible_button(seq, "drag", 300.0, 32.0);
        *button.borrow_mut() = rect_of(&answer);
        let answer = probe.text(seq, "after");
        *after.borrow_mut() = rect_of(&answer);
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the frame with the button");
    let painted = raster(&gpu, &probe);

    let button_rect = *button_rect.borrow();
    let after_rect = *after_rect.borrow();
    assert!(
        (button_rect.1 - control_rect.1).abs() <= 0.5,
        "the button starts where the control text would: {button_rect:?} vs {control_rect:?}"
    );
    assert!(
        (button_rect.3 - button_rect.1 - 32.0).abs() <= 0.5,
        "the button is 32 points tall: {button_rect:?}"
    );
    assert!(
        (button_rect.2 - button_rect.0 - 300.0).abs() <= 0.5,
        "the button is 300 points wide: {button_rect:?}"
    );
    assert!(
        (after_rect.1 - control_rect.1 - 32.0).abs() <= 1.0,
        "the following text moves down 32 points: after={after_rect:?} control={control_rect:?}"
    );
    assert!(
        (after_rect.0 - control_rect.0).abs() <= 0.5,
        "the following text keeps its column: after={after_rect:?} control={control_rect:?}"
    );

    let band = window_rect(button_rect);
    assert!(
        identical_pixels(&painted, &background, band),
        "the invisible button drew a pixel in {band:?}"
    );
}

#[test]
fn invisible_buttons_refuse_unknown_or_wrong_options() {
    let _guard = imgui_guard();
    let probe = Probe::new("refused");
    run_frame(&probe, |probe, seq| {
        let renamed = Value::record([
            ("id", Value::from("drag")),
            ("maxx", Value::from(100.0)),
            (
                "size",
                Value::record([("width", Value::from(300.0)), ("height", Value::from(32.0))]),
            ),
        ]);
        let error = probe
            .frame_widget("invisible_button", seq, &renamed)
            .expect_err("an unknown key must be refused");
        assert!(error.contains("maxx"), "{error}");
        assert!(error.contains("invisible_button"), "{error}");

        let wrong = Value::record([
            ("id", Value::from(4.0)),
            (
                "size",
                Value::record([("width", Value::from(300.0)), ("height", Value::from(32.0))]),
            ),
        ]);
        let error = probe
            .frame_widget("invisible_button", seq, &wrong)
            .expect_err("a wrong id type must be refused");
        assert!(error.contains("id"), "{error}");
        assert!(error.contains("text"), "{error}");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the frame ran");
}

#[test]
fn mouse_delta_and_dragging_read_the_io() {
    let _guard = imgui_guard();
    let probe = Probe::new("mouse");

    probe.edit_io(|io| io.add_mouse_pos_event([10.0, 10.0]));
    run_frame(&probe, |probe, seq| {
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("warmup 0");

    probe.edit_io(|io| io.add_mouse_pos_event([40.0, 10.0]));
    run_frame(&probe, |probe, seq| {
        let (dx, dy) = probe.mouse_delta(seq);
        assert!(
            (dx - 30.0).abs() <= 0.5 && dy.abs() <= 0.5,
            "delta was ({dx}, {dy})"
        );
        assert!(!probe.is_mouse_dragging(seq, None), "no button is held");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("warmup 1");

    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    probe.edit_io(|io| io.add_mouse_pos_event([50.0, 10.0]));
    run_frame(&probe, |probe, seq| {
        assert!(
            probe.is_mouse_dragging(seq, None),
            "a held left button past the threshold is a drag"
        );
        assert!(
            !probe.is_mouse_dragging(seq, Some("right")),
            "the right button is not down"
        );
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("drag frame");
}

fn identical_pixels(a: &Snapshot, b: &Snapshot, area: (u32, u32, u32, u32)) -> bool {
    let (left, top, right, bottom) = area;
    for y in top..bottom.min(a.height()).min(b.height()) {
        for x in left..right.min(a.width()).min(b.width()) {
            if a.pixel(x, y) != b.pixel(x, y) {
                return false;
            }
        }
    }
    true
}

// VAL-WIDGET-022: a widget called with no live frame errors instead of
// crashing, and the error names the widget.
#[test]
fn a_widget_outside_a_live_frame_errors() {
    let _guard = imgui_guard();

    // (a) No frame at all: the sentence names the widget and the window.
    let probe = Probe::new("w");
    let outside = probe
        .frame_widget("button", 1, &Value::record([("label", Value::from("x"))]))
        .expect_err("a widget with no frame must be refused");
    assert!(outside.contains("button"), "{outside}");
    assert!(outside.contains("\"w\""), "{outside}");
    assert!(
        outside.contains("outside") && outside.contains("render callback"),
        "{outside}"
    );

    // Another window's live frame: the old `ui` is refused, the frame goes on.
    let seen = Rc::new(RefCell::new(None::<String>));
    let saved = seen.clone();
    let stale = probe.clone();
    run_frame(&probe, move |probe, seq| {
        let wrong = stale.frame_widget(
            "button",
            seq + 7,
            &Value::record([("label", Value::from("x"))]),
        );
        *saved.borrow_mut() = wrong.err();
        // The frame's own items are unaffected.
        probe.text(seq, "still drawn");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the frame ran");
    let sentence = seen.borrow().clone().expect("the stale ui was refused");
    assert!(sentence.contains("button"), "{sentence}");
    assert!(
        sentence.contains("another window's render callback"),
        "{sentence}"
    );

    // (b) After the window closed, mid-frame: refused, naming `text`.
    let closed = Rc::new(RefCell::new(None::<String>));
    let recorded = closed.clone();
    let window = probe.clone();
    run_frame(&probe, move |probe, seq| {
        window.open.set(false);
        *recorded.borrow_mut() = probe
            .frame_widget(
                "text",
                seq,
                &Value::record([("text", Value::from("after close"))]),
            )
            .err();
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the frame ended");
    let sentence = closed.borrow().clone().expect("the closed ui was refused");
    assert!(sentence.contains("text"), "{sentence}");
    assert!(
        sentence.contains("after window \"w\" was closed"),
        "{sentence}"
    );
    probe.open.set(true);
}

#[test]
fn an_overrun_callback_is_aborted_and_reported() {
    let _guard = imgui_guard();
    let probe = Probe::new("slow");

    // A clean first frame, so the overrun below is a later failure: a
    // first-frame failure is terminal now (its own tests prove that).
    let end = run_frame(&probe, |probe, seq| {
        probe.text(seq, "fine");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the first frame");
    assert!(end.painted, "{end:?}");
    assert_eq!(probe.delivered.borrow().len(), 1);

    // A callback that spends more than its 8 ms: the frame is thrown away.
    // (The host refuses the widget once the budget is gone, as the SDK does
    // from the script side, so the call's own error is expected here.)
    let end = run_frame(&probe, |probe, seq| {
        let until = Instant::now() + Duration::from_millis(12);
        let mut spin = 0_u64;
        while Instant::now() < until {
            spin = spin.wrapping_add(1);
            std::hint::black_box(spin);
        }
        let _ = probe.frame_widget(
            "text",
            seq,
            &Value::record([("text", Value::from("too slow"))]),
        );
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the frame was attempted");
    assert!(!end.painted, "an overrun frame must not paint: {end:?}");
    assert!(!end.first, "the window painted its first frame: {end:?}");
    let error = end.error.expect("the overrun is reported");
    assert!(error.contains("too long"), "{error}");
    assert_eq!(
        probe.delivered.borrow().len(),
        1,
        "nothing new was handed to the GPU view"
    );
    assert_eq!(probe.frame_state().report().errors, 1);

    // The window still draws: the next clean frame paints.
    let end = run_frame(&probe, |probe, seq| {
        probe.text(seq, "fine");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the next frame");
    assert!(end.painted, "{end:?}");
    assert_eq!(probe.delivered.borrow().len(), 2);
}

#[test]
fn the_first_attempted_frame_is_exempt_from_the_budget() {
    let _guard = imgui_guard();
    let probe = Probe::new("warmup");
    let failed = Rc::new(RefCell::new(None::<String>));

    // A window's first frame pays for the font atlas and the frame machinery
    // on the UI thread; under load that alone exceeds 8 ms. It must paint
    // rather than be cut, which would report the window broken and close it.
    let end = run_frame_reporting(&probe, failed.clone(), |probe, seq| {
        spend(12);
        probe.text(seq, "warm");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the first frame");
    assert!(
        end.painted,
        "the first frame must not be cut for the budget: {end:?}"
    );
    assert!(end.first, "{end:?}");
    assert_eq!(probe.delivered.borrow().len(), 1);
    assert_eq!(probe.frame_state().report().errors, 0);
    assert!(
        failed.borrow().is_none(),
        "the first frame must not report the window broken"
    );

    // The exemption is the first attempt only. The very next overrun is cut
    // and reported against the window; the window stays open and the plugin
    // is never withdrawn.
    let end = run_frame_reporting(&probe, failed.clone(), |probe, seq| {
        spend(12);
        let _ = probe.frame_widget("text", seq, &Value::record([("text", Value::from("warm"))]));
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the second frame");
    assert!(!end.painted, "a later overrun must be cut: {end:?}");
    assert!(!end.first, "{end:?}");
    assert!(
        end.error
            .as_deref()
            .is_some_and(|error| error.contains("too long")),
        "{end:?}"
    );
    assert!(
        failed.borrow().is_none(),
        "a later overrun keeps the plugin loaded"
    );
    assert!(probe.frame_open(), "a later overrun keeps the window open");
    assert_eq!(probe.delivered.borrow().len(), 1);

    // The next clean frame draws as if nothing happened.
    let end = run_frame(&probe, |probe, seq| {
        probe.text(seq, "warm");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the next clean frame");
    assert!(end.painted, "{end:?}");
    assert_eq!(probe.delivered.borrow().len(), 2);
}

#[test]
fn a_frame_that_uploads_a_new_atlas_is_exempt_from_the_budget() {
    let _guard = imgui_guard();
    let probe = Probe::new("rebuild");

    // A clean first frame: the probe's renderer holds the atlas afterwards.
    let end = run_frame(&probe, |probe, seq| {
        probe.text(seq, "steady");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the first frame");
    assert!(end.painted, "{end:?}");
    assert!(
        !probe.frame_needs_atlas(),
        "the first frame uploaded the atlas"
    );

    // A device change dropped every GPU object, so this frame uploads the
    // atlas again. That is host work, not the plugin's callback, and must not
    // cost the plugin its frame even though it is not the first attempt.
    probe.device_changed();
    let end = run_frame(&probe, |probe, seq| {
        spend(12);
        probe.text(seq, "steady");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the frame after a device change");
    assert!(
        end.painted,
        "an atlas-uploading frame must not be cut for the budget: {end:?}"
    );
    assert!(!end.first, "{end:?}");
    assert!(!probe.frame_needs_atlas(), "the atlas was uploaded again");
    assert_eq!(probe.delivered.borrow().len(), 2);

    // The exemption ends with that upload: the next overrun, which carries no
    // atlas, is still cut.
    let end = run_frame(&probe, |probe, seq| {
        spend(12);
        let _ = probe.frame_widget(
            "text",
            seq,
            &Value::record([("text", Value::from("steady"))]),
        );
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the frame after the upload");
    assert!(
        !end.painted,
        "an overrun without an atlas is still cut: {end:?}"
    );
    assert!(
        end.error
            .as_deref()
            .is_some_and(|error| error.contains("too long")),
        "{end:?}"
    );
    assert!(probe.frame_open(), "the window stays open");
}

#[test]
fn a_first_frame_error_reports_the_window_once_and_closes_it() {
    let _guard = imgui_guard();
    let probe = Probe::new("boom");
    let failed = Rc::new(RefCell::new(None::<String>));

    // A script error on the window's first attempted frame is terminal: it
    // never becomes visible, so it is reported with the window's id and the
    // script's own words, and taken away (VAL-LIFE-012's host half).
    let end = run_frame_reporting(&probe, failed.clone(), |probe, seq| {
        let _ = probe.frame_script_end(seq, Some("probe open failure".into()), false);
    })
    .expect("the first frame ended");
    assert!(!end.painted);
    assert!(end.first, "{end:?}");
    let sentence = failed.borrow().clone().expect("the failure is reported");
    assert!(
        sentence.contains("\"boom\"") && sentence.contains("probe open failure"),
        "the report names the window and the script's words: {sentence}"
    );
    assert!(probe.closed.get(), "a failed first frame closes the window");
    assert!(!probe.frame_open(), "the window is gone: {end:?}");
    assert!(probe.delivered.borrow().is_empty(), "nothing was painted");
    assert_eq!(probe.frame_state().report().frames, 0, "no frame painted");

    // Nothing more is pumped for it, and the failure is not reported again:
    // deriving "first" from painted frames would retry this window forever.
    *failed.borrow_mut() = None;
    assert!(
        run_frame_reporting(&probe, failed.clone(), |probe, seq| {
            probe.text(seq, "retried");
            let _ = probe.frame_script_end(seq, None, false);
        })
        .is_none(),
        "a closed window asks for no further frame"
    );
    assert!(failed.borrow().is_none(), "the failure is reported once");
    assert_eq!(probe.frame_state().report().errors, 1);
    assert_eq!(
        probe.frame_state().report().first_error.as_deref(),
        Some("probe open failure"),
        "the first attempt's error is kept for diagnostics"
    );
}

#[test]
fn a_first_frame_error_closes_the_window_without_a_failed_callback() {
    let _guard = imgui_guard();
    let probe = Probe::new("unreported");

    // The dev route reports to its log and the installed route to its bridge;
    // neither reporter is what closes the window. A no-op reporter must still
    // leave no window behind, half-drawn or retried.
    let end = run_frame(&probe, |probe, seq| {
        let _ = probe.frame_script_end(seq, Some("probe open failure".into()), false);
    })
    .expect("the first frame ended");
    assert!(!end.painted);
    assert!(end.first);
    assert!(probe.closed.get());
    assert!(!probe.frame_open());
    assert!(
        run_frame(&probe, |probe, seq| {
            probe.text(seq, "retried");
            let _ = probe.frame_script_end(seq, None, false);
        })
        .is_none(),
        "no further frame is pumped for it"
    );
}

#[test]
fn a_later_error_keeps_the_plugin_and_closes_only_after_three() {
    let _guard = imgui_guard();
    let probe = Probe::new("boom");
    let failed = Rc::new(RefCell::new(None::<String>));

    // A painted first frame: the next attempt is a later one.
    let end = run_frame_reporting(&probe, failed.clone(), |probe, seq| {
        probe.text(seq, "ok");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("a good first frame");
    assert!(end.painted && end.first, "{end:?}");
    assert_eq!(probe.delivered.borrow().len(), 1);

    // Later errors are recorded, not fatal: the window keeps painting and
    // the plugin stays loaded.
    for _ in 0..2 {
        let end = run_frame_reporting(&probe, failed.clone(), |probe, seq| {
            let _ = probe.frame_script_end(seq, Some("a later error".into()), false);
        })
        .expect("a failed frame");
        assert!(!end.painted);
        assert!(!end.first, "a painted first frame is first only once");
    }
    assert!(
        failed.borrow().is_none(),
        "a later error is reported against the window, not as a broken plugin"
    );
    assert!(probe.frame_open(), "a later error keeps today's policy");
    assert_eq!(probe.frame_state().report().consecutive_errors, 2);

    // The third in a row closes the window; the plugin itself is untouched.
    run_frame_reporting(&probe, failed.clone(), |probe, seq| {
        let _ = probe.frame_script_end(seq, Some("a third error".into()), false);
    })
    .expect("the third failed frame");
    assert!(
        probe.closed.get(),
        "three failed frames in a row close the window"
    );
    assert!(failed.borrow().is_none(), "the plugin stays loaded");
}

#[test]
fn the_first_attempted_frame_is_first_not_the_first_painted_one() {
    let _guard = imgui_guard();
    let probe = Probe::new("attempts");
    let failed = Rc::new(RefCell::new(None::<String>));

    // The first attempt ends in a widget the host refused: no frame was
    // painted and it is not a failure (VAL-WIDGET-022).
    let end = run_frame_reporting(&probe, failed.clone(), |probe, seq| {
        let error = probe
            .frame_widget(
                "text",
                seq + 99,
                &Value::record([("text", Value::from("x"))]),
            )
            .expect_err("another frame's seq");
        let _ = probe.frame_script_end(seq, Some(error), true);
    })
    .expect("the refused first attempt");
    assert!(end.refused && end.first, "{end:?}");
    assert_eq!(probe.frame_state().report().frames, 0);
    assert!(probe.frame_open(), "a refusal never closes the window");

    // The second attempt is not a first frame even though nothing painted:
    // "first" counts attempts, not painted frames. It follows the later-frame
    // policy instead of reporting a window that is not on its first try.
    let end = run_frame_reporting(&probe, failed.clone(), |probe, seq| {
        let _ = probe.frame_script_end(seq, Some("a later error".into()), false);
    })
    .expect("the second attempt");
    assert!(!end.painted);
    assert!(
        !end.first,
        "only the first attempted frame is first: {end:?}"
    );
    assert!(
        failed.borrow().is_none(),
        "the plugin is not reported broken for a later frame"
    );
    assert!(probe.frame_open(), "a later failure keeps today's policy");
    assert_eq!(probe.frame_state().report().consecutive_errors, 1);
}

#[test]
fn a_refused_widget_never_takes_the_plugin_down() {
    let _guard = imgui_guard();
    let probe = Probe::new("refused");
    let failed = Rc::new(RefCell::new(None::<String>));

    // The callback dies on a widget the host refused (here, another frame's
    // seq). The frame is dropped, and although it is the window's first,
    // the plugin is not withdrawn: a refusal is the plugin's drawing
    // mistake, not a broken script (VAL-WIDGET-022 beside VAL-LIFE-012).
    let refused_frame = |probe: &Rc<Probe>, failed: Rc<RefCell<Option<String>>>| {
        run_frame_reporting(probe, failed, |probe, seq| {
            let error = probe
                .frame_widget(
                    "text",
                    seq + 99,
                    &Value::record([("text", Value::from("x"))]),
                )
                .expect_err("another frame's seq");
            assert!(error.contains("ui.text"), "{error}");
            let _ = probe.frame_script_end(seq, Some(error), true);
        })
        .expect("the refused frame ended")
    };

    let end = refused_frame(&probe, failed.clone());
    assert!(!end.painted);
    assert!(end.refused, "{end:?}");
    assert!(end.first, "{end:?}");
    assert!(
        failed.borrow().is_none(),
        "a refusal does not report the plugin broken"
    );
    assert_eq!(
        probe.frame_state().report().consecutive_errors,
        0,
        "a refused frame is not a failed frame in a row"
    );
    assert!(
        probe
            .frame_state()
            .report()
            .last_error
            .is_some_and(|error| error.contains("ui.text")),
        "the refusal is recorded, naming the widget"
    );

    // Three in a row still only drop frames: the window stays open.
    for _ in 0..2 {
        refused_frame(&probe, failed.clone());
    }
    assert!(!probe.closed.get(), "a refusal never closes the window");
    assert!(probe.delivered.borrow().is_empty());

    // The next clean frame draws as if nothing happened.
    let end = run_frame(&probe, |probe, seq| {
        probe.text(seq, "clean");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the clean frame");
    assert!(end.painted, "{end:?}");
    assert_eq!(probe.delivered.borrow().len(), 1);
}

#[test]
fn a_frame_the_script_never_finished_is_aborted() {
    let _guard = imgui_guard();
    let probe = Probe::new("half");
    // A clean first frame, so the abandoned frame below is a later failure
    // rather than the terminal first-frame one.
    run_frame(&probe, |probe, seq| {
        probe.text(seq, "fine");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the first frame");
    // The push closure draws one widget and never calls frame_end: an error
    // that escaped the SDK's guard. The pump must still close the frame.
    let end = run_frame(&probe, |probe, seq| {
        probe.text(seq, "drawn");
    })
    .expect("the frame was aborted");
    assert!(!end.painted);
    assert!(!end.first, "{end:?}");
    assert!(
        end.error
            .as_deref()
            .is_some_and(|error| error.contains("without finishing")),
        "{end:?}"
    );

    // The context is usable afterwards: the next frame runs and paints.
    let end = run_frame(&probe, |probe, seq| {
        probe.text(seq, "again");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the next frame");
    assert!(end.painted, "{end:?}");
    assert_eq!(probe.delivered.borrow().len(), 2);
}

#[test]
fn a_widget_argument_that_is_not_text_names_the_widget() {
    let _guard = imgui_guard();
    let probe = Probe::new("args");
    let refused = Rc::new(RefCell::new(None::<String>));
    let saved = refused.clone();
    run_frame(&probe, move |probe, seq| {
        *saved.borrow_mut() = probe
            .frame_widget("text", seq, &Value::record([("text", Value::from(42.0))]))
            .err();
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the frame ran");
    let sentence = refused.borrow().clone().expect("a number is not text");
    assert!(sentence.contains("ui.text"), "{sentence}");
    assert!(sentence.contains("text"), "{sentence}");
}

#[test]
fn frames_from_two_windows_do_not_cross() {
    let _guard = imgui_guard();
    let a = Probe::new("a");
    let b = Probe::new("b");
    // Two contexts exist; each takes its turn (only one may be active).
    // Each first frame deliberately spends more than the budget, standing in
    // for the atlas build and upload it pays for: whether these frames paint
    // must not depend on how loaded this machine is, because their failure is
    // terminal and would close the window before its second frame.
    let drawn = Rc::new(RefCell::new(Vec::<&'static str>::new()));
    let seen = drawn.clone();
    run_frame(&a, {
        let seen = seen.clone();
        move |probe, seq| {
            spend(12);
            probe.text(seq, "A");
            seen.borrow_mut().push("a");
            let _ = probe.frame_script_end(seq, None, false);
        }
    })
    .expect("A's frame");
    run_frame(&b, {
        let seen = seen.clone();
        move |probe, seq| {
            spend(12);
            probe.text(seq, "B");
            seen.borrow_mut().push("b");
            let _ = probe.frame_script_end(seq, None, false);
        }
    })
    .expect("B's frame");
    assert_eq!(*drawn.borrow(), vec!["a", "b"]);
    assert_eq!(a.delivered.borrow().len(), 1);
    assert_eq!(b.delivered.borrow().len(), 1);
    // A's saved `ui` in B's turn is refused, not applied to A.
    let stale = Rc::new(RefCell::new(None::<String>));
    let saved = stale.clone();
    let first = a.clone();
    run_frame(&b, move |probe, seq| {
        *saved.borrow_mut() = first
            .frame_widget("text", seq, &Value::record([("text", Value::from("x"))]))
            .err();
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("B's second frame");
    assert!(
        stale
            .borrow()
            .as_deref()
            .is_some_and(|s| s.contains("\"a\"")),
        "{:?}",
        stale.borrow()
    );
}

#[test]
fn the_view_replays_the_frame_the_pump_prepared() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let probe = Probe::new("view");
    run_frame(&probe, |probe, seq| {
        probe.text(seq, "replayed");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("a frame");
    let painted = raster(&gpu, &probe);
    assert!(
        painted.non_background_pixels() > 100,
        "the prepared frame drew nothing: {} non-background pixels",
        painted.non_background_pixels()
    );
}

#[test]
fn a_draw_panic_fails_the_window_and_the_pump_keeps_going() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let probe = Probe::new("panic");
    let failed = Rc::new(RefCell::new(None));

    let end = run_frame_reporting(&probe, failed.clone(), |probe, seq| {
        probe.text(seq, "before the panic");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("a first frame");
    assert!(end.painted);

    // A panic inside the draw is caught at the frame boundary: this window's
    // frame fails and the reason is recorded, but nothing else is disturbed.
    let end = run_frame_reporting(&probe, failed.clone(), |_, _| panic!("draw exploded"))
        .expect("the pump survives a panicking draw");
    assert!(!end.painted);
    let error = end
        .error
        .clone()
        .expect("the panic is recorded as this frame's error");
    assert!(
        error.contains("panicked") && error.contains("draw exploded"),
        "unexpected sentence: {error}"
    );
    assert!(
        probe.frame_open(),
        "a panic must not close the window on its first failure"
    );
    assert_eq!(
        *failed.borrow(),
        None,
        "a host panic must not withdraw the plugin"
    );

    // The pump keeps serving the same window after the panic.
    let end = run_frame_reporting(&probe, failed.clone(), |probe, seq| {
        probe.text(seq, "after the panic");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("a frame after the panic");
    assert!(
        end.painted,
        "the window did not draw again after a panicking frame: {:?}",
        end.error
    );
    assert_eq!(probe.frame_state().report().consecutive_errors, 0);
    let image = raster(&gpu, &probe);
    assert!(
        image.non_background_pixels() > 100,
        "the frame after the panic was blank"
    );

    // Three panics in a row close the window; the plugin is still not
    // withdrawn (the bridge is never told it failed).
    for _ in 0..3 {
        let end = run_frame_reporting(&probe, failed.clone(), |_, _| panic!("draw exploded again"))
            .expect("the pump survives repeated panics");
        assert!(!end.painted);
    }
    assert!(
        !probe.frame_open(),
        "the window closes after three failed frames"
    );
    assert_eq!(*failed.borrow(), None, "the plugin stays loaded");
    assert!(
        run_frame_reporting(&probe, failed.clone(), |probe, seq| {
            probe.text(seq, "no frame for a closed window");
        })
        .is_none(),
        "a closed window asks for no frames"
    );
}
fn flag_of(value: &Value, field: &str) -> bool {
    matches!(value.field(field), Some(Value::Bool(true)))
}

fn num_of(value: &Value, field: &str) -> f64 {
    match value.field(field) {
        Some(Value::Num(number)) => *number,
        other => panic!("{field} is not a number: {other:?}"),
    }
}

fn str_of(value: &Value, field: &str) -> String {
    match value.field(field) {
        Some(Value::Str(text)) => text.to_string(),
        Some(Value::Text(text)) => text.to_string(),
        other => panic!("{field} is not text: {other:?}"),
    }
}

fn str_list(items: &[&str]) -> Value {
    Value::List(
        items
            .iter()
            .map(|item| Value::from(*item))
            .collect::<Vec<_>>()
            .into(),
    )
}

fn move_pointer(probe: &Rc<Probe>, x: f32, y: f32) {
    probe.edit_io(|io| io.add_mouse_pos_event([x, y]));
}

fn press_pointer(probe: &Rc<Probe>) {
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
}

fn release_pointer(probe: &Rc<Probe>) {
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
}

fn select_all(probe: &Rc<Probe>) {
    probe.edit_io(|io| io.add_key_event(Key::ModCtrl, true));
    probe.edit_io(|io| io.add_key_event(Key::A, true));
    probe.edit_io(|io| io.add_key_event(Key::A, false));
    probe.edit_io(|io| io.add_key_event(Key::ModCtrl, false));
}

fn tap_key(probe: &Rc<Probe>, key: Key) {
    probe.edit_io(|io| io.add_key_event(key, true));
    probe.edit_io(|io| io.add_key_event(key, false));
}

fn ink_colour(
    image: &Snapshot,
    background: &Snapshot,
    area: (u32, u32, u32, u32),
) -> (usize, f64, f64) {
    let (left, top, right, bottom) = area;
    let mut count = 0_usize;
    let mut reds = 0_u64;
    let mut greens = 0_u64;
    for y in top..bottom.min(image.height()) {
        for x in left..right.min(image.width()) {
            let (Some(here), Some(was)) = (image.pixel(x, y), background.pixel(x, y)) else {
                continue;
            };
            if here
                .iter()
                .zip(was.iter())
                .take(3)
                .any(|(a, b)| a.abs_diff(*b) >= 16)
            {
                count += 1;
                reds += u64::from(here[0]);
                greens += u64::from(here[1]);
            }
        }
    }
    let mean = |sum: u64| {
        if count == 0 {
            0.0
        } else {
            sum as f64 / count as f64
        }
    };
    (count, mean(reds), mean(greens))
}

fn ink_count(image: &Snapshot, background: &Snapshot, area: (u32, u32, u32, u32)) -> usize {
    ink_colour(image, background, area).0
}

fn row_span(
    image: &Snapshot,
    background: &Snapshot,
    y: u32,
    left: u32,
    right: u32,
    threshold: u8,
) -> (u32, u32, usize) {
    let mut min_x = None::<u32>;
    let mut max_x = None::<u32>;
    let mut count = 0;
    for x in left..right.min(image.width()) {
        let (Some(here), Some(was)) = (image.pixel(x, y), background.pixel(x, y)) else {
            continue;
        };
        if here
            .iter()
            .zip(was.iter())
            .take(3)
            .any(|(a, b)| a.abs_diff(*b) >= threshold)
        {
            min_x = Some(min_x.map_or(x, |m| m.min(x)));
            max_x = Some(max_x.map_or(x, |m| m.max(x)));
            count += 1;
        }
    }
    (min_x.unwrap_or(0), max_x.unwrap_or(0), count)
}

fn filled_fraction(image: &Snapshot, rect: (f32, f32, f32, f32)) -> f64 {
    let area = window_rect(rect);
    let unfilled = image
        .pixel(area.0 + (area.2 - area.0) * 9 / 10, (area.1 + area.3) / 2)
        .expect("a sample pixel");
    let mut filled = 0;
    let mut total = 0;
    for y in area.1..area.3.min(image.height()) {
        for x in area.0..area.2.min(image.width()) {
            let here = image.pixel(x, y).expect("a bar pixel");
            total += 1;
            if here
                .iter()
                .zip(unfilled.iter())
                .take(3)
                .any(|(a, b)| a.abs_diff(*b) >= 40)
            {
                filled += 1;
            }
        }
    }
    filled as f64 / total as f64
}

#[test]
fn text_colored_draws_the_same_glyphs_in_red() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();

    let empty = Probe::new("coloured-empty");
    run_frame(&empty, |probe, seq| {
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("empty");
    let background = raster(&gpu, &empty);

    let plain_rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let plain = Probe::new("coloured-plain");
    {
        let rect = plain_rect.clone();
        run_frame(&plain, move |probe, seq| {
            let answer = probe
                .frame_widget(
                    "text",
                    seq,
                    &Value::record([("text", Value::from("Alert"))]),
                )
                .expect("text");
            *rect.borrow_mut() = rect_of(&answer);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("plain");
    }
    let plain_image = raster(&gpu, &plain);

    let coloured_rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let coloured = Probe::new("coloured-red");
    {
        let rect = coloured_rect.clone();
        run_frame(&coloured, move |probe, seq| {
            let answer = probe
                .frame_widget(
                    "text_colored",
                    seq,
                    &Value::record([
                        ("text", Value::from("Alert")),
                        ("colour", Value::from("#ff0000")),
                    ]),
                )
                .expect("text_colored");
            *rect.borrow_mut() = rect_of(&answer);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("coloured");
    }
    let coloured_image = raster(&gpu, &coloured);

    let plain_area = window_rect(*plain_rect.borrow());
    let coloured_area = window_rect(*coloured_rect.borrow());
    let (plain_ink, plain_r, plain_g) = ink_colour(&plain_image, &background, plain_area);
    let (red_ink, red_r, red_g) = ink_colour(&coloured_image, &background, coloured_area);

    assert!(plain_ink > 0 && red_ink > 0, "both drew ink");
    let ratio = red_ink as f64 / plain_ink as f64;
    assert!(
        (ratio - 1.0).abs() <= 0.15,
        "same glyph count: {red_ink} vs {plain_ink}"
    );
    assert!(
        red_r - red_g >= 80.0,
        "red ink: mean red {red_r}, green {red_g}"
    );
    assert!(
        plain_r - plain_g <= 20.0,
        "neutral ink: mean red {plain_r}, green {plain_g}"
    );

    let (a, b) = (*plain_rect.borrow(), *coloured_rect.borrow());
    assert!(
        (a.0 - b.0).abs() <= 1.0
            && (a.1 - b.1).abs() <= 1.0
            && (a.2 - b.2).abs() <= 1.0
            && (a.3 - b.3).abs() <= 1.0,
        "same bounding box: {a:?} vs {b:?}"
    );
}
#[test]
fn separator_draws_a_full_width_divider() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();

    let sep_rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let probe = Probe::new("separator");
    {
        let rect = sep_rect.clone();
        run_frame(&probe, move |probe, seq| {
            probe
                .frame_widget("text", seq, &Value::record([("text", Value::from("top"))]))
                .expect("top");
            let answer = probe
                .frame_widget(
                    "separator",
                    seq,
                    &Value::record(std::iter::empty::<(&str, Value)>()),
                )
                .expect("separator");
            *rect.borrow_mut() = rect_of(&answer);
            probe
                .frame_widget(
                    "text",
                    seq,
                    &Value::record([("text", Value::from("bottom"))]),
                )
                .expect("bottom");
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("with separator");
    }
    let with_image = raster(&gpu, &probe);

    let control = Probe::new("separator-control");
    run_frame(&control, |probe, seq| {
        probe
            .frame_widget("text", seq, &Value::record([("text", Value::from("top"))]))
            .expect("top");
        probe
            .frame_widget(
                "text",
                seq,
                &Value::record([("text", Value::from("bottom"))]),
            )
            .expect("bottom");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("control");
    let control_image = raster(&gpu, &control);

    let empty = Probe::new("separator-empty");
    run_frame(&empty, |probe, seq| {
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("empty");
    let background = raster(&gpu, &empty);

    let (_, top, _, bottom) = *sep_rect.borrow();
    let centre = ((top + bottom) / 2.0).round() as i32;
    let content_left = 8_u32;
    let content_right = with_image.width().saturating_sub(8);
    let mut tall = 0;
    let mut spanned = false;
    for y in (centre - 2)..=(centre + 2) {
        let (min_x, max_x, count) = row_span(
            &with_image,
            &background,
            y as u32,
            content_left,
            content_right,
            8,
        );
        if count > 0 {
            tall += 1;
        }
        if max_x >= min_x
            && (max_x - min_x + 1) as f64 >= (content_right - content_left) as f64 * 0.9
        {
            spanned = true;
        }
    }
    assert!(spanned, "separator spans at least 90% of the content width");
    assert!(
        (1..=3).contains(&tall),
        "separator is 1..3 px tall, got {tall}"
    );
    let (_, _, control_count) = row_span(
        &control_image,
        &background,
        centre as u32,
        content_left,
        content_right,
        2,
    );
    assert_eq!(control_count, 0, "no divider without the separator call");
}

#[test]
fn small_button_clicks_on_release_only() {
    let _guard = imgui_guard();
    let probe = Probe::new("small");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let hits = Rc::new(Cell::new(0));
    let draw = {
        let (rect, hits) = (rect.clone(), hits.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "small_button",
                    seq,
                    &Value::record([("label", Value::from("x"))]),
                )
                .expect("small_button");
            *rect.borrow_mut() = rect_of(&answer);
            if flag_of(&answer, "value") {
                hits.set(hits.get() + 1);
            }
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);

    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("down");
    assert_eq!(hits.get(), 0, "press alone is not a click");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("up");
    assert_eq!(hits.get(), 1, "release is the click");
    for _ in 0..5 {
        run_frame(&probe, draw.clone()).expect("idle");
    }
    assert_eq!(hits.get(), 1, "one click total");
}

#[test]
fn checkbox_toggles_once_per_click() {
    let _guard = imgui_guard();
    let probe = Probe::new("checkbox");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let on = Rc::new(Cell::new(false));
    let changes = Rc::new(RefCell::new(Vec::<bool>::new()));
    let draw = {
        let (rect, on, changes) = (rect.clone(), on.clone(), changes.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "checkbox",
                    seq,
                    &Value::record([
                        ("label", Value::from("Enabled")),
                        ("value", Value::Bool(on.get())),
                    ]),
                )
                .expect("checkbox");
            *rect.borrow_mut() = rect_of(&answer);
            let now = flag_of(&answer, "value");
            if now != on.get() {
                changes.borrow_mut().push(now);
            }
            on.set(now);
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);

    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("down");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("up");
    for _ in 0..3 {
        run_frame(&probe, draw.clone()).expect("idle");
    }
    assert_eq!(
        *changes.borrow(),
        vec![true],
        "exactly one false->true change on the release frame"
    );

    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover 2");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("down 2");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("up 2");
    for _ in 0..3 {
        run_frame(&probe, draw.clone()).expect("idle 2");
    }
    assert_eq!(
        *changes.borrow(),
        vec![true, false],
        "second click records exactly one true->false"
    );
}

#[test]
fn radio_button_reports_the_clicked_radio_once() {
    let _guard = imgui_guard();
    let probe = Probe::new("radio");
    let rect_b = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let kind = Rc::new(RefCell::new(String::from("a")));
    let writes = Rc::new(RefCell::new(Vec::<String>::new()));
    let both = Rc::new(Cell::new(false));
    let draw = {
        let (rect_b, kind, writes, both) =
            (rect_b.clone(), kind.clone(), writes.clone(), both.clone());
        move |probe: &Probe, seq: u64| {
            let a = probe
                .frame_widget(
                    "radio_button",
                    seq,
                    &Value::record([
                        ("label", Value::from("a")),
                        ("active", Value::Bool(kind.borrow().as_str() == "a")),
                    ]),
                )
                .expect("a");
            let a_hit = flag_of(&a, "value");
            if a_hit {
                *kind.borrow_mut() = "a".into();
            }
            let b = probe
                .frame_widget(
                    "radio_button",
                    seq,
                    &Value::record([
                        ("label", Value::from("b")),
                        ("active", Value::Bool(kind.borrow().as_str() == "b")),
                    ]),
                )
                .expect("b");
            *rect_b.borrow_mut() = rect_of(&b);
            let b_hit = flag_of(&b, "value");
            if a_hit && b_hit {
                both.set(true);
            }
            if b_hit {
                if kind.borrow().as_str() != "b" {
                    writes.borrow_mut().push("b".to_string());
                }
                *kind.borrow_mut() = "b".into();
            }
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect_b.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);

    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("down");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("up");
    for _ in 0..3 {
        run_frame(&probe, draw.clone()).expect("idle");
    }
    assert_eq!(
        *writes.borrow(),
        vec!["b".to_string()],
        "one write of kind b"
    );
    assert!(!both.get(), "never both radios true in one frame");
    assert_eq!(kind.borrow().as_str(), "b");
}

#[test]
fn input_text_reports_typed_characters_in_order() {
    let _guard = imgui_guard();
    let probe = Probe::new("input_text");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let name = Rc::new(RefCell::new(String::new()));
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let draw = {
        let (rect, name, seen) = (rect.clone(), name.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "input_text",
                    seq,
                    &Value::record([
                        ("label", Value::from("name")),
                        ("value", Value::from(name.borrow().as_str())),
                    ]),
                )
                .expect("input_text");
            *rect.borrow_mut() = rect_of(&answer);
            let now = str_of(&answer, "value");
            *name.borrow_mut() = now.clone();
            seen.borrow_mut().push(now);
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);
    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("down");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("up");
    for ch in ["a", "b", "c"] {
        probe.edit_io(|io| io.add_input_characters_utf8(ch));
        run_frame(&probe, draw.clone()).expect("type");
    }
    for _ in 0..2 {
        run_frame(&probe, draw.clone()).expect("idle");
    }
    assert_eq!(
        seen.borrow().last().cloned(),
        Some("abc".to_string()),
        "final value abc"
    );
    let mut runs: Vec<String> = Vec::new();
    for item in seen.borrow().iter() {
        if runs.last() != Some(item) {
            runs.push(item.clone());
        }
    }
    let wanted = ["", "a", "ab", "abc"]
        .iter()
        .map(|item| item.to_string())
        .collect::<Vec<_>>();
    assert!(runs.ends_with(&wanted), "typed in order: {runs:?}");
}

#[test]
fn input_text_multiline_preserves_newlines() {
    let _guard = imgui_guard();
    let probe = Probe::new("multiline");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let notes = Rc::new(RefCell::new(String::from("one\ntwo")));
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let draw = {
        let (rect, notes, seen) = (rect.clone(), notes.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "input_text_multiline",
                    seq,
                    &Value::record([
                        ("label", Value::from("notes")),
                        ("value", Value::from(notes.borrow().as_str())),
                        (
                            "options",
                            Value::record([
                                ("width", Value::from(300.0)),
                                ("height", Value::from(90.0)),
                            ]),
                        ),
                    ]),
                )
                .expect("input_text_multiline");
            *rect.borrow_mut() = rect_of(&answer);
            let now = str_of(&answer, "value");
            *notes.borrow_mut() = now.clone();
            seen.borrow_mut().push(now);
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) * 0.75);
    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("focus");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release");
    for ch in "\nthree".chars() {
        let mut buffer = [0; 4];
        probe.edit_io(|io| io.add_input_characters_utf8(ch.encode_utf8(&mut buffer)));
        run_frame(&probe, draw.clone()).expect("type");
    }
    for _ in 0..2 {
        run_frame(&probe, draw.clone()).expect("idle");
    }
    assert_eq!(
        seen.borrow().last().cloned(),
        Some("one\ntwo\nthree".to_string()),
        "two newlines preserved, none substituted"
    );
}
#[test]
fn input_int_clamps_to_min_and_max() {
    let _guard = imgui_guard();
    let probe = Probe::new("input_int");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let value = Rc::new(Cell::new(5.0));
    let seen = Rc::new(RefCell::new(Vec::<f64>::new()));
    let draw = {
        let (rect, value, seen) = (rect.clone(), value.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "input_int",
                    seq,
                    &Value::record([
                        ("label", Value::from("count")),
                        ("value", Value::from(value.get())),
                        (
                            "options",
                            Value::record([("min", Value::from(0.0)), ("max", Value::from(100.0))]),
                        ),
                    ]),
                )
                .expect("input_int");
            *rect.borrow_mut() = rect_of(&answer);
            value.set(num_of(&answer, "value"));
            seen.borrow_mut().push(value.get());
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);
    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("focus");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release");
    select_all(&probe);
    run_frame(&probe, draw.clone()).expect("select all");
    for ch in ["9", "9", "9"] {
        probe.edit_io(|io| io.add_input_characters_utf8(ch));
        run_frame(&probe, draw.clone()).expect("type");
    }
    tap_key(&probe, Key::Enter);
    run_frame(&probe, draw.clone()).expect("enter");
    assert_eq!(seen.borrow().last().copied(), Some(100.0), "clamped to max");
    assert!(
        !seen.borrow().iter().any(|value| *value == 999.0),
        "999 never recorded"
    );
    for value in seen.borrow().iter() {
        assert!(
            (0.0..=100.0).contains(value) && value.fract() == 0.0,
            "value in 0..100: {value}"
        );
    }

    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover 2");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("focus 2");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release 2");
    select_all(&probe);
    run_frame(&probe, draw.clone()).expect("select all 2");
    for ch in ["-", "4"] {
        probe.edit_io(|io| io.add_input_characters_utf8(ch));
        run_frame(&probe, draw.clone()).expect("type 2");
    }
    tap_key(&probe, Key::Enter);
    run_frame(&probe, draw.clone()).expect("enter 2");
    assert_eq!(seen.borrow().last().copied(), Some(0.0), "clamped to min");
    for value in seen.borrow().iter() {
        assert!((0.0..=100.0).contains(value), "value in 0..100: {value}");
    }
}

#[test]
fn input_float_clamps_to_min_and_max() {
    let _guard = imgui_guard();
    let probe = Probe::new("input_float");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let value = Rc::new(Cell::new(0.5));
    let seen = Rc::new(RefCell::new(Vec::<f64>::new()));
    let draw = {
        let (rect, value, seen) = (rect.clone(), value.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "input_float",
                    seq,
                    &Value::record([
                        ("label", Value::from("ratio")),
                        ("value", Value::from(value.get())),
                        (
                            "options",
                            Value::record([("min", Value::from(0.0)), ("max", Value::from(1.0))]),
                        ),
                    ]),
                )
                .expect("input_float");
            *rect.borrow_mut() = rect_of(&answer);
            value.set(num_of(&answer, "value"));
            seen.borrow_mut().push(value.get());
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);
    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("focus");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release");
    select_all(&probe);
    run_frame(&probe, draw.clone()).expect("select all");
    for ch in ["9", ".", "5"] {
        probe.edit_io(|io| io.add_input_characters_utf8(ch));
        run_frame(&probe, draw.clone()).expect("type");
    }
    tap_key(&probe, Key::Enter);
    run_frame(&probe, draw.clone()).expect("enter");
    let now = seen.borrow()[seen.borrow().len() - 1];
    assert!((now - 1.0).abs() <= 1e-4, "clamped to max, got {now}");
    assert!(
        !seen
            .borrow()
            .iter()
            .any(|value| (*value - 9.5).abs() <= 1e-4),
        "9.5 never recorded"
    );
    for value in seen.borrow().iter() {
        assert!((0.0..=1.0).contains(value), "value in 0..1: {value}");
    }

    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover 2");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("focus 2");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release 2");
    select_all(&probe);
    run_frame(&probe, draw.clone()).expect("select all 2");
    for ch in ["0", ".", "2", "5"] {
        probe.edit_io(|io| io.add_input_characters_utf8(ch));
        run_frame(&probe, draw.clone()).expect("type 2");
    }
    tap_key(&probe, Key::Enter);
    run_frame(&probe, draw.clone()).expect("enter 2");
    let now = seen.borrow()[seen.borrow().len() - 1];
    assert!(
        (now - 0.25).abs() <= 1e-4,
        "fractional value kept, got {now}"
    );
}

#[test]
fn slider_int_drags_through_the_track() {
    let _guard = imgui_guard();
    let probe = Probe::new("slider_int");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let value = Rc::new(Cell::new(0.0));
    let seen = Rc::new(RefCell::new(Vec::<f64>::new()));
    let draw = {
        let (rect, value, seen) = (rect.clone(), value.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "slider_int",
                    seq,
                    &Value::record([
                        ("label", Value::from("level")),
                        ("value", Value::from(value.get())),
                        (
                            "options",
                            Value::record([("min", Value::from(0.0)), ("max", Value::from(100.0))]),
                        ),
                    ]),
                )
                .expect("slider_int");
            *rect.borrow_mut() = rect_of(&answer);
            value.set(num_of(&answer, "value"));
            seen.borrow_mut().push(value.get());
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let y = (top + bottom) / 2.0;
    let width = right - left;

    move_pointer(&probe, left + width * 0.10, y);
    run_frame(&probe, draw.clone()).expect("hover 10%");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("press 10%");
    for step in [20, 30, 40, 50, 60, 70, 80, 90] {
        move_pointer(&probe, left + width * (step as f32 / 100.0), y);
        run_frame(&probe, draw.clone()).expect("move");
    }
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release");
    for _ in 0..3 {
        run_frame(&probe, draw.clone()).expect("idle");
    }

    let values = seen.borrow();
    assert!(
        values.len() >= 13,
        "full interaction recorded: {:?}",
        values
    );
    for index in 2..10 {
        assert!(
            values[index] < values[index + 1],
            "held frames increase: {values:?}"
        );
    }
    assert!(
        (values[10] - 90.0).abs() <= 1.0,
        "90% is 90, got {}",
        values[10]
    );
    for index in 10..seen.borrow().len() {
        assert!(
            (values[index] - values[10]).abs() <= 0.5,
            "stable after release"
        );
    }
}

#[test]
fn slider_float_resolves_fractional_positions() {
    let _guard = imgui_guard();
    let probe = Probe::new("slider_float");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let value = Rc::new(Cell::new(0.0));
    let seen = Rc::new(RefCell::new(Vec::<f64>::new()));
    let draw = {
        let (rect, value, seen) = (rect.clone(), value.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "slider_float",
                    seq,
                    &Value::record([
                        ("label", Value::from("speed")),
                        ("value", Value::from(value.get())),
                        (
                            "options",
                            Value::record([("min", Value::from(0.0)), ("max", Value::from(10.0))]),
                        ),
                    ]),
                )
                .expect("slider_float");
            *rect.borrow_mut() = rect_of(&answer);
            value.set(num_of(&answer, "value"));
            seen.borrow_mut().push(value.get());
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let y = (top + bottom) / 2.0;
    let width = right - left;

    move_pointer(&probe, left + width * 0.10, y);
    run_frame(&probe, draw.clone()).expect("hover 10%");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("press 10%");
    move_pointer(&probe, left + width * 0.25, y);
    run_frame(&probe, draw.clone()).expect("move 25%");
    move_pointer(&probe, left + width * 0.50, y);
    run_frame(&probe, draw.clone()).expect("move 50%");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release");
    for _ in 0..3 {
        run_frame(&probe, draw.clone()).expect("idle");
    }

    let values = seen.borrow();
    assert!(
        (values[3] - 2.5).abs() <= 0.05,
        "25% is 2.5, got {}",
        values[3]
    );
    assert!(
        (values[4] - 5.0).abs() <= 0.05,
        "50% is 5.0, got {}",
        values[4]
    );
    for index in 4..seen.borrow().len() {
        assert!(
            (values[index] - values[4]).abs() <= 0.05,
            "stable after release"
        );
    }
}

#[test]
fn drag_int_moves_by_pointer_delta_times_speed() {
    let _guard = imgui_guard();
    let probe = Probe::new("drag_int");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let value = Rc::new(Cell::new(10.0));
    let seen = Rc::new(RefCell::new(Vec::<f64>::new()));
    let draw = {
        let (rect, value, seen) = (rect.clone(), value.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "drag_int",
                    seq,
                    &Value::record([
                        ("label", Value::from("count")),
                        ("value", Value::from(value.get())),
                        (
                            "options",
                            Value::record([
                                ("speed", Value::from(1.0)),
                                ("min", Value::from(0.0)),
                                ("max", Value::from(1000.0)),
                            ]),
                        ),
                    ]),
                )
                .expect("drag_int");
            *rect.borrow_mut() = rect_of(&answer);
            value.set(num_of(&answer, "value"));
            seen.borrow_mut().push(value.get());
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);
    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("press");
    for step in 1..=4 {
        let dx = step * 5;
        move_pointer(&probe, x + dx as f32, y);
        run_frame(&probe, draw.clone()).expect("move");
    }
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release");
    for _ in 0..3 {
        run_frame(&probe, draw.clone()).expect("idle");
    }

    let values = seen.borrow();
    let reached = *values.last().expect("a value");
    assert!(
        (reached - 30.0).abs() <= 1.0,
        "20px times speed 1: {reached}"
    );
    assert!(
        values[3] < values[4] && values[4] < values[5] && values[5] < values[6],
        "held frames change the value: {values:?}"
    );
}

#[test]
fn drag_float_moves_linear_and_back() {
    let _guard = imgui_guard();
    let probe = Probe::new("drag_float");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let value = Rc::new(Cell::new(1.0));
    let seen = Rc::new(RefCell::new(Vec::<f64>::new()));
    let draw = {
        let (rect, value, seen) = (rect.clone(), value.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "drag_float",
                    seq,
                    &Value::record([
                        ("label", Value::from("gain")),
                        ("value", Value::from(value.get())),
                        (
                            "options",
                            Value::record([
                                ("speed", Value::from(0.1)),
                                ("min", Value::from(0.0)),
                                ("max", Value::from(100.0)),
                            ]),
                        ),
                    ]),
                )
                .expect("drag_float");
            *rect.borrow_mut() = rect_of(&answer);
            value.set(num_of(&answer, "value"));
            seen.borrow_mut().push(value.get());
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);
    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("press");
    for step in 1..=3 {
        let dx = step * 10;
        move_pointer(&probe, x + dx as f32, y);
        run_frame(&probe, draw.clone()).expect("move right");
    }
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release");
    let reached = *seen.borrow().last().expect("a value");
    assert!((reached - 4.0).abs() <= 0.02, "30px times 0.1: {reached}");

    move_pointer(&probe, x + 10.0, y);
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("press back");
    for step in 0..3 {
        let dx = step * 10;
        move_pointer(&probe, x - dx as f32, y);
        run_frame(&probe, draw.clone()).expect("move left");
    }
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("release back");
    let back = *seen.borrow().last().expect("a value");
    assert!((back - 1.0).abs() <= 0.02, "back to 1.0: {back}");
}
#[test]
fn progress_bar_fills_the_fraction_given() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let rect_a = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let rect_b = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let probe = Probe::new("bars");
    {
        let (a, b) = (rect_a.clone(), rect_b.clone());
        run_frame(&probe, move |probe, seq| {
            let answer = probe
                .frame_widget(
                    "progress_bar",
                    seq,
                    &Value::record([("fraction", Value::from(0.25))]),
                )
                .expect("bar a");
            *a.borrow_mut() = rect_of(&answer);
            let answer = probe
                .frame_widget(
                    "progress_bar",
                    seq,
                    &Value::record([("fraction", Value::from(0.75))]),
                )
                .expect("bar b");
            *b.borrow_mut() = rect_of(&answer);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("bars");
    }
    let image = raster(&gpu, &probe);
    let (a, b) = (*rect_a.borrow(), *rect_b.borrow());
    let (fa, fb) = (filled_fraction(&image, a), filled_fraction(&image, b));
    assert!((fa - 0.25).abs() <= 0.08, "0.25 bar filled {fa}");
    assert!((fb - 0.75).abs() <= 0.08, "0.75 bar filled {fb}");
    assert!(
        ((fb - fa) - 0.5).abs() <= 0.04,
        "lengths differ by half the bar: {fa} vs {fb}"
    );
}

#[test]
fn combo_returns_the_chosen_value_not_its_index() {
    let _guard = imgui_guard();
    let probe = Probe::new("combo");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let popup = Rc::new(RefCell::new(Vec::<((f32, f32, f32, f32), String)>::new()));
    let theme = Rc::new(RefCell::new(String::from("dark")));
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    let draw = {
        let (rect, popup, theme, seen) = (rect.clone(), popup.clone(), theme.clone(), seen.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "combo",
                    seq,
                    &Value::record([
                        ("label", Value::from("theme")),
                        ("value", Value::from(theme.borrow().as_str())),
                        ("options", str_list(&["dark", "light", "system"])),
                    ]),
                )
                .expect("combo");
            *rect.borrow_mut() = rect_of(&answer);
            let now = str_of(&answer, "value");
            *theme.borrow_mut() = now.clone();
            seen.borrow_mut().push(now);
            let items = answer
                .field("items")
                .map(|items| items.list().to_vec())
                .unwrap_or_default();
            *popup.borrow_mut() = items
                .iter()
                .map(|item| (rect_of(item), str_of(item, "label")))
                .collect();
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);
    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("open down");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("open up");
    run_frame(&probe, draw.clone()).expect("popup");

    let light = popup
        .borrow()
        .iter()
        .find(|(_, label)| label == "light")
        .map(|(rect, _)| *rect)
        .expect("light option rect");
    let (lx, ly) = ((light.0 + light.2) / 2.0, (light.1 + light.3) / 2.0);
    move_pointer(&probe, lx, ly);
    run_frame(&probe, draw.clone()).expect("pick hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("pick down");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("pick up");
    for _ in 0..2 {
        run_frame(&probe, draw.clone()).expect("idle");
    }
    assert_eq!(
        seen.borrow().last().cloned(),
        Some("light".to_string()),
        "chosen value, not index"
    );
    assert!(
        !seen
            .borrow()
            .iter()
            .any(|value| value == "1" || value == "2"),
        "no index was ever returned"
    );
}

#[test]
fn selectable_reports_the_clicked_row_once() {
    let _guard = imgui_guard();
    let probe = Probe::new("selectable");
    let rect_b = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let sel = Rc::new(RefCell::new(String::new()));
    let writes = Rc::new(RefCell::new(Vec::<String>::new()));
    let both = Rc::new(Cell::new(false));
    let draw = {
        let (rect_b, sel, writes, both) =
            (rect_b.clone(), sel.clone(), writes.clone(), both.clone());
        move |probe: &Probe, seq: u64| {
            let a = probe
                .frame_widget(
                    "selectable",
                    seq,
                    &Value::record([
                        ("label", Value::from("Row A")),
                        ("selected", Value::Bool(sel.borrow().as_str() == "A")),
                    ]),
                )
                .expect("selectable a");
            let a_clicked = flag_of(&a, "value");
            let b = probe
                .frame_widget(
                    "selectable",
                    seq,
                    &Value::record([
                        ("label", Value::from("Row B")),
                        ("selected", Value::Bool(sel.borrow().as_str() == "B")),
                    ]),
                )
                .expect("selectable b");
            *rect_b.borrow_mut() = rect_of(&b);
            let b_clicked = flag_of(&b, "value");
            if a_clicked && b_clicked {
                both.set(true);
            }
            if a_clicked {
                *sel.borrow_mut() = "A".into();
            }
            if b_clicked {
                if sel.borrow().as_str() != "B" {
                    writes.borrow_mut().push("B".to_string());
                }
                *sel.borrow_mut() = "B".into();
            }
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect_b.borrow();
    let (x, y) = ((left + right) / 2.0, (top + bottom) / 2.0);
    move_pointer(&probe, x, y);
    run_frame(&probe, draw.clone()).expect("hover");
    press_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("down");
    assert!(writes.borrow().is_empty(), "no write on the down frame");
    release_pointer(&probe);
    run_frame(&probe, draw.clone()).expect("up");
    for _ in 0..3 {
        run_frame(&probe, draw.clone()).expect("idle");
    }
    assert_eq!(
        *writes.borrow(),
        vec!["B".to_string()],
        "one write for row B"
    );
    assert!(!both.get(), "never both rows true in one frame");
}

#[test]
fn bullet_text_draws_a_bullet_before_its_words() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let probe = Probe::new("bullet");
    {
        let saved = rect.clone();
        run_frame(&probe, move |probe, seq| {
            probe
                .frame_widget(
                    "text",
                    seq,
                    &Value::record([("text", Value::from("Heading"))]),
                )
                .expect("heading");
            let answer = probe
                .frame_widget(
                    "bullet_text",
                    seq,
                    &Value::record([("text", Value::from("Item one"))]),
                )
                .expect("bullet_text");
            *saved.borrow_mut() = rect_of(&answer);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("bullet");
    }
    let image = raster(&gpu, &probe);

    let empty = Probe::new("bullet-empty");
    run_frame(&empty, |probe, seq| {
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("empty");
    let background = raster(&gpu, &empty);

    let (left, top, right, bottom) = *rect.borrow();
    let (left, top) = (left as u32, top as u32);
    let (right, bottom) = (right.ceil() as u32, bottom.ceil() as u32);
    let bullet_band = (left + 16).min(image.width());
    let first = ink_count(&image, &background, (left, top, bullet_band, bottom));
    let rest = ink_count(&image, &background, (bullet_band, top, right, bottom));
    assert!(first >= 12, "bullet blob before the words: {first}");
    assert!(rest >= 40, "word ink right of the bullet: {rest}");
}

#[test]
fn an_unknown_option_key_is_refused_naming_it() {
    let _guard = imgui_guard();
    let probe = Probe::new("unknown-option");
    run_frame(&probe, |probe, seq| {
        probe
            .frame_widget(
                "text",
                seq,
                &Value::record([("text", Value::from("before"))]),
            )
            .expect("before");
        let error = probe
            .frame_widget(
                "input_int",
                seq,
                &Value::record([
                    ("label", Value::from("count")),
                    ("value", Value::from(5.0)),
                    (
                        "options",
                        Value::record([("min", Value::from(0.0)), ("maxx", Value::from(100.0))]),
                    ),
                ]),
            )
            .expect_err("an unknown key must be refused");
        assert!(error.contains("input_int"), "{error}");
        assert!(error.contains("maxx"), "{error}");
        assert!(error.contains("min"), "{error}");
        assert!(error.contains("max"), "{error}");
        let _ = probe.frame_script_end(seq, Some(error), true);
    })
    .expect("the frame ended as a refusal");
}

#[test]
fn a_wrong_argument_type_is_refused_naming_them() {
    let _guard = imgui_guard();
    let probe = Probe::new("wrong-type");
    run_frame(&probe, |probe, seq| {
        probe
            .frame_widget(
                "text",
                seq,
                &Value::record([("text", Value::from("before"))]),
            )
            .expect("before");
        let error = probe
            .frame_widget(
                "drag_float",
                seq,
                &Value::record([
                    ("label", Value::from("gain")),
                    ("value", Value::from("fast")),
                ]),
            )
            .expect_err("a wrong type must be refused");
        assert!(error.contains("drag_float"), "{error}");
        assert!(error.contains("value"), "{error}");
        assert!(error.contains("number"), "{error}");
        assert!(error.contains("string"), "{error}");
        let _ = probe.frame_script_end(seq, Some(error), true);
    })
    .expect("the frame ended as a refusal");
}

#[test]
fn widget_options_and_colours_are_checked() {
    let _guard = imgui_guard();
    let probe = Probe::new("checks");
    let messages = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let messages = messages.clone();
        run_frame(&probe, move |probe, seq| {
            messages.borrow_mut().push(
                probe
                    .frame_widget(
                        "text_colored",
                        seq,
                        &Value::record([
                            ("text", Value::from("x")),
                            ("colour", Value::from("red")),
                        ]),
                    )
                    .expect_err("a colour without hex must be refused"),
            );
            messages.borrow_mut().push(
                probe
                    .frame_widget(
                        "progress_bar",
                        seq,
                        &Value::record([("fraction", Value::from(1.5))]),
                    )
                    .expect_err("an out-of-range fraction must be refused"),
            );
            let under = probe
                .frame_widget(
                    "progress_bar",
                    seq,
                    &Value::record([("fraction", Value::from(-0.1))]),
                )
                .expect_err("a negative fraction must be refused");
            messages.borrow_mut().push(under.clone());
            let _ = probe.frame_script_end(seq, Some(under), true);
        })
        .expect("the frame ended as a refusal");
    }
    let messages = messages.borrow();
    assert!(messages[0].contains("colour"), "{}", messages[0]);
    assert!(messages[1].contains("progress_bar"), "{}", messages[1]);
    assert!(messages[1].contains("1.5"), "{}", messages[1]);
    assert!(messages[2].contains("0 to 1"), "{}", messages[2]);
}

fn no_arg() -> Value {
    Value::record(std::iter::empty::<(&str, Value)>())
}

fn sheet(probe: &Probe, seq: u64, out: &mut Vec<(f32, f32, f32, f32)>) {
    out.push(rect_of(&probe.text(seq, "A")));

    probe
        .frame_widget("same_line", seq, &no_arg())
        .expect("same_line");
    out.push(rect_of(&probe.text(seq, "B")));

    probe
        .frame_widget("new_line", seq, &no_arg())
        .expect("new_line");
    out.push(rect_of(&probe.text(seq, "C")));

    probe
        .frame_widget("spacing", seq, &no_arg())
        .expect("spacing");
    out.push(rect_of(
        &probe
            .frame_widget(
                "dummy",
                seq,
                &Value::record([("width", Value::from(40.0)), ("height", Value::from(60.0))]),
            )
            .expect("dummy"),
    ));

    probe
        .frame_widget("indent", seq, &no_arg())
        .expect("indent");
    out.push(rect_of(&probe.text(seq, "D")));
    probe
        .frame_widget("unindent", seq, &no_arg())
        .expect("unindent");

    out.push(rect_of(
        &probe
            .frame_widget(
                "begin_child",
                seq,
                &Value::record([
                    ("id", Value::from("log")),
                    ("width", Value::from(240.0)),
                    ("height", Value::from(80.0)),
                ]),
            )
            .expect("begin_child"),
    ));
    out.push(rect_of(&probe.text(seq, "inside")));
    probe
        .frame_widget("end_child", seq, &no_arg())
        .expect("end_child");

    probe
        .frame_widget(
            "columns",
            seq,
            &Value::record([("count", Value::from(2.0))]),
        )
        .expect("columns");
    out.push(rect_of(&probe.text(seq, "left")));
    probe
        .frame_widget("next_column", seq, &no_arg())
        .expect("next_column");
    out.push(rect_of(&probe.text(seq, "right")));
    probe
        .frame_widget(
            "columns",
            seq,
            &Value::record([("count", Value::from(1.0))]),
        )
        .expect("columns close");

    probe
        .frame_widget("begin_group", seq, &no_arg())
        .expect("begin_group");
    out.push(rect_of(&probe.text(seq, "group label")));
    out.push(rect_of(
        &probe
            .frame_widget(
                "button",
                seq,
                &Value::record([("label", Value::from("Go"))]),
            )
            .expect("button"),
    ));
    out.push(rect_of(
        &probe
            .frame_widget("end_group", seq, &no_arg())
            .expect("end_group"),
    ));

    probe
        .frame_widget(
            "set_next_item_width",
            seq,
            &Value::record([("width", Value::from(160.0))]),
        )
        .expect("set_next_item_width");
    out.push(rect_of(
        &probe
            .frame_widget(
                "progress_bar",
                seq,
                &Value::record([("fraction", Value::from(0.5))]),
            )
            .expect("progress_bar"),
    ));
}

#[test]
fn same_line_puts_the_next_item_beside_the_previous() {
    let _guard = imgui_guard();
    let probe = Probe::new("same-line");
    let spacing = probe.style().item_spacing()[0];

    let a = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let b = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (a, b) = (a.clone(), b.clone());
        run_frame(&probe, move |probe, seq| {
            *a.borrow_mut() = rect_of(&probe.text(seq, "A"));
            probe
                .frame_widget("same_line", seq, &no_arg())
                .expect("same_line");
            *b.borrow_mut() = rect_of(&probe.text(seq, "B"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let (a, b) = (*a.borrow(), *b.borrow());
    assert!((a.1 - b.1).abs() <= 0.5, "same row: {a:?} {b:?}");
    assert!(
        b.0 >= a.2 + spacing - 0.5,
        "B starts after A plus spacing: {a:?} {b:?}"
    );

    let control = Probe::new("same-line-control");
    let ca = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let cb = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (ca, cb) = (ca.clone(), cb.clone());
        run_frame(&control, move |probe, seq| {
            *ca.borrow_mut() = rect_of(&probe.text(seq, "A"));
            *cb.borrow_mut() = rect_of(&probe.text(seq, "B"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let (ca, cb) = (*ca.borrow(), *cb.borrow());
    assert!(
        cb.1 > ca.3,
        "without the call B stays on the next row: {ca:?} {cb:?}"
    );
}

#[test]
fn new_line_breaks_the_row_back_to_the_column_edge() {
    let _guard = imgui_guard();
    let probe = Probe::new("new-line");

    let a = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let b = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (a, b) = (a.clone(), b.clone());
        run_frame(&probe, move |probe, seq| {
            *a.borrow_mut() = rect_of(&probe.text(seq, "A"));
            probe
                .frame_widget("same_line", seq, &no_arg())
                .expect("same_line");
            probe
                .frame_widget("new_line", seq, &no_arg())
                .expect("new_line");
            *b.borrow_mut() = rect_of(&probe.text(seq, "B"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let (a, b) = (*a.borrow(), *b.borrow());
    assert!(
        (b.0 - a.0).abs() <= 0.5,
        "back to the column edge: {a:?} {b:?}"
    );
    assert!(b.1 > a.3, "B is below A: {a:?} {b:?}");

    let control = Probe::new("new-line-control");
    let ca = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let cb = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (ca, cb) = (ca.clone(), cb.clone());
        run_frame(&control, move |probe, seq| {
            *ca.borrow_mut() = rect_of(&probe.text(seq, "A"));
            probe
                .frame_widget("same_line", seq, &no_arg())
                .expect("same_line");
            *cb.borrow_mut() = rect_of(&probe.text(seq, "B"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let (ca, cb) = (*ca.borrow(), *cb.borrow());
    assert!(
        cb.0 > ca.2,
        "without new_line B stays beside A: {ca:?} {cb:?}"
    );
}

#[test]
fn spacing_adds_exactly_one_item_spacing() {
    let _guard = imgui_guard();
    let probe = Probe::new("spacing");
    let spacing = probe.style().item_spacing()[1];

    let a_with = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let b_with = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let gap_with = Rc::new(Cell::new(0.0_f32));
    {
        let (a, b, gap) = (a_with.clone(), b_with.clone(), gap_with.clone());
        run_frame(&probe, move |probe, seq| {
            *a.borrow_mut() = rect_of(&probe.text(seq, "A"));
            probe
                .frame_widget("spacing", seq, &no_arg())
                .expect("spacing");
            *b.borrow_mut() = rect_of(&probe.text(seq, "B"));
            gap.set((*b.borrow()).1 - (*a.borrow()).3);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }

    let control = Probe::new("spacing-control");
    let ca = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let cb = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let gap_control = Rc::new(Cell::new(0.0_f32));
    {
        let (ca, cb, gap) = (ca.clone(), cb.clone(), gap_control.clone());
        run_frame(&control, move |probe, seq| {
            *ca.borrow_mut() = rect_of(&probe.text(seq, "A"));
            *cb.borrow_mut() = rect_of(&probe.text(seq, "B"));
            gap.set((*cb.borrow()).1 - (*ca.borrow()).3);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }

    assert!(
        (gap_with.get() - gap_control.get() - spacing).abs() <= 0.5,
        "spacing adds one item spacing: {} vs {} (want {spacing})",
        gap_with.get(),
        gap_control.get()
    );
}

#[test]
fn dummy_reserves_its_height_and_draws_nothing() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let probe = Probe::new("dummy");
    let spacing = probe.style().item_spacing()[1];

    let a = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let dummy = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let b = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (a, dummy, b) = (a.clone(), dummy.clone(), b.clone());
        run_frame(&probe, move |probe, seq| {
            *a.borrow_mut() = rect_of(&probe.text(seq, "A"));
            *dummy.borrow_mut() = rect_of(
                &probe
                    .frame_widget(
                        "dummy",
                        seq,
                        &Value::record([
                            ("width", Value::from(40.0)),
                            ("height", Value::from(60.0)),
                        ]),
                    )
                    .expect("dummy"),
            );
            *b.borrow_mut() = rect_of(&probe.text(seq, "B"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let painted = raster(&gpu, &probe);

    let control = Probe::new("dummy-control");
    let ca = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let cb = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (ca, cb) = (ca.clone(), cb.clone());
        run_frame(&control, move |probe, seq| {
            *ca.borrow_mut() = rect_of(&probe.text(seq, "A"));
            *cb.borrow_mut() = rect_of(&probe.text(seq, "B"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }

    let (b, cb) = (*b.borrow(), *cb.borrow());
    assert!(
        (b.1 - cb.1 - 60.0 - spacing).abs() <= 0.5,
        "the next item moves down by the dummy height plus spacing: {b:?} {cb:?}"
    );
    assert!((b.0 - cb.0).abs() <= 0.5, "x is unchanged: {b:?} {cb:?}");

    let empty = Probe::new("dummy-empty");
    run_frame(&empty, |probe, seq| {
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("empty");
    let background = raster(&gpu, &empty);
    assert!(
        identical_pixels(&painted, &background, window_rect(*dummy.borrow())),
        "the dummy band draws nothing"
    );
}

#[test]
fn indent_shifts_subsequent_items_by_one_step() {
    let _guard = imgui_guard();
    let probe = Probe::new("indent");
    let indent = probe.style().indent_spacing();

    let a = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let b = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (a, b) = (a.clone(), b.clone());
        run_frame(&probe, move |probe, seq| {
            *a.borrow_mut() = rect_of(&probe.text(seq, "A"));
            probe
                .frame_widget("indent", seq, &no_arg())
                .expect("indent");
            *b.borrow_mut() = rect_of(&probe.text(seq, "B"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let (a, b) = (*a.borrow(), *b.borrow());
    assert!(
        (b.0 - a.0 - indent).abs() <= 0.5,
        "shifted by one indent step: {a:?} {b:?} indent {indent}"
    );
    assert!(b.1 > a.3, "B is below A: {a:?} {b:?}");
}

#[test]
fn unindent_removes_exactly_one_indent_level() {
    let _guard = imgui_guard();
    let probe = Probe::new("unindent");
    let indent = probe.style().indent_spacing();

    let a = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let c = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (a, c) = (a.clone(), c.clone());
        run_frame(&probe, move |probe, seq| {
            *a.borrow_mut() = rect_of(&probe.text(seq, "A"));
            probe
                .frame_widget("indent", seq, &no_arg())
                .expect("indent");
            probe
                .frame_widget("indent", seq, &no_arg())
                .expect("indent");
            probe
                .frame_widget("unindent", seq, &no_arg())
                .expect("unindent");
            *c.borrow_mut() = rect_of(&probe.text(seq, "C"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let (a, c) = (*a.borrow(), *c.borrow());
    assert!(
        (c.0 - a.0 - indent).abs() <= 0.5,
        "one indent remains: {a:?} {c:?} indent {indent}"
    );

    let again = Probe::new("unindent-again");
    let c2 = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let c2 = c2.clone();
        run_frame(&again, move |probe, seq| {
            probe.text(seq, "A");
            probe
                .frame_widget("indent", seq, &no_arg())
                .expect("indent");
            probe
                .frame_widget("indent", seq, &no_arg())
                .expect("indent");
            probe
                .frame_widget("unindent", seq, &no_arg())
                .expect("unindent");
            probe
                .frame_widget("unindent", seq, &no_arg())
                .expect("unindent");
            *c2.borrow_mut() = rect_of(&probe.text(seq, "C"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    assert!(
        (c2.borrow().0 - a.0).abs() <= 0.5,
        "two unindents return to the column edge: {a:?} {c2:?}"
    );
}

#[test]
fn begin_child_clips_its_content() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let probe = Probe::new("child-clip");

    let child = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let after = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (child, after) = (child.clone(), after.clone());
        run_frame(&probe, move |probe, seq| {
            *child.borrow_mut() = rect_of(
                &probe
                    .frame_widget(
                        "begin_child",
                        seq,
                        &Value::record([
                            ("id", Value::from("log")),
                            ("width", Value::from(240.0)),
                            ("height", Value::from(80.0)),
                        ]),
                    )
                    .expect("begin_child"),
            );
            for line in 0..40 {
                probe.text(seq, &format!("line {line}"));
            }
            probe
                .frame_widget("end_child", seq, &no_arg())
                .expect("end_child");
            *after.borrow_mut() = rect_of(&probe.text(seq, "after"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let painted = raster(&gpu, &probe);

    let empty = Probe::new("child-clip-empty");
    run_frame(&empty, |probe, seq| {
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("empty");
    let background = raster(&gpu, &empty);

    let child = *child.borrow();
    let after_top = after.borrow().1.floor().max(0.0) as u32;
    let area = window_rect(child);
    let inside = differing(&painted, &background, area);
    assert!(
        inside >= 40,
        "at least one line ink inside the child: {inside}"
    );

    let left = differing_above(
        &painted,
        &background,
        (0, area.1, area.0.saturating_sub(1), area.3),
        2,
    );
    let right = differing_above(
        &painted,
        &background,
        (area.2 + 1, area.1, painted.width(), area.3),
        2,
    );
    let below = differing_above(
        &painted,
        &background,
        (area.0, area.3 + 1, area.2, after_top),
        2,
    );
    assert_eq!(left, 0, "no ink left of the child: {child:?}");
    assert_eq!(right, 0, "no ink right of the child: {child:?}");
    assert_eq!(
        below, 0,
        "no clipped ink between the child and after: {child:?}"
    );
}

#[test]
fn a_child_region_scrolls_with_the_wheel() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let probe = Probe::new("child-scroll");

    let child = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let draw = {
        let child = child.clone();
        move |probe: &Probe, seq: u64| {
            *child.borrow_mut() = rect_of(
                &probe
                    .frame_widget(
                        "begin_child",
                        seq,
                        &Value::record([
                            ("id", Value::from("log")),
                            ("width", Value::from(240.0)),
                            ("height", Value::from(80.0)),
                        ]),
                    )
                    .expect("begin_child"),
            );
            for line in 0..40 {
                probe.text(seq, &format!("line {line}"));
            }
            probe
                .frame_widget("end_child", seq, &no_arg())
                .expect("end_child");
            let _ = probe.frame_script_end(seq, None, false);
        }
    };
    run_frame(&probe, draw.clone()).expect("before");
    let before = raster(&gpu, &probe);
    let area = window_rect(*child.borrow());
    let (left, top, right, bottom) = area;
    let child_before = *child.borrow();

    let centre = [(left + right) as f32 / 2.0, (top + bottom) as f32 / 2.0];
    move_pointer(&probe, centre[0], centre[1]);
    for _ in 0..3 {
        probe.edit_io(|io| io.add_mouse_wheel_event([0.0, -1.0]));
        run_frame(&probe, draw.clone()).expect("wheel");
    }
    for _ in 0..2 {
        run_frame(&probe, draw.clone()).expect("idle");
    }
    let after = raster(&gpu, &probe);
    let child_after = *child.borrow();
    assert!(
        (child_before.0 - child_after.0).abs() <= 0.5
            && (child_before.1 - child_after.1).abs() <= 0.5
            && (child_before.2 - child_after.2).abs() <= 0.5
            && (child_before.3 - child_after.3).abs() <= 0.5,
        "the child rect stays put, so the window did not scroll: {child_before:?} vs {child_after:?}"
    );
    let changed = differing(&after, &before, area);
    let total = (right - left).saturating_mul(bottom - top) as usize;
    assert!(
        changed * 100 >= total * 5,
        "at least 5% of the child changes when scrolled: {changed}/{total}"
    );
    let strip_bottom = (top + 12).min(bottom);
    let strip_total = (right - left).saturating_mul(strip_bottom - top) as usize;
    let strip_changed = differing(&after, &before, (left, top, right, strip_bottom));
    assert!(
        strip_changed * 20 >= strip_total,
        "the first visible line moved inside the child: {strip_changed}/{strip_total}"
    );
}

#[test]
fn columns_split_into_equal_halves() {
    let _guard = imgui_guard();
    let probe = Probe::new("columns");

    let full = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let left = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let right = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (full, left, right) = (full.clone(), left.clone(), right.clone());
        run_frame(&probe, move |probe, seq| {
            *full.borrow_mut() = rect_of(
                &probe
                    .frame_widget(
                        "progress_bar",
                        seq,
                        &Value::record([("fraction", Value::from(0.5))]),
                    )
                    .expect("full width bar"),
            );
            probe
                .frame_widget(
                    "columns",
                    seq,
                    &Value::record([("count", Value::from(2.0))]),
                )
                .expect("columns");
            *left.borrow_mut() = rect_of(&probe.text(seq, "left"));
            probe
                .frame_widget("next_column", seq, &no_arg())
                .expect("next_column");
            *right.borrow_mut() = rect_of(&probe.text(seq, "right"));
            probe
                .frame_widget(
                    "columns",
                    seq,
                    &Value::record([("count", Value::from(1.0))]),
                )
                .expect("columns close");
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let (full, left, right) = (*full.borrow(), *left.borrow(), *right.borrow());
    let mid = (full.0 + full.2) / 2.0;
    assert!(
        (left.1 - right.1).abs() <= 0.5,
        "same row: {left:?} {right:?}"
    );
    assert!(
        left.2 <= mid && right.0 >= mid,
        "each text stays in its half: {left:?} {right:?} mid {mid}"
    );
    assert!(
        left.0 >= full.0 - 1.0 && right.2 <= full.2 + 1.0,
        "the columns stay inside the content region: {full:?} {left:?} {right:?}"
    );
}

#[test]
fn a_group_advances_as_one_item() {
    let _guard = imgui_guard();
    let probe = Probe::new("group");

    let label = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let button = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let group = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let beside = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (label, button, group, beside) =
            (label.clone(), button.clone(), group.clone(), beside.clone());
        run_frame(&probe, move |probe, seq| {
            probe
                .frame_widget("begin_group", seq, &no_arg())
                .expect("begin_group");
            *label.borrow_mut() = rect_of(&probe.text(seq, "a long label for the group"));
            *button.borrow_mut() = rect_of(
                &probe
                    .frame_widget(
                        "button",
                        seq,
                        &Value::record([("label", Value::from("Go"))]),
                    )
                    .expect("button"),
            );
            *group.borrow_mut() = rect_of(
                &probe
                    .frame_widget("end_group", seq, &no_arg())
                    .expect("end_group"),
            );
            probe
                .frame_widget("same_line", seq, &no_arg())
                .expect("same_line");
            *beside.borrow_mut() = rect_of(&probe.text(seq, "beside"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let (label, button, group, beside) = (
        *label.borrow(),
        *button.borrow(),
        *group.borrow(),
        *beside.borrow(),
    );
    assert!(
        beside.0 >= group.2 - 0.5,
        "beside starts at or right of the group edge: {group:?} {beside:?}"
    );
    assert!(
        beside.1 >= label.1 && beside.1 < label.3,
        "beside overlaps the group's first row: {label:?} {beside:?}"
    );
    assert!(
        group.3 - group.1 >= 2.0 * (label.3 - label.1),
        "the group covers both rows: {label:?} {button:?} {group:?}"
    );

    let control = Probe::new("group-control");
    let clabel = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let cbeside = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (clabel, cbeside) = (clabel.clone(), cbeside.clone());
        run_frame(&control, move |probe, seq| {
            *clabel.borrow_mut() = rect_of(&probe.text(seq, "a long label for the group"));
            probe
                .frame_widget(
                    "button",
                    seq,
                    &Value::record([("label", Value::from("Go"))]),
                )
                .expect("button");
            probe
                .frame_widget("same_line", seq, &no_arg())
                .expect("same_line");
            *cbeside.borrow_mut() = rect_of(&probe.text(seq, "beside"));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let (clabel, cbeside) = (*clabel.borrow(), *cbeside.borrow());
    assert!(
        cbeside.1 > clabel.3,
        "without the group beside sits beside the button, below the label: {clabel:?} {cbeside:?}"
    );
}

#[test]
fn set_next_item_width_sizes_exactly_the_next_item() {
    let _guard = imgui_guard();
    let probe = Probe::new("next-width");

    let first = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let second = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (first, second) = (first.clone(), second.clone());
        run_frame(&probe, move |probe, seq| {
            probe
                .frame_widget(
                    "set_next_item_width",
                    seq,
                    &Value::record([("width", Value::from(160.0))]),
                )
                .expect("set_next_item_width");
            *first.borrow_mut() = rect_of(
                &probe
                    .frame_widget(
                        "progress_bar",
                        seq,
                        &Value::record([("fraction", Value::from(0.5))]),
                    )
                    .expect("bar 1"),
            );
            *second.borrow_mut() = rect_of(
                &probe
                    .frame_widget(
                        "progress_bar",
                        seq,
                        &Value::record([("fraction", Value::from(0.5))]),
                    )
                    .expect("bar 2"),
            );
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }

    let control = Probe::new("next-width-control");
    let full = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let full = full.clone();
        run_frame(&control, move |probe, seq| {
            *full.borrow_mut() = rect_of(
                &probe
                    .frame_widget(
                        "progress_bar",
                        seq,
                        &Value::record([("fraction", Value::from(0.5))]),
                    )
                    .expect("full width bar"),
            );
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let content_width = width_of(*full.borrow());

    assert!(
        (width_of(*first.borrow()) - 160.0).abs() <= 0.5,
        "the first bar is 160 wide: {:?}",
        *first.borrow()
    );
    assert!(
        (width_of(*second.borrow()) - content_width).abs() <= 1.0,
        "the second bar is full width: {:?} content {content_width}",
        *second.borrow()
    );

    let probe = Probe::new("next-width-input");
    let control_width = Rc::new(Cell::new(0.0_f32));
    {
        let control_width = control_width.clone();
        run_frame(&probe, move |probe, seq| {
            let answer = probe
                .frame_widget(
                    "input_text",
                    seq,
                    &Value::record([("label", Value::from("name")), ("value", Value::from(""))]),
                )
                .expect("control input_text");
            control_width.set(width_of(rect_of(&answer)));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("control frame");
    }
    let probe = Probe::new("next-width-input-wide");
    let sized_width = Rc::new(Cell::new(0.0_f32));
    {
        let sized_width = sized_width.clone();
        run_frame(&probe, move |probe, seq| {
            probe
                .frame_widget(
                    "set_next_item_width",
                    seq,
                    &Value::record([("width", Value::from(160.0))]),
                )
                .expect("set_next_item_width");
            let answer = probe
                .frame_widget(
                    "input_text",
                    seq,
                    &Value::record([("label", Value::from("name")), ("value", Value::from(""))]),
                )
                .expect("sized input_text");
            sized_width.set(width_of(rect_of(&answer)));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("sized frame");
    }
    assert!(
        (sized_width.get() - control_width.get() - (160.0 - content_width)).abs() <= 1.5,
        "the width applies to the next item, not only bars: {} vs {} content {content_width}",
        sized_width.get(),
        control_width.get()
    );
}

#[test]
fn text_wraps_at_the_region_width() {
    let _guard = imgui_guard();
    let sentence = "wrapped words in a paragraph ".repeat(8);

    let probe = Probe::new("wrap");
    let child = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let inside = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let (child, inside) = (child.clone(), inside.clone());
        let sentence = sentence.clone();
        run_frame(&probe, move |probe, seq| {
            *child.borrow_mut() = rect_of(
                &probe
                    .frame_widget(
                        "begin_child",
                        seq,
                        &Value::record([
                            ("id", Value::from("narrow")),
                            ("width", Value::from(120.0)),
                            ("height", Value::from(200.0)),
                        ]),
                    )
                    .expect("begin_child"),
            );
            *inside.borrow_mut() = rect_of(&probe.text(seq, &sentence));
            probe
                .frame_widget("end_child", seq, &no_arg())
                .expect("end_child");
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }

    let wide = Probe::new("wrap-wide");
    let outside = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let outside = outside.clone();
        let sentence = sentence.clone();
        run_frame(&wide, move |probe, seq| {
            *outside.borrow_mut() = rect_of(&probe.text(seq, &sentence));
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }

    let inside = *inside.borrow();
    let outside = *outside.borrow();
    assert!(
        height_of(inside) >= 3.0 * height_of(outside),
        "the child wraps the text: inside {inside:?} outside {outside:?}"
    );
    assert!(
        width_of(outside) > 300.0,
        "the full-width text is one line: {outside:?}"
    );
    let child = *child.borrow();
    let content_left = child.0 + 8.0;
    assert!(
        inside.2 - content_left <= 121.0,
        "wrapped lines stay within the child width: inside {inside:?} child {child:?}"
    );
}

#[test]
fn items_lay_out_in_submission_order() {
    let _guard = imgui_guard();
    let probe = Probe::new("order");

    let rects = Rc::new(RefCell::new(Vec::<(f32, f32, f32, f32)>::new()));
    let y = Rc::new(RefCell::new(Vec::<f32>::new()));
    {
        let (rects, y) = (rects.clone(), y.clone());
        run_frame(&probe, move |probe, seq| {
            for word in ["one", "two", "three"] {
                let value = probe.text(seq, word);
                rects.borrow_mut().push(rect_of(&value));
                y.borrow_mut().push(rect_of(&value).1);
            }
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let ys = y.borrow();
    assert!(ys[0] < ys[1] && ys[1] < ys[2], "top to bottom: {ys:?}");
    let r = rects.borrow();
    assert!(
        r[0].3 <= r[1].1 && r[1].3 <= r[2].1,
        "no vertical overlap: {r:?}"
    );

    let reverse = Probe::new("order-reverse");
    let seen = Rc::new(RefCell::new(Vec::<String>::new()));
    {
        let seen = seen.clone();
        run_frame(&reverse, move |probe, seq| {
            for word in ["three", "two", "one"] {
                probe.text(seq, word);
                seen.borrow_mut().push(word.to_string());
            }
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    assert_eq!(
        *seen.borrow(),
        vec!["three".to_string(), "two".to_string(), "one".to_string()],
        "the draw order is the call order"
    );
}

#[test]
fn layout_is_deterministic_across_frames() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();
    let probe = Probe::new("deterministic");
    let rects60 = Rc::new(RefCell::new(None::<Vec<(f32, f32, f32, f32)>>));
    let rects120 = Rc::new(RefCell::new(None::<Vec<(f32, f32, f32, f32)>>));

    for _ in 0..120 {
        let (r60, r120) = (rects60.clone(), rects120.clone());
        run_frame(&probe, move |probe, seq| {
            let mut rects = Vec::new();
            sheet(probe, seq, &mut rects);
            if seq == 60 {
                *r60.borrow_mut() = Some(rects.clone());
            }
            if seq == 120 {
                *r120.borrow_mut() = Some(rects);
            }
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a deterministic frame");
    }
    let frame60 = raster_upto(&gpu, &probe, 60);
    let frame120 = raster_upto(&gpu, &probe, 120);
    assert_eq!(
        frame60.pixels(),
        frame120.pixels(),
        "frame 60 and frame 120 render identically"
    );
    let r60 = rects60.borrow();
    let r120 = rects120.borrow();
    let (a, b) = (
        r60.as_ref().expect("frame 60 rects"),
        r120.as_ref().expect("frame 120 rects"),
    );
    assert_eq!(a.len(), b.len(), "the same items are drawn");
    for (one, two) in a.iter().zip(b.iter()) {
        assert!(
            (one.0 - two.0).abs() <= 0.5
                && (one.1 - two.1).abs() <= 0.5
                && (one.2 - two.2).abs() <= 0.5
                && (one.3 - two.3).abs() <= 0.5,
            "no drift: {one:?} vs {two:?}"
        );
    }
}

#[test]
fn a_hidden_second_window_does_not_shift_layout() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();

    let a = Probe::new("a");
    let b = Probe::new("b-hidden");
    b.shown.set(false);
    let rects_with = Rc::new(RefCell::new(None::<Vec<(f32, f32, f32, f32)>>));
    for _ in 0..30 {
        let rects = rects_with.clone();
        run_frame(&a, move |probe, seq| {
            let mut out = Vec::new();
            sheet(probe, seq, &mut out);
            *rects.borrow_mut() = Some(out);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a's frame with b hidden");
    }
    let snapshot_with = raster(&gpu, &a);

    let a2 = Probe::new("a2");
    let rects_without = Rc::new(RefCell::new(None::<Vec<(f32, f32, f32, f32)>>));
    for _ in 0..30 {
        let rects = rects_without.clone();
        run_frame(&a2, move |probe, seq| {
            let mut out = Vec::new();
            sheet(probe, seq, &mut out);
            *rects.borrow_mut() = Some(out);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a2's frame with no second window");
    }
    let snapshot_without = raster(&gpu, &a2);

    assert_eq!(
        snapshot_with.pixels(),
        snapshot_without.pixels(),
        "a hidden or absent second window does not change a's snapshot"
    );
    let with = rects_with.borrow();
    let without = rects_without.borrow();
    let (with, without) = (
        with.as_ref().expect("with rects"),
        without.as_ref().expect("without rects"),
    );
    assert_eq!(with.len(), without.len());
    for (one, two) in with.iter().zip(without.iter()) {
        assert!(
            (one.0 - two.0).abs() <= 0.5
                && (one.1 - two.1).abs() <= 0.5
                && (one.2 - two.2).abs() <= 0.5
                && (one.3 - two.3).abs() <= 0.5,
            "same item rects: {one:?} vs {two:?}"
        );
    }
}

#[test]
fn an_unclosed_child_region_is_reported_once() {
    let _guard = imgui_guard();
    let probe = Probe::new("unclosed-child");

    let end = run_frame(&probe, |probe, seq| {
        probe
            .frame_widget(
                "begin_child",
                seq,
                &Value::record([
                    ("id", Value::from("log")),
                    ("width", Value::from(240.0)),
                    ("height", Value::from(80.0)),
                ]),
            )
            .expect("begin_child");
        probe.text(seq, "inside");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("a frame");
    assert!(!end.painted, "the unbalanced frame is dropped: {end:?}");
    assert!(end.refused, "the unclosed child is a refusal: {end:?}");
    let error = end.error.as_deref().expect("an error");
    assert!(error.contains("begin_child"), "{error}");
    assert!(error.contains("end_child"), "{error}");
    assert!(
        probe.frame_state().report().last_error.as_deref() == Some(error),
        "the error is recorded: {end:?}"
    );

    let recovered = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    run_frame(&probe, {
        let recovered = recovered.clone();
        move |probe, seq| {
            let child = probe
                .frame_widget(
                    "begin_child",
                    seq,
                    &Value::record([
                        ("id", Value::from("log")),
                        ("width", Value::from(240.0)),
                        ("height", Value::from(80.0)),
                    ]),
                )
                .expect("begin_child");
            probe.text(seq, "inside");
            probe
                .frame_widget("end_child", seq, &no_arg())
                .expect("end_child");
            *recovered.borrow_mut() = rect_of(&child);
            let _ = probe.frame_script_end(seq, None, false);
        }
    })
    .expect("the recovery frame");

    let baseline = Probe::new("child-balanced");
    let benchmark = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    {
        let benchmark = benchmark.clone();
        run_frame(&baseline, move |probe, seq| {
            let child = probe
                .frame_widget(
                    "begin_child",
                    seq,
                    &Value::record([
                        ("id", Value::from("log")),
                        ("width", Value::from(240.0)),
                        ("height", Value::from(80.0)),
                    ]),
                )
                .expect("begin_child");
            probe.text(seq, "inside");
            probe
                .frame_widget("end_child", seq, &no_arg())
                .expect("end_child");
            *benchmark.borrow_mut() = rect_of(&child);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("a frame");
    }
    let recovered = *recovered.borrow();
    let benchmark = *benchmark.borrow();
    assert!(
        (recovered.0 - benchmark.0).abs() <= 0.5
            && (recovered.1 - benchmark.1).abs() <= 0.5
            && (recovered.2 - benchmark.2).abs() <= 0.5
            && (recovered.3 - benchmark.3).abs() <= 0.5,
        "the next frame lays out like a balanced run: {recovered:?} vs {benchmark:?}"
    );
}
fn point_of(value: &Value, field: &str) -> (f32, f32) {
    let point = value.field(field).expect("the point field");
    (
        match point.field("x") {
            Some(Value::Num(number)) => *number as f32,
            _ => panic!("{field}.x is not a number"),
        },
        match point.field("y") {
            Some(Value::Num(number)) => *number as f32,
            _ => panic!("{field}.y is not a number"),
        },
    )
}

fn draw_probe_table(probe: &Probe, seq: u64) -> (Value, Value) {
    let begin = probe
        .frame_widget(
            "begin_table",
            seq,
            &Value::record([
                ("id", Value::from("files")),
                ("columns", Value::from(3.0)),
                (
                    "flags",
                    Value::List(
                        ["borders", "row_bg", "resizable"]
                            .iter()
                            .map(|flag| Value::from(*flag))
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                ),
            ]),
        )
        .expect("begin_table");
    probe
        .frame_widget(
            "table_setup_column",
            seq,
            &Value::record([
                ("label", Value::from("Name")),
                ("width", Value::from(200.0)),
            ]),
        )
        .expect("column Name");
    probe
        .frame_widget(
            "table_setup_column",
            seq,
            &Value::record([
                ("label", Value::from("Size")),
                ("width", Value::from(90.0)),
            ]),
        )
        .expect("column Size");
    probe
        .frame_widget(
            "table_setup_column",
            seq,
            &Value::record([("label", Value::from("Modified"))]),
        )
        .expect("column Modified");
    let headers = probe
        .frame_widget("table_headers_row", seq, &no_arg())
        .expect("headers row");
    (begin, headers)
}

fn end_probe_table(probe: &Probe, seq: u64) {
    probe
        .frame_widget("end_table", seq, &no_arg())
        .expect("end_table");
}

#[test]
fn a_table_draws_headers_columns_rows_and_borders() {
    let _guard = imgui_guard();
    let gpu = headless_gpu();

    let empty = Probe::new("table-empty");
    run_frame(&empty, |probe, seq| {
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("empty frame");
    let background = raster(&gpu, &empty);

    let origin = Rc::new(RefCell::new((0.0_f32, 0.0_f32)));
    let probe = Probe::new("table");
    {
        let origin = origin.clone();
        run_frame(&probe, move |probe, seq| {
            let (begin, _headers) = draw_probe_table(probe, seq);
            *origin.borrow_mut() = point_of(&begin, "min");
            for row in 0..3 {
                probe
                    .frame_widget("table_next_row", seq, &no_arg())
                    .expect("next row");
                for column in 0..3 {
                    probe
                        .frame_widget("table_next_column", seq, &no_arg())
                        .expect("next column");
                    probe.text(seq, &format!("r{row}c{column}"));
                }
            }
            end_probe_table(probe, seq);
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("table frame");
    }
    let painted = raster(&gpu, &probe);

    let (x0, y0) = *origin.borrow();
    assert!(x0 > 0.0 && y0 > 0.0, "the table has an origin: {x0},{y0}");
    for border_x in [200.0_f32, 290.0_f32] {
        let x = (x0 + border_x).round() as u32;
        let mut ink = 0;
        for y in (y0 as u32 + 4)..(y0 as u32 + 90) {
            if let (Some(here), Some(was)) = (painted.pixel(x, y), background.pixel(x, y))
                && here
                    .iter()
                    .zip(was.iter())
                    .take(3)
                    .any(|(a, b)| a.abs_diff(*b) >= 16)
            {
                ink += 1;
            }
        }
        assert!(ink > 4, "the border at x={x} is missing");
    }
    let cells = [
        (x0 + 8.0, y0 + 26.0),
        (x0 + 208.0, y0 + 26.0),
        (x0 + 300.0, y0 + 26.0),
        (x0 + 8.0, y0 + 46.0),
    ];
    for (cell_x, cell_y) in cells {
        let area = window_rect((cell_x, cell_y, cell_x + 60.0, cell_y + 18.0));
        assert!(
            differing(&painted, &background, area) >= 4,
            "the cell at {cell_x},{cell_y} is empty"
        );
    }
}

#[test]
fn a_clicked_table_row_selectable_reports_once() {
    let _guard = imgui_guard();
    let probe = Probe::new("row-select");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let clicks = Rc::new(Cell::new(0));

    let draw = {
        let (rect, clicks) = (rect.clone(), clicks.clone());
        move |probe: &Probe, seq: u64| {
            probe
                .frame_widget(
                    "begin_table",
                    seq,
                    &Value::record([
                        ("id", Value::from("rows")),
                        ("columns", Value::from(2.0)),
                    ]),
                )
                .expect("begin_table");
            probe
                .frame_widget(
                    "table_setup_column",
                    seq,
                    &Value::record([("label", Value::from("name"))]),
                )
                .expect("column");
            probe
                .frame_widget(
                    "table_setup_column",
                    seq,
                    &Value::record([("label", Value::from("kind"))]),
                )
                .expect("column");
            probe
                .frame_widget("table_headers_row", seq, &no_arg())
                .expect("headers");
            probe
                .frame_widget("table_next_row", seq, &no_arg())
                .expect("row");
            probe
                .frame_widget("table_next_column", seq, &no_arg())
                .expect("column");
            let answer = probe
                .frame_widget(
                    "selectable",
                    seq,
                    &Value::record([
                        ("label", Value::from("row.two")),
                        ("selected", Value::Bool(true)),
                        ("span_all_columns", Value::Bool(true)),
                    ]),
                )
                .expect("selectable");
            *rect.borrow_mut() = rect_of(&answer);
            if matches!(answer.field("value"), Some(Value::Bool(true))) {
                clicks.set(clicks.get() + 1);
            }
            end_probe_table(probe, seq);
            let _ = probe.frame_script_end(seq, None, false);
        }
    };

    run_frame(&probe, draw.clone()).expect("frame 0");
    let (left, top, right, bottom) = *rect.borrow();
    let centre = [(left + right) / 2.0, (top + bottom) / 2.0];

    probe.edit_io(|io| io.add_mouse_pos_event(centre));
    run_frame(&probe, draw.clone()).expect("hover");
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("press");
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("release");
    run_frame(&probe, draw.clone()).expect("settle");

    assert_eq!(clicks.get(), 1, "one click is one selectable hit");
}

#[test]
fn a_menu_opens_on_click_and_its_item_fires_once() {
    let _guard = imgui_guard();
    let probe = Probe::new("menu");
    let file = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let item_rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let open = Rc::new(RefCell::new(false));
    let fired = Rc::new(Cell::new(0));

    let draw = {
        let (file, item_rect, open, fired) = (file.clone(), item_rect.clone(), open.clone(), fired.clone());
        move |probe: &Probe, seq: u64| {
            probe.frame_widget("begin_menu_bar", seq, &no_arg()).expect("menu bar");
            let menu = probe
                .frame_widget("begin_menu", seq, &Value::record([("label", Value::from("File"))]))
                .expect("File menu");
            *file.borrow_mut() = rect_of(&menu);
            let was_open = matches!(menu.field("open"), Some(Value::Bool(true)));
            *open.borrow_mut() = was_open;
            if was_open {
                let item = probe
                    .frame_widget("menu_item", seq, &Value::record([("label", Value::from("Open"))]))
                    .expect("menu item");
                *item_rect.borrow_mut() = rect_of(&item);
                if matches!(item.field("value"), Some(Value::Bool(true))) {
                    fired.set(fired.get() + 1);
                }
            }
            probe.frame_widget("end_menu", seq, &no_arg()).expect("end menu");
            probe.frame_widget("end_menu_bar", seq, &no_arg()).expect("end menu bar");
            let _ = probe.frame_script_end(seq, None, false);
        }
    };

    run_frame(&probe, draw.clone()).expect("frame 0");
    assert!(!*open.borrow(), "the menu starts closed");

    let (left, top, right, bottom) = *file.borrow();
    let centre = [(left + right) / 2.0, (top + bottom) / 2.0];
    probe.edit_io(|io| io.add_mouse_pos_event(centre));
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("press File");
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("release File");
    assert!(*open.borrow(), "the click opens the menu");

    let (item_left, item_top, item_right, item_bottom) = *item_rect.borrow();
    let item_centre = [
        (item_left + item_right) / 2.0,
        (item_top + item_bottom) / 2.0,
    ];
    probe.edit_io(|io| io.add_mouse_pos_event(item_centre));
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("press item");
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("release item");
    assert_eq!(fired.get(), 1, "the menu item fired exactly once");
}

#[test]
fn a_list_box_selects_the_clicked_row() {
    let _guard = imgui_guard();
    let probe = Probe::new("listbox");
    let chosen = Rc::new(RefCell::new(String::new()));
    let rows = ["alpha", "gamma", "delta"];
    let rects = Rc::new(RefCell::new(Vec::<(f32, f32, f32, f32)>::new()));

    let draw = {
        let (chosen, rects) = (chosen.clone(), rects.clone());
        move |probe: &Probe, seq: u64| {
            probe
                .frame_widget(
                    "begin_list_box",
                    seq,
                    &Value::record([
                        ("id", Value::from("##kinds")),
                        ("width", Value::from(180.0)),
                        ("height", Value::from(120.0)),
                    ]),
                )
                .expect("list box");
            let mut captured = Vec::new();
            for row in rows {
                let listed = chosen.borrow().clone();
                let answer = probe
                    .frame_widget(
                        "selectable",
                        seq,
                        &Value::record([
                            ("label", Value::from(row)),
                            ("selected", Value::Bool(listed == *row)),
                        ]),
                    )
                    .expect("selectable");
                captured.push(rect_of(&answer));
                if matches!(answer.field("value"), Some(Value::Bool(true))) {
                    *chosen.borrow_mut() = row.to_string();
                }
            }
            *rects.borrow_mut() = captured;
            probe.frame_widget("end_list_box", seq, &no_arg()).expect("end list box");
            let _ = probe.frame_script_end(seq, None, false);
        }
    };

    run_frame(&probe, draw.clone()).expect("frame 0");
    let third = rects.borrow()[2];
    let centre = [(third.0 + third.2) / 2.0, (third.1 + third.3) / 2.0];
    probe.edit_io(|io| io.add_mouse_pos_event(centre));
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("press");
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("release");
    assert_eq!(&*chosen.borrow(), "delta", "the third row became chosen");
}#[test]
fn a_context_menu_opens_on_right_click_and_not_left() {
    let _guard = imgui_guard();
    let right = Probe::new("ctx-right");
    let rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let open = Rc::new(RefCell::new(false));

    let draw = {
        let (rect, open) = (rect.clone(), open.clone());
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "selectable",
                    seq,
                    &Value::record([("label", Value::from("row-a")), ("selected", Value::Bool(false))]),
                )
                .expect("selectable");
            *rect.borrow_mut() = rect_of(&answer);
            let popup = probe
                .frame_widget("begin_popup_context_item", seq, &Value::record([("id", Value::from("ctx"))]))
                .expect("context popup");
            *open.borrow_mut() = matches!(popup.field("open"), Some(Value::Bool(true)));
            if matches!(popup.field("open"), Some(Value::Bool(true))) {
                probe.frame_widget("end_popup", seq, &no_arg()).expect("end popup");
            }
            let _ = probe.frame_script_end(seq, None, false);
        }
    };

    run_frame(&right, draw.clone()).expect("frame 0");
    assert!(!*open.borrow(), "the menu starts closed");

    let (left, top, right_edge, bottom) = *rect.borrow();
    let centre = [(left + right_edge) / 2.0, (top + bottom) / 2.0];
    right.edit_io(|io| io.add_mouse_pos_event(centre));
    right.edit_io(|io| io.add_mouse_button_event(MouseButton::Right, true));
    run_frame(&right, draw.clone()).expect("press");
    right.edit_io(|io| io.add_mouse_button_event(MouseButton::Right, false));
    run_frame(&right, draw.clone()).expect("release");
    assert!(*open.borrow(), "a right-click opens the context menu");

    let left = Probe::new("ctx-left");
    let open_left = Rc::new(RefCell::new(false));
    {
        let (_open, open_left) = (open.clone(), open_left.clone());
        let left = left.clone();
        run_frame(&left, move |probe, seq| {
            probe
                .frame_widget(
                    "selectable",
                    seq,
                    &Value::record([("label", Value::from("row-a")), ("selected", Value::Bool(false))]),
                )
                .expect("selectable");
            let popup = probe
                .frame_widget("begin_popup_context_item", seq, &Value::record([("id", Value::from("ctx"))]))
                .expect("context popup");
            *open_left.borrow_mut() = matches!(popup.field("open"), Some(Value::Bool(true)));
            if matches!(popup.field("open"), Some(Value::Bool(true))) {
                probe.frame_widget("end_popup", seq, &no_arg()).expect("end popup");
            }
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("left frame 0");
        let (l, t, r, b) = *rect.borrow();
        let centre = [(l + r) / 2.0, (t + b) / 2.0];
        left.edit_io(|io| io.add_mouse_pos_event(centre));
        left.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
        run_frame(&left, |probe, seq| {
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("left press");
        left.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
        run_frame(&left, |probe, seq| {
            let _ = probe.frame_script_end(seq, None, false);
        })
        .expect("left release");
    }
    assert!(!*open_left.borrow(), "a left-click must not open the menu");
}

#[test]
fn a_modal_popup_blocks_behind_and_reports_its_choice() {
    let _guard = imgui_guard();
    let probe = Probe::new("modal");
    let behind = Rc::new(Cell::new(0));
    let confirm = Rc::new(RefCell::new(None::<String>));
    let behind_rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let ok_rect = Rc::new(RefCell::new((0.0_f32, 0.0_f32, 0.0_f32, 0.0_f32)));
    let modal_open = Rc::new(Cell::new(true));

    let draw = {
        let (behind, confirm, behind_rect, ok_rect, modal_open) = (
            behind.clone(),
            confirm.clone(),
            behind_rect.clone(),
            ok_rect.clone(),
            modal_open.clone(),
        );
        move |probe: &Probe, seq: u64| {
            let button = probe
                .frame_widget("button", seq, &Value::record([("label", Value::from("Behind"))]))
                .expect("behind button");
            *behind_rect.borrow_mut() = rect_of(&button);
            if matches!(button.field("value"), Some(Value::Bool(true))) {
                behind.set(behind.get() + 1);
            }
            if modal_open.get() {
                probe
                    .frame_widget("open_popup", seq, &Value::record([("id", Value::from("confirm"))]))
                    .expect("open modal");
                let popup = probe
                    .frame_widget(
                        "begin_popup_modal",
                        seq,
                        &Value::record([
                            ("id", Value::from("confirm")),
                            ("options", Value::record([("always_auto_resize", Value::Bool(true))])),
                        ]),
                    )
                    .expect("modal");
                if matches!(popup.field("open"), Some(Value::Bool(true))) {
                    let ok = probe
                        .frame_widget("button", seq, &Value::record([("label", Value::from("OK"))]))
                        .expect("OK");
                    *ok_rect.borrow_mut() = rect_of(&ok);
                    if matches!(ok.field("value"), Some(Value::Bool(true))) {
                        *confirm.borrow_mut() = Some("ok".to_string());
                        modal_open.set(false);
                    }
                    let cancel = probe
                        .frame_widget("button", seq, &Value::record([("label", Value::from("Cancel"))]))
                        .expect("Cancel");
                    if matches!(cancel.field("value"), Some(Value::Bool(true))) {
                        *confirm.borrow_mut() = Some("cancel".to_string());
                        modal_open.set(false);
                    }
                    probe.frame_widget("end_popup", seq, &no_arg()).expect("end popup");
                }
            }
            let _ = probe.frame_script_end(seq, None, false);
        }
    };

    run_frame(&probe, draw.clone()).expect("frame 0");
    let (bl, bt, br, bb) = *behind_rect.borrow();
    let behind_centre = [(bl + br) / 2.0, (bt + bb) / 2.0];
    // Clicking Behind while the modal is up must not leak through.
    probe.edit_io(|io| io.add_mouse_pos_event(behind_centre));
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("press behind");
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("release behind");
    assert_eq!(behind.get(), 0, "the modal blocks the widget behind it");

    // Click OK: the modal records the choice, closes, and Behind works again.
    let (ol, ot, orr, ob) = *ok_rect.borrow();
    let ok_centre = [(ol + orr) / 2.0, (ot + ob) / 2.0];
    probe.edit_io(|io| io.add_mouse_pos_event(ok_centre));
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("press OK");
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("release OK");
    assert_eq!(confirm.borrow().as_deref(), Some("ok"), "OK reports the choice");
    assert!(!modal_open.get(), "the modal closed");

    run_frame(&probe, draw.clone()).expect("settle after close");
    run_frame(&probe, draw.clone()).expect("settle after close");

    probe.edit_io(|io| io.add_mouse_pos_event(behind_centre));
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, true));
    run_frame(&probe, draw.clone()).expect("press behind again");
    probe.edit_io(|io| io.add_mouse_button_event(MouseButton::Left, false));
    run_frame(&probe, draw.clone()).expect("release behind again");
    assert_eq!(behind.get(), 1, "Behind works once the modal is gone");
}

#[test]
fn a_shortcut_fires_only_its_chord() {
    let _guard = imgui_guard();
    let probe = Probe::new("shortcut");
    let hits = Rc::new(Cell::new(0));

    let draw = {
        let hits = hits.clone();
        move |probe: &Probe, seq: u64| {
            let answer = probe
                .frame_widget(
                    "shortcut",
                    seq,
                    &Value::record([
                        (
                            "mods",
                            Value::List(
                                ["ctrl"].iter().map(|m| Value::from(*m)).collect::<Vec<_>>().into(),
                            ),
                        ),
                        ("key", Value::from("K")),
                    ]),
                )
                .expect("shortcut");
            if matches!(answer.field("value"), Some(Value::Bool(true))) {
                hits.set(hits.get() + 1);
            }
            let _ = probe.frame_script_end(seq, None, false);
        }
    };

    probe.edit_io(|io| io.add_key_event(Key::ModCtrl, true));
    probe.edit_io(|io| io.add_key_event(Key::K, true));
    run_frame(&probe, draw.clone()).expect("ctrl+K");
    probe.edit_io(|io| io.add_key_event(Key::K, false));
    run_frame(&probe, draw.clone()).expect("release K");
    probe.edit_io(|io| io.add_key_event(Key::ModCtrl, false));
    run_frame(&probe, draw.clone()).expect("release ctrl");
    assert_eq!(hits.get(), 1, "ctrl+K fires the shortcut once");

    tap_key(&probe, Key::K);
    run_frame(&probe, draw.clone()).expect("plain K");
    assert_eq!(hits.get(), 1, "a plain K must not fire the chord");

    probe.edit_io(|io| io.add_key_event(Key::ModCtrl, true));
    probe.edit_io(|io| io.add_key_event(Key::K, true));
    run_frame(&probe, draw.clone()).expect("ctrl+K again");
    probe.edit_io(|io| io.add_key_event(Key::K, false));
    run_frame(&probe, draw.clone()).expect("release K again");
    probe.edit_io(|io| io.add_key_event(Key::ModCtrl, false));
    run_frame(&probe, draw.clone()).expect("release ctrl again");
    assert_eq!(hits.get(), 2, "a second chord fires once more");
}

#[test]
fn date_and_time_pickers_are_refused_naming_the_version() {
    let _guard = imgui_guard();
    let probe = Probe::new("refusals");
    run_frame(&probe, |probe, seq| {
        let date = probe
            .frame_widget("date_picker", seq, &Value::record([("id", Value::from("start"))]))
            .expect_err("date_picker must be refused");
        assert!(date.contains("date_picker"), "{date}");
        assert!(date.contains("1.93"), "{date}");
        let time = probe
            .frame_widget("time_picker", seq, &Value::record([("id", Value::from("start"))]))
            .expect_err("time_picker must be refused");
        assert!(time.contains("time_picker"), "{time}");
        assert!(time.contains("1.93"), "{time}");
        probe.text(seq, "still alive");
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the frame after both refusals still paints");
}

#[test]
fn a_colour_editor_returns_the_colour_it_was_given() {
    let _guard = imgui_guard();
    let probe = Probe::new("colour");
    let returned = Rc::new(RefCell::new((0.0_f64, 0.0_f64, 0.0_f64)));
    let captured = returned.clone();
    run_frame(&probe, move |probe, seq| {
        let answer = probe
            .frame_widget(
                "color_edit3",
                seq,
                &Value::record([
                    ("label", Value::from("Tint")),
                    (
                        "value",
                        Value::record([
                            ("r", Value::from(0.2)),
                            ("g", Value::from(0.4)),
                            ("b", Value::from(0.9)),
                        ]),
                    ),
                    (
                        "options",
                        Value::record([("display_mode", Value::from("rgb"))]),
                    ),
                ]),
            )
            .expect("color_edit3");
        let value = answer.field("value").expect("the colour table");
        *captured.borrow_mut() = (num_of(value, "r"), num_of(value, "g"), num_of(value, "b"));
        let probe_answered = probe
            .frame_widget(
                "color_button",
                seq,
                &Value::record([
                    ("id", Value::from("swatch")),
                    ("value", value.clone()),
                    ("width", Value::from(40.0)),
                    ("height", Value::from(40.0)),
                ]),
            )
            .expect("color_button");
        let _ = rect_of(&probe_answered);
        let _ = probe.frame_script_end(seq, None, false);
    })
    .expect("the colour frame");
    let (r, g, b) = *returned.borrow();
    assert!((r - 0.2).abs() < 0.01 && (g - 0.4).abs() < 0.01 && (b - 0.9).abs() < 0.01);
}