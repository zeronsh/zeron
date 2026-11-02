//! Pixel-art scenes painted behind the sidebar (aquarium, night sky,
//! sakura) plus user themes. A scene is a [`DecorSpec`]: the built-ins are
//! written in the same JSON a user drops into `<data dir>/sidebar-themes/`,
//! so a custom theme can do everything a built-in does.
//!
//! The scene is one `canvas` behind the cached sidebar pane. While it
//! animates, a single task repaints the windows that drew it at a low,
//! pixel-art cadence; it stops on its own when nothing draws a scene
//! (decoration off, sidebar collapsed, reduced motion, energy saver).
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    AnyElement, AnyWindowHandle, App, BorderStyle, Bounds, Hsla, IntoElement, Pixels, Point,
    Styled, Window, canvas, fill, linear_color_stop, linear_gradient, point, px, quad, size,
};
use serde::{Deserialize, Serialize};

pub const DEFAULT_THEME: &str = "aquarium";
pub const OFF: &str = "off";
/// Built-in scenes: (id, label).
pub const BUILT_IN: [(&str, &str); 3] = [
    ("aquarium", "Aquarium"),
    ("night", "Night sky"),
    ("sakura", "Sakura"),
];
const CUSTOM_PREFIX: &str = "custom:";
const THEMES_DIR: &str = "sidebar-themes";
/// ~12 fps: smooth enough for drifting fish, cheap enough to leave on.
const FRAME: Duration = Duration::from_millis(83);
/// A window that stopped drawing the scene drops off the repaint list.
const DEMAND: Duration = Duration::from_millis(600);
/// Frozen scenes (reduced motion) show this moment, mid-swim.
const FROZEN_AT: f32 = 14.0;

/// Tile edge and pitch for floor, plants and flowers.
const TILE: f32 = 6.0;
const PITCH: f32 = 7.0;
/// Fish are drawn finer.
const FISH_TILE: f32 = 4.0;
const FISH_PITCH: f32 = 5.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ParticleKind {
    #[default]
    Bubbles,
    Petals,
    Fireflies,
    Snow,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ParticleSpec {
    pub kind: ParticleKind,
    pub count: usize,
    pub colors: Vec<String>,
}

/// A sidebar scene. Colors are `#rgb`, `#rrggbb` or `#rrggbbaa`; any part
/// left out is simply not drawn.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DecorSpec {
    pub name: String,
    /// Wash under the traffic lights, fading down.
    pub top_glow: Option<String>,
    pub top_glow_height: f32,
    /// Wash rising from the bottom edge.
    pub bottom_glow: Option<String>,
    /// Slanted light beams from the top (aquarium sunlight).
    pub light_rays: Option<String>,
    /// Twinkling stars in the top band.
    pub stars: usize,
    pub star_color: Option<String>,
    pub moon: Option<String>,
    /// Silhouette layers along the bottom, back to front.
    pub hills: Vec<String>,
    /// Floor tile colors (mixed at random).
    pub floor: Vec<String>,
    /// Swaying seaweed / grass stalks.
    pub plants: Vec<String>,
    /// Flowers on stems.
    pub flowers: Vec<String>,
    /// One fish per color.
    pub fish: Vec<String>,
    pub particles: Option<ParticleSpec>,
    /// Overall strength, 0-1.
    pub opacity: f32,
}

impl Default for DecorSpec {
    fn default() -> Self {
        Self {
            name: "Custom".into(),
            top_glow: None,
            top_glow_height: 120.0,
            bottom_glow: None,
            light_rays: None,
            stars: 0,
            star_color: None,
            moon: None,
            hills: Vec::new(),
            floor: Vec::new(),
            plants: Vec::new(),
            flowers: Vec::new(),
            fish: Vec::new(),
            particles: None,
            opacity: 1.0,
        }
    }
}

