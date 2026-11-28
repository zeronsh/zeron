//! Mermaid diagrams for the scripts (`ds/mermaid`): Merman 0.11 (a headless
//! port of Mermaid's own parser, layouts and SVG, so diagrams lay out as
//! Mermaid lays them out) in Zeron's style, as vectors zgx draws sharp at
//! any size. Labels are measured with rustybuzz over the very faces usvg
//! then draws them with (the app's Geist first), so text fits its boxes.
//!
//!   mermaid.render { source, dark, font, colors = { background, code,
//!                    text, secondary, tertiary, border, strong, accent } }
//!   mermaid.save   { to, plate, svg }   a drawn diagram written to `to`
//!                  (SVG for a `.svg` name, else the plate's PNG)
//!
//! answers `{ svg, plate_svg, plate, plate_color, w, h }`: `svg` the diagram on a clear
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
//! The work goes on a thread of its own, one diagram at a time, under a
//! deadline and Merman's interactive resource policy; the pictures are kept on disk by a
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

pub const ENGINE_VERSION: &str = "merman/0.11.0";
/// Bumped when what is drawn for the same request changes.
const STYLE_VERSION: u32 = 7;
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

/// The longest one diagram may take to lay out and draw.
const DEADLINE: std::time::Duration = std::time::Duration::from_secs(4);

// Bound inputs and serialize the CPU work across diagrams (the shipping
// app's RENDER_LOCK); Merman stops a diagram at its deadline.
static RENDER_LOCK: Mutex<()> = Mutex::new(());

pub fn install(host: &ScriptHost) {
    host.async_action("mermaid.render", |arg| async move {
        let request = Request::from_value(&arg)?;
        // (The plate's colour: the lightbox's panel is it.)
        let plate_color = Palette::from_request(&request).plate.hex();
        let dir = diagrams_dir();
        let drawn = off_thread(move || draw_cached(&request, &dir)).await.ok_or("Diagram could not be rendered")??;
        Ok(Value::record([
            ("svg", Value::from(drawn.svg.as_str())),
            ("plate_svg", Value::from(drawn.plate_svg.as_str())),
            ("plate", Value::from(drawn.plate.to_string_lossy().as_ref())),
            ("plate_color", Value::from(plate_color.as_str())),
            ("w", Value::from(drawn.w as f64)),
            ("h", Value::from(drawn.h as f64)),
        ]))
    });
    // `mermaid.save { to, plate, svg }`: the diagram written where the
    // reader chose in the save panel.
    host.async_action("mermaid.save", |arg| async move {
        let text = |k: &str| arg.field(k).map(crate::auth::text).unwrap_or_default();
        let (to, plate, svg) = (text("to"), text("plate"), text("svg"));
        off_thread(move || save(to, plate, svg)).await.ok_or("Diagram could not be saved")??;
        Ok(Value::Bool(true))
    });
}

/// Where diagrams are kept: the engine's data dir, else the user's cache.
fn diagrams_dir() -> PathBuf {
    crate::engine::data_dir().map(|d| d.join("mermaid")).unwrap_or_else(cache_dir)
}

/// A diagram saved where the reader chose (`ui.save_file`'s answer): as SVG
/// (its vectors on their plate, text as outlines, so it reads anywhere) for
/// a `.svg` name, else its PNG. The PNG is only ever one this module drew,
/// in its own folder: the scripts can't have it copy any other file.
fn save(to: String, plate: String, doc: String) -> Result<(), String> {
    let to = PathBuf::from(to);
    if to.as_os_str().is_empty() || !to.is_absolute() {
        return Err("No place to save the diagram".into());
    }
    let svg = to.extension().is_some_and(|x| x.eq_ignore_ascii_case("svg"));
    if svg {
        if !doc.trim_start().starts_with("<svg") {
            return Err("The diagram's vectors aren't there".into());
        }
        return std::fs::write(&to, doc).map_err(|e| e.to_string());
    }
    let from = PathBuf::from(plate);
    let ours = diagrams_dir().canonicalize().ok();
    let from = from.canonicalize().map_err(|_| "The diagram's picture is gone".to_string())?;
    let drawn_here = ours.is_some_and(|dir| from.starts_with(dir))
        && from.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".plate.png"));
    if !drawn_here {
        return Err("Not a diagram's picture".into());
    }
    std::fs::copy(&from, &to).map(|_| ()).map_err(|e| e.to_string())
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

    /// As CSS, alpha kept: a wash that lets what's under it show.
    fn css(self) -> String {
        let b = |v: f32| (v.clamp(0., 1.) * 255.).round() as u8;
        format!("rgba({}, {}, {}, {:.3})", b(self.0[0]), b(self.0[1]), b(self.0[2]), self.0[3].clamp(0., 1.))
    }

    /// WCAG relative luminance (opaque).
    fn luminance(self) -> f32 {
        let lin = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(self.0[0]) + 0.7152 * lin(self.0[1]) + 0.0722 * lin(self.0[2])
    }

    /// WCAG contrast ratio between two opaque colours (1 to 21).
    fn contrast(self, other: Rgba) -> f32 {
        let (a, b) = (self.luminance(), other.luminance());
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Hue (0..1), saturation and lightness.
    fn hsl(self) -> (f32, f32, f32) {
        let [r, g, b, _] = self.0;
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let l = (max + min) / 2.;
        if max - min < 1e-6 {
            return (0., 0., l);
        }
        let d = max - min;
        let s = if l > 0.5 { d / (2. - max - min) } else { d / (max + min) };
        let h = if max == r {
            (g - b) / d + if g < b { 6. } else { 0. }
        } else if max == g {
            (b - r) / d + 2.
        } else {
            (r - g) / d + 4.
        };
        (h / 6., s, l)
    }

    fn from_hsl(h: f32, s: f32, l: f32) -> Rgba {
        let q = if l < 0.5 { l * (1. + s) } else { l + s - l * s };
        let p = 2. * l - q;
        let channel = |t: f32| {
            let t = t.rem_euclid(1.);
            if t < 1. / 6. {
                p + (q - p) * 6. * t
            } else if t < 0.5 {
                q
            } else if t < 2. / 3. {
                p + (q - p) * (2. / 3. - t) * 6.
            } else {
                p
            }
        };
        Rgba([channel(h + 1. / 3.), channel(h), channel(h - 1. / 3.), 1.])
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
    /// The accent, opaque: the hue a mindmap's or a pie's sections turn from.
    accent: Rgba,
    /// An edge label's pill: a step deeper than the fence in dark, the
    /// window's own ground in light.
    badge: String,
    /// A subgraph's ground: an ink wash over the fence; a composite
    /// state's title strip a step stronger.
    cluster: String,
    cluster_head: String,
    /// Ink for labels on a fill of the diagram's own (dark and light).
    ink_dark: Rgba,
    ink_light: Rgba,
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
            accent: r.accent.over(canvas),
            // (Washes, not solid colours: the fence they sit on is glass over
            // whatever the window shows, lighter than any opaque guess.)
            badge: if r.dark { Rgba::BLACK.opacity(0.32).css() } else { Rgba::WHITE.opacity(0.85).css() },
            cluster: r.text.opacity(if r.dark { 0.035 } else { 0.03 }).css(),
            cluster_head: r.text.opacity(if r.dark { 0.07 } else { 0.06 }).css(),
            ink_dark: Rgba([0.11, 0.11, 0.12, 1.]),
            ink_light: Rgba([0.96, 0.96, 0.97, 1.]),
        }
    }
}

