//! Desktop pet: an animated companion that floats above other apps and acts
//! out what the agents are doing, following Codex's pet contract.
//!
//! A pet is one sprite sheet (192×208 cells, 8 columns by default; v1 sheets
//! have 9 rows, v2 adds two look-direction rows we don't use). Animations are
//! named tracks of sprite indices — Codex's defaults, overridable from
//! `pet.json` — picked by the agents' status (`running`, `waiting`, `review`,
//! `failed`, else `idle`) and by the pointer, like the Codex app mascot: a
//! hover jumps, a grab holds the live state, a drag runs left or right.
//!
//! Pets are the bundled Rocky plus any `{data_dir}/pets/<id>/` folder holding
//! a `pet.json` (`displayName`, `description`, `spritesheetPath`, optional
//! `frame` grid and `animations`) and its PNG/WebP sheet.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use gpui::{
    App, Context, Entity, MouseButton, RenderImage, SharedString, Task, Window, WindowBounds,
    WindowHandle, WindowKind, WindowOptions, div, img, point, prelude::*, px, size,
};
use zeron_proto::ChatIndicator;

use crate::state::AppState;
use crate::theme::Theme;

pub const CELL_WIDTH: u32 = 192;
pub const CELL_HEIGHT: u32 = 208;
const COLUMNS: u32 = 8;
const SHEET_WIDTH: u32 = CELL_WIDTH * COLUMNS;
/// v1 sheets hold the 9 state rows; v2 adds 2 look-direction rows.
const SHEET_HEIGHTS: [u32; 2] = [CELL_HEIGHT * 9, CELL_HEIGHT * 11];
const MAX_SHEET_BYTES: u64 = 12 * 1024 * 1024;
const MAX_FRAMES: u32 = 256;
const MAX_FPS: f64 = 60.0;

pub const SIZE_MIN: f32 = 80.0;
pub const SIZE_MAX: f32 = 224.0;
pub const SIZE_DEFAULT: f32 = 112.0;
pub const DEFAULT_PET: &str = "rocky";

static ROCKY: &[u8] = include_bytes!("../assets/pets/rocky.webp");

// ---------------------------------------------------------------------------
// Animations (Codex: tui/src/pets/model.rs and ambient.rs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Frame {
    sprite: usize,
    duration: Duration,
}

/// A named track. `loop_start: None` is one-shot: once played it hands off
/// to `fallback`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Animation {
    frames: Vec<Frame>,
    loop_start: Option<usize>,
    fallback: String,
}

impl Animation {
    fn total(&self) -> Duration {
        self.frames.iter().map(|f| f.duration).sum()
    }
}

type Animations = HashMap<String, Animation>;

fn idle_animation() -> Animation {
    Animation {
        frames: [(0, 1680), (1, 660), (2, 660), (3, 840), (4, 840), (5, 1920)]
            .into_iter()
            .map(|(sprite, ms)| Frame {
                sprite,
                duration: Duration::from_millis(ms),
            })
            .collect(),
        loop_start: Some(0),
        fallback: "idle".into(),
    }
}

/// A sheet row played three times, then the idle loop.
fn state_animation(row: usize, count: usize, ms: u64, last_ms: u64) -> Animation {
    let burst: Vec<Frame> = (0..count)
        .map(|column| Frame {
            sprite: row * COLUMNS as usize + column,
            duration: Duration::from_millis(if column == count - 1 { last_ms } else { ms }),
        })
        .collect();
    let mut frames = burst.repeat(3);
    let loop_start = frames.len();
    frames.extend(idle_animation().frames);
    Animation {
        frames,
        loop_start: Some(loop_start),
        fallback: "idle".into(),
    }
}

fn default_animations() -> Animations {
    [
        ("running-right", 1, 8, 120, 220),
        ("running-left", 2, 8, 120, 220),
        ("waving", 3, 4, 140, 280),
        ("jumping", 4, 5, 140, 280),
        ("failed", 5, 8, 140, 240),
        ("waiting", 6, 6, 150, 260),
        ("running", 7, 6, 120, 220),
        ("review", 8, 6, 150, 280),
        // The Codex app's legacy names.
        ("move_right", 1, 8, 120, 220),
        ("move_left", 2, 8, 120, 220),
        ("wave", 3, 4, 140, 280),
        ("bounce", 4, 5, 140, 280),
        ("sad", 5, 8, 140, 240),
    ]
    .into_iter()
    .map(|(name, row, count, ms, last)| (name.to_owned(), state_animation(row, count, ms, last)))
    .chain([("idle".to_owned(), idle_animation())])
    .collect()
}

/// One `pet.json` track: sprite indices at a steady `fps`.
#[derive(serde::Deserialize)]
struct AnimationSpec {
    #[serde(default)]
    frames: Vec<usize>,
    fps: Option<f64>,
    #[serde(rename = "loop")]
    looping: Option<bool>,
    #[serde(default)]
    fallback: String,
}

/// Codex's defaults with the manifest's tracks on top, checked against the
/// sheet's `frame_count` cells.
fn load_animations(
    specs: HashMap<String, AnimationSpec>,
    frame_count: usize,
) -> Result<Animations, String> {
    let mut animations = default_animations();
    for (name, spec) in specs {
        let fps = spec.fps.unwrap_or(8.0);
        let duration = (fps.is_finite() && fps > 0.0 && fps <= MAX_FPS)
            .then(|| Duration::try_from_secs_f64(1.0 / fps).ok())
            .flatten()
            .ok_or_else(|| format!("animation {name} fps must be above 0 and at most {MAX_FPS}"))?;
        if spec.frames.is_empty() {
            return Err(format!("animation {name} needs at least one frame"));
        }
        let animation = Animation {
            frames: spec
                .frames
                .into_iter()
                .map(|sprite| Frame { sprite, duration })
                .collect(),
            loop_start: spec.looping.unwrap_or(true).then_some(0),
            fallback: if spec.fallback.is_empty() {
                "idle".into()
            } else {
                spec.fallback
            },
        };
        animations.insert(name, animation);
    }
    for (name, animation) in &animations {
        if let Some(frame) = animation.frames.iter().find(|f| f.sprite >= frame_count) {
            return Err(format!(
                "animation {name} uses sprite {}, but the sheet has {frame_count}",
                frame.sprite
            ));
        }
        if !animations.contains_key(&animation.fallback) {
            return Err(format!(
                "animation {name} falls back to missing {}",
                animation.fallback
            ));
        }
    }
    Ok(animations)
}