/// The built-in scene for `id`, as a spec (also the template written for
/// users into the themes folder).
pub fn built_in(id: &str) -> Option<DecorSpec> {
    let value = match id {
        "aquarium" => serde_json::json!({
            "name": "Aquarium",
            "topGlow": "#1d6f6c70",
            "topGlowHeight": 150.0,
            "bottomGlow": "#12494640",
            "lightRays": "#62d6c8",
            "floor": ["#b59a4a", "#9c8a3c", "#c9a85a", "#7d8f3f"],
            "plants": ["#3f9e7a", "#2f8a6a", "#4fb08a"],
            "flowers": ["#c45a8a", "#9a6ad0", "#d0506a"],
            "fish": ["#3fb8a8", "#b8a040", "#9a6ad0", "#d08a3a"],
            "particles": { "kind": "bubbles", "count": 10, "colors": ["#a6dcd4"] },
            "opacity": 0.85
        }),
        "night" => serde_json::json!({
            "name": "Night sky",
            "topGlow": "#4b36a890",
            "topGlowHeight": 110.0,
            "bottomGlow": "#2a1f6a40",
            "stars": 34,
            "starColor": "#ddd8ff",
            "moon": "#f1ebc8",
            "hills": ["#2a2366", "#1a1546"],
            "particles": { "kind": "fireflies", "count": 7, "colors": ["#ece27a"] },
            "opacity": 0.9
        }),
        "sakura" => serde_json::json!({
            "name": "Sakura",
            "topGlow": "#a8487066",
            "topGlowHeight": 130.0,
            "bottomGlow": "#5a2a4030",
            "hills": ["#3a2234"],
            "floor": ["#6a4f3a", "#5a4632", "#7a5a40"],
            "plants": ["#4f8a5a", "#3f7a4a"],
            "flowers": ["#f0a0c0", "#e88ab0", "#f6c0d4"],
            "particles": { "kind": "petals", "count": 14, "colors": ["#f4b6cc", "#f8d0de"] },
            "opacity": 0.85
        }),
        _ => return None,
    };
    serde_json::from_value(value).ok()
}

pub fn themes_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(THEMES_DIR)
}

pub fn custom_id(stem: &str) -> String {
    format!("{CUSTOM_PREFIX}{stem}")
}

/// User themes in the themes folder as (id, label), sorted by label.
pub fn custom_themes(data_dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(themes_dir(data_dir)) else {
        return Vec::new();
    };
    let mut themes: Vec<(String, String)> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter_map(|path| {
            let stem = path.file_stem()?.to_str()?.to_owned();
            let name = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<DecorSpec>(&text).ok())
                .map(|spec| spec.name)
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| stem.clone());
            Some((custom_id(&stem), name))
        })
        .collect();
    themes.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    themes
}

/// Make the themes folder (with an example to copy) and reveal it.
pub fn open_themes_folder(data_dir: &Path) {
    let dir = themes_dir(data_dir);
    let _ = std::fs::create_dir_all(&dir);
    let example = dir.join("example.json");
    let has_theme = std::fs::read_dir(&dir).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
    });
    if !has_theme && let Some(mut spec) = built_in(DEFAULT_THEME) {
        spec.name = "My aquarium".into();
        if let Ok(text) = serde_json::to_string_pretty(&spec) {
            let _ = std::fs::write(&example, text);
        }
    }
    let _ = std::process::Command::new("/usr/bin/open").arg(&dir).spawn();
}