/// The app's Geist faces (fonts/), always there for a diagram's labels.
const GEIST: [&[u8]; 5] = [
    include_bytes!("../fonts/Geist.ttf"),
    include_bytes!("../fonts/Geist-Medium.ttf"),
    include_bytes!("../fonts/Geist-SemiBold.ttf"),
    include_bytes!("../fonts/Geist-Bold.ttf"),
    include_bytes!("../fonts/Geist-Italic.ttf"),
];

/// This machine's fonts and the app's Geist: what labels are measured with
/// (`measurement`) and drawn with (`svg_options`), so what is drawn fits
/// the boxes Merman laid out.
fn fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            // The app's own sans, installed or not: what the transcript
            // reads in, so a diagram's labels match the prose around it.
            for face in GEIST {
                db.load_font_data(face.to_vec());
            }
            Arc::new(db)
        })
        .clone()
}

/// The theme's sans when this machine has it, else the app's Geist, then
/// common sans faces, in the order they're looked for.
fn font_family(theme: &str) -> String {
    // (A family the user chose is its token quoted: ds/look's font_theme.)
    let theme = theme.trim().trim_matches('"');
    let fallbacks = "Geist, Helvetica Neue, Helvetica, Arial, Liberation Sans, DejaVu Sans, Noto Sans, sans-serif";
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

/// The source as SVG in Zeron's style: Merman's `base` theme with every
/// role the palette's, labels measured in the faces they're drawn with,
/// and the output kept to what resvg draws (`resvg_safe`: no
/// `foreignObject`, inline labels as text).
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
        use merman::svg::{RenderResourcePolicy, ScopedCssPostprocessor, CssOverridePolicy, SvgPipeline, SvgRenderOptions};
        use merman::{MermaidEngine, OperationControl, RenderOutput, RenderRequest, Renderer, SourcePresentationConfig, SvgEnvironment, SvgRequest};
        let config = SourcePresentationConfig::from_value(site_config(palette));
        let renderer = Renderer::new().with_presentation_engine(MermaidEngine::new().with_site_config(config));
        let environment = SvgEnvironment::deterministic()
            .with_resource_policy(RenderResourcePolicy::interactive())
            .with_text_measurement_policy(measurement());
        let pipeline = SvgPipeline::resvg_safe().with_postprocessor(
            ScopedCssPostprocessor::new(host_css(palette)).with_override_policy(CssOverridePolicy::StripExistingImportant),
        );
        let request = SvgRequest {
            environment,
            pipeline: Some(pipeline),
            options: SvgRenderOptions { diagram_id: Some("zeron-mermaid".into()), ..Default::default() },
            ..Default::default()
        };
        let control = OperationControl::new().with_deadline(DEADLINE);
        // Decisions as Zeron's question cards; should the rewrite not parse,
        // the source as written.
        let softened = soften_decisions(source, palette);
        let output = match softened.as_deref() {
            Some(soft) => renderer
                .render(RenderRequest::svg(soft, control.clone(), request.clone()))
                .or_else(|_| renderer.render(RenderRequest::svg(source, control, request))),
            None => renderer.render(RenderRequest::svg(source, control, request)),
        }
        .map_err(|e| short(&e.to_string()))?;
        let RenderOutput::Svg(Some(svg)) = output else {
            return Err("Not a Mermaid diagram".into());
        };
        let svg = legible_labels(badges(thin_arrows(restyle(svg.svg().to_string())), palette), palette);
        if svg.len() > 2 * 1024 * 1024 {
            return Err("Diagram output exceeds preview size limit".into());
        }
        Ok(svg)
    })
    .unwrap_or_else(|_| Err("Diagram could not be rendered".into()))
}