/// A sprite to show, and how long until the next (`None`: it stays).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tick {
    sprite: usize,
    delay: Option<Duration>,
}

/// Codex's `current_animation_frame`: the prefix plays once, then
/// `loop_start..` repeats; a one-shot stops on its last frame.
fn frame_at(animation: &Animation, mut elapsed: Duration) -> Option<Tick> {
    let last = animation.frames.last()?.sprite;
    let still = Tick {
        sprite: last,
        delay: None,
    };
    if animation.frames.len() == 1 {
        return Some(still);
    }
    let total = animation.total();
    if elapsed >= total {
        let Some(start) = animation
            .loop_start
            .filter(|&start| start < animation.frames.len())
        else {
            return Some(still);
        };
        let prefix: Duration = animation.frames[..start].iter().map(|f| f.duration).sum();
        let cycle = (total - prefix).as_nanos();
        if cycle == 0 {
            return Some(still);
        }
        elapsed = prefix + Duration::from_nanos(((elapsed - prefix).as_nanos() % cycle) as u64);
    }
    for frame in &animation.frames {
        if elapsed < frame.duration {
            return Some(Tick {
                sprite: frame.sprite,
                delay: Some(frame.duration - elapsed),
            });
        }
        elapsed -= frame.duration;
    }
    Some(still)
}

