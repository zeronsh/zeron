use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeron_text::WidthCache;

use super::layout::{self, SurfaceNode};
use super::paint::TextCtx;
use super::palette::Palette;
use super::*;

enum Event {
    Display(VisualDisplay),
    Frame(SurfaceFrame),
}

struct Collect(Mutex<mpsc::Sender<Event>>);

impl VisualListener for Collect {
    fn display(&self, display: VisualDisplay) {
        let _ = self.0.lock().unwrap().send(Event::Display(display));
    }
    fn frame(&self, frame: SurfaceFrame) {
        let _ = self.0.lock().unwrap().send(Event::Frame(frame));
    }
}

const PATH: &str = "uploads/v_1-visual.luau";

struct Open {
    _hub: Arc<VisualHub>,
    session: Arc<VisualSession>,
    slot: Arc<VisualSlot>,
    events: Receiver<Event>,
    remeasured: Receiver<u64>,
}

fn open(script: &str, width: f32) -> Open {
    let (changed_tx, remeasured) = mpsc::channel();
    let changed_tx = Mutex::new(changed_tx);
    let hub = Arc::new(VisualHub::new(
        crate::layout::tests::text_system(),
        Arc::new(move |key| {
            let _ = changed_tx.lock().unwrap().send(key);
        }),
    ));
    let files: HashMap<String, Vec<u8>> = [(PATH.to_owned(), script.as_bytes().to_vec())].into();
    hub.set_fetch(Arc::new(move |path: &str| files.get(path).cloned()), 2.0);
    let slot = VisualSlot::new("v_1", PATH, "Test");
    hub.ensure(&slot, 42, width, 1.0);
    let session = hub.session("v_1").expect("spawned");
    let (tx, events) = mpsc::channel();
    session.attach(Arc::new(Collect(Mutex::new(tx))), false, false);
    Open {
        _hub: hub,
        session,
        slot,
        events,
        remeasured,
    }
}

impl Open {
    /// The first display (and the frames before it) satisfying `want`.
    fn display_where(
        &self,
        want: impl Fn(&VisualDisplay) -> bool,
    ) -> (VisualDisplay, Vec<SurfaceFrame>) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut frames = Vec::new();
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left).expect("a display in time") {
                Event::Display(d) if want(&d) => {
                    // Frames follow their display (animated ones keep coming).
                    let until = Instant::now() + Duration::from_millis(300);
                    while let Ok(event) = self
                        .events
                        .recv_timeout(until.saturating_duration_since(Instant::now()))
                    {
                        if let Event::Frame(f) = event {
                            frames.push(f);
                        }
                    }
                    return (d, frames);
                }
                Event::Display(_) => {}
                Event::Frame(f) => frames.push(f),
            }
        }
    }

    fn ready(&self) -> (VisualDisplay, Vec<SurfaceFrame>) {
        self.display_where(|d| !d.paint.text.contains("Loading visual"))
    }

    fn frame_where(&self, want: impl Fn(&SurfaceFrame) -> bool) -> SurfaceFrame {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if let Event::Frame(f) = self.events.recv_timeout(left).expect("a frame in time")
                && want(&f)
            {
                return f;
            }
        }
    }
}

fn text_of(d: &VisualDisplay) -> String {
    let units: Vec<u16> = d.paint.text.encode_utf16().collect();
    d.paint
        .runs
        .iter()
        .map(|r| String::from_utf16_lossy(&units[r.start as usize..(r.start + r.len) as usize]))
        .collect::<Vec<_>>()
        .join("|")
}

fn lay(script: &str, width: f32) -> layout::Laid {
    let mut visual = Visual::load(script, 1).unwrap();
    let tree = visual.render().unwrap();
    let text = crate::layout::tests::text_system();
    let mut typo = crate::layout::typography(&text);
    let mut cache = WidthCache::new();
    let mut ctx = TextCtx {
        typo: &mut typo,
        cache: &mut cache,
    };
    layout::lay_out(&mut ctx, &Palette::new(true), &tree, width)
}

