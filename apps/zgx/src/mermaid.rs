//! Mermaid diagrams for the scripts (`ds/mermaid`): the shipping app's
//! renderer (crates/ui/src/markdown/mermaid.rs, mermaid-rs-renderer 0.3.1)
//! in the shipping app's style, as vectors zgx draws sharp at any size.
//!
//!   mermaid.render { source, dark, font, colors = { background, code,
//!                    text, secondary, tertiary, border, strong, accent } }
//!
//! answers `{ svg, plate_svg, plate, w, h }`: `svg` the diagram on a clear
//! ground (it sits on its code block's fill), `plate_svg` the same on that
//! fill (the lightbox's, over its scrim), both SVG documents with their
//! text as outlines (zgx's own SVG drawing has no fonts), for `ui.svg`'s
//! `src = "code:…"`, which rasterizes them at the size and pixel scale they
//! show at; `plate` the plate as a PNG file at twice the diagram's size
//! (less for a large one), for the lightbox's size, Copy and Reveal; `w` by
//! `h` the diagram's own size in points. A diagram that can't be drawn
//! rejects with a short reason. The colours are the
//! theme's tokens as `ui.theme.get` gives them (`#rrggbbaa`), so it follows
//! the appearance and any template's palette.
//!
//! The work goes on a thread of its own, one diagram at a time (the
//! renderer can't be stopped part way); the pictures are kept on disk by a
//! hash of the source and its style, so a diagram seen before (this run or
//! an earlier one) answers at once, and failures are kept for this run.
//! Text is drawn with this machine's fonts, the ones the renderer measured
//! it with.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use resvg::tiny_skia;
use sha2::{Digest, Sha256};
use zgx_script::{ScriptHost, Value};

pub const ENGINE_VERSION: &str = "mermaid-rs-renderer/0.3.1";
/// Bumped when what is drawn for the same request changes.
const STYLE_VERSION: u32 = 2;
pub const MAX_SOURCE_BYTES: usize = 16 * 1024;
/// The pictures' pixel density over the diagram's own size.
const DENSITY: f32 = 2.0;
/// A picture's largest side and area, in pixels: past them it is drawn at a
/// lower density.
const MAX_SIDE: f32 = 8192.0;
const MAX_PIXELS: f32 = 12.0 * 1024.0 * 1024.0;
/// The largest outlined SVG handed over (zgx draws up to 4 MiB of one).
const MAX_SVG_BYTES: usize = 3 * 1024 * 1024;
/// The layers drawing a diagram may take, in square points. The renderer
/// doesn't escape every value it writes, so a `style` or `classDef` in an
/// agent's source can add elements of its own: nested opacity groups or
/// filters ran to gigabytes and minutes from 16 KB. A layer's cost shows in
/// usvg's tree whatever wrote it. Real diagrams stay under 150 thousand here.
const MAX_LAYERS_AREA: f32 = 16.0 * 1024.0 * 1024.0;
/// A filter pass's cost over a layer's (a blur's, measured).
const FILTER_COST: f32 = 10.0;
/// Pictures kept on disk; past this the oldest go.
const KEEP_FILES: usize = 600;
/// Failures remembered this run.
const KEEP_FAILURES: usize = 256;

// This backend has no cooperative cancellation. Bound inputs and serialize
// its CPU work across diagrams (the shipping app's RENDER_LOCK).
static RENDER_LOCK: Mutex<()> = Mutex::new(());

pub fn install(host: &ScriptHost) {
    host.async_action("mermaid.render", |arg| async move {
        let request = Request::from_value(&arg)?;
        let dir = crate::engine::data_dir().map(|d| d.join("mermaid")).unwrap_or_else(cache_dir);
        let drawn = off_thread(move || draw_cached(&request, &dir)).await.ok_or("Diagram could not be rendered")??;
        Ok(Value::record([
            ("svg", Value::from(drawn.svg.as_str())),
            ("plate_svg", Value::from(drawn.plate_svg.as_str())),
            ("plate", Value::from(drawn.plate.to_string_lossy().as_ref())),
            ("w", Value::from(drawn.w as f64)),
            ("h", Value::from(drawn.h as f64)),
        ]))
    });
}

/// Where diagrams are kept without an engine's data dir (a fixture's run):
/// the user's cache.
fn cache_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cache = if cfg!(target_os = "macos") {
        home.map(|h| h.join("Library/Caches"))
    } else {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| home.map(|h| h.join(".cache")))
    };
    cache.unwrap_or_else(std::env::temp_dir).join("zeron-zgx/mermaid")
}