// ---------------------------------------------------------------------------
// Library
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PetSource {
    Bundled(&'static [u8]),
    File(PathBuf),
}

/// How the sheet is cut: `columns × rows` cells of `width × height` pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
struct Grid {
    width: u32,
    height: u32,
    columns: u32,
    rows: u32,
}

impl Grid {
    /// The standard grid for a sheet `height` pixels tall.
    fn standard(height: u32) -> Self {
        Self {
            width: CELL_WIDTH,
            height: CELL_HEIGHT,
            columns: COLUMNS,
            rows: height / CELL_HEIGHT,
        }
    }

    fn frame_count(&self) -> usize {
        (self.columns * self.rows) as usize
    }

    /// The grid must cover a `width × height` sheet exactly.
    fn check(&self, width: u32, height: u32) -> Result<(), String> {
        let covers = self.width.checked_mul(self.columns) == Some(width)
            && self.height.checked_mul(self.rows) == Some(height);
        if self.width == 0 || self.height == 0 || !covers {
            return Err(format!(
                "the frame grid must cover the {width}×{height} sprite sheet exactly"
            ));
        }
        if self.columns * self.rows > MAX_FRAMES {
            return Err(format!(
                "the sprite sheet has more than {MAX_FRAMES} frames"
            ));
        }
        Ok(())
    }
}

fn check_sheet_size(width: u32, height: u32) -> Result<(), String> {
    if width != SHEET_WIDTH || !SHEET_HEIGHTS.contains(&height) {
        return Err(format!(
            "the sprite sheet must be {SHEET_WIDTH}×{} or {SHEET_WIDTH}×{} pixels",
            SHEET_HEIGHTS[0], SHEET_HEIGHTS[1]
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PetInfo {
    pub id: String,
    pub name: String,
    pub description: String,
    pub source: PetSource,
    grid: Grid,
    animations: Arc<Animations>,
}

impl PetInfo {
    /// On-screen height for a pet `width` pixels wide.
    fn height_for(&self, width: f32) -> f32 {
        width * self.grid.height as f32 / self.grid.width as f32
    }
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    id: Option<String>,
    display_name: Option<String>,
    #[serde(default)]
    description: String,
    spritesheet_path: Option<String>,
    frame: Option<Grid>,
    #[serde(default)]
    animations: HashMap<String, AnimationSpec>,
}

pub fn pets_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("pets")
}

fn bundled() -> PetInfo {
    PetInfo {
        id: DEFAULT_PET.into(),
        name: "Rocky".into(),
        description: "The original companion.".into(),
        source: PetSource::Bundled(ROCKY),
        // Rocky is a v2 sheet.
        grid: Grid::standard(SHEET_HEIGHTS[1]),
        animations: Arc::new(default_animations()),
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id != DEFAULT_PET
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Every pet: Rocky first, then valid folders by id. Broken folders become
/// one-line issues instead of hiding the rest.
pub fn library(data_dir: Option<&Path>) -> (Vec<PetInfo>, Vec<String>) {
    let mut pets = vec![bundled()];
    let mut issues = Vec::new();
    let Some(dir) = data_dir.map(pets_dir) else {
        return (pets, issues);
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return (pets, issues);
    };
    let mut folders: Vec<_> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .filter(|(id, _)| valid_id(id))
        .collect();
    folders.sort();
    for (id, folder) in folders.into_iter().take(100) {
        match read_pet(&id, &folder) {
            Ok(Some(pet)) => pets.push(pet),
            Ok(None) => {}
            Err(error) => issues.push(format!("{id}: {error}")),
        }
    }
    (pets, issues)
}

/// `Ok(None)` for a folder without `pet.json` (still being written). Field
/// defaults follow Codex: the name falls back to the id, the sheet to
/// `spritesheet.webp`, the grid to the standard one.
fn read_pet(id: &str, folder: &Path) -> Result<Option<PetInfo>, String> {
    let Ok(text) = std::fs::read_to_string(folder.join("pet.json")) else {
        return Ok(None);
    };
    let manifest: Manifest =
        serde_json::from_str(&text).map_err(|e| format!("invalid pet.json ({e})"))?;
    let relative = manifest
        .spritesheet_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .unwrap_or("spritesheet.webp");
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("spritesheetPath must stay inside the pet's folder".into());
    }
    // Resolve symlinks so a link can't point the sheet outside the folder.
    let sheet = folder
        .join(relative)
        .canonicalize()
        .map_err(|_| "sprite sheet not found".to_string())?;
    let root = folder.canonicalize().map_err(|e| e.to_string())?;
    if !sheet.starts_with(&root) {
        return Err("spritesheetPath must stay inside the pet's folder".into());
    }
    let unreadable = |_| "the sprite sheet could not be read".to_string();
    let (width, height) = image::ImageReader::open(&sheet)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(unreadable)?
        .into_dimensions()
        .map_err(|_| "the sprite sheet could not be read".to_string())?;
    check_sheet_size(width, height)?;
    let grid = manifest.frame.unwrap_or_else(|| Grid::standard(height));
    grid.check(width, height)?;
    let animations = load_animations(manifest.animations, grid.frame_count())?;
    let name = [manifest.display_name.as_deref(), manifest.id.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|name| !name.is_empty())
        .unwrap_or(id);
    Ok(Some(PetInfo {
        id: id.to_owned(),
        name: name.chars().take(100).collect(),
        description: manifest.description.trim().chars().take(300).collect(),
        source: PetSource::File(sheet),
        grid,
        animations: Arc::new(animations),
    }))
}

/// The pet with `id`, falling back to Rocky.
pub fn find(id: &str, data_dir: Option<&Path>) -> PetInfo {
    library(data_dir)
        .0
        .into_iter()
        .find(|pet| pet.id == id)
        .unwrap_or_else(bundled)
}

fn decode_sheet(pet: &PetInfo) -> Result<image::RgbaImage, String> {
    let bytes: std::borrow::Cow<[u8]> = match &pet.source {
        PetSource::Bundled(bytes) => (*bytes).into(),
        PetSource::File(path) => {
            let len = std::fs::metadata(path).map_err(|e| e.to_string())?.len();
            if len > MAX_SHEET_BYTES {
                return Err("the sprite sheet is larger than 12 MB".into());
            }
            std::fs::read(path).map_err(|e| e.to_string())?.into()
        }
    };
    let sheet = image::load_from_memory(&bytes)
        .map_err(|_| "the sprite sheet could not be decoded".to_string())?
        .to_rgba8();
    check_sheet_size(sheet.width(), sheet.height())?;
    // The file may have changed since the library read it.
    pet.grid.check(sheet.width(), sheet.height())?;
    Ok(sheet)
}

/// Cell `sprite` (row-major) as a GPU-ready (BGRA) image.
fn cell(sheet: &image::RgbaImage, grid: Grid, sprite: usize) -> Arc<RenderImage> {
    let sprite = sprite as u32;
    let mut pixels = image::imageops::crop_imm(
        sheet,
        sprite % grid.columns * grid.width,
        sprite / grid.columns * grid.height,
        grid.width,
        grid.height,
    )
    .to_image();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new([image::Frame::new(pixels)]))
}

/// The first idle frame, for the settings list. Slow: call off the UI thread.
pub fn preview(pet: &PetInfo) -> Result<Arc<RenderImage>, String> {
    let sprite = pet
        .animations
        .get("idle")
        .and_then(|idle| idle.frames.first())
        .map_or(0, |frame| frame.sprite);
    Ok(cell(&decode_sheet(pet)?, pet.grid, sprite))
}

/// Every cell the animations use, by sprite index.
type Sprites = HashMap<usize, Arc<RenderImage>>;

/// Slow: call off the UI thread.
fn decode_sprites(pet: &PetInfo) -> Result<Sprites, String> {
    let sheet = decode_sheet(pet)?;
    let mut sprites = Sprites::new();
    for frame in pet.animations.values().flat_map(|a| &a.frames) {
        sprites
            .entry(frame.sprite)
            .or_insert_with(|| cell(&sheet, pet.grid, frame.sprite));
    }
    Ok(sprites)
}

// ---------------------------------------------------------------------------
// Mood from chat status
// ---------------------------------------------------------------------------

/// What the agents are doing, as Codex's pet notifications name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    Idle,
    Running,
    Waiting,
    Review,
    Failed,
}

impl Mood {
    fn animation(self) -> &'static str {
        match self {
            Mood::Idle => "idle",
            Mood::Running => "running",
            Mood::Waiting => "waiting",
            Mood::Review => "review",
            Mood::Failed => "failed",
        }
    }

    /// The bubble's second line when there is nothing more specific.
    fn fallback_body(self) -> &'static str {
        match self {
            Mood::Idle => "",
            Mood::Running => "Running",
            Mood::Waiting => "Needs input",
            Mood::Review => "Ready",
            Mood::Failed => "Blocked",
        }
    }

    /// How long the news stays worth acting out (Codex's notification
    /// lifetimes; running is live session state, so it has none).
    fn lifetime(self) -> Option<chrono::Duration> {
        match self {
            Mood::Waiting => Some(chrono::Duration::hours(24)),
            Mood::Review => Some(chrono::Duration::days(7)),
            Mood::Failed => Some(chrono::Duration::hours(1)),
            Mood::Idle | Mood::Running => None,
        }
    }
}

/// What the pet reports: the most urgent fresh chat, and how it feels about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focus {
    pub mood: Mood,
    pub chat_id: Option<String>,
    pub title: SharedString,
    /// The finished turn's preview, else the mood's word.
    pub body: SharedString,
}

/// One sidebar chat, as far as the pet cares.
struct Row<'a> {
    indicator: ChatIndicator,
    last_message_at: Option<DateTime<Utc>>,
    id: &'a str,
    title: Option<&'a str>,
    preview: Option<&'a str>,
}