/// Pure: `#rgb`, `#rrggbb`, `#rrggbbaa` to a color.
pub fn parse_color(hex: &str) -> Option<Hsla> {
    let hex = hex.trim().strip_prefix('#')?;
    let digits: Vec<u8> = hex
        .chars()
        .map(|c| c.to_digit(16).map(|d| d as u8))
        .collect::<Option<_>>()?;
    let (r, g, b, a) = match digits.as_slice() {
        [r, g, b] => (r * 17, g * 17, b * 17, 255),
        [r1, r2, g1, g2, b1, b2] => (r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2, 255),
        [r1, r2, g1, g2, b1, b2, a1, a2] => {
            (r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2, a1 * 16 + a2)
        }
        _ => return None,
    };
    Some(
        gpui::Rgba {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
        .into(),
    )
}

/// A spec with its colors parsed, ready to paint.
#[derive(Debug)]
struct Scene {
    top_glow: Option<Hsla>,
    top_glow_height: f32,
    bottom_glow: Option<Hsla>,
    light_rays: Option<Hsla>,
    stars: usize,
    star_color: Hsla,
    moon: Option<Hsla>,
    hills: Vec<Hsla>,
    floor: Vec<Hsla>,
    plants: Vec<Hsla>,
    flowers: Vec<Hsla>,
    fish: Vec<Hsla>,
    particles: Option<(ParticleKind, usize, Vec<Hsla>)>,
    opacity: f32,
}

impl Scene {
    fn new(spec: &DecorSpec) -> Self {
        let one = |hex: &Option<String>| hex.as_deref().and_then(parse_color);
        let many = |list: &[String]| list.iter().filter_map(|hex| parse_color(hex)).collect();
        Self {
            top_glow: one(&spec.top_glow),
            top_glow_height: spec.top_glow_height.clamp(0.0, 600.0),
            bottom_glow: one(&spec.bottom_glow),
            light_rays: one(&spec.light_rays),
            stars: spec.stars.min(200),
            star_color: one(&spec.star_color).unwrap_or(gpui::white()),
            moon: one(&spec.moon),
            hills: many(&spec.hills),
            floor: many(&spec.floor),
            plants: many(&spec.plants),
            flowers: many(&spec.flowers),
            fish: spec.fish.iter().take(12).filter_map(|h| parse_color(h)).collect(),
            particles: spec.particles.as_ref().map(|particles| {
                let colors: Vec<Hsla> = many(&particles.colors);
                (
                    particles.kind,
                    particles.count.min(80),
                    if colors.is_empty() {
                        vec![gpui::white()]
                    } else {
                        colors
                    },
                )
            }),
            opacity: spec.opacity.clamp(0.0, 1.0),
        }
    }
}

/// The scene for the selected id; `None` when decoration is off or a
/// custom file is gone or unreadable. Custom files reload when they change.
fn scene_for(id: &str, cx: &App) -> Option<Arc<Scene>> {
    static CACHE: Mutex<Option<(String, Option<SystemTime>, Option<Arc<Scene>>)>> =
        Mutex::new(None);
    if id == OFF || id.is_empty() {
        return None;
    }
    let custom_path = id
        .strip_prefix(CUSTOM_PREFIX)
        .and_then(|stem| Some(themes_dir(&crate::settings::data_dir(cx)?).join(format!("{stem}.json"))));
    let stamp = custom_path
        .as_ref()
        .and_then(|path| std::fs::metadata(path).ok()?.modified().ok());
    let mut cache = CACHE.lock().ok()?;
    if let Some((cached_id, cached_stamp, scene)) = cache.as_ref()
        && cached_id == id
        && *cached_stamp == stamp
    {
        return scene.clone();
    }
    let spec = match &custom_path {
        Some(path) => std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str::<DecorSpec>(&text).ok()),
        None => built_in(id),
    };
    let scene = spec.map(|spec| Arc::new(Scene::new(&spec)));
    *cache = Some((id.to_owned(), stamp, scene.clone()));
    scene
}

/// The decoration layer for the sidebar column, or `None` when off.
pub fn render(dark: bool, cx: &App) -> Option<AnyElement> {
    let id = crate::settings::sidebar_decor(cx);
    let scene = scene_for(&id, cx)?;
    let frozen = crate::motion::reduced_motion(cx);
    let strength = scene.opacity * if dark { 1.0 } else { 0.55 };
    Some(
        canvas(
            |_, _, _| {},
            move |bounds, _, window, cx| {
                let t = if frozen {
                    FROZEN_AT
                } else {
                    start().elapsed().as_secs_f32()
                };
                let painted = paint(&scene, bounds, t, strength, window);
                // A collapsed sidebar or a background window holds still;
                // the next ordinary repaint picks the motion back up.
                if painted && !frozen && window.is_window_active() {
                    note_painted(window.window_handle(), cx);
                }
            },
        )
        .size_full()
        .into_any_element(),
    )
}

fn start() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

/// Pure, deterministic 0..1 noise for scene layout.
fn hash(i: u32, salt: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B).wrapping_add(0x27D4_EB2F);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7FEB_352D);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846C_A68B);
    x ^= x >> 16;
    x as f32 / u32::MAX as f32
}

fn pick(colors: &[Hsla], i: u32, salt: u32) -> Hsla {
    colors[((hash(i, salt) * colors.len() as f32) as usize).min(colors.len() - 1)]
}