/// `work` on a thread of its own, its answer awaited here (None only if it
/// panicked), as images.rs's.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, mut rx) = zgx::task::latest();
    std::thread::spawn(move || {
        tx.send(work());
    });
    rx.recv().await
}

/// A colour, opaque or not.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rgba([f32; 4]);

impl Rgba {
    const BLACK: Rgba = Rgba([0., 0., 0., 1.]);
    const WHITE: Rgba = Rgba([1., 1., 1., 1.]);

    /// `#rgb`, `#rrggbb` or `#rrggbbaa`.
    fn parse(text: &str) -> Option<Rgba> {
        let hex = text.trim().strip_prefix('#')?;
        let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok().map(|b| b as f32 / 255.);
        match hex.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok().map(|b| (b * 17) as f32 / 255.);
                Some(Rgba([n(0)?, n(1)?, n(2)?, 1.]))
            }
            6 => Some(Rgba([byte(0)?, byte(2)?, byte(4)?, 1.])),
            8 => Some(Rgba([byte(0)?, byte(2)?, byte(4)?, byte(6)?])),
            _ => None,
        }
    }

    fn opacity(self, a: f32) -> Rgba {
        Rgba([self.0[0], self.0[1], self.0[2], self.0[3] * a])
    }

    /// This over an opaque `under`: opaque.
    fn over(self, under: Rgba) -> Rgba {
        let a = self.0[3];
        let mix = |i: usize| self.0[i] * a + under.0[i] * (1. - a);
        Rgba([mix(0), mix(1), mix(2), 1.])
    }

    fn hex(self) -> String {
        let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
        format!("#{:02x}{:02x}{:02x}", b(self.0[0]), b(self.0[1]), b(self.0[2]))
    }
}

/// The theme's colours a diagram is drawn in (from the scripts' tokens).
#[derive(Clone, Debug)]
pub struct Request {
    source: String,
    dark: bool,
    font: String,
    background: Rgba,
    code: Rgba,
    text: Rgba,
    secondary: Rgba,
    tertiary: Rgba,
    border: Rgba,
    strong: Rgba,
    accent: Rgba,
}

impl Request {
    fn from_value(arg: &Value) -> Result<Request, String> {
        let text = |v: Option<&Value>| v.map(crate::auth::text).unwrap_or_default();
        let source = text(arg.field("source"));
        let dark = arg.field("dark").is_some_and(Value::truthy);
        let colors = arg.field("colors");
        let color = |name: &str, fallback: Rgba| {
            Rgba::parse(&text(colors.and_then(|c| c.field(name)))).unwrap_or(fallback)
        };
        let (ink, ground) = if dark { (Rgba::WHITE, Rgba::BLACK) } else { (Rgba::BLACK, Rgba::WHITE) };
        let text_color = color("text", ink.opacity(0.92));
        Ok(Request {
            dark,
            font: text(arg.field("font")),
            background: color("background", ground),
            code: color("code", text_color.opacity(0.04)),
            text: text_color,
            secondary: color("secondary", text_color.opacity(0.6)),
            tertiary: color("tertiary", text_color.opacity(0.42)),
            border: color("border", ink.opacity(0.08)),
            strong: color("strong", ink.opacity(0.18)),
            accent: color("accent", Rgba([0.57, 0.47, 0.69, 1.])),
            source,
        })
    }

    /// Everything that decides the pictures, as a file name.
    fn key(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(format!("{ENGINE_VERSION}/{STYLE_VERSION}/{DENSITY}\0"));
        hash.update(self.source.as_bytes());
        hash.update(format!(
            "\0{}\0{}\0{:?}{:?}{:?}{:?}{:?}{:?}{:?}{:?}",
            self.dark, self.font, self.background, self.code, self.text, self.secondary, self.tertiary, self.border, self.strong, self.accent
        ));
        hash.finalize().iter().take(16).map(|b| format!("{b:02x}")).collect()
    }
}

/// Zeron's diagram style (the shipping app's `Palette`): the canvas left
/// clear so the diagram sits on its fence body; `canvas` is that body's
/// opaque approximation, used where the engine needs a solid mask (edge
/// label pills, hollow markers) and as the lightbox's plate.
#[derive(Clone, Debug)]
pub struct Palette {
    dark: bool,
    font: String,
    canvas: String,
    node: String,
    group: String,
    text: String,
    label: String,
    line: String,
    border: String,
    grid: String,
    accent_line: String,
    accent_wash: String,
    plate: Rgba,
}