/// Lower is more urgent: waiting, failed, running, then review.
fn rank(
    indicator: ChatIndicator,
    last_message_at: Option<DateTime<Utc>>,
    now: DateTime<Utc>,
) -> Option<(u8, Mood)> {
    let (rank, mood) = match indicator {
        ChatIndicator::AwaitingInput => (0, Mood::Waiting),
        ChatIndicator::Errored => (1, Mood::Failed),
        ChatIndicator::Working => (2, Mood::Running),
        ChatIndicator::Completed => (3, Mood::Review),
        ChatIndicator::Idle => return None,
    };
    let fresh = match (mood.lifetime(), last_message_at) {
        (Some(lifetime), Some(at)) => now.signed_duration_since(at) <= lifetime,
        _ => true,
    };
    fresh.then_some((rank, mood))
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Rows come in sidebar order; ties keep the first (most recent) row.
fn pick<'a>(rows: impl IntoIterator<Item = Row<'a>>, now: DateTime<Utc>) -> Focus {
    let mut best: Option<(u8, Mood, Row<'a>)> = None;
    for row in rows {
        if let Some((rank, mood)) = rank(row.indicator, row.last_message_at, now)
            && best
                .as_ref()
                .is_none_or(|(best_rank, ..)| rank < *best_rank)
        {
            best = Some((rank, mood, row));
        }
    }
    let Some((_, mood, row)) = best else {
        return Focus {
            mood: Mood::Idle,
            chat_id: None,
            title: SharedString::default(),
            body: SharedString::default(),
        };
    };
    let preview = row
        .preview
        .filter(|_| mood == Mood::Review)
        .map(one_line)
        .filter(|preview| !preview.is_empty());
    Focus {
        mood,
        chat_id: Some(row.id.to_owned()),
        title: row
            .title
            .map(one_line)
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "New session".into())
            .into(),
        body: preview
            .map(|preview| preview.chars().take(200).collect::<String>())
            .unwrap_or_else(|| mood.fallback_body().into())
            .into(),
    }
}

fn focus(state: &AppState, now: DateTime<Utc>) -> Focus {
    pick(
        state
            .overview_chats(now)
            .into_iter()
            .map(|(indicator, chat)| Row {
                indicator,
                last_message_at: chat.last_message_at,
                id: chat.id.as_str(),
                title: chat.title.as_deref(),
                preview: chat.last_message_preview.as_deref(),
            }),
        now,
    )
}

// ---------------------------------------------------------------------------
// Pointer: the Codex app mascot's hover, grab and drag
// ---------------------------------------------------------------------------

/// Horizontal travel (pt) before a drag picks or flips its running direction.
const DRAG_DEADZONE: f32 = 4.0;
const DRAG_POLL: Duration = Duration::from_millis(16);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Heading {
    Left,
    Right,
}