/// Mermaid's own theme variables, every role from the palette (`base` is
/// the theme Mermaid derives nothing surprising from). Explicit `style`,
/// `classDef` and `%%{init}%%` in a source still win over these.
fn site_config(p: &Palette) -> serde_json::Value {
    use serde_json::{Map, Value as Json, json};
    let colors: &[(&str, &String)] = &[
        ("background", &p.canvas),
        ("primaryColor", &p.node),
        ("primaryTextColor", &p.text),
        ("primaryBorderColor", &p.border),
        ("secondaryColor", &p.group),
        ("secondaryTextColor", &p.text),
        ("secondaryBorderColor", &p.border),
        ("tertiaryColor", &p.group),
        ("tertiaryTextColor", &p.text),
        ("tertiaryBorderColor", &p.border),
        ("mainBkg", &p.node),
        ("nodeBorder", &p.border),
        ("nodeTextColor", &p.text),
        ("textColor", &p.label),
        ("titleColor", &p.text),
        ("lineColor", &p.line),
        ("defaultLinkColor", &p.line),
        ("clusterBkg", &p.group),
        ("clusterBorder", &p.border),
        ("actorBkg", &p.node),
        ("actorBorder", &p.border),
        ("actorTextColor", &p.text),
        ("actorLineColor", &p.border),
        ("signalColor", &p.line),
        ("signalTextColor", &p.text),
        ("labelBoxBkgColor", &p.node),
        ("labelBoxBorderColor", &p.border),
        ("labelTextColor", &p.text),
        ("loopTextColor", &p.label),
        ("noteBkgColor", &p.accent_wash),
        ("noteBorderColor", &p.accent_line),
        ("noteTextColor", &p.text),
        ("activationBkgColor", &p.accent_wash),
        ("activationBorderColor", &p.accent_line),
        ("sequenceNumberColor", &p.node),
        ("labelColor", &p.text),
        ("altBackground", &p.group),
        ("stateBkg", &p.node),
        ("stateBorder", &p.border),
        // (Unset, Mermaid derives it as the node's own fill: invisible.)
        ("stateLabelColor", &p.text),
        ("compositeBackground", &p.group),
        ("compositeTitleBackground", &p.node),
        ("transitionColor", &p.line),
        ("transitionLabelColor", &p.label),
        ("specialStateColor", &p.line),
        ("innerEndBackground", &p.line),
        ("classText", &p.text),
        ("attributeBackgroundColorOdd", &p.node),
        ("attributeBackgroundColorEven", &p.group),
        ("gridColor", &p.grid),
        ("sectionBkgColor", &p.accent_wash),
        ("altSectionBkgColor", &p.group),
        ("sectionBkgColor2", &p.group),
        ("taskBkgColor", &p.accent_wash),
        ("taskBorderColor", &p.accent_line),
        ("taskTextColor", &p.text),
        ("taskTextLightColor", &p.text),
        ("taskTextOutsideColor", &p.label),
        ("taskTextDarkColor", &p.text),
        ("activeTaskBkgColor", &p.accent_wash),
        ("activeTaskBorderColor", &p.accent_line),
        ("doneTaskBkgColor", &p.group),
        ("doneTaskBorderColor", &p.border),
        ("critBkgColor", &p.accent_wash),
        ("critBorderColor", &p.accent_line),
        ("todayLineColor", &p.accent_line),
        ("pie1", &p.accent_line),
        ("pieStrokeColor", &p.canvas),
        ("pieOuterStrokeColor", &p.border),
        ("pieTitleTextColor", &p.text),
        ("pieSectionTextColor", &p.text),
        ("pieLegendTextColor", &p.label),
        ("git0", &p.accent_line),
    ];
    let mut variables: Map<String, Json> = colors.iter().map(|(k, v)| (k.to_string(), Json::from(v.as_str()))).collect();
    // No ground of Mermaid's own under edge labels: their pills are
    // (`badges`), and a baked square would show inside them.
    variables.insert("edgeLabelBackground".into(), Json::from("transparent"));
    // A git graph's branches after the first, each its own hue turned from
    // the accent's (unset, `base` greys them all), labels in the theme's
    // text on them.
    let (hue, sat, _) = p.accent.hsl();
    for i in 1..8 {
        let h = hue + i as f32 / 8. * 0.85;
        let line = Rgba::from_hsl(h, sat.clamp(0.35, 0.6), if p.dark { 0.55 } else { 0.6 });
        variables.insert(format!("git{i}"), Json::from(line.hex()));
        let label = if line.contrast(p.ink_dark) >= line.contrast(p.ink_light) { p.ink_dark } else { p.ink_light };
        variables.insert(format!("gitBranchLabel{i}"), Json::from(label.hex()));
    }
    variables.insert("darkMode".into(), Json::from(p.dark));
    variables.insert("fontFamily".into(), Json::from(p.font.as_str()));
    variables.insert("fontSize".into(), Json::from("14px"));
    json!({
        "theme": "base",
        "darkMode": p.dark,
        "fontFamily": p.font,
        "fontSize": "14px",
        // ELK's orthogonal routing with its bends rounded: edges read as
        // tidy wiring rather than crossing curves. (A source's own
        // `%%{init}%%` still picks another layout or curve.)
        "layout": "elk",
        "flowchart": { "padding": 12, "nodeSpacing": 40, "rankSpacing": 44, "curve": "rounded" },
        "sequence": { "mirrorActors": false, "actorMargin": 40, "boxMargin": 8, "noteMargin": 10, "messageMargin": 32 },
        "themeVariables": Json::Object(variables),
    })
}

/// What Mermaid's CSS can't be told through its variables: decisions in
/// the accent, a quiet ground under edge labels, no drop shadow on actors,
/// and a mindmap's deep branches drawn (Mermaid sizes them `17 - 3 * depth`,
/// negative past the fourth level: a browser falls back to 1px, resvg draws
/// nothing).
fn host_css(p: &Palette) -> String {
    format!(
        ".node polygon.label-container {{ fill: {wash}; stroke: {accent}; }}\n\
         .edgeLabel rect {{ fill: none; stroke: none; }}\n\
         .edgeLabel rect.zeron-badge {{ fill: {badge}; stroke: {badge_line}; stroke-width: 1px; opacity: 1; }}\n\
         .actor {{ filter: none; }}\n\
         .cluster rect {{ stroke-dasharray: 0; fill: {cluster}; }}\n\
         .statediagram-cluster rect, [data-look=\"neo\"].statediagram-cluster rect, .stateGroup .composit {{ fill: {cluster_head}; }}\n\
         .statediagram-cluster.statediagram-cluster .inner, .statediagram-cluster.statediagram-cluster-alt .inner {{ fill: {cluster}; }}\n\
         .grid .tick line {{ stroke: {grid}; opacity: 1; }}\n\
         .grid path {{ stroke-width: 0; }}\n\
         .edge-depth-5, .edge-depth-6, .edge-depth-7, .edge-depth-8, .edge-depth-9, .edge-depth-10, .edge-depth-11 {{ stroke-width: 2; }}\n{sections}",
        wash = p.accent_wash,
        accent = p.accent_line,
        badge = p.badge,
        cluster = p.cluster,
        cluster_head = p.cluster_head,
        badge_line = p.border,
        grid = p.grid,
        sections = section_css(p),
    )
}