impl Palette {
    pub fn from_request(r: &Request) -> Palette {
        // The window's ground (glass, a scrim over the desktop) as an opaque
        // colour, and the fence body (an ink wash) over it.
        let ground = r.background.over(if r.dark { Rgba::BLACK } else { Rgba::WHITE });
        let canvas = r.code.over(ground);
        // Nodes are cards lifted off the fence: the window's own ground in
        // light, one ink step up in dark, where the fence is the deepest.
        let node = if r.dark { r.text.opacity(0.06).over(canvas) } else { ground };
        Palette {
            dark: r.dark,
            font: font_family(&r.font),
            canvas: canvas.hex(),
            node: node.hex(),
            group: r.text.opacity(0.03).over(canvas).hex(),
            text: r.text.over(node).hex(),
            label: r.secondary.over(canvas).hex(),
            line: r.tertiary.over(canvas).hex(),
            border: r.strong.over(canvas).hex(),
            grid: r.border.over(canvas).hex(),
            accent_line: r.accent.opacity(0.6).over(canvas).hex(),
            accent_wash: r.accent.opacity(0.12).over(node).hex(),
            plate: canvas,
        }
    }
}

/// This machine's fonts: what the renderer measures text with (it loads
/// the system's), so what is drawn fits the boxes it laid out.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// The theme's sans when this machine has it (the renderer measures with
/// the system's fonts), else common sans faces, in the order they're
/// looked for.
fn font_family(theme: &str) -> String {
    // (A family the user chose is its token quoted: ds/look's font_theme.)
    let theme = theme.trim().trim_matches('"');
    let fallbacks = "Helvetica Neue, Helvetica, Arial, Liberation Sans, DejaVu Sans, Noto Sans, sans-serif";
    let installed = !theme.is_empty()
        && fonts().faces().any(|face| face.families.iter().any(|(name, _)| name.eq_ignore_ascii_case(theme)));
    if installed { format!("{theme}, {fallbacks}") } else { fallbacks.to_string() }
}

/// What a request drew: the diagram as vectors, clear and on its plate,
/// the plate as pixels, and the diagram's own size.
#[derive(Clone, Debug, PartialEq)]
pub struct Drawn {
    svg: String,
    plate_svg: String,
    plate: PathBuf,
    w: f32,
    h: f32,
}

fn failures() -> &'static Mutex<(HashMap<String, String>, VecDeque<String>)> {
    static FAILED: OnceLock<Mutex<(HashMap<String, String>, VecDeque<String>)>> = OnceLock::new();
    FAILED.get_or_init(Default::default)
}

/// The pictures for `request`, from `dir` when drawn before.
fn draw_cached(request: &Request, dir: &Path) -> Result<Drawn, String> {
    let key = request.key();
    if let Some(why) = failures().lock().unwrap_or_else(|e| e.into_inner()).0.get(&key) {
        return Err(why.clone());
    }
    let files = Files::of(dir, &key);
    if let Some(drawn) = files.kept() {
        return Ok(drawn);
    }
    let _guard = RENDER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // (Drawn by another request for the same diagram while this one waited.)
    if let Some(drawn) = files.kept() {
        return Ok(drawn);
    }
    let palette = Palette::from_request(request);
    let result = render(&request.source, &palette).and_then(|svg| draw(&svg, &palette)).and_then(|pictures| {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        write_atomic(&files.svg, pictures.svg.as_bytes())?;
        write_atomic(&files.plate_svg, pictures.plate_svg.as_bytes())?;
        write_png(&files.plate, &pictures.plate)?;
        write_atomic(&files.size, format!("{} {}", pictures.w, pictures.h).as_bytes())?;
        Ok(Drawn { svg: pictures.svg, plate_svg: pictures.plate_svg, plate: files.plate.clone(), w: pictures.w, h: pictures.h })
    });
    match &result {
        Ok(_) => prune(dir),
        Err(why) => {
            let mut failed = failures().lock().unwrap_or_else(|e| e.into_inner());
            failed.0.insert(key.clone(), why.clone());
            failed.1.push_back(key);
            while failed.1.len() > KEEP_FAILURES {
                if let Some(old) = failed.1.pop_front() {
                    failed.0.remove(&old);
                }
            }
        }
    }
    result
}