/// The heading after moving `dx` from the last accepted spot; `None` inside
/// the deadzone, so mostly vertical drags keep running the same way.
fn heading_for(dx: f32) -> Option<Heading> {
    if dx >= DRAG_DEADZONE {
        Some(Heading::Right)
    } else if dx <= -DRAG_DEADZONE {
        Some(Heading::Left)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pointer {
    Away,
    Hover,
    Grab(Option<Heading>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Playback {
    /// The track as authored (a one-shot hands off to its fallback).
    Play,
    /// Its first frame, still.
    Hold,
    /// Its first burst, over and over.
    Loop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Track {
    name: &'static str,
    playback: Playback,
}

/// A drag runs toward the pointer, a grab freezes the live state, a hover
/// jumps; a live task keeps its run cycle going, and otherwise the agents'
/// state (or the opening wave) plays out.
fn track(mood: Mood, pointer: Pointer, greeting: bool) -> Track {
    let (name, playback) = match pointer {
        Pointer::Grab(Some(Heading::Right)) => ("running-right", Playback::Loop),
        Pointer::Grab(Some(Heading::Left)) => ("running-left", Playback::Loop),
        Pointer::Grab(None) => (mood.animation(), Playback::Hold),
        Pointer::Hover => ("jumping", Playback::Play),
        Pointer::Away if greeting => ("waving", Playback::Play),
        // A running task is live state, not news: it must not idle out.
        Pointer::Away if mood == Mood::Running => (mood.animation(), Playback::Loop),
        Pointer::Away => (mood.animation(), Playback::Play),
    };
    Track { name, playback }
}

/// The sprite `elapsed` into `track` (Codex's `current_animation`: unknown
/// names play idle, a finished one-shot plays its fallback). Reduced motion
/// shows the first frame, still.
fn current_frame(
    animations: &Animations,
    track: Track,
    elapsed: Duration,
    reduced_motion: bool,
) -> Option<Tick> {
    let mut animation = animations
        .get(track.name)
        .or_else(|| animations.get("idle"))?;
    if track.playback == Playback::Play
        && animation.loop_start.is_none()
        && elapsed >= animation.total()
    {
        animation = animations.get(&animation.fallback).unwrap_or(animation);
    }
    if reduced_motion || track.playback == Playback::Hold {
        return animation.frames.first().map(|frame| Tick {
            sprite: frame.sprite,
            delay: None,
        });
    }
    if track.playback == Playback::Loop {
        let burst = animation
            .loop_start
            .filter(|&start| start > 0)
            .unwrap_or(animation.frames.len());
        let looped = Animation {
            frames: animation.frames[..burst].to_vec(),
            loop_start: Some(0),
            fallback: String::new(),
        };
        return frame_at(&looped, elapsed);
    }
    frame_at(animation, elapsed)
}

/// The pointer's screen x while the left button is held, else `None`.
#[cfg(target_os = "macos")]
fn held_pointer_x() -> Option<f32> {
    use objc2_app_kit::NSEvent;
    (NSEvent::pressedMouseButtons() & 1 != 0).then(|| NSEvent::mouseLocation().x as f32)
}

#[cfg(not(target_os = "macos"))]
fn held_pointer_x() -> Option<f32> {
    None
}

// ---------------------------------------------------------------------------
// The floating window
// ---------------------------------------------------------------------------

const BUBBLE_WIDTH: f32 = 240.0;
const BUBBLE_HEIGHT: f32 = 48.0;
const GAP: f32 = 6.0;
const MARGIN: f32 = 24.0;
/// Recency expiry has no state change to announce it.
const REFRESH: Duration = Duration::from_secs(30);

fn window_size(pet_width: f32, pet_height: f32) -> gpui::Size<gpui::Pixels> {
    size(
        px(pet_width.max(BUBBLE_WIDTH)),
        px(pet_height + GAP + BUBBLE_HEIGHT),
    )
}

pub struct PetWindow {
    state: Entity<AppState>,
    pet_width: f32,
    pet_height: f32,
    animations: Arc<Animations>,
    sprites: Option<Sprites>,
    focus: Focus,
    pointer: Pointer,
    /// The pointer is over the sprite (tracked through grabs too).
    hovered: bool,
    /// Waving hello until anything else happens.
    greeting: bool,
    track: Track,
    started: Instant,
    _tick: Task<()>,
    _drag: Task<()>,
    _refresh: Task<()>,
    _subscriptions: Vec<gpui::Subscription>,
    _decode: Task<()>,
}

impl PetWindow {
    fn new(
        state: Entity<AppState>,
        pet: PetInfo,
        pet_width: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus = focus(state.read(cx), Utc::now());
        let subscriptions = vec![
            cx.observe(&state, |this, _, cx| this.refresh(cx)),
            cx.observe_window_bounds(window, |_, window, cx| save_position(window, cx)),
        ];
        let pet_height = pet.height_for(pet_width);
        let animations = pet.animations.clone();
        let decode = cx.spawn(async move |this, cx| {
            let name = pet.id.clone();
            let sprites = cx
                .background_executor()
                .spawn(async move { decode_sprites(&pet) })
                .await;
            match sprites {
                Ok(sprites) => {
                    this.update(cx, |this, cx| {
                        this.sprites = Some(sprites);
                        cx.notify();
                    })
                    .ok();
                }
                Err(error) => tracing::warn!(pet = %name, %error, "pet: sprite sheet rejected"),
            }
        });
        let refresh = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(REFRESH).await;
                if this.update(cx, |this, cx| this.refresh(cx)).is_err() {
                    return;
                }
            }
        });
        // Say hello first unless there is already news to act out.
        let greeting = focus.mood == Mood::Idle;
        let first = track(focus.mood, Pointer::Away, greeting);
        let mut this = Self {
            state,
            pet_width,
            pet_height,
            animations,
            sprites: None,
            focus,
            pointer: Pointer::Away,
            hovered: false,
            greeting,
            track: first,
            started: Instant::now(),
            _tick: Task::ready(()),
            _drag: Task::ready(()),
            _refresh: refresh,
            _subscriptions: subscriptions,
            _decode: decode,
        };
        this.play(first, cx);
        this
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let next = focus(self.state.read(cx), Utc::now());
        if next == self.focus {
            return;
        }
        let mood_changed = next.mood != self.focus.mood;
        self.focus = next;
        if mood_changed {
            // News replays from its first frame, like a Codex notification.
            self.greeting = false;
            let next = track(self.focus.mood, self.pointer, false);
            self.play(next, cx);
        }
        cx.notify();
    }

    fn frame(&self, cx: &App) -> Option<Tick> {
        current_frame(
            &self.animations,
            self.track,
            self.started.elapsed(),
            crate::motion::reduced_motion(cx),
        )
    }

    /// Restart the frame clock on `track`; it sleeps until each next frame.
    fn play(&mut self, track: Track, cx: &mut Context<Self>) {
        self.track = track;
        self.started = Instant::now();
        self._tick = cx.spawn(async move |this, cx| {
            loop {
                let Ok(Some(delay)) = this.update(cx, |this, cx| {
                    cx.notify();
                    this.frame(cx).and_then(|tick| tick.delay)
                }) else {
                    return;
                };
                cx.background_executor().timer(delay).await;
            }
        });
        cx.notify();
    }

    fn set_pointer(&mut self, pointer: Pointer, cx: &mut Context<Self>) {
        if pointer == self.pointer {
            return;
        }
        self.pointer = pointer;
        // Any pointer play ends the hello wave.
        self.greeting &= pointer == Pointer::Away;
        let next = track(self.focus.mood, pointer, self.greeting);
        if next != self.track {
            self.play(next, cx);
        }
    }

    fn hover(&mut self, hovered: bool, cx: &mut Context<Self>) {
        self.hovered = hovered;
        if !matches!(self.pointer, Pointer::Grab(_)) {
            let pointer = if hovered {
                Pointer::Hover
            } else {
                Pointer::Away
            };
            self.set_pointer(pointer, cx);
        }
    }

    /// Hold still, run toward the drag once it clears the deadzone, and let
    /// go when the button comes up. The window moves natively, which swallows
    /// the mouse events, so the pointer is polled meanwhile.
    fn grab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Must run while the mouse-down is still AppKit's current event.
        window.start_window_move();
        let Some(mut baseline) = held_pointer_x() else {
            return;
        };
        self.set_pointer(Pointer::Grab(None), cx);
        self._drag = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(DRAG_POLL).await;
                let x = held_pointer_x();
                let held = this.update(cx, |this, cx| {
                    let Some(x) = x else {
                        let rest = if this.hovered {
                            Pointer::Hover
                        } else {
                            Pointer::Away
                        };
                        this.set_pointer(rest, cx);
                        return false;
                    };
                    if let Some(heading) = heading_for(x - baseline) {
                        baseline = x;
                        this.set_pointer(Pointer::Grab(Some(heading)), cx);
                    }
                    true
                });
                if !matches!(held, Ok(true)) {
                    return;
                }
            }
        });
    }

    fn open_focus(&self, cx: &mut App) {
        let Some(chat_id) = self.focus.chat_id.clone() else {
            crate::activate_main_window(cx);
            return;
        };
        let state = self.state.clone();
        cx.defer(move |cx| crate::open_notification_target(chat_id, &state, cx));
    }
}