struct Painter<'a> {
    window: &'a mut Window,
    origin: Point<Pixels>,
    strength: f32,
}

impl Painter<'_> {
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32) -> Bounds<Pixels> {
        Bounds::new(
            point(self.origin.x + px(x), self.origin.y + px(y)),
            size(px(w), px(h)),
        )
    }

    /// One glyph-like tile: a hairline box, a faint fill and a center dot.
    fn tile(&mut self, x: f32, y: f32, s: f32, color: Hsla, alpha: f32) {
        let alpha = alpha * self.strength;
        if alpha <= 0.02 {
            return;
        }
        let bounds = self.rect(x, y, s, s);
        self.window.paint_quad(quad(
            bounds,
            px(1.0),
            color.opacity(alpha * 0.3),
            px(0.8),
            color.opacity(alpha),
            BorderStyle::Solid,
        ));
        if s >= 5.0 {
            let dot = s * 0.34;
            let inner = self.rect(x + (s - dot) / 2.0, y + (s - dot) / 2.0, dot, dot);
            self.window.paint_quad(fill(inner, color.opacity(alpha * 0.7)));
        }
    }

    fn dot(&mut self, x: f32, y: f32, d: f32, color: Hsla, alpha: f32) {
        let alpha = alpha * self.strength;
        if alpha <= 0.02 {
            return;
        }
        let bounds = self.rect(x - d / 2.0, y - d / 2.0, d, d);
        self.window
            .paint_quad(quad(bounds, px(d / 2.0), color.opacity(alpha), px(0.0), gpui::transparent_black(), BorderStyle::Solid));
    }

    fn ring(&mut self, x: f32, y: f32, d: f32, color: Hsla, alpha: f32) {
        let alpha = alpha * self.strength;
        if alpha <= 0.02 {
            return;
        }
        let bounds = self.rect(x - d / 2.0, y - d / 2.0, d, d);
        self.window.paint_quad(quad(
            bounds,
            px(d / 2.0),
            color.opacity(alpha * 0.15),
            px(0.9),
            color.opacity(alpha),
            BorderStyle::Solid,
        ));
    }

    /// A vertical wash: `color` at `from_top`, clear at the other end.
    fn wash(&mut self, y: f32, w: f32, h: f32, color: Hsla, from_top: bool) {
        let color = color.opacity(self.strength);
        let bounds = self.rect(0.0, y, w, h);
        let (a, b) = if from_top {
            (color, color.opacity(0.0))
        } else {
            (color.opacity(0.0), color)
        };
        self.window.paint_quad(fill(
            bounds,
            linear_gradient(180.0, linear_color_stop(a, 0.0), linear_color_stop(b, 1.0)),
        ));
    }
}

/// Fish facing right: `#` body, `o` eye.
const FISH: [&str; 5] = ["#..####.", "##.###o#", "########", "##.#####", "#..####."];

