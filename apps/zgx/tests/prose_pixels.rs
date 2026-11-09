//! A reply's prose drawn by zgx's markdown view (`ui.markdown_view` with
//! ds/prose's style) against ds/markdown, pixel for pixel: the same text in
//! the same window, theme and width, drawn offscreen by zgx's renderer, at
//! rest and after every chunk of it streaming in. Any pixel apart fails,
//! naming the case, the chunk and where (`PROSE_PIXELS_OUT=<dir>` writes
//! both pictures and their difference for each).
//!
//! The fixture (ui/prose_pixels_fixture.luau) draws either renderer. A case
//! is a reply's text; another block kind's replies, or another scheme, are
//! a case or a `SCHEMES` entry more.
//!
//!     cargo test --test prose_pixels -- --nocapture

use std::path::PathBuf;
use std::sync::Arc;

use zgx_desktop::FontData;
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The window's size, in points, and its scale (a Retina display's).
const W: f32 = 760.;
const H: f32 = 900.;
/// The widths each case is drawn at besides `W`: a narrow pane, a mid one,
/// a wide window. Each wraps the words at its own places.
const WIDTHS: &[f32] = &[360., 560., 760., 900.];
const SCALE: f32 = 2.;

/// The schemes each case is drawn in.
const SCHEMES: &[&str] = &["dark", "light"];

/// Replies of paragraphs alone: each kind of run, and how they wrap.
const CASES: &[(&str, &str)] = &[
    (
        "plain",
        "The layout pass runs first and measures every row it shows, then paint fills the scene, then present hands the frame to the display.\n\n\
         A width change asks layout again for every line that wraps, so a long paragraph like this one takes a different number of lines at each width; \
         its words stay where they were otherwise.\nA soft break joins the lines.",
    ),
    (
        "emphasis",
        "Some **bold** words, some *italic* ones, ~~struck~~ text, ***bold and italic***, \
         **bold with *italic inside* it**, and _underscored emphasis_ next to __strong__.\n\n\
         ~~A whole struck sentence that runs long enough to wrap onto a second line of the reply, struck all the way.~~",
    ),
    (
        "code",
        "Call `parse(text)` then `render()`; a long `std::collections::HashMap<String, Vec<u32>>` wraps where it must. \
         Punctuation after code: `a`, `b`; `c`. `d`! (`e`) and `f`?\n\n\
         `A paragraph that starts with code` and ends with it: `end`",
    ),
    (
        "math",
        "The bound is $x^2$ and $O(n \\log n)$ for each part, the sum $\\sum_{k=1}^{n} k$; \
         it costs $5 and $10 a seat, or \\$3 with the discount.\n\n\
         Inline $a_{1}$, $b$, and `$not math$` stays code.",
    ),
    (
        "links",
        "A [link](https://example.com) and [the docs](https://zgx.zeron.sh/docs), bare https://example.com/path?q=1&r=2, \
         (see https://en.wikipedia.org/wiki/Rust_(programming_language)) and https://a.b/c. Then [mail](mailto:a@b.c) \
         and [a file](src/main.rs:12), [a **bold** link](https://x.y/z) and [`code link`](https://c.d).\n\n\
         Ends with a bare address: https://zeron.sh/download.",
    ),
    (
        "long",
        "A very long paragraph that keeps going well past the width of the reply so that it wraps several times, \
         with a long unbroken word like Pneumonoultramicroscopicsilicovolcanoconiosis and an address like \
         https://example.com/a/very/long/path/that/does/not/break/anywhere/at/all/because/it/has/no/spaces/in/it \
         and then **bold that wraps across the end of one line and onto the next one as it goes on** and \
         `code that wraps across a line end too, being long` and *italic running on at length past the edge of the column*.",
    ),
    (
        "mixed",
        "**Bold `code` inside**, *italic `code`*, ~~struck `code`~~ and **[bold link](https://b.c)**.\n\n\
         Second paragraph.\n\nThird, after the second.",
    ),
];