/// A key's files: its vectors, its plate's pixels, and its size (written
/// last, so a key with a size has the rest).
struct Files {
    svg: PathBuf,
    plate_svg: PathBuf,
    plate: PathBuf,
    size: PathBuf,
}

impl Files {
    fn of(dir: &Path, key: &str) -> Files {
        Files {
            svg: dir.join(format!("{key}.svg")),
            plate_svg: dir.join(format!("{key}.plate.svg")),
            plate: dir.join(format!("{key}.plate.png")),
            size: dir.join(format!("{key}.size")),
        }
    }

    /// What was drawn for the key before, if all of it is there.
    fn kept(&self) -> Option<Drawn> {
        let text = std::fs::read_to_string(&self.size).ok()?;
        let mut parts = text.split_whitespace().map(|n| n.parse::<f32>().ok());
        let (w, h) = (parts.next()??, parts.next()??);
        if !(w > 0. && h > 0. && self.plate.is_file()) {
            return None;
        }
        let svg = std::fs::read_to_string(&self.svg).ok()?;
        let plate_svg = std::fs::read_to_string(&self.plate_svg).ok()?;
        Some(Drawn { svg, plate_svg, plate: self.plate.clone(), w, h })
    }
}

/// Only the newest KEEP_FILES diagrams stay.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut sizes: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "size"))
        .map(|e| (e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), e.path()))
        .collect();
    if sizes.len() <= KEEP_FILES {
        return;
    }
    sizes.sort();
    for (_, size) in &sizes[..sizes.len() - KEEP_FILES] {
        let key = size.with_extension("");
        for path in [size.clone(), key.with_extension("svg"), key.with_extension("plate.svg"), key.with_extension("plate.png")] {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let part = path.with_extension(format!("{}.part", std::process::id()));
    std::fs::write(&part, bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&part, path).map_err(|e| e.to_string())
}

/// A pixmap (premultiplied) as a PNG file (straight alpha).
fn write_png(path: &Path, pixmap: &tiny_skia::Pixmap) -> Result<(), String> {
    let mut rgba = Vec::with_capacity(pixmap.data().len());
    for p in pixmap.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, pixmap.width(), pixmap.height());
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
        writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    }
    write_atomic(path, &bytes)
}

/// How the engine's SVG is read: this machine's fonts, and no `<image>`
/// resolved, neither a path or URL (a file on this machine read into the
/// picture) nor inline data. The source is an agent's, so untrusted; the
/// shipping app's `svg_options` refuses them the same way. (zgx's own SVG
/// drawing, which draws what this writes, resolves no paths either, and an
/// image dropped here isn't written out.)
fn svg_options(font: &str) -> usvg::Options<'static> {
    usvg::Options {
        font_family: font.to_string(),
        fontdb: fonts(),
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    }
}

/// A diagram drawn: its vectors, clear and on the plate, the plate's
/// pixels, and its own size in points.
struct Pictures {
    svg: String,
    plate_svg: String,
    plate: tiny_skia::Pixmap,
    w: f32,
    h: f32,
}