/// A mindmap's (and timeline's) sections: Mermaid colours them from
/// `cScale0..11`, which `base` derives as near black here. Each turns from
/// the accent's hue, soft enough for the theme's text on it; its edges a
/// step stronger, so they read on the canvas. The root (`-1`) is the accent.
fn section_css(p: &Palette) -> String {
    let (hue, sat, _) = p.accent.hsl();
    let sat = sat.clamp(0.25, 0.55);
    let (fill_l, edge_l) = if p.dark { (0.30, 0.52) } else { (0.88, 0.62) };
    let mut css = String::new();
    for i in -1..12_i32 {
        let (fill, edge) = if i < 0 {
            (p.accent_line.clone(), p.accent_line.clone())
        } else {
            let h = hue + i as f32 / 12. * 0.85;
            (Rgba::from_hsl(h, sat, fill_l).hex(), Rgba::from_hsl(h, sat, edge_l).hex())
        };
        css.push_str(&format!(
            ".section-{i} rect, .section-{i} circle, .section-{i} polygon, .section-{i} path {{ fill: {fill}; }}\n\
             .section-{i} text {{ fill: {text}; }}\n\
             .section-{i} line {{ stroke: {edge}; }}\n\
             .section-edge-{i} {{ stroke: {edge}; fill: none; }}\n",
            text = p.text,
        ));
    }
    css
}

/// An error's first line, kept short: it shows under the code.
fn short(error: &str) -> String {
    let line = error.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("Diagram could not be rendered");
    if line.chars().count() > 160 { format!("{}…", line.chars().take(159).collect::<String>()) } else { line.to_string() }
}

/// Finishing touches on Merman's output: softer corners on plain node and
/// cluster boxes (Zeron's card corner), whatever corner Mermaid gave them.
fn restyle(svg: String) -> String {
    let mut out = String::with_capacity(svg.len() + 1024);
    let mut rest = svg.as_str();
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..=end];
        rest = &rest[end + 1..];
        if tag.starts_with("<rect ") && tag.contains("class=\"basic label-container\"") {
            let tag = strip_attribute(strip_attribute(tag.to_string(), "rx"), "ry");
            out.push_str(&tag.replacen("<rect ", "<rect rx=\"8\" ry=\"8\" ", 1));
        } else {
            out.push_str(tag);
        }
    }
    out.push_str(rest);
    out
}

/// Edge labels as pills: a rounded badge under each (a flowchart's, a
/// state's, a class's relation), its size the label's own (Merman places
/// each label's box at `translate(-w/2, -h/2)`), in place of Mermaid's
/// square ground.
fn badges(svg: String, _palette: &Palette) -> String {
    const EDGE_LABEL: &str = "<g class=\"edgeLabel\"";
    const LABEL: &str = "<g class=\"label\"";
    let mut out = String::with_capacity(svg.len() + 4096);
    let mut rest = svg.as_str();
    // The label documents edge labels draw (`<use href>`), whose baked
    // ground goes.
    let mut grounds: Vec<String> = Vec::new();
    while let Some(at) = rest.find(EDGE_LABEL) {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        // Its label, before the edge label closes (an empty one has none).
        let close = rest.find("</g>").unwrap_or(rest.len());
        let Some(label) = rest[..close].find(LABEL) else {
            out.push_str(&rest[..EDGE_LABEL.len()]);
            rest = &rest[EDGE_LABEL.len()..];
            continue;
        };
        let Some(end) = rest[label..].find('>').map(|i| label + i + 1) else { break };
        let tag = &rest[label..end];
        if let Some(href) = rest[end..close.max(end)].find("href=\"#").map(|i| end + i + 7) {
            if let Some(len) = rest[href..].find('"') {
                grounds.push(rest[href..href + len].to_string());
            }
        }
        out.push_str(&rest[..end]);
        rest = &rest[end..];
        if let Some((w, h)) = label_size(tag) {
            let (px, py) = (8., 3.);
            let (bw, bh) = (w + 2. * px, h + 2. * py);
            out.push_str(&format!(
                "<rect class=\"zeron-badge\" x=\"{}\" y=\"{}\" width=\"{bw}\" height=\"{bh}\" rx=\"{r}\" ry=\"{r}\"/>",
                -px,
                -py,
                r = bh / 2.
            ));
        }
    }
    out.push_str(rest);
    drop_grounds(out, &grounds)
}

/// `svg` with the filled backdrop rect taken out of each of the `ids`'
/// label documents: the HTML label's `background-color`, which Merman
/// bakes in as a square (Mermaid's theme turns a clear one into half
/// black).
fn drop_grounds(mut svg: String, ids: &[String]) -> String {
    for id in ids {
        let needle = format!("id=\"{id}\"");
        let Some(from) = svg.find(&needle) else { continue };
        let to = svg[from + needle.len()..].find(" id=\"").map_or(svg.len(), |i| from + needle.len() + i);
        let mut region = svg[from..to].to_string();
        let mut at = 0;
        while let Some(i) = region[at..].find("<rect ").map(|i| at + i) {
            let end = region[i..].find('>').map_or(region.len(), |j| i + j + 1);
            let tag = &region[i..end];
            if tag.contains(" x=\"0\" y=\"0\"") && !tag.contains("fill=\"none\"") {
                region.replace_range(i..end, "");
            } else {
                at = end;
            }
        }
        svg.replace_range(from..to, &region);
    }
    svg
}

/// A label box's size from its `translate(-w/2,-h/2)`; none for an empty
/// label.
fn label_size(tag: &str) -> Option<(f32, f32)> {
    let at = tag.find("translate(")? + "translate(".len();
    let inner = &tag[at..at + tag[at..].find(')')?];
    let mut parts = inner.split([',', ' ']).filter(|p| !p.is_empty()).map(|p| p.parse::<f32>().ok());
    let (x, y) = (parts.next()??, parts.next()??);
    let (w, h) = (-2. * x, -2. * y);
    (w > 1. && h > 1.).then_some((w, h))
}

