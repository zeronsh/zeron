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

use dear_imgui_rs::{Io, MouseButton};
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
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = WgpuRenderer::new(gpu.device.clone(), gpu.queue.clone(), format);
    let target = OffscreenTarget::new(&gpu.device, (720, 480), format);
    let delivered = probe.delivered.borrow();
    assert!(!delivered.is_empty(), "a delivered frame");
    let mut painted = None;
    for prepared in delivered.iter() {
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
                .any(|(a, b)| a.abs_diff(*b) >= 16)
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