/// The engine's SVG with its text set in this machine's fonts: as SVG again
/// with the text as outlines, clear and on the plate, and the plate drawn
/// at DENSITY (less for a large diagram).
fn draw(svg: &str, palette: &Palette) -> Result<Pictures, String> {
    let tree = usvg::Tree::from_str(svg, &svg_options(&palette.font)).map_err(|e| e.to_string())?;
    let (w, h) = (tree.size().width(), tree.size().height());
    if !(w >= 1. && h >= 1.) {
        return Err("Diagram is empty".into());
    }
    if layers_area(tree.root()) > MAX_LAYERS_AREA {
        return Err("Diagram exceeds preview complexity limit".into());
    }
    // (Text written as the paths it was laid out to: `preserve_text` off.)
    let outlined = tree.to_string(&usvg::WriteOptions::default());
    let plate_svg = on_plate(&outlined, w, h, &palette.plate.hex());
    if plate_svg.len() > MAX_SVG_BYTES {
        return Err("Diagram output exceeds preview size limit".into());
    }
    let density = DENSITY.min(MAX_SIDE / w.max(h)).min((MAX_PIXELS / (w * h)).sqrt());
    if density < 0.5 {
        return Err("Diagram exceeds preview size limit".into());
    }
    let (pw, ph) = ((w * density).ceil() as u32, (h * density).ceil() as u32);
    let mut clear = tiny_skia::Pixmap::new(pw, ph).ok_or("Diagram exceeds preview size limit")?;
    resvg::render(&tree, tiny_skia::Transform::from_scale(density, density), &mut clear.as_mut());
    let mut plate = tiny_skia::Pixmap::new(pw, ph).ok_or("Diagram exceeds preview size limit")?;
    let [r, g, b, a] = palette.plate.0;
    plate.fill(tiny_skia::Color::from_rgba(r, g, b, a).unwrap_or(tiny_skia::Color::BLACK));
    plate.draw_pixmap(0, 0, clear.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    Ok(Pictures { svg: outlined, plate_svg, plate, w, h })
}

/// The area of the layers drawing `group` takes, in square points: resvg
/// allocates and composites one for each isolated group and each pattern's
/// tile, here and in `ui.svg`, which draws on the frame's path.
fn layers_area(group: &usvg::Group) -> f32 {
    let mut area = 0.;
    if group.should_isolate() {
        let bbox = group.abs_layer_bounding_box();
        let passes: usize = group.filters().iter().map(|f| f.primitives().len()).sum();
        area += bbox.width() * bbox.height() * (1. + FILTER_COST * passes as f32);
    }
    for node in group.children() {
        match node {
            usvg::Node::Group(child) => area += layers_area(child),
            usvg::Node::Path(path) => {
                for paint in [path.fill().map(|f| f.paint()), path.stroke().map(|s| s.paint())].into_iter().flatten() {
                    if let usvg::Paint::Pattern(pattern) = paint {
                        area += pattern.rect().width() * pattern.rect().height();
                    }
                }
            }
            usvg::Node::Text(text) => area += layers_area(text.flattened()),
            usvg::Node::Image(_) => {}
        }
        node.subroots(|root| area += layers_area(root));
    }
    area
}

/// `svg` with a `w` by `h` ground of `fill` under everything it draws.
fn on_plate(svg: &str, w: f32, h: f32, fill: &str) -> String {
    let ground = format!("<rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" fill=\"{fill}\"/>");
    match svg.find("<svg").and_then(|at| svg[at..].find('>').map(|end| at + end + 1)) {
        Some(at) => format!("{}{ground}{}", &svg[..at], &svg[at..]),
        None => svg.to_string(),
    }
}

/// The shipping app's `mermaid::render`: the source as SVG in Zeron's style.
pub fn render(source: &str, palette: &Palette) -> Result<String, String> {
    if source.trim().is_empty() {
        return Err("Diagram is empty".into());
    }
    if source.len() > MAX_SOURCE_BYTES
        || source.lines().count() > 256
        || source.split(|c: char| c.is_whitespace() || matches!(c, ';' | '>' | '{' | '}')).count() > 2048
    {
        return Err("Diagram exceeds preview complexity limit".into());
    }
    std::panic::catch_unwind(|| {
        let mut options = mermaid_rs_renderer::RenderOptions::default();
        options.theme = if palette.dark { mermaid_rs_renderer::Theme::dark() } else { mermaid_rs_renderer::Theme::modern() };
        // A denser layout than the engine's default: diagrams are scaled to
        // the reading column, so slack spacing shrinks the labels.
        options.layout.node_spacing = 36.0;
        options.layout.rank_spacing = 40.0;
        options.layout.node_padding_x = 18.0;
        options.layout.node_padding_y = 10.0;
        let theme = &mut options.theme;
        theme.font_family = palette.font.clone();
        theme.font_size = 14.0;
        theme.background = palette.canvas.clone();
        theme.primary_color = palette.node.clone();
        theme.primary_text_color = palette.text.clone();
        theme.primary_border_color = palette.border.clone();
        theme.text_color = palette.label.clone();
        theme.line_color = palette.line.clone();
        theme.secondary_color = palette.node.clone();
        theme.tertiary_color = palette.group.clone();
        theme.edge_label_background = palette.canvas.clone();
        theme.cluster_background = palette.group.clone();
        theme.cluster_border = palette.border.clone();
        theme.sequence_actor_fill = palette.node.clone();
        theme.sequence_actor_border = palette.border.clone();
        theme.sequence_actor_line = palette.border.clone();
        theme.sequence_note_fill = palette.accent_wash.clone();
        theme.sequence_note_border = palette.accent_line.clone();
        theme.sequence_activation_fill = palette.accent_wash.clone();
        theme.sequence_activation_border = palette.accent_line.clone();
        if diagram_keyword(source) == Some("gantt") {
            // Gantt derives its bar hues from this color; a neutral gray
            // would turn every section red. The softened accent keeps bar
            // labels legible in both appearances.
            theme.primary_border_color = palette.accent_line.clone();
        }
        let svg = mermaid_rs_renderer::render_with_options(source, options).map_err(|e| short(&e.to_string()))?;
        let svg = restyle(svg, palette);
        if svg.len() > 2 * 1024 * 1024 {
            return Err("Diagram output exceeds preview size limit".into());
        }
        Ok(svg)
    })
    .unwrap_or_else(|_| Err("Diagram could not be rendered".into()))
}

/// An error's first line, kept short: it shows under the code.
fn short(error: &str) -> String {
    let line = error.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Diagram could not be rendered");
    if line.chars().count() > 160 { format!("{}…", line.chars().take(159).collect::<String>()) } else { line.to_string() }
}

/// The diagram type: the first word after any front matter and comments.
fn diagram_keyword(source: &str) -> Option<&str> {
    let mut lines = source.lines().map(str::trim).filter(|line| !line.is_empty()).peekable();
    if lines.next_if_eq(&"---").is_some() {
        lines.by_ref().find(|line| *line == "---");
    }
    lines.find(|line| !line.starts_with("%%")).and_then(|line| line.split_whitespace().next())
}

/// Finishing touches the engine's theme cannot express, applied to its
/// pinned output (the shipping app's): a transparent canvas, softer node
/// corners, cards for boxes the engine fills with its background, decisions
/// in the accent and themed Gantt gridlines. Only elements still carrying
/// the palette's own colors change, so explicit `style`/`classDef` colors
/// survive.
fn restyle(svg: String, palette: &Palette) -> String {
    let canvas_fill = format!(" fill=\"{}\"", palette.canvas);
    let node_fill = format!(" fill=\"{}\"", palette.node);
    let border_stroke = format!(" stroke=\"{}\"", palette.border);
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg.as_str();
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..=end];
        rest = &rest[end + 1..];
        if is_canvas(tag, palette) {
            continue;
        }
        if tag.starts_with("<polygon ") && is_default_diamond(tag, palette) {
            out.push_str(
                &tag.replacen(&node_fill, &format!(" fill=\"{}\"", palette.accent_wash), 1)
                    .replacen(&border_stroke, &format!(" stroke=\"{}\"", palette.accent_line), 1),
            );
        } else if tag.starts_with("<rect ") {
            let mut tag = tag.replacen(" rx=\"3\" ry=\"3\" ", " rx=\"8\" ry=\"8\" ", 1);
            if tag.contains(&border_stroke) {
                tag = tag.replacen(&canvas_fill, &node_fill, 1);
            }
            out.push_str(&tag);
        } else if tag.starts_with("<line ") {
            out.push_str(&tag.replacen(" stroke=\"#E2E8F0\"", &format!(" stroke=\"{}\"", palette.grid), 1));
        } else {
            out.push_str(tag);
        }
    }
    out.push_str(rest);
    out
}