/// A flowchart's arrowheads as fine open chevrons (Mermaid's are solid
/// wedges): the same markers and placement, their paths only.
fn thin_arrows(svg: String) -> String {
    const STYLE: &str = "fill:none;stroke-width:1.4;stroke-linecap:round;stroke-linejoin:round";
    let shapes = [
        ("flowchart-v2-pointEnd\"", "M 0 0 L 10 5 L 0 10 z", "M 2 1.5 L 9 5 L 2 8.5"),
        ("flowchart-v2-pointStart\"", "M 0 5 L 10 10 L 10 0 z", "M 8 1.5 L 1 5 L 8 8.5"),
        ("flowchart-v2-pointEnd-margin\"", "M 0 0 L 11.5 7 L 0 14 z", "M 3 2.5 L 11 7 L 3 11.5"),
        ("flowchart-v2-pointStart-margin\"", "M 0 7 L 11.5 14 L 11.5 0 z", "M 8.5 2.5 L 0.5 7 L 8.5 11.5"),
    ];
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg.as_str();
    while let Some(at) = rest.find("<marker ") {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let end = rest.find("</marker>").map_or(rest.len(), |i| i + "</marker>".len());
        let marker = &rest[..end];
        let head = &marker[..marker.find('>').unwrap_or(marker.len())];
        match shapes.iter().find(|(id, wedge, _)| head.contains(id) && marker.contains(wedge)) {
            Some((_, wedge, chevron)) => {
                let mut m = marker.replacen(&format!("d=\"{wedge}\""), &format!("d=\"{chevron}\""), 1);
                if let Some(s) = m.find("style=\"") {
                    let from = s + "style=\"".len();
                    if let Some(len) = m[from..].find('"') {
                        m.replace_range(from..from + len, STYLE);
                    }
                }
                out.push_str(&m);
            }
            None => out.push_str(marker),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// A flowchart's decisions (`id{text}`) as Zeron's question cards: rounded
/// boxes, dashed in the accent, rather than Mermaid's diamonds, which take
/// twice the room for the same words and shrink a diagram fitted to the
/// reading column. Only decisions the source doesn't style itself
/// (`style`, `class`, `:::`); `{{hexagons}}`, `@{ shape }` and braces in
/// quotes or edge labels stay as they are. None when there's nothing to
/// change, or not a flowchart.
fn soften_decisions(source: &str, palette: &Palette) -> Option<String> {
    if !matches!(diagram_keyword(source), Some("flowchart" | "graph")) {
        return None;
    }
    const STATEMENTS: [&str; 7] = ["%%", "classDef ", "class ", "style ", "linkStyle ", "click ", "---"];
    let ident = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let mut styled: Vec<String> = Vec::new();
    for line in source.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("style ") {
            styled.extend(rest.split_whitespace().next().map(str::to_string));
        } else if let Some(rest) = line.strip_prefix("class ") {
            if let Some(ids) = rest.split_whitespace().next() {
                styled.extend(ids.split(',').map(|id| id.trim().to_string()));
            }
        }
    }
    let mut decisions: Vec<String> = Vec::new();
    let mut out = String::with_capacity(source.len() + 256);
    for (n, line) in source.split_inclusive('\n').enumerate() {
        let trimmed = line.trim();
        if n == 0 || STATEMENTS.iter().any(|s| trimmed.starts_with(s)) || !line.contains('{') {
            out.push_str(line);
            continue;
        }
        let chars: Vec<char> = line.chars().collect();
        let mut written = String::with_capacity(line.len() + 8);
        let (mut quoted, mut piped) = (false, false);
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '"' => quoted = !quoted,
                '|' if !quoted => piped = !piped,
                '{' if !quoted && !piped => {
                    let prev = if i > 0 { chars[i - 1] } else { ' ' };
                    let next = chars.get(i + 1).copied().unwrap_or(' ');
                    let id_start = (0..i).rev().take_while(|&j| ident(chars[j])).last();
                    // A body with no brace in it (a hexagon's `{{` or a
                    // `@{` shape aren't decisions), closed on this line.
                    let close = (i + 1..chars.len()).find(|&j| chars[j] == '}' || chars[j] == '{').filter(|&j| chars[j] == '}');
                    if let (Some(start), Some(close)) = (id_start, close)
                        && ident(prev)
                        && next != '{'
                        && chars.get(close + 1) != Some(&'}')
                    {
                        let id: String = chars[start..i].iter().collect();
                        let body: String = chars[i + 1..close].iter().collect();
                        let body = body.trim();
                        let inline_class = chars[close + 1..].iter().collect::<String>().starts_with(":::");
                        let quoted_body = body.len() >= 2 && body.starts_with('"') && body.ends_with('"');
                        if !inline_class && !styled.contains(&id) && (quoted_body || !body.contains('"')) {
                            written.push_str(&if quoted_body { format!("({body})") } else { format!("(\"{body}\")") });
                            if !decisions.contains(&id) {
                                decisions.push(id);
                            }
                            i = close + 1;
                            continue;
                        }
                    }
                }
                _ => {}
            }
            written.push(c);
            i += 1;
        }
        out.push_str(&written);
    }
    if decisions.is_empty() {
        return None;
    }
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!(
        "classDef zeronDecision fill:{},stroke:{},stroke-dasharray:4 3,stroke-width:1px,color:{}\nclass {} zeronDecision\n",
        palette.accent_wash,
        palette.accent_line,
        palette.text,
        decisions.join(",")
    ));
    Some(out)
}

/// The diagram type: the first word after any front matter and comments.
fn diagram_keyword(source: &str) -> Option<&str> {
    let mut lines = source.lines().map(str::trim).filter(|line| !line.is_empty()).peekable();
    if lines.next_if_eq(&"---").is_some() {
        lines.by_ref().find(|line| *line == "---");
    }
    lines.find(|line| !line.starts_with("%%")).and_then(|line| line.split_whitespace().next())
}