#[test]
fn rows_share_width_and_columns_stack_with_gaps() {
    let laid = lay(
        r#"return ui.column { gap = 10,
          ui.row { gap = 8,
            ui.stat { value = "182 ms", label = "p95" },
            ui.canvas { height = 60, draw = function(g) end },
          },
          ui.chart { type = "bar", height = 120, x = { "a", "b" }, series = { { name = "n", values = { 1, 2 } } } },
        }"#,
        300.0,
    );
    assert_eq!(laid.surfaces.len(), 2);
    let canvas = &laid.surfaces[0];
    let chart = &laid.surfaces[1];
    assert!(matches!(canvas.node, SurfaceNode::Canvas(_)));
    // The canvas fills what the stat leaves of the row.
    let [cx, cy, cw, ch] = canvas.rect;
    assert!(cx > 40.0 && cy == 0.0 && ch == 60.0, "{:?}", canvas.rect);
    assert!((cx + cw - 300.0).abs() < 0.5, "{:?}", canvas.rect);
    // The chart sits a gap below the row, full width, legend band on top.
    let [x, y, w, h] = chart.rect;
    assert_eq!((x, w), (0.0, 300.0));
    assert!((y - 70.0).abs() < 0.5, "{y}");
    assert!(h > 120.0);
    assert!((laid.height - (y + h)).abs() < 0.5);
}

#[test]
fn rows_center_shorter_children_and_justify_between() {
    let laid = lay(
        r#"return ui.row { align = "center", justify = "between",
          ui.button { label = "A", on_click = function() end },
          ui.canvas { height = 100, width = 50, draw = function(g) end },
        }"#,
        300.0,
    );
    let button = &laid.controls[0].control;
    assert_eq!(button.x, 0.0);
    // Centered on the 100-tall canvas.
    assert!(
        (button.y - (100.0 - layout::metrics::BUTTON_H) / 2.0).abs() < 0.5,
        "{}",
        button.y
    );
    assert!(
        (laid.surfaces[0].rect[0] - 250.0).abs() < 0.5,
        "{:?}",
        laid.surfaces[0].rect
    );
}

#[test]
fn wrapped_rows_and_grids_flow_onto_new_lines() {
    let laid = lay(
        r#"return ui.column {
          ui.row { wrap = true, gap = 4,
            ui.button { label = "First button", on_click = function() end },
            ui.button { label = "Second button", on_click = function() end },
            ui.button { label = "Third button", on_click = function() end },
          },
          ui.grid { columns = 2, gap = 6,
            ui.canvas { height = 40, draw = function(g) end },
            ui.canvas { height = 40, draw = function(g) end },
            ui.canvas { height = 40, draw = function(g) end },
          },
        }"#,
        220.0,
    );
    let ys: Vec<f32> = laid.controls.iter().map(|c| c.control.y).collect();
    assert!(ys[2] > ys[0], "{ys:?}");
    let cells: Vec<[f32; 4]> = laid.surfaces.iter().map(|s| s.rect).collect();
    assert_eq!(cells[0][1], cells[1][1]);
    assert!((cells[0][2] - 107.0).abs() < 0.5, "{cells:?}");
    assert!((cells[2][1] - cells[0][1] - 46.0).abs() < 0.5, "{cells:?}");
}

#[test]
fn tables_lay_cells_in_columns_under_a_header() {
    let laid = lay(
        r#"return ui.table { columns = { "Region", { label = "p95", align = "end" } }, rows = { { "us-east", 182 }, { "eu-west", 140 } } }"#,
        300.0,
    );
    let units: Vec<u16> = laid.paint.text.encode_utf16().collect();
    let run = |s: &str| {
        laid.paint
            .runs
            .iter()
            .find(|r| {
                String::from_utf16_lossy(&units[r.start as usize..(r.start + r.len) as usize]) == s
            })
            .unwrap_or_else(|| panic!("{s}"))
            .clone()
    };
    let (region, p95, east, value) = (run("Region"), run("p95"), run("us-east"), run("182"));
    assert_eq!(region.x, layout::metrics::TABLE_CELL_PAD);
    assert!(east.baseline > region.baseline);
    // The second column ends its cells at its right padding.
    let right = 300.0 - layout::metrics::TABLE_CELL_PAD;
    assert!((value.x + value.width - right).abs() < 0.5, "{value:?}");
    assert!((p95.x + p95.width - right).abs() < 0.5, "{p95:?}");
}