/// `name="value"` pairs of a start tag, as the engine writes them.
fn attributes(tag: &str) -> Vec<(&str, &str)> {
    let parts: Vec<_> = tag.split('"').collect();
    parts
        .chunks_exact(2)
        .filter_map(|pair| {
            let name = pair[0].trim_end().strip_suffix('=')?;
            Some((name.rsplit(' ').next()?, pair[1]))
        })
        .collect()
}

/// A full-bleed background: a plain rectangle in the canvas color.
fn is_canvas(tag: &str, palette: &Palette) -> bool {
    if !tag.starts_with("<rect ") {
        return false;
    }
    let mut fill = None;
    for (name, value) in attributes(tag) {
        match name {
            "x" | "y" | "width" | "height" => {}
            "fill" => fill = Some(value),
            _ => return false,
        }
    }
    fill == Some(palette.canvas.as_str())
}

fn is_default_diamond(tag: &str, palette: &Palette) -> bool {
    let attrs = attributes(tag);
    let get = |key: &str| attrs.iter().find(|(name, _)| *name == key).map(|(_, v)| *v);
    if get("fill") != Some(palette.node.as_str()) || get("stroke") != Some(palette.border.as_str()) {
        return false;
    }
    let Some(points) = get("points") else {
        return false;
    };
    let points: Vec<(f32, f32)> = points
        .split_whitespace()
        .filter_map(|point| {
            let (x, y) = point.split_once(',')?;
            Some((x.parse().ok()?, y.parse().ok()?))
        })
        .collect();
    let [top, right, bottom, left] = points[..] else {
        return false;
    };
    (top.0 - bottom.0).abs() < 0.05
        && (left.1 - right.1).abs() < 0.05
        && left.0 < top.0
        && top.0 < right.0
        && top.1 < left.1
        && left.1 < bottom.1
}