/// Labels kept legible on fills a source gives its nodes (`classDef`,
/// `style`): a light fill written for Mermaid's light theme leaves the
/// theme's light text unreadable in dark, and the other way round. A label
/// (drawn from `<defs>`, by `<use href>`) whose ink has less than 3:1
/// against its node's own fill takes whichever of dark and light ink reads
/// best; an ink the source chose that reads is kept.
fn legible_labels(svg: String, palette: &Palette) -> String {
    const NODE: &str = "<g class=\"node";
    // Each node's own fill, by the labels it draws.
    let mut fills: HashMap<String, Rgba> = HashMap::new();
    let mut at = 0;
    while let Some(start) = svg[at..].find(NODE).map(|i| at + i) {
        let end = svg[start + NODE.len()..].find(NODE).map_or(svg.len(), |i| start + NODE.len() + i);
        let node = &svg[start..end];
        if let Some(fill) = own_fill(node) {
            let mut rest = node;
            while let Some(i) = rest.find("href=\"#") {
                rest = &rest[i + 7..];
                if let Some(close) = rest.find('"') {
                    fills.insert(rest[..close].to_string(), fill);
                }
            }
        }
        at = end;
    }
    if fills.is_empty() {
        return svg;
    }
    let mut out = svg;
    for (id, fill) in fills {
        let needle = format!("id=\"{id}\"");
        let Some(from) = out.find(&needle) else { continue };
        // Its label: up to the next label's definition.
        let to = out[from + needle.len()..].find(" id=\"").map_or(out.len(), |i| from + needle.len() + i);
        let region = &out[from..to];
        let mut fixed = String::with_capacity(region.len());
        let mut rest = region;
        while let Some(i) = rest.find("<text") {
            fixed.push_str(&rest[..i]);
            let close = rest[i..].find('>').map_or(rest.len(), |j| i + j + 1);
            fixed.push_str(&legible_tag(&rest[i..close], fill, palette));
            rest = &rest[close..];
        }
        fixed.push_str(rest);
        out = format!("{}{}{}", &out[..from], fixed, &out[to..]);
    }
    out
}

/// A node's own fill: its shape's `style="fill:#…"` (a source's), not the
/// theme's (which comes from the stylesheet).
fn own_fill(node: &str) -> Option<Rgba> {
    let at = node.find("style=\"fill:#")?;
    let hex = &node[at + 12..];
    let end = hex.find(|c: char| !(c == '#' || c.is_ascii_hexdigit()))?;
    Rgba::parse(&hex[..end]).filter(|c| c.0[3] > 0.5)
}

/// A `<text>` tag with its ink, if it can't be read on `fill`, made one
/// that can.
fn legible_tag(tag: &str, fill: Rgba, palette: &Palette) -> String {
    let ink_of = |tag: &str| -> Option<(String, Rgba)> {
        let at = tag.find(" fill=\"#")?;
        let hex = &tag[at + 7..];
        let end = hex.find('"')?;
        Some((hex[..end].to_string(), Rgba::parse(&hex[..end])?))
    };
    let Some((written, ink)) = ink_of(tag) else { return tag.to_string() };
    if ink.contrast(fill) >= 3. {
        return tag.to_string();
    }
    let better = if palette.ink_dark.contrast(fill) >= palette.ink_light.contrast(fill) { palette.ink_dark } else { palette.ink_light };
    tag.replace(&format!("fill=\"{written}\""), &format!("fill=\"{}\"", better.hex()))
        .replace(&format!("fill:{written}"), &format!("fill:{}", better.hex()))
}

/// `tag` without its ` name="…"`.
fn strip_attribute(tag: String, name: &str) -> String {
    let needle = format!(" {name}=\"");
    let Some(at) = tag.find(&needle) else { return tag };
    let Some(close) = tag[at + needle.len()..].find('"') else { return tag };
    format!("{}{}", &tag[..at], &tag[at + needle.len() + close + 1..])
}

/// How labels are measured: Merman's own line breaking and heights, each
/// line's width shaped by rustybuzz in the face usvg will draw it with.
fn measurement() -> merman::svg::TextMeasurementPolicy {
    use merman::svg::{DeterministicTextMeasurer, MeasurementProfileId, TextMeasurementPolicy, TextMeasurementProfile, TextMeasurementProfileIdentity};
    static POLICY: OnceLock<TextMeasurementPolicy> = OnceLock::new();
    POLICY
        .get_or_init(|| {
            let widths = Widths::default();
            let measurer = DeterministicTextMeasurer::default().with_width_callback(move |text, style| widths.width(text, style));
            let identity = TextMeasurementProfileIdentity::new(
                MeasurementProfileId::new("zeron.rustybuzz").expect("a fixed, valid id"),
                format!("{STYLE_VERSION}"),
            )
            .expect("a fixed, valid identity");
            TextMeasurementPolicy::uniform(TextMeasurementProfile::new(identity, measurer))
        })
        .clone()
}

/// Shaped widths, in ems, by face and text; kept for the run (labels repeat
/// across a diagram's wrap probes and across diagrams).
#[derive(Default)]
struct Widths {
    kept: Mutex<HashMap<(usvg::fontdb::ID, String), f64>>,
    fallbacks: Mutex<HashMap<char, Option<usvg::fontdb::ID>>>,
}

impl Widths {
    /// `text`'s width in pixels at `style`: the face usvg picks for the
    /// style's families, weight and slant, and for any character it lacks,
    /// the first face that has it (as usvg falls back when drawing).
    fn width(&self, text: &str, style: &merman::svg::TextStyle) -> f64 {
        if text.is_empty() {
            return 0.;
        }
        let size = style.font_size.max(1.);
        let db = fonts();
        let Some(face) = query(&db, style) else {
            return text.chars().count() as f64 * size * 0.55;
        };
        // Runs of characters the face has, and of each it lacks.
        let runs = db
            .with_face_data(face, |data, index| {
                let parsed = ttf_parser::Face::parse(data, index).ok()?;
                let mut runs: Vec<(bool, String)> = Vec::new();
                for c in text.chars() {
                    let has = c.is_control() || c.is_whitespace() || parsed.glyph_index(c).is_some();
                    match runs.last_mut() {
                        Some((true, run)) if has => run.push(c),
                        _ => runs.push((has, c.to_string())),
                    }
                }
                Some(runs)
            })
            .flatten()
            .unwrap_or_else(|| vec![(true, text.to_string())]);
        let mut em = 0.;
        for (has, run) in runs {
            em += if has {
                self.shaped(&db, face, &run)
            } else {
                let c = run.chars().next().unwrap_or(' ');
                match self.fallback(&db, c) {
                    Some(other) => self.shaped(&db, other, &run),
                    None => if c.len_utf8() > 2 { 1.0 } else { 0.55 },
                }
            };
        }
        em * size
    }

    /// The first face that has `c`, found once a run.
    fn fallback(&self, db: &usvg::fontdb::Database, c: char) -> Option<usvg::fontdb::ID> {
        if let Some(found) = self.fallbacks.lock().unwrap_or_else(|e| e.into_inner()).get(&c) {
            return *found;
        }
        let found = db.faces().map(|f| f.id).find(|&id| {
            db.with_face_data(id, |data, index| ttf_parser::Face::parse(data, index).ok().and_then(|f| f.glyph_index(c)).is_some())
                .unwrap_or(false)
        });
        self.fallbacks.lock().unwrap_or_else(|e| e.into_inner()).insert(c, found);
        found
    }