impl Render for PetWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        let frame = self
            .frame(cx)
            .and_then(|tick| self.sprites.as_ref()?.get(&tick.sprite).cloned());
        let grabbing = matches!(self.pointer, Pointer::Grab(_));
        let pet = div()
            .id("pet")
            .w(px(self.pet_width))
            .h(px(self.pet_height))
            .when(grabbing, |pet| pet.cursor_grabbing())
            .when(!grabbing, |pet| pet.cursor_grab())
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| this.hover(*hovered, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| this.grab(window, cx)),
            )
            .children(frame.map(|frame| img(frame).size_full()));

        let bubble = (self.focus.mood != Mood::Idle).then(|| {
            div()
                .id("pet-bubble")
                .role(gpui::Role::Button)
                .aria_label(format!(
                    "{}: {}. Open chat",
                    self.focus.title, self.focus.body
                ))
                .cursor_pointer()
                .w(px(BUBBLE_WIDTH))
                .h(px(BUBBLE_HEIGHT))
                .px(px(16.0))
                .flex()
                .flex_col()
                .justify_center()
                .rounded_full()
                // Opaque: the window behind it is transparent.
                .bg(theme.surface_overlay.opacity(1.0))
                .border_1()
                .border_color(theme.border)
                .child(
                    div()
                        .text_size(px(13.0))
                        .text_color(theme.text)
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .truncate()
                        .child(self.focus.title.clone()),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.text_muted)
                        .truncate()
                        .child(self.focus.body.clone()),
                )
                .on_click(cx.listener(|this, _, _, cx| this.open_focus(cx)))
        });

        div()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(GAP))
            .font_family(theme.font_sans.clone())
            .child(pet)
            .children(bubble)
    }
}

/// Remembered as display-relative bounds origin, like the main window.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PetPosition {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_uuid: Option<uuid::Uuid>,
    pub x: f32,
    pub y: f32,
}

fn save_position(window: &mut Window, cx: &mut App) {
    let origin = window.bounds().origin;
    let position = PetPosition {
        display_uuid: window.display(cx).and_then(|d| d.uuid().ok()),
        x: origin.x.into(),
        y: origin.y.into(),
    };
    crate::settings::update(crate::settings::SavePolicy::Debounced, cx, |settings| {
        settings.pet_position = Some(position);
    });
}

/// Where to open: the saved spot kept on screen, else the primary display's
/// bottom-right corner.
fn placement(
    saved: Option<PetPosition>,
    window: gpui::Size<gpui::Pixels>,
    cx: &App,
) -> (gpui::Point<gpui::Pixels>, Option<gpui::DisplayId>) {
    let displays = cx.displays();
    let saved_display = saved.and_then(|saved| {
        let uuid = saved.display_uuid?;
        displays.iter().find(|d| d.uuid().ok() == Some(uuid))
    });
    let Some(display) = saved_display
        .cloned()
        .or_else(|| cx.primary_display())
        .or_else(|| displays.first().cloned())
    else {
        return (point(px(MARGIN), px(MARGIN)), None);
    };
    // Bounds origins are relative to the display's frame.
    let frame = display.bounds();
    let visible = display.visible_bounds();
    let min_x = (visible.origin.x - frame.origin.x).as_f32();
    let min_y = (visible.origin.y - frame.origin.y).as_f32();
    let max_x = (min_x + (visible.size.width - window.width).as_f32()).max(min_x);
    let max_y = (min_y + (visible.size.height - window.height).as_f32()).max(min_y);
    let (x, y) = match saved {
        Some(saved) if saved_display.is_some() => (saved.x, saved.y),
        _ => (max_x - MARGIN, max_y - MARGIN),
    };
    (
        point(px(x.clamp(min_x, max_x)), px(y.clamp(min_y, max_y))),
        Some(display.id()),
    )
}

fn open(state: Entity<AppState>, cx: &mut App) -> Option<WindowHandle<PetWindow>> {
    let settings = crate::settings::current(cx);
    let pet_width = settings.pet_size;
    let pet = find(&settings.pet_id, crate::settings::data_dir(cx).as_deref());
    let window_size = window_size(pet_width, pet.height_for(pet_width));
    let (origin, display_id) = placement(settings.pet_position, window_size, cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(gpui::Bounds::new(
            origin,
            window_size,
        ))),
        titlebar: None,
        focus: false,
        show: true,
        // macOS: a non-activating panel above other apps, on every Space.
        kind: WindowKind::PopUp,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        window_background: gpui::WindowBackgroundAppearance::Transparent,
        app_id: Some("zeron".into()),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        window.set_rem_size(px(crate::typography::font_size(cx).pixels()));
        float_above_apps(window);
        cx.new(|cx| PetWindow::new(state, pet, pet_width, window, cx))
    });
    match opened {
        Ok(handle) => Some(handle),
        Err(error) => {
            tracing::warn!(%error, "pet: window failed to open");
            None
        }
    }
}