/// A renderer's app: the fixture with its text, scheme and key.
struct Shown {
    app: TestApp,
    /// The window's width, in points.
    w: f32,
    gpu: zgx_gpu::GpuRenderer,
    text: zgx_script::Shared,
}

fn fonts() -> Vec<FontData> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fonts");
    [
        "Geist.ttf",
        "Geist-Italic.ttf",
        "Geist-Medium.ttf",
        "Geist-SemiBold.ttf",
        "Geist-Bold.ttf",
        "GeistMono.ttf",
        "GeistMono-Medium.ttf",
        "InstrumentSerif-Regular.ttf",
        "InstrumentSerif-Italic.ttf",
    ]
    .iter()
    .map(|name| {
        let bytes = std::fs::read(dir.join(name)).expect("a font");
        FontData::new(Arc::<[u8]>::from(bytes)).expect("a font")
    })
    .collect()
}

impl Shown {
    fn new(renderer: &str, scheme: &str, key: &str, text: &str) -> Self {
        Self::at(W, renderer, scheme, key, text)
    }
    /// In a window `w` points wide.
    fn at(w: f32, renderer: &str, scheme: &str, key: &str, text: &str) -> Self {
        Self::made(w, renderer, scheme, key, text, &[])
    }
    /// With more of the fixture's shared values (`pixels.veil`, say).
    fn made(w: f32, renderer: &str, scheme: &str, key: &str, text: &str, more: &[(&str, Value)]) -> Self {
        let mut shared = None;
        let app = TestApp::with_desktop(|runtime| {
            let host = ScriptHost::new(runtime, ui_dir().join("prose_pixels_fixture.luau"));
            host.shared("pixels.renderer", Value::from(renderer));
            host.shared("pixels.theme", Value::from(scheme));
            host.shared("pixels.key", Value::from(key));
            for (name, value) in more {
                host.shared(name, value.clone());
            }
            shared = Some(host.shared("pixels.text", Value::from(text)));
            host
        });
        app.ui.scene.borrow_mut().resize(w, H);
        let mut gpu = zgx_gpu::GpuRenderer::new((w * SCALE) as u32, (H * SCALE) as u32).expect("a GPU");
        gpu.set_scale_factor(SCALE);
        for font in fonts() {
            font.install(&mut gpu.text_system().borrow_mut());
        }
        gpu.install_text(&mut app.ui.scene.borrow_mut());
        app.ui.refresh_text_geometry();
        app.frames(4);
        assert_eq!(app.error(), None, "{renderer}");
        Shown { app, w, gpu, text: shared.unwrap() }
    }

    fn set(&self, text: &str) {
        self.text.set(Value::from(text));
        self.app.frames(2);
    }

    /// The window's pixels now, RGBA, all of it drawn.
    fn pixels(&mut self) -> Vec<u8> {
        self.app.ui.prepare_frame();
        let _ = self.app.ui.scene.borrow_mut().flush();
        let all = [zgx::scene::Rect { x: 0., y: 0., width: self.w, height: H }];
        self.gpu.render(&self.app.ui.scene.borrow(), &all).expect("drawn");
        self.gpu.readback().expect("read back")
    }

    /// The reply's box's height, in points.
    fn height(&mut self) -> f32 {
        self.reply().height
    }
    /// The reply's box, in points.
    fn reply(&mut self) -> zgx::scene::Rect {
        self.app.ui.prepare_frame();
        let node = self.app.find("reply").expect("the reply");
        self.app.ui.scene.borrow().bounds(node)
    }