/// Draw the scene; false when the column is too small to hold one.
fn paint(scene: &Scene, bounds: Bounds<Pixels>, t: f32, strength: f32, window: &mut Window) -> bool {
    let w = f32::from(bounds.size.width);
    let h = f32::from(bounds.size.height);
    if w < 40.0 || h < 80.0 {
        return false;
    }
    let mut p = Painter {
        window,
        origin: bounds.origin,
        strength,
    };
    let floor_rows = if scene.floor.is_empty() { 0.0 } else { 2.0 };
    let floor_top = h - floor_rows * PITCH;

    // Washes.
    if let Some(glow) = scene.top_glow {
        p.wash(0.0, w, scene.top_glow_height, glow, true);
    }
    if let Some(glow) = scene.bottom_glow {
        let gh = (h * 0.35).min(320.0);
        p.wash(h - gh, w, gh, glow, false);
    }

    // Sunlight: stepped diagonal beams fading down.
    if let Some(ray) = scene.light_rays {
        for k in 0..3u32 {
            let base = w * (0.08 + 0.32 * k as f32) + (t * 0.18 + k as f32 * 2.1).sin() * 12.0;
            let rows = 34;
            for row in 0..rows {
                let fade = 1.0 - row as f32 / rows as f32;
                let y = row as f32 * 10.0;
                let x = base + row as f32 * 3.2;
                let a = 0.07 * fade * strength;
                let bounds = p.rect(x, y, 22.0, 10.0);
                p.window.paint_quad(fill(bounds, ray.opacity(a)));
            }
        }
    }

    // Moon with a soft halo, top right.
    if let Some(moon) = scene.moon {
        let (mx, my) = (w - 34.0, 22.0);
        p.dot(mx, my, 34.0, moon, 0.08);
        p.dot(mx, my, 20.0, moon, 0.12);
        p.dot(mx, my, 11.0, moon, 0.9);
    }

    // Stars in the top band.
    let band = scene.top_glow_height.max(80.0) * 0.9;
    for i in 0..scene.stars as u32 {
        let x = hash(i, 1) * w;
        let y = 4.0 + hash(i, 2) * band;
        let speed = 0.5 + hash(i, 3) * 1.6;
        let twinkle = 0.3 + 0.7 * (0.5 + 0.5 * (t * speed + i as f32).sin());
        let fade = 1.0 - (y / band) * 0.6;
        if hash(i, 4) > 0.86 {
            let a = twinkle * fade;
            p.dot(x, y, 2.4, scene.star_color, a);
            let arm = 2.6;
            let c = scene.star_color.opacity(a * 0.5 * strength);
            let hb = p.rect(x - arm - 1.2, y - 0.5, arm * 2.0 + 2.4, 1.0);
            p.window.paint_quad(fill(hb, c));
            let vb = p.rect(x - 0.5, y - arm - 1.2, 1.0, arm * 2.0 + 2.4);
            p.window.paint_quad(fill(vb, c));
        } else {
            p.dot(x, y, 1.6, scene.star_color, twinkle * fade * 0.8);
        }
    }

    // Hills: back layers higher and fainter.
    let layers = scene.hills.len();
    for (layer, color) in scene.hills.iter().enumerate() {
        let depth = (layers - layer) as f32;
        let cols = (w / PITCH).ceil() as u32 + 1;
        for c in 0..cols {
            let phase = c as f32 * (0.18 + 0.05 * layer as f32) + layer as f32 * 1.7;
            let height = (2.0 + depth * 1.6 + phase.sin() * 1.8 + hash(c, 40 + layer as u32) * 0.9)
                .max(1.0) as u32;
            for r in 0..height {
                let y = floor_top - (r + 1) as f32 * PITCH;
                let alpha = if r + 1 == height { 0.55 } else { 0.42 };
                p.tile(c as f32 * PITCH, y, TILE, *color, alpha * (0.7 + 0.3 / depth));
            }
        }
    }

    // Floor: two rows, a ragged top edge.
    if !scene.floor.is_empty() {
        let cols = (w / PITCH).ceil() as u32 + 1;
        for c in 0..cols {
            for r in 0..2u32 {
                if r == 0 && hash(c, 7) < 0.22 {
                    continue;
                }
                let color = pick(&scene.floor, c * 3 + r, 8);
                let y = floor_top + r as f32 * PITCH;
                p.tile(c as f32 * PITCH, y, TILE, color, if r == 0 { 0.5 } else { 0.65 });
            }
        }
    }

    // Plants: stalks swaying more toward the tip.
    if !scene.plants.is_empty() {
        let count = ((w / 46.0) as u32).max(2);
        for i in 0..count {
            let x0 = (i as f32 + 0.5) * w / count as f32 + (hash(i, 11) - 0.5) * 22.0;
            let x0 = (x0 / PITCH).round() * PITCH;
            let tall = 5 + (hash(i, 12) * 10.0) as u32;
            let color = pick(&scene.plants, i, 13);
            for k in 0..tall {
                let lift = k as f32 / tall as f32;
                let sway = (t * 0.9 + i as f32 * 1.3 + k as f32 * 0.35).sin() * lift * 1.5;
                let x = x0 + sway.round() * PITCH;
                let y = floor_top - (k + 1) as f32 * PITCH;
                p.tile(x, y, TILE, color, 0.55 - lift * 0.15);
                if k % 3 == 1 && k + 1 < tall {
                    let side = if hash(i * 31 + k, 14) > 0.5 { PITCH } else { -PITCH };
                    p.tile(x + side, y, TILE, color, 0.42);
                }
            }
        }
    }

    // Flowers: a short stem and a small bloom.
    if !scene.flowers.is_empty() {
        let count = ((w / 95.0) as u32).max(1) + 1;
        let stem = scene.plants.first().copied().unwrap_or(scene.flowers[0]);
        for i in 0..count {
            let x0 = ((hash(i, 21) * (w - 3.0 * PITCH) + PITCH) / PITCH).round() * PITCH;
            let tall = 3 + (hash(i, 22) * 5.0) as u32;
            let color = pick(&scene.flowers, i, 23);
            let sway = ((t * 0.6 + i as f32 * 2.0).sin() * 0.6).round() * PITCH;
            for k in 0..tall {
                let y = floor_top - (k + 1) as f32 * PITCH;
                let x = if k + 2 >= tall { x0 + sway } else { x0 };
                p.tile(x, y, TILE, stem, 0.5);
            }
            let cy = floor_top - (tall + 2) as f32 * PITCH;
            let cx = x0 + sway;
            for (dx, dy) in [(0, -1), (-1, 0), (1, 0), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
                let alpha = if dx != 0 && dy != 0 { 0.38 } else { 0.62 };
                p.tile(cx + dx as f32 * PITCH, cy + dy as f32 * PITCH, TILE, color, alpha);
            }
            let center = Hsla { l: (color.l + 0.25).min(0.9), ..color };
            p.tile(cx, cy, TILE, center, 0.75);
        }
    }

    // Fish: back and forth across the lower half, bobbing.
    let fish_w = FISH[0].len() as f32 * FISH_PITCH;
    for (i, color) in scene.fish.iter().enumerate() {
        let i = i as u32;
        let speed = 9.0 + hash(i, 31) * 12.0;
        let span = w + fish_w * 2.0;
        let pos = (t * speed + hash(i, 32) * span * 2.0) % (span * 2.0);
        let (x, right) = if pos < span {
            (pos - fish_w, true)
        } else {
            (span * 2.0 - pos - fish_w, false)
        };
        let lane = 0.5 + hash(i, 33) * 0.32;
        let y = (h * lane).min(floor_top - 60.0) + (t * 0.8 + i as f32 * 1.9).sin() * 6.0;
        for (row, line) in FISH.iter().enumerate() {
            for (col, cell) in line.chars().enumerate() {
                let col = if right { col } else { line.len() - 1 - col };
                let (tx, ty) = (x + col as f32 * FISH_PITCH, y + row as f32 * FISH_PITCH);
                match cell {
                    '#' => p.tile(tx, ty, FISH_TILE, *color, 0.6),
                    'o' => p.tile(tx, ty, FISH_TILE, Hsla { l: 0.12, ..*color }, 0.8),
                    _ => {}
                }
            }
        }
    }

    // Particles.
    if let Some((kind, count, colors)) = &scene.particles {
        for i in 0..*count as u32 {
            let color = pick(colors, i, 51);
            let period = 9.0 + hash(i, 52) * 8.0;
            let phase = (t / period + hash(i, 53)).fract();
            let base_x = hash(i, 54) * w;
            match kind {
                ParticleKind::Bubbles => {
                    let x = base_x + (t * 1.3 + i as f32).sin() * 4.0;
                    let y = floor_top - phase * h * 0.85;
                    let d = 3.5 + hash(i, 55) * 3.0;
                    p.ring(x, y, d, color, (1.0 - phase) * 0.75);
                }
                ParticleKind::Petals | ParticleKind::Snow => {
                    let x = base_x + (t * 0.8 + i as f32 * 1.7).sin() * 18.0;
                    let y = phase * (h + 20.0) - 10.0;
                    let fade = (phase * 6.0).min(1.0) * (1.0 - phase * 0.5);
                    if *kind == ParticleKind::Petals {
                        let tilt = (t * 1.6 + i as f32).sin();
                        let (pw, ph) = (4.5 + tilt * 1.5, 3.0);
                        let a = fade * 0.75 * strength;
                        let bounds = p.rect(x, y, pw.max(2.0), ph);
                        p.window.paint_quad(quad(
                            bounds,
                            px(1.5),
                            color.opacity(a),
                            px(0.0),
                            gpui::transparent_black(),
                            BorderStyle::Solid,
                        ));
                    } else {
                        p.dot(x, y, 2.0 + hash(i, 56) * 2.0, color, fade * 0.7);
                    }
                }
                ParticleKind::Fireflies => {
                    let x = base_x + (t * 0.3 + i as f32 * 2.0).sin() * 20.0;
                    let y = h * (0.58 + 0.3 * hash(i, 57)) + (t * 0.4 + i as f32).cos() * 16.0;
                    let glow = (t * 1.4 + i as f32 * 1.7).sin().max(0.0);
                    p.dot(x, y, 10.0, color, glow * 0.14);
                    p.dot(x, y, 2.6, color, 0.2 + glow * 0.8);
                }
            }
        }
    }
    true
}