#[cfg(test)]
mod tests {
    use super::*;

    const FLOWCHART: &str = "flowchart TD\n  A[Start] --> B{Ready?}\n  B -->|yes| C[Ship it]\n  B -->|no| D[Fix]\n  D --> B";
    const SEQUENCE: &str =
        "sequenceDiagram\n  participant U as User\n  participant Z as Zeron\n  U->>Z: Ask\n  Z-->>U: Answer\n  Note over U,Z: Both ways";

    fn request(source: &str, dark: bool) -> Request {
        let colors = if dark {
            [("background", "#0b0b0dcc"), ("code", "#ededef0a"), ("text", "#ededefff"), ("accent", "#9277b0ff")]
        } else {
            [("background", "#f5f5f7b8"), ("code", "#1d1d1f0a"), ("text", "#1d1d1fff"), ("accent", "#6e56cfff")]
        };
        Request::from_value(&Value::record([
            ("source", Value::from(source)),
            ("dark", Value::from(dark)),
            ("font", Value::from("Geist")),
            ("colors", Value::record(colors.map(|(k, v)| (k, Value::from(v))))),
        ]))
        .unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-mermaid-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn flowcharts_and_sequences_draw_in_both_appearances() {
        let dir = scratch("draw");
        for dark in [false, true] {
            for source in [FLOWCHART, SEQUENCE] {
                let drawn = draw_cached(&request(source, dark), &dir).unwrap();
                assert!(drawn.w > 50. && drawn.h > 50., "{drawn:?}");
                // Its text as outlines, drawn by zgx's own SVG drawing (no
                // fonts there) at 3x, as a zoomed lightbox or a dense
                // display asks: a clear ground, and ink on it (text drawn
                // as glyphs: more than the boxes' outlines).
                assert!(!drawn.svg.contains("<text") && drawn.svg.contains("<path"));
                let (pw, ph) = ((drawn.w * 3.).ceil() as u32, (drawn.h * 3.).ceil() as u32);
                let image = zgx_gpu::assets::decode_svg(drawn.svg.as_bytes(), pw, ph).unwrap();
                let pixels = image.pixels();
                assert_eq!(pixels[3], 0, "the corner is clear");
                let inked = pixels.chunks_exact(4).filter(|p| p[3] > 200).count();
                assert!(inked > 4000, "{inked} inked pixels");
                let on_plate = zgx_gpu::assets::decode_svg(drawn.plate_svg.as_bytes(), pw, ph).unwrap();
                // (Inside its edge, which a fit to whole pixels may leave
                // part covered.)
                let inside = ((4 * pw + 4) * 4 + 3) as usize;
                assert_eq!(on_plate.pixels()[inside], 255, "the plate's vectors are opaque");
                let plate = zgx_gpu::assets::decode_image(&std::fs::read(&drawn.plate).unwrap()).unwrap();
                assert_eq!(plate.pixels()[3], 255, "the plate is opaque");
                // Asked again: the same pictures, kept.
                assert_eq!(draw_cached(&request(source, dark), &dir).unwrap(), drawn);
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_image_in_the_svg_reads_nothing() {
        // A picture on this machine, and the same inline: neither is read
        // into the diagram, nor written out for zgx to read.
        let dir = scratch("image");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("secret.png");
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 2, 2);
            encoder.set_color(png::ColorType::Rgba);
            encoder.write_header().unwrap().write_image_data(&[255, 0, 0, 255].repeat(4)).unwrap();
        }
        std::fs::write(&file, &png).unwrap();
        let palette = Palette::from_request(&request(FLOWCHART, true));
        for href in [format!("file://{}", file.display()), file.display().to_string(), "file:///etc/hostname".into(), {
            use base64::Engine;
            format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&png))
        }] {
            let svg = format!(
                r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="40" height="40"><image href="{href}" xlink:href="{href}" width="40" height="40"/><rect x="1" y="1" width="4" height="4" fill="#00ff00"/></svg>"##
            );
            let drawn = draw(&svg, &palette).unwrap();
            assert!(!drawn.svg.contains("<image") && !drawn.svg.contains("secret") && !drawn.svg.contains("base64"), "{href}: {}", drawn.svg);
            // Only the square is drawn: nothing red anywhere.
            let red = drawn.plate.pixels().iter().filter(|p| p.red() > 200 && p.green() < 50).count();
            assert_eq!(red, 0, "{href}");
        }
        // And from a diagram's source, as an agent would write it.
        let source = format!("flowchart TD\n  A[\"<img src='file://{}'>\"] --> B[\"<image href='file:///etc/hostname'/>\"]", file.display());
        if let Ok(svg) = render(&source, &palette) {
            let drawn = draw(&svg, &palette).unwrap();
            assert!(!drawn.svg.contains("<image") && !drawn.plate_svg.contains("<image"));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn layers_past_the_budget_are_refused() {
        let palette = Palette::from_request(&request(FLOWCHART, true));
        let nest = |attr: &str, n: usize| {
            format!("<g {attr}>").repeat(n) + "<rect width='3000' height='2000' fill='red'/>" + &"</g>".repeat(n)
        };
        let blur = "<filter id='f'><feGaussianBlur stdDeviation='50'/></filter>";
        // As an agent would write them: out of a `style` value, a canvas wide
        // enough to matter (these took gigabytes and minutes to draw).
        for body in [nest("opacity='.9'", 300), format!("{blur}{}", nest("filter='url(#f)'", 60))] {
            let source = format!("flowchart TD\n  A[\"{}\"]\n  style A fill:#fff\"/>{body}<rect x=\"0", "w".repeat(420));
            let svg = render(&source, &palette).unwrap();
            assert_eq!(draw(&svg, &palette).err().as_deref(), Some("Diagram exceeds preview complexity limit"));
        }
        // A pattern's tiles, and layers inside a mask, count too.
        let doc = |body: String| format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400">{body}</svg>"##);
        let tiles = "<pattern id='p' width='3000' height='3000' patternUnits='userSpaceOnUse'><rect width='9' height='9'/></pattern>".to_string()
            + &"<rect width='400' height='400' fill='url(#p)'/>".repeat(4);
        assert!(draw(&doc(tiles), &palette).is_err());
        let masked = format!("<mask id='m'>{}</mask><rect width='400' height='400' mask='url(#m)'/>", nest("opacity='.9'", 10));
        assert!(draw(&doc(masked), &palette).is_err());
        // One layer the size of the diagram is ordinary.
        assert!(draw(&doc(nest("opacity='.9'", 1)), &palette).is_ok());
    }

    #[test]
    fn appearances_and_sources_are_kept_apart() {
        let light = request(FLOWCHART, false);
        assert_ne!(light.key(), request(FLOWCHART, true).key());
        assert_ne!(light.key(), request(SEQUENCE, false).key());
        assert_eq!(light.key(), request(FLOWCHART, false).key());
    }

    #[test]
    fn malformed_and_oversized_diagrams_fail_with_a_reason() {
        let dir = scratch("fail");
        let bad = request("this is not a diagram", true);
        let why = draw_cached(&bad, &dir).unwrap_err();
        assert!(!why.is_empty() && !why.contains('\n'), "{why}");
        // Remembered: the second answer is the first's.
        assert_eq!(draw_cached(&bad, &dir).unwrap_err(), why);
        assert!(draw_cached(&request(&"x".repeat(MAX_SOURCE_BYTES + 1), true), &dir).is_err());
        assert!(draw_cached(&request("  \n", true), &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn colours_compose_over_the_ground() {
        assert_eq!(Rgba::parse("#ffffff80").unwrap().over(Rgba::BLACK).hex(), "#808080");
        assert_eq!(Rgba::parse("#abc").unwrap().hex(), "#aabbcc");
        assert!(Rgba::parse("red").is_none());
        let palette = Palette::from_request(&request(FLOWCHART, false));
        let svg = render("flowchart TD\nA[Start] --> B{Ready?}\nB --> C[End]", &palette).unwrap();
        // The fence body shows through: no full-bleed background remains.
        assert!(!svg.contains(&format!("fill=\"{}\"/>", palette.canvas)));
        assert!(svg.contains(" rx=\"8\" ry=\"8\" "));
    }

    #[test]
    fn diagram_keyword_skips_front_matter_and_comments() {
        assert_eq!(diagram_keyword("gantt\n  title X"), Some("gantt"));
        assert_eq!(diagram_keyword("---\ntitle: Plan\n---\n%% note\n\n gantt"), Some("gantt"));
        assert_eq!(diagram_keyword("  \n%% only"), None);
    }
}