    /// The accessibility tree a screen reader is given, a node a line: its
    /// role, name, value and box (to the half point), in order, the window's
    /// own ids left out.
    fn accessible(&mut self) -> String {
        self.app.ui.prepare_frame();
        let _ = self.app.ui.scene.borrow_mut().flush();
        let mut tree = zgx_desktop::accessibility::AccessibilityTree::new();
        let update = tree.update(&self.app.ui.scene.borrow(), &self.app.ui.semantics.borrow(), None, "Zeron", SCALE as f64);
        let nodes: std::collections::HashMap<_, _> = update.nodes.iter().map(|(id, n)| (*id, n)).collect();
        let mut out = String::new();
        let mut stack = vec![(update.tree.as_ref().expect("a whole tree").root, 0)];
        let half = |v: f64| (v / SCALE as f64 * 2.).round() / 2.;
        while let Some((id, depth)) = stack.pop() {
            let Some(node) = nodes.get(&id) else { continue };
            let b = node.bounds().map(|b| (half(b.x0), half(b.y0), half(b.x1), half(b.y1)));
            out.push_str(&format!(
                "{}{:?} {:?} {:?} {:?}\n",
                "  ".repeat(depth),
                node.role(),
                node.label(),
                node.value(),
                b
            ));
            for child in node.children().iter().rev() {
                stack.push((*child, depth + 1));
            }
        }
        out
    }

    /// What a selection made by `how` holds, as Copy puts it on the
    /// clipboard: its text and its HTML (zgx's rich copy writes
    /// `selected_text` and `selected_html`).
    fn select(&mut self, how: Select) -> (String, String) {
        use zgx::input::{InputEvent, PointerButton};
        use zgx_script::driver;
        let r = self.reply();
        let at = |(fx, fy): (f32, f32)| (r.x + r.width * fx, r.y + fy);
        // (Away first: a click counts with the last.)
        driver::click_at(&mut self.app.ui, zgx::scene::Rect { x: 2., y: H - 4., width: 1., height: 1. });
        self.app.frames(2);
        match how {
            Select::Drag(from, to) => driver::drag_between(&mut self.app.ui, at(from), at(to)),
            Select::Clicks(p, n) => {
                let (x, y) = at(p);
                driver::hover_at(&mut self.app.ui, (x, y));
                let button = PointerButton::Primary;
                // (With the keys held, as a window's presses come: a press
                // without them counts as one alone.)
                let keys = zgx::input::Modifiers::default();
                for _ in 0..n {
                    self.app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, keys);
                    self.app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x, y, button }, keys);
                }
                self.app.ui.input.forget_clicks();
            }
        }
        self.app.settle();
        self.app.frames(2);
        (self.app.ui.selected_text(), self.app.ui.selected_html())
    }

    /// The scene's boxes, a line each: where two draws part, what differs.
    fn tree(&self) -> String {
        self.app.ui.prepare_frame();
        let scene = self.app.ui.scene.borrow();
        let mut out = String::new();
        let mut stack = vec![(scene.root(), 0)];
        while let Some((node, depth)) = stack.pop() {
            let b = scene.bounds(node);
            let text = match scene.kind(node) {
                zgx::scene::NodeKind::RichText { text } => format!(" {:?} {:?}", text.text(), text.runs()),
                _ => String::new(),
            };
            out.push_str(&format!(
                "{}{}{text} {:.2},{:.2} {:.2}x{:.2}\n",
                "  ".repeat(depth),
                zgx::inspect::kind_name(&scene, node),
                b.x,
                b.y,
                b.width,
                b.height
            ));
            for child in scene.children(node).iter().rev() {
                stack.push((*child, depth + 1));
            }
        }
        out
    }
}

/// A selection made with the pointer, at points across the reply (a
/// fraction of its width) and down it (points from its top).
#[derive(Clone, Copy, Debug)]
enum Select {
    Drag((f32, f32), (f32, f32)),
    /// `n` clicks at a point: 2 a word, 3 a paragraph.
    Clicks((f32, f32), usize),
}

/// How two pictures differ: pixels apart, the largest channel's difference,
/// and the box round them (in device pixels).
#[derive(Debug, Default, PartialEq)]
struct Diff {
    pixels: usize,
    max: u8,
    bbox: Option<(u32, u32, u32, u32)>,
}