struct Driver {
    windows: Vec<(AnyWindowHandle, Instant)>,
    running: bool,
}

static DRIVER: Mutex<Driver> = Mutex::new(Driver {
    windows: Vec::new(),
    running: false,
});

/// Keep repainting `handle` while it keeps drawing an animated scene.
fn note_painted(handle: AnyWindowHandle, cx: &mut App) {
    let start_task = {
        let Ok(mut driver) = DRIVER.lock() else {
            return;
        };
        let now = Instant::now();
        match driver.windows.iter_mut().find(|(window, _)| *window == handle) {
            Some((_, at)) => *at = now,
            None => driver.windows.push((handle, now)),
        }
        !std::mem::replace(&mut driver.running, true)
    };
    if start_task {
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(FRAME).await;
                let windows: Vec<AnyWindowHandle> = {
                    let Ok(mut driver) = DRIVER.lock() else {
                        break;
                    };
                    driver.windows.retain(|(_, at)| at.elapsed() < DEMAND);
                    if driver.windows.is_empty() {
                        driver.running = false;
                        break;
                    }
                    driver.windows.iter().map(|(window, _)| *window).collect()
                };
                cx.update(|cx| {
                    for handle in windows {
                        let _ = handle.update(cx, |_, window, _| window.refresh());
                    }
                });
            }
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_ins_parse_and_have_labels() {
        for (id, label) in BUILT_IN {
            let spec = built_in(id).unwrap_or_else(|| panic!("{id} parses"));
            assert_eq!(spec.name, label);
            let scene = Scene::new(&spec);
            assert!(scene.opacity > 0.0);
        }
        assert!(built_in(OFF).is_none());
        assert!(built_in(DEFAULT_THEME).is_some());
    }

    #[test]
    fn colors_parse_in_three_lengths() {
        let full = parse_color("#ff0000").unwrap();
        assert!((full.a - 1.0).abs() < 1e-3);
        let short = parse_color("#f00").unwrap();
        assert!((short.h - full.h).abs() < 1e-3);
        let alpha = parse_color("#ff000080").unwrap();
        assert!((alpha.a - 128.0 / 255.0).abs() < 1e-3);
        assert!(parse_color("red").is_none());
        assert!(parse_color("#12345").is_none());
    }

    #[test]
    fn a_partial_custom_theme_fills_the_rest_with_defaults() {
        let spec: DecorSpec =
            serde_json::from_str(r##"{ "name": "Mine", "fish": ["#ffcc00"] }"##).unwrap();
        assert_eq!(spec.name, "Mine");
        assert_eq!(spec.fish.len(), 1);
        assert!(spec.floor.is_empty());
        assert!((spec.opacity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn custom_themes_list_json_files_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let themes = themes_dir(dir.path());
        std::fs::create_dir_all(&themes).unwrap();
        std::fs::write(themes.join("zz.json"), r#"{ "name": "Alpha" }"#).unwrap();
        std::fs::write(themes.join("broken.json"), "not json").unwrap();
        std::fs::write(themes.join("notes.txt"), "ignored").unwrap();
        assert_eq!(
            custom_themes(dir.path()),
            vec![
                (custom_id("zz"), "Alpha".to_owned()),
                (custom_id("broken"), "broken".to_owned()),
            ]
        );
    }

    #[test]
    fn noise_stays_in_unit_range() {
        for i in 0..500 {
            let v = hash(i, 9);
            assert!((0.0..=1.0).contains(&v));
        }
    }
}