    fn shaped(&self, db: &usvg::fontdb::Database, face: usvg::fontdb::ID, text: &str) -> f64 {
        let key = (face, text.to_string());
        if let Some(em) = self.kept.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
            return *em;
        }
        let em = db
            .with_face_data(face, |data, index| {
                let face = rustybuzz::Face::from_slice(data, index)?;
                let upem = face.units_per_em() as f64;
                if upem <= 0. {
                    return None;
                }
                let mut buffer = rustybuzz::UnicodeBuffer::new();
                buffer.push_str(text);
                let shaped = rustybuzz::shape(&face, &[], buffer);
                let advance: i64 = shaped.glyph_positions().iter().map(|g| g.x_advance as i64).sum();
                Some(advance as f64 / upem)
            })
            .flatten()
            .unwrap_or(text.chars().count() as f64 * 0.55);
        let mut kept = self.kept.lock().unwrap_or_else(|e| e.into_inner());
        if kept.len() > 20_000 {
            kept.clear();
        }
        kept.insert(key, em);
        em
    }
}

/// The face usvg would draw `style` in: its families in order, then sans.
fn query(db: &usvg::fontdb::Database, style: &merman::svg::TextStyle) -> Option<usvg::fontdb::ID> {
    use usvg::fontdb::{Family, Query, Stretch, Style, Weight};
    let names: Vec<String> = style
        .font_family
        .as_deref()
        .unwrap_or("Geist")
        .split(',')
        .map(|f| f.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .filter(|f| !f.is_empty())
        .collect();
    let mut families: Vec<Family<'_>> = names
        .iter()
        .map(|name| match name.to_ascii_lowercase().as_str() {
            "sans-serif" => Family::SansSerif,
            "serif" => Family::Serif,
            "monospace" => Family::Monospace,
            _ => Family::Name(name),
        })
        .collect();
    families.push(Family::Name("Geist"));
    families.push(Family::SansSerif);
    let weight = match style.font_weight.as_deref().map(str::trim) {
        Some("bold" | "bolder") => Weight::BOLD,
        Some("lighter") => Weight::LIGHT,
        Some(w) => w.parse::<u16>().map(Weight).unwrap_or(Weight::NORMAL),
        None => Weight::NORMAL,
    };
    let slant = match style.font_style.as_deref().map(str::trim) {
        Some("italic") => Style::Italic,
        Some("oblique") => Style::Oblique,
        _ => Style::Normal,
    };
    db.query(&Query { families: &families, weight, stretch: Stretch::Normal, style: slant })
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
        // enough to matter (these took gigabytes and minutes to draw with
        // the old renderer, which wrote the value out unescaped). Merman
        // escapes it: no element of the agent's reaches the SVG, and what
        // is drawn stays cheap; refused is fine too.
        for body in [nest("opacity='.9'", 300), format!("{blur}{}", nest("filter='url(#f)'", 60))] {
            let source = format!("flowchart TD\n  A[\"{}\"]\n  style A fill:#fff\"/>{body}<rect x=\"0", "w".repeat(420));
            let Ok(svg) = render(&source, &palette) else { continue };
            assert!(!svg.contains("<g opacity='.9'>") && !svg.contains("<feGaussianBlur"), "the source's elements leaked into the SVG");
            if let Err(why) = draw(&svg, &palette) {
                assert_eq!(why, "Diagram exceeds preview complexity limit");
            }
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

    /// Labels are measured in the faces they're drawn with: every node's
    /// label sits inside its box (the old renderer's measure overflowed).
    #[test]
    fn labels_fit_their_boxes() {
        let palette = Palette::from_request(&request(FLOWCHART, false));
        let source = "flowchart LR\n  A[A rather long label for a node] --> B[Short]\n  B --> C[Spawn detached engine process]";
        let svg = render(source, &palette).unwrap();
        let tree = usvg::Tree::from_str(&svg, &svg_options(&palette.font)).unwrap();
        fn texts(group: &usvg::Group, out: &mut Vec<usvg::Rect>) {
            for node in group.children() {
                match node {
                    usvg::Node::Group(g) => texts(g, out),
                    usvg::Node::Text(t) => out.push(t.abs_bounding_box()),
                    _ => {}
                }
            }
        }
        let mut boxes = Vec::new();
        texts(tree.root(), &mut boxes);
        assert!(!boxes.is_empty());
        let size = tree.size();
        for b in boxes {
            assert!(b.left() >= -0.5 && b.right() <= size.width() + 0.5, "{b:?} past {size:?}");
        }
    }

    /// No label is inked in the fill it sits on: Mermaid derives some label
    /// colours (a state's) from the node's when its variable is unset.
    #[test]
    fn labels_are_never_inked_in_their_node_s_fill() {
        let sources = [
            FLOWCHART,
            SEQUENCE,
            "stateDiagram-v2\n  [*] --> Idle\n  Idle --> Running: send\n  Running --> [*]",
            "classDiagram\n  class Harness {\n    +start() Run\n  }\n  Harness <|.. Codex",
            "erDiagram\n  CHAT ||--|{ MESSAGE : has\n  CHAT {\n    string id\n  }",
            "gantt\n  dateFormat YYYY-MM-DD\n  section Build\n  Engine :a1, 2026-01-01, 10d",
        ];
        for dark in [false, true] {
            let palette = Palette::from_request(&request(FLOWCHART, dark));
            for source in sources {
                let svg = render(source, &palette).unwrap();
                let inked = format!("fill:{};", palette.node);
                for text in svg.split("<text").skip(1) {
                    let tag = &text[..text.find('>').unwrap_or(text.len())];
                    assert!(
                        !tag.contains(&format!("fill=\"{}\"", palette.node)) && !tag.contains(&inked),
                        "{source:?} (dark {dark}): a label inked in the node's fill: {tag}"
                    );
                }
            }
        }
    }

    /// A light `classDef` fill (written for Mermaid's light theme) keeps a
    /// readable label in dark; an ink the source chose that reads is kept.
    #[test]
    fn labels_on_a_source_s_fills_stay_legible() {
        let palette = Palette::from_request(&request(FLOWCHART, true));
        let source = "flowchart LR\n  A[Pale] --> B[Chosen]\n  classDef pale fill:#dbeafe,stroke:#1d4ed8\n  classDef chosen fill:#fde68a,color:#78350f\n  class A pale\n  class B chosen";
        let svg = render(source, &palette).unwrap();
        let fill = Rgba::parse("#dbeafe").unwrap();
        let inks: Vec<Rgba> = svg
            .split("<text")
            .skip(1)
            .filter_map(|t| {
                let tag = &t[..t.find('>')?];
                let at = tag.find(" fill=\"#")?;
                Rgba::parse(&tag[at + 7..at + 14])
            })
            .collect();
        assert!(inks.iter().all(|ink| ink.contrast(fill) >= 3. || ink.hex() == "#78350f"), "{inks:?}");
        assert!(inks.iter().any(|ink| ink.hex() == "#78350f"), "the source's own ink is kept");
    }

    /// A mindmap's deep branches are drawn (Mermaid sizes them negative
    /// past the fourth level) and its sections aren't black.
    #[test]
    fn mindmap_branches_are_all_drawn() {
        let palette = Palette::from_request(&request(FLOWCHART, true));
        let source = "mindmap\n  root((Root))\n    A\n      B\n        C\n          D";
        let svg = render(source, &palette).unwrap();
        assert!(svg.contains(".edge-depth-5") && svg.contains("stroke-width: 2"), "deep edges get a width");
        assert!(!svg.contains("hsl(240, 4%, 0%)") || svg.contains(".section-0 rect"), "sections are coloured");
        let pics = draw(&svg, &palette).unwrap();
        assert!(pics.w > 50.);
    }

    /// A flowchart's own decisions become question cards; a styled one, a
    /// hexagon, a shape, and braces in quotes or edge labels stay put.
    #[test]
    fn decisions_become_cards_unless_the_source_says_otherwise() {
        let palette = Palette::from_request(&request(FLOWCHART, true));
        let source = "flowchart TD\n  A{Ready?} -->|\"x{y}\"| B{\"Quoted?\"}\n  B --> C{Styled}\n  C --> D{{Hex}}\n  D --> E@{ shape: diam }\n  E --> F[\"a {b} c\"]\n  F --> G{Inline}:::mine\n  style C fill:#f00";
        let soft = soften_decisions(source, &palette).unwrap();
        assert!(soft.contains("A(\"Ready?\")") && soft.contains("B(\"Quoted?\")"), "{soft}");
        assert!(soft.contains("C{Styled}") && soft.contains("D{{Hex}}") && soft.contains("E@{ shape: diam }"), "{soft}");
        assert!(soft.contains("F[\"a {b} c\"]") && soft.contains("G{Inline}:::mine") && soft.contains("|\"x{y}\"|"), "{soft}");
        assert!(soft.trim_end().ends_with("class A,B zeronDecision"), "{soft}");
        // It still draws, with no diamond for them.
        let svg = render(source, &palette).unwrap();
        assert!(svg.contains("zeronDecision"));
        // Other diagrams, and a flowchart with none, are left alone.
        assert_eq!(soften_decisions(SEQUENCE, &palette), None);
        assert_eq!(soften_decisions("flowchart LR\n  A --> B", &palette), None);
    }

    /// A rewrite Mermaid can't parse falls back to the source as written.
    #[test]
    fn a_decision_rewrite_that_fails_draws_the_source() {
        let palette = Palette::from_request(&request(FLOWCHART, false));
        assert!(render("flowchart TD\n  A{Ready?} --> B\n  click A call x()", &palette).is_ok());
    }

    /// Edge labels sit on pills sized to them; arrowheads are open
    /// chevrons; edges are routed by ELK.
    #[test]
    fn labels_are_pills_and_arrows_are_fine() {
        let palette = Palette::from_request(&request(FLOWCHART, true));
        let svg = render(FLOWCHART, &palette).unwrap();
        assert!(svg.matches("class=\"zeron-badge\"").count() >= 2, "a pill per labelled edge");
        assert!(!svg.contains("d=\"M 0 0 L 10 5 L 0 10 z\""), "no solid wedge");
        assert!(svg.contains("M 2 1.5 L 9 5 L 2 8.5"));
        assert_eq!(label_size("<g class=\"label\" transform=\"translate(-18.5,-10.5)\">"), Some((37., 21.)));
        assert_eq!(label_size("<g class=\"label\" transform=\"translate(0,0)\">"), None);
        assert!(draw(&svg, &palette).is_ok());
    }

    /// Save writes the vectors for a `.svg` name, else copies a PNG this
    /// module drew; it copies no other file.
    #[test]
    fn save_writes_only_diagrams() {
        let out = scratch("save");
        std::fs::create_dir_all(&out).unwrap();
        let to_svg = out.join("d.svg");
        save(to_svg.display().to_string(), String::new(), "<svg xmlns=\"http://www.w3.org/2000/svg\"/>".into()).unwrap();
        assert!(std::fs::read_to_string(&to_svg).unwrap().starts_with("<svg"));
        let secret = out.join("secret.plate.png");
        std::fs::write(&secret, b"not a diagram").unwrap();
        let err = save(out.join("x.png").display().to_string(), secret.display().to_string(), String::new()).unwrap_err();
        assert_eq!(err, "Not a diagram's picture");
        assert!(save("relative.png".into(), String::new(), String::new()).is_err());
        let _ = std::fs::remove_dir_all(&out);
    }

    /// Class diagrams with annotations (`<<interface>>`), which the old
    /// renderer refused, draw.
    #[test]
    fn class_annotations_draw() {
        let palette = Palette::from_request(&request(FLOWCHART, true));
        let source = "classDiagram\n  class Harness {\n    <<trait>>\n    +start(cfg) Run\n  }\n  Harness <|.. Codex";
        let svg = render(source, &palette).unwrap();
        assert!(draw(&svg, &palette).is_ok());
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
        // Nodes as Zeron's cards, in the palette; no foreign HTML for usvg.
        assert!(svg.contains("<rect rx=\"8\" ry=\"8\" "));
        assert!(svg.contains(&palette.node) && svg.contains(&palette.accent_wash));
        assert!(!svg.contains("<foreignObject"));
    }
}