fn diff(a: &[u8], b: &[u8], width: u32) -> Diff {
    assert_eq!(a.len(), b.len());
    let mut out = Diff::default();
    for (i, (pa, pb)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
        let most = pa.iter().zip(pb).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        if most == 0 {
            continue;
        }
        out.pixels += 1;
        out.max = out.max.max(most);
        let (x, y) = (i as u32 % width, i as u32 / width);
        out.bbox = Some(match out.bbox {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }
    out
}

/// Both pictures and their difference, as PNGs, where `PROSE_PIXELS_OUT`
/// says.
fn write_pictures(name: &str, luau: &[u8], native: &[u8], w: f32) {
    let Some(dir) = std::env::var_os("PROSE_PIXELS_OUT").map(PathBuf::from) else {
        return;
    };
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = ((w * SCALE) as u32, (H * SCALE) as u32);
    let marked: Vec<u8> = luau
        .chunks_exact(4)
        .zip(native.chunks_exact(4))
        .flat_map(|(a, b)| if a == b { [0, 0, 0, 255] } else { [255, 0, 255, 255] })
        .collect();
    for (suffix, data) in [("luau", luau), ("native", native), ("diff", &marked[..])] {
        let file = std::fs::File::create(dir.join(format!("{name}.{suffix}.png"))).unwrap();
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().unwrap().write_image_data(data).unwrap();
    }
}

/// The two renderers' pictures now, compared: `None` when they're the same.
/// (`PROSE_PIXELS_ALL=1` writes the pictures of those that are, too.)
fn compare(name: &str, luau: &mut Shown, native: &mut Shown) -> Option<String> {
    compared(name, luau, native, true)
}
/// `compare`; `drawn`: the page's words must show (not words all fading
/// in from clear, as they land).
fn compared(name: &str, luau: &mut Shown, native: &mut Shown, drawn: bool) -> Option<String> {
    let (a, b) = (luau.pixels(), native.pixels());
    // (Something drawn: the page's words, not its bare card.)
    let first = &a[..4];
    assert!(!drawn || a.chunks_exact(4).any(|p| p != first), "{name}: nothing drawn");
    assert_eq!(luau.w, native.w);
    let d = diff(&a, &b, (luau.w * SCALE) as u32);
    if d.pixels == 0 {
        if std::env::var_os("PROSE_PIXELS_ALL").is_some() {
            write_pictures(name, &a, &b, luau.w);
        }
        return None;
    }
    write_pictures(name, &a, &b, luau.w);
    Some(format!(
        "{name}: {} pixels apart, most {} a channel, in {:?} (device pixels); heights {} and {}\n\
         --- ds/markdown:\n{}--- markdown_view:\n{}",
        d.pixels,
        d.max,
        d.bbox,
        luau.height(),
        native.height(),
        luau.tree(),
        native.tree(),
    ))
}

/// The comparison sees what it's for: a run's look a step off (bold's
/// weight), or a word more, is pixels apart.
#[test]
fn a_look_a_step_off_is_pixels_apart() {
    let text = "Some **bold** words and `code`.";
    let mut luau = Shown::new("luau", "dark", "", text);
    let mut native = Shown::new("native", "dark", "", "Some **bold** words and `code`!");
    let d = diff(&luau.pixels(), &native.pixels(), (W * SCALE) as u32);
    assert!(d.pixels > 0 && d.max > 100, "a character apart: {d:?}");
    let mut off = Shown::new("native-bold-700", "dark", "", text);
    let d = diff(&luau.pixels(), &off.pixels(), (W * SCALE) as u32);
    assert!(d.pixels > 0, "bold at 700, not 600: {d:?}");
    native.set(text);
    assert_eq!(diff(&luau.pixels(), &native.pixels(), (W * SCALE) as u32), Diff::default());
}

#[test]
fn paragraphs_at_rest_are_drawn_as_ds_markdown_draws_them() {
    let mut failed = Vec::new();
    for (scheme, w) in SCHEMES.iter().flat_map(|s| WIDTHS.iter().map(move |w| (s, *w))) {
        for (name, text) in CASES {
            let mut luau = Shown::at(w, "luau", scheme, "", text);
            let mut native = Shown::at(w, "native", scheme, "", text);
            assert!(luau.height() > 20., "{name}: drawn");
            match compare(&format!("{name}-{scheme}-{w}"), &mut luau, &mut native) {
                Some(why) => failed.push(why),
                None => eprintln!("{name} ({scheme}, {w} pt): the same, every pixel"),
            }
            assert_eq!(native.app.error(), None, "{name}");
            assert_eq!(luau.app.error(), None, "{name}");
            // (No design lint the Luau renderer doesn't have: its style is
            // tokens where ds/markdown's are.)
            assert_eq!(native.app.host.warnings(), luau.app.host.warnings(), "{name}");
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// A link under the pointer, clicked and asked for its menu: the same
/// look, the same opening (ds/links: no opener in a test, so the system's),
/// the same menu.
#[test]
fn a_link_hovered_clicked_and_its_menu_are_ds_markdowns() {
    use zgx_script::driver;
    let text = CASES.iter().find(|(name, _)| *name == "links").unwrap().1;
    let mut luau = Shown::new("luau", "dark", "", text);
    let mut native = Shown::new("native", "dark", "", text);
    // ("link", the first line's second word.)
    let at = (58., 35.);
    let opened = |shown: &Shown| {
        (shown.app.host.states().into_iter())
            .find(|(name, _)| name.ends_with("::links_last"))
            .map(|(_, v)| v.to_string())
            .unwrap_or_default()
    };
    for shown in [&mut luau, &mut native] {
        driver::hover_at(&mut shown.app.ui, at);
        shown.app.frames(3);
    }
    assert_eq!(compare("link-hover", &mut luau, &mut native), None);
    for shown in [&mut luau, &mut native] {
        let b = zgx::scene::Rect { x: at.0 - 1., y: at.1 - 1., width: 2., height: 2. };
        driver::click_at(&mut shown.app.ui, b);
        shown.app.frames(3);
    }
    assert_eq!(opened(&luau), "system:https://example.com");
    assert_eq!(opened(&native), opened(&luau));
    for shown in [&mut luau, &mut native] {
        let before = shown.tree().lines().count();
        driver::right_click_at(&mut shown.app.ui, at);
        shown.app.frames(10);
        assert!(shown.tree().lines().count() > before + 3, "a menu opened");
    }
    assert_eq!(compare("link-menu", &mut luau, &mut native), None);
    assert_eq!(native.app.error(), None);
    assert_eq!(luau.app.error(), None);
}

/// A reply streaming in a chunk at a time: 24 bytes (about what a stream
/// brings), then pieces of 1 to 7, its markers left open as it goes.
#[test]
fn paragraphs_streaming_in_are_drawn_as_ds_markdown_draws_them_after_every_chunk() {
    let text = CASES.iter().map(|(_, t)| *t).collect::<Vec<_>>().join("\n\n");
    let mut failed = Vec::new();
    // (Dark and light at the reply's usual width, and a narrow pane.)
    let runs = SCHEMES.iter().map(|s| (*s, false, W)).chain([("dark", true, W), ("dark", false, 360.)]);
    for (scheme, small, w) in runs {
        let mut luau = Shown::at(w, "luau", scheme, "reply:pixels", "");
        let mut native = Shown::at(w, "native", scheme, "reply:pixels", "");
        let (mut end, mut chunks) = (0, 0);
        while end < text.len() {
            let step = if small { 1 + chunks % 7 } else { 24 };
            end = (end + step).min(text.len());
            while !text.is_char_boundary(end) {
                end += 1;
            }
            chunks += 1;
            luau.set(&text[..end]);
            native.set(&text[..end]);
            if let Some(why) = compare(&format!("stream-{scheme}-{small}-{w}-{end}"), &mut luau, &mut native) {
                let mut from = end.saturating_sub(24);
                while !text.is_char_boundary(from) {
                    from -= 1;
                }
                failed.push(format!("after {end} bytes ({:?}): {why}", &text[from..end]));
                // (The first few: one usually explains the rest.)
                if failed.len() >= 3 {
                    break;
                }
            }
        }
        assert_eq!(native.app.error(), None);
        assert_eq!(luau.app.error(), None);
        eprintln!(
            "streamed ({scheme}, {w} pt, {}): {chunks} chunks of {} bytes, the same every pixel after each",
            if small { "1 to 7 bytes a chunk" } else { "24 bytes a chunk" },
            text.len()
        );
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// Selecting with the pointer is as ds/markdown's words select: a drag
/// across runs, links, code and paragraphs, a word's double click and a
/// paragraph's triple click hold the same text, copy the same text and
/// HTML, and are drawn the same, at each width.
#[test]
fn selections_and_copies_are_ds_markdowns() {
    let text = CASES.iter().map(|(_, t)| *t).collect::<Vec<_>>().join("\n\n");
    let ways = [
        // From the first line into the second paragraph and beyond.
        Select::Drag((0.02, 6.), (0.6, 160.)),
        // Within a line, from inside one link to inside a later run.
        Select::Drag((0.1, 12.), (0.45, 12.)),
        // Up, against the text's order.
        Select::Drag((0.7, 300.), (0.2, 40.)),
        Select::Clicks((0.3, 12.), 2),
        Select::Clicks((0.5, 12.), 3),
        Select::Clicks((0.4, 200.), 2),
        Select::Clicks((0.4, 200.), 3),
    ];
    let mut failed = Vec::new();
    for w in WIDTHS {
        let mut luau = Shown::at(*w, "luau", "dark", "", &text);
        let mut native = Shown::at(*w, "native", "dark", "", &text);
        let mut selected = 0;
        for (i, how) in ways.iter().enumerate() {
            let a = luau.select(*how);
            let b = native.select(*how);
            selected += usize::from(!a.0.is_empty());
            if a != b {
                failed.push(format!("{how:?} at {w} pt:\n  ds/markdown {a:?}\n  markdown_view {b:?}"));
            }
            if let Some(why) = compare(&format!("select-{w}-{i}"), &mut luau, &mut native) {
                failed.push(format!("{how:?} at {w} pt, drawn: {}", why.lines().next().unwrap_or("")));
            }
        }
        assert_eq!(selected, ways.len(), "at {w}: each way selects something");
        eprintln!("selections at {w} pt: {} ways, the same text, HTML and pixels", ways.len());
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// What a screen reader is given is ds/markdown's: the same nodes, roles,
/// names, values (a paragraph's text, its links) and boxes.
#[test]
fn the_accessibility_tree_is_ds_markdowns() {
    let mut failed = Vec::new();
    for (name, text) in CASES {
        let mut luau = Shown::new("luau", "dark", "", text);
        let mut native = Shown::new("native", "dark", "", text);
        let (a, b) = (luau.accessible(), native.accessible());
        if *name == "links" {
            assert!(a.contains("Link"), "{name}: {a}");
        }
        if a != b {
            failed.push(format!("{name}:\n--- ds/markdown:\n{a}--- markdown_view:\n{b}"));
        } else {
            eprintln!("{name}: the same accessibility tree ({} nodes)", a.lines().count());
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
}

/// The streaming veil: a reply's new words fade in as ds/markdown fades
/// them, chunk by chunk (the same pieces, starts, lengths and boost, which
/// follow how fast chunks come). The clock is the test's, moved 25 ms a
/// chunk (40 a second) with a pause now and then, and each chunk drawn as
/// it lands and again partway into its fade: every pixel the same. Then a
/// reply opened mid-stream (`seed`): what was there shows at once.
#[test]
fn the_veil_fades_words_in_as_ds_markdowns() {
    let clock = zgx::clock::VirtualClock::new();
    // (`VEIL_CASES=code,links`: those cases alone; `VEIL_FAILURES=n`: go on
    // past the first three differences, to see them all.)
    let only: Option<Vec<String>> = std::env::var("VEIL_CASES").ok().map(|v| v.split(',').map(str::to_owned).collect());
    let text = CASES
        .iter()
        .filter(|(name, _)| only.as_ref().is_none_or(|o| o.iter().any(|n| n == name)))
        .map(|(_, t)| *t)
        .collect::<Vec<_>>()
        .join("\n\n");
    let veiled = [("pixels.veil", Value::from(true))];
    let mut luau = Shown::made(W, "luau", "dark", "reply:veil", "", &veiled);
    let mut native = Shown::made(W, "native", "dark", "reply:veil", "", &veiled);
    let mut failed = Vec::new();
    let (mut end, mut chunks, mut ms) = (0, 0u64, 0u64);
    let mut fading = 0;
    let most: usize = std::env::var("VEIL_FAILURES").ok().and_then(|v| v.parse().ok()).unwrap_or(3);
    while end < text.len() && failed.len() < most {
        end = (end + 24).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        chunks += 1;
        // 25 ms a chunk; every ninth after a pause, and a burst of quick ones.
        ms += match chunks % 9 {
            0 => 700,
            1..=3 => 5,
            _ => 25,
        };
        clock.advance(std::time::Duration::from_millis(ms), || {});
        luau.set(&text[..end]);
        native.set(&text[..end]);
        let landed = native.pixels();
        if let Some(why) = compared(&format!("veil-{end}"), &mut luau, &mut native, false) {
            failed.push(format!("after {end} bytes, as it lands: {why}"));
        }
        // Partway into its fade.
        ms += 11;
        clock.advance(std::time::Duration::from_millis(ms), || {});
        luau.app.frames(1);
        native.app.frames(1);
        fading += usize::from(native.pixels() != landed);
        if let Some(why) = compare(&format!("veil-{end}-later"), &mut luau, &mut native) {
            failed.push(format!("after {end} bytes, 11 ms on: {why}"));
        }
    }
    assert!(failed.is_empty(), "{}", failed.join("\n"));
    assert!(fading > (chunks as usize) / 2, "fading as it streamed: {fading} of {chunks}");
    eprintln!("veil: {chunks} chunks, each the same every pixel as it lands and 11 ms on");

    // Opened mid-reply: what's there as it's made shows at once, what
    // comes after fades.
    let half = &text[..text.len() / 2];
    let seeded = [("pixels.veil", Value::from(true)), ("pixels.seed", Value::from(true))];
    let mut luau = Shown::made(W, "luau", "dark", "reply:seeded", half, &seeded);
    let mut native = Shown::made(W, "native", "dark", "reply:seeded", half, &seeded);
    let shown = native.pixels();
    assert_eq!(compare("seeded", &mut luau, &mut native), None);
    ms += 500;
    clock.advance(std::time::Duration::from_millis(ms), || {});
    native.app.frames(1);
    assert!(native.pixels() == shown, "seeded: nothing faded in");
    let more = &text[..text.len() / 2 + 40];
    luau.set(more);
    native.set(more);
    let landed = native.pixels();
    assert_eq!(compare("seeded-more", &mut luau, &mut native), None);
    ms += 500;
    clock.advance(std::time::Duration::from_millis(ms), || {});
    native.app.frames(1);
    assert!(native.pixels() != landed, "what came after faded in");
    assert_eq!(native.app.error(), None);
    assert_eq!(luau.app.error(), None);
}