#[test]
fn a_visual_loads_measures_its_row_and_reacts_to_controls() {
    let open = open(
        r#"
        local s = ui.state { n = 0, on = false, v = 2 }
        return function()
          return ui.column { gap = 8,
            ui.text { "Count " .. s.n },
            s.on and ui.text "Shown" or ui.space(0),
            ui.button { label = "Add", on_click = function() s.n = s.n + 1 end },
            ui.toggle { label = "Show", value = s.on, on_change = function(v) s.on = v end },
            ui.slider { label = "Value", value = s.v, min = 0, max = 10, step = 1, on_change = function(v) s.v = v end },
          }
        end
        "#,
        320.0,
    );
    let (display, _) = open.ready();
    assert!(
        text_of(&display).contains("Count 0"),
        "{}",
        text_of(&display)
    );
    // The transcript heard the visual's height.
    assert_eq!(
        open.remeasured
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        42
    );
    assert_eq!(open.slot.height(), Some(display.height));

    let id = |kind: fn(&ControlKind) -> bool, d: &VisualDisplay| {
        d.controls.iter().find(|c| kind(&c.kind)).unwrap().id
    };
    open.session
        .activate(id(|k| matches!(k, ControlKind::Button { .. }), &display));
    let (display, _) = open.display_where(|d| text_of(d).contains("Count 1"));

    open.session
        .activate(id(|k| matches!(k, ControlKind::Toggle { .. }), &display));
    let (shown, _) = open.display_where(|d| text_of(d).contains("Shown"));
    assert!(shown.height > display.height, "a new line makes it taller");
    assert!(
        shown
            .controls
            .iter()
            .any(|c| c.kind == ControlKind::Toggle { on: true })
    );

    open.session.slide(
        id(|k| matches!(k, ControlKind::Slider { .. }), &shown),
        0.74,
    );
    open.display_where(|d| {
        d.controls
            .iter()
            .any(|c| matches!(&c.kind, ControlKind::Slider { value, .. } if *value == 7.0))
    });
}

#[test]
fn commands_in_one_batch_each_see_the_tree_the_last_one_left() {
    let open = open(
        r#"
        local s = ui.state { on = false }
        return function()
          return ui.toggle { label = "Flag", value = s.on, on_change = function(v) s.on = v end }
        end
        "#,
        200.0,
    );
    let (display, _) = open.ready();
    let toggle = display.controls[0].id;
    // A quick double tap: on, then off again (not on twice).
    open.session.activate(toggle);
    open.session.activate(toggle);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut last = None;
    while Instant::now() < deadline {
        match open.events.recv_timeout(Duration::from_millis(400)) {
            Ok(Event::Display(d)) => last = Some(d.controls[0].kind.clone()),
            Ok(Event::Frame(_)) => {}
            Err(_) if last.is_some() => break,
            Err(_) => {}
        }
    }
    assert_eq!(last, Some(ControlKind::Toggle { on: false }));
}

#[test]
fn a_tappable_stack_sits_under_the_controls_inside_it() {
    let laid = lay(
        r#"return ui.row { on_click = function() end,
          ui.text "Row",
          ui.button { label = "Go", on_click = function() end },
        }"#,
        300.0,
    );
    let kinds: Vec<&ControlKind> = laid.controls.iter().map(|c| &c.control.kind).collect();
    assert_eq!(
        kinds,
        [&ControlKind::Tap, &ControlKind::Button { primary: false }]
    );
}

#[test]
fn a_script_the_host_couldnt_send_loads_on_a_retry() {
    let tries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hub = Arc::new(VisualHub::new(
        crate::layout::tests::text_system(),
        Arc::new(|_| {}),
    ));
    let counter = tries.clone();
    hub.set_fetch(
        Arc::new(move |path: &str| {
            // Unreachable at first, then there.
            let first = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0;
            (!first && path == PATH).then(|| br#"return ui.text "Back""#.to_vec())
        }),
        2.0,
    );
    let slot = VisualSlot::new("v_1", PATH, "Test");
    hub.ensure(&slot, 1, 300.0, 1.0);
    let (tx, events) = mpsc::channel();
    hub.session("v_1")
        .unwrap()
        .attach(Arc::new(Collect(Mutex::new(tx))), false, false);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Event::Display(d) = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("a display")
            && text_of(&d) == "Back"
        {
            break;
        }
    }
}