/// Keep the panel visible while another app is active, drop the window
/// shadow, and hide the traffic lights gpui adds to every titled window.
#[cfg(target_os = "macos")]
fn float_above_apps(window: &Window) {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    let view = handle.ns_view.as_ptr() as *mut Object;
    // SAFETY: AppKit messages on the live NSView/NSWindow gpui just created,
    // sent from the main thread inside its open_window callback.
    unsafe {
        let native: *mut Object = msg_send![view, window];
        if native.is_null() {
            return;
        }
        let _: () = msg_send![native, setHidesOnDeactivate: false];
        let _: () = msg_send![native, setHasShadow: false];
        for button in 0..3usize {
            // NSWindowCloseButton, MiniaturizeButton, ZoomButton.
            let button: *mut Object = msg_send![native, standardWindowButton: button];
            if !button.is_null() {
                let _: () = msg_send![button, setHidden: true];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn float_above_apps(_window: &Window) {}

// ---------------------------------------------------------------------------
// Controller: one pet window following the settings
// ---------------------------------------------------------------------------

struct PetController {
    state: Entity<AppState>,
    window: Option<WindowHandle<PetWindow>>,
    /// (pet id, size) the open window was built with.
    applied: Option<(String, f32)>,
    _settings: gpui::Subscription,
}

impl gpui::Global for PetController {}

/// Whether this build shows pets (macOS only: other platforms quit when the
/// main window closes, which a lingering pet window would block).
pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

pub fn init(state: Entity<AppState>, cx: &mut App) {
    if !supported() {
        return;
    }
    let subscription = cx.observe_global::<crate::settings::SettingsStore>(sync);
    cx.set_global(PetController {
        state,
        window: None,
        applied: None,
        _settings: subscription,
    });
    sync(cx);
}

fn sync(cx: &mut App) {
    let settings = crate::settings::current(cx);
    let wanted = settings
        .pet_enabled
        .then(|| (settings.pet_id.clone(), settings.pet_size));
    let controller = cx.global::<PetController>();
    if controller.applied == wanted {
        return;
    }
    let state = controller.state.clone();
    if let Some(window) = cx.global_mut::<PetController>().window.take() {
        let _ = window.update(cx, |_, window, _| window.remove_window());
    }
    let window = wanted.is_some().then(|| open(state, cx)).flatten();
    let controller = cx.global_mut::<PetController>();
    controller.window = window;
    controller.applied = wanted;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row<'a>(
        indicator: ChatIndicator,
        minutes_ago: i64,
        id: &'a str,
        title: Option<&'a str>,
        now: DateTime<Utc>,
    ) -> Row<'a> {
        Row {
            indicator,
            last_message_at: Some(now - chrono::Duration::minutes(minutes_ago)),
            id,
            title,
            preview: None,
        }
    }

    fn ms(ms: u64) -> Duration {
        Duration::from_millis(ms)
    }

    #[test]
    fn most_urgent_fresh_chat_wins() {
        let now = Utc::now();
        let focus = pick(
            [
                row(ChatIndicator::Working, 1, "a", Some("Build"), now),
                row(ChatIndicator::Completed, 5, "b", None, now),
                row(
                    ChatIndicator::AwaitingInput,
                    30,
                    "c",
                    Some("Ask \n me"),
                    now,
                ),
                row(ChatIndicator::Errored, 2, "d", None, now),
            ],
            now,
        );
        assert_eq!(focus.mood, Mood::Waiting);
        assert_eq!(focus.chat_id.as_deref(), Some("c"));
        assert_eq!(focus.title.as_ref(), "Ask me");
        assert_eq!(focus.body.as_ref(), "Needs input");
        // Work in progress outranks a finished turn.
        let focus = pick(
            [
                row(ChatIndicator::Completed, 1, "a", None, now),
                row(ChatIndicator::Working, 600, "b", None, now),
            ],
            now,
        );
        assert_eq!(
            (focus.mood, focus.body.as_ref()),
            (Mood::Running, "Running")
        );
    }

    #[test]
    fn stale_news_and_idle_chats_leave_the_pet_idle() {
        let now = Utc::now();
        let focus = pick(
            [
                row(ChatIndicator::Completed, 8 * 24 * 60, "a", None, now),
                row(ChatIndicator::Errored, 90, "b", None, now),
                row(ChatIndicator::Idle, 0, "c", None, now),
            ],
            now,
        );
        assert_eq!(focus.mood, Mood::Idle);
        assert_eq!(focus.chat_id, None);
    }

    #[test]
    fn review_shows_the_turn_preview() {
        let now = Utc::now();
        let mut done = row(ChatIndicator::Completed, 10, "a", None, now);
        done.preview = Some("  Fixed the\nbuild  ");
        let focus = pick([done], now);
        assert_eq!(focus.mood, Mood::Review);
        assert_eq!(focus.title.as_ref(), "New session");
        assert_eq!(focus.body.as_ref(), "Fixed the build");
    }

    #[test]
    fn default_tracks_follow_the_codex_contract() {
        let animations = default_animations();
        let review = &animations["review"];
        assert_eq!(review.loop_start, Some(18));
        assert_eq!(review.frames[0].sprite, 64);
        assert_eq!(review.frames[4].duration, ms(150));
        assert_eq!(review.frames[5].duration, ms(280));
        assert_eq!(review.frames[18].sprite, 0);
        let durations: Vec<_> = animations["idle"]
            .frames
            .iter()
            .map(|f| f.duration)
            .collect();
        assert_eq!(
            durations,
            [ms(1680), ms(660), ms(660), ms(840), ms(840), ms(1920)]
        );
        let left = &animations["running-left"];
        assert_eq!((left.frames[0].sprite, left.frames[7].sprite), (16, 23));
        assert_eq!(left.frames[7].duration, ms(220));
    }

    #[test]
    fn frames_play_the_prefix_then_loop() {
        let animations = default_animations();
        let play = |name, ms_in| {
            current_frame(
                &animations,
                Track {
                    name,
                    playback: Playback::Play,
                },
                ms(ms_in),
                false,
            )
            .unwrap()
        };
        assert_eq!(
            play("waiting", 0),
            Tick {
                sprite: 48,
                delay: Some(ms(150))
            }
        );
        // 5×150 ms in: the long last frame of the first burst.
        assert_eq!(play("waiting", 760).sprite, 53);
        // Past three bursts (3×1010 ms) the idle loop takes over and repeats.
        let idle_cycle = 1680 + 660 + 660 + 840 + 840 + 1920;
        assert_eq!(play("waiting", 3030).sprite, 0);
        assert_eq!(play("waiting", 3030 + idle_cycle + 1700).sprite, 1);
        // Unknown names play idle.
        assert_eq!(play("nope", 0).sprite, 0);
        // Reduced motion: the first frame, still.
        let still = current_frame(
            &animations,
            Track {
                name: "failed",
                playback: Playback::Play,
            },
            ms(500),
            true,
        );
        assert_eq!(
            still,
            Some(Tick {
                sprite: 40,
                delay: None
            })
        );
    }

    #[test]
    fn drag_tracks_loop_one_burst_and_grab_holds() {
        let animations = default_animations();
        let at = |track, ms_in| current_frame(&animations, track, ms(ms_in), false).unwrap();
        let right = track(Mood::Idle, Pointer::Grab(Some(Heading::Right)), false);
        assert_eq!(right.name, "running-right");
        // One burst is 7×120 + 220 = 1060 ms; it never falls back to idle.
        assert_eq!(at(right, 0).sprite, 8);
        assert_eq!(at(right, 1060 * 5 + 130).sprite, 9);
        let hold = track(Mood::Running, Pointer::Grab(None), false);
        assert_eq!(
            at(hold, 5000),
            Tick {
                sprite: 56,
                delay: None
            }
        );
    }

    #[test]
    fn a_live_task_runs_instead_of_idling_out() {
        let animations = default_animations();
        let live = track(Mood::Running, Pointer::Away, false);
        assert_eq!((live.name, live.playback), ("running", Playback::Loop));
        let at = |ms_in| current_frame(&animations, live, ms(ms_in), false).unwrap();
        // One run cycle is 5×120 + 220 ms, and it repeats for as long as the
        // task does instead of handing off to the idle loop.
        let cycle = 5 * 120 + 220;
        let idle: Vec<usize> = animations["idle"].frames.iter().map(|f| f.sprite).collect();
        for round in 0..20 {
            let start = cycle * round;
            assert_eq!(at(start).sprite, 56, "round {round}");
            assert_eq!(at(start + 600).sprite, 61, "round {round}");
        }
        for ms_in in (0..120_000).step_by(70) {
            assert!(!idle.contains(&at(ms_in).sprite), "idled out at {ms_in} ms");
        }
    }

    #[test]
    fn pointer_overrides_the_mood() {
        assert_eq!(track(Mood::Failed, Pointer::Hover, false).name, "jumping");
        assert_eq!(track(Mood::Failed, Pointer::Away, false).name, "failed");
        assert_eq!(track(Mood::Idle, Pointer::Away, true).name, "waving");
        assert_eq!(
            track(Mood::Waiting, Pointer::Grab(Some(Heading::Left)), true).name,
            "running-left"
        );
        assert_eq!(heading_for(3.9), None);
        assert_eq!(heading_for(-4.0), Some(Heading::Left));
        assert_eq!(heading_for(4.0), Some(Heading::Right));
    }

    #[test]
    fn manifest_tracks_are_checked() {
        let spec = |frames: Vec<usize>, fps, fallback: &str| AnimationSpec {
            frames,
            fps: Some(fps),
            looping: Some(false),
            fallback: fallback.into(),
        };
        let load = |name: &str, spec| load_animations(HashMap::from([(name.into(), spec)]), 72);
        let ok = load("idle", spec(vec![3, 4], 4.0, "")).unwrap();
        assert_eq!(
            ok["idle"],
            Animation {
                frames: vec![
                    Frame {
                        sprite: 3,
                        duration: ms(250)
                    },
                    Frame {
                        sprite: 4,
                        duration: ms(250)
                    },
                ],
                loop_start: None,
                fallback: "idle".into(),
            }
        );
        assert!(load("x", spec(vec![72], 8.0, "")).is_err());
        assert!(load("x", spec(vec![1], 0.0, "")).is_err());
        assert!(load("x", spec(vec![1], 8.0, "missing")).is_err());
        assert!(load("x", spec(vec![], 8.0, "")).is_err());
        // A v1 sheet can't hold the 88-cell v2 grid's rows, but every default fits in 72.
        assert!(load_animations(HashMap::new(), 72).is_ok());
    }

    #[test]
    fn bundled_sheet_decodes_every_frame() {
        let pet = bundled();
        let sprites = decode_sprites(&pet).expect("rocky decodes");
        for frame in pet.animations.values().flat_map(|a| &a.frames) {
            assert!(sprites.contains_key(&frame.sprite));
        }
        assert!(preview(&pet).is_ok());
    }

    #[test]
    fn library_reads_folders_and_rejects_escapes() {
        let dir = std::env::temp_dir().join(format!("zeron-pets-{}", std::process::id()));
        let pets = pets_dir(&dir);
        let good = pets.join("blob");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(good.join("sheet.webp"), ROCKY).unwrap();
        std::fs::write(
            good.join("pet.json"),
            r#"{"displayName":"Blob","description":"Hi","spritesheetPath":"sheet.webp",
                "animations":{"waving":{"frames":[24,25],"fps":6}}}"#,
        )
        .unwrap();
        // Codex defaults: the name falls back to the id, the sheet to spritesheet.webp.
        let plain = pets.join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        std::fs::write(plain.join("spritesheet.webp"), ROCKY).unwrap();
        std::fs::write(plain.join("pet.json"), r#"{"id":"Plain One"}"#).unwrap();
        let bad = pets.join("sneaky");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(
            bad.join("pet.json"),
            r#"{"displayName":"S","spritesheetPath":"../blob/sheet.webp"}"#,
        )
        .unwrap();
        let grid = pets.join("wonky");
        std::fs::create_dir_all(&grid).unwrap();
        std::fs::write(grid.join("spritesheet.webp"), ROCKY).unwrap();
        std::fs::write(
            grid.join("pet.json"),
            r#"{"frame":{"width":100,"height":208,"columns":8,"rows":11}}"#,
        )
        .unwrap();
        std::fs::create_dir_all(pets.join("hatching")).unwrap(); // no manifest yet

        let (found, issues) = library(Some(&dir));
        std::fs::remove_dir_all(&dir).ok();
        let ids: Vec<_> = found.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["rocky", "blob", "plain"]);
        assert_eq!(found[2].name, "Plain One");
        assert_eq!(found[1].animations["waving"].frames.len(), 2);
        assert_eq!(issues.len(), 2, "{issues:?}");
        assert!(issues[0].starts_with("sneaky:"));
        assert!(issues[1].starts_with("wonky:"));
    }
}