#[test]
fn canvases_that_read_the_pointer_take_drags_and_get_touches() {
    let open = open(
        r#"
        return ui.canvas { height = 80, draw = function(g, w, h)
          local p = g.pointer
          if p then g:text(p.x, p.y, string.format("%d,%d", p.x, p.y)) end
        end }
        "#,
        200.0,
    );
    let (display, _) = open.display_where(|d| {
        d.surfaces
            .first()
            .is_some_and(|s| s.gesture == SurfaceGesture::Drag)
    });
    let surface = display.surfaces[0].id;
    open.session
        .pointer(surface, PointerPhase::Down, 30.0, 40.0);
    open.frame_where(|f| f.paint.text.contains("30,40"));
}

#[test]
fn charts_read_out_the_category_under_a_finger() {
    let open = open(
        r#"return ui.chart { type = "bar", height = 120, x = { "Mon", "Tue" },
             series = { { name = "a", values = { 3, 5 } }, { name = "b", values = { 1, 2 } } } }"#,
        300.0,
    );
    let (display, frames) = open.ready();
    let chart = &display.surfaces[0];
    assert_eq!(chart.gesture, SurfaceGesture::HorizontalDrag);
    assert!(
        frames
            .iter()
            .any(|f| f.surface == chart.id && !f.paint.shapes.is_empty())
    );
    // Over the right half of the plot: Tuesday, with both series' values.
    open.session
        .pointer(chart.id, PointerPhase::Down, chart.w * 0.8, chart.h * 0.6);
    let readout = open.frame_where(|f| f.paint.text.starts_with("Tue"));
    assert!(
        readout.paint.text.contains('5') && readout.paint.text.contains('2'),
        "{}",
        readout.paint.text
    );
    // Lifting the finger clears it.
    open.session
        .pointer(chart.id, PointerPhase::Up, chart.w * 0.8, chart.h * 0.6);
    open.frame_where(|f| !f.paint.text.starts_with("Tue"));
}

#[test]
fn scenes_and_shaders_render_premultiplied_frames_on_the_gpu() {
    if !zeron_visual_gpu::available() {
        return;
    }
    let open = open(
        r#"return ui.column { gap = 8,
             ui.scene { height = 120, draw = function(s) s:box { at = { 0, 0.5, 0 }, size = 1, color = "accent" } end },
             ui.shader { height = 60, code = [[
               void mainImage(out vec4 fragColor, in vec2 fragCoord) { fragColor = vec4(1.0, 0.5, 0.0, 0.5); }
             ]], blend = "alpha" },
           }"#,
        200.0,
    );
    let (display, frames) = open.ready();
    assert_eq!(display.surfaces.len(), 2);
    for surface in &display.surfaces {
        let frame = frames
            .iter()
            .rev()
            .find(|f| f.surface == surface.id)
            .expect("a frame per surface");
        let image = frame.image.as_ref().expect("an image");
        // Rendered at the pixel scale given (2×).
        assert_eq!(
            (image.width, image.height),
            ((surface.w * 2.0) as u32, (surface.h * 2.0) as u32)
        );
        // Premultiplied: no channel above its alpha.
        assert!(
            image
                .bgra
                .chunks_exact(4)
                .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
        );
        assert!(
            image.bgra.chunks_exact(4).any(|p| p[3] > 0),
            "something drew"
        );
    }
}

#[test]
fn a_missing_script_says_so() {
    let hub = Arc::new(VisualHub::new(
        crate::layout::tests::text_system(),
        Arc::new(|_| {}),
    ));
    hub.set_fetch(Arc::new(|_: &str| None), 2.0);
    let slot = VisualSlot::new("gone", "uploads/gone-visual.luau", "Gone");
    hub.ensure(&slot, 1, 300.0, 1.0);
    let (tx, events) = mpsc::channel();
    hub.session("gone")
        .unwrap()
        .attach(Arc::new(Collect(Mutex::new(tx))), false, false);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Event::Display(d) = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("a display")
            && text_of(&d) == "Visual unavailable"
        {
            break;
        }
    }
}

#[test]
fn sliders_land_on_steps_within_range() {
    assert_eq!(slider_value(0.0, 10.0, 1.0, 0.74), 7.0);
    assert_eq!(slider_value(0.0, 1.0, 0.25, 0.6), 0.5);
    assert_eq!(slider_value(5.0, 6.0, 0.0, 2.0), 6.0);
}

#[test]
fn frames_are_premultiplied_for_core_graphics() {
    let image = image_of(2, 1, vec![200, 100, 50, 128, 10, 20, 30, 255]);
    assert_eq!(image.bgra, vec![100, 50, 25, 128, 10, 20, 30, 255]);
}
