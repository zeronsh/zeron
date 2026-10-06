use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui::{
    AnyElement, Bounds, Context, DragMoveEvent, Entity, Focusable as _, MouseButton, Pixels,
    RenderImage, ScrollHandle, SharedString, Task, Window, div, prelude::*, px,
};
use zeron_music::source::{self, Origin, Track};
use zeron_music::{Event, Player};

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::motion;
use crate::popover::{self, Popup};
use crate::settings::widgets::text_tooltip;
use crate::theme::{Theme, hairline, ink, wash};

const PLAYER_WIDTH: f32 = 296.0;
const QUEUE_WIDTH: f32 = 252.0;
const ART_PX: u32 = 96;
const ART_MAX_SOURCE_PX: u32 = 4096;
const TICK: Duration = Duration::from_millis(250);
const RESTART_THRESHOLD: f64 = 3.0;
const MAX_AUTO_SKIPS: u32 = 3;
const BUTTON_SIZE: f32 = 28.0;
const CONTROL_GROUP_WIDTH: f32 = 3.0 * BUTTON_SIZE + 4.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoopMode {
    Off,
    All,
    One,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slider {
    Seek,
    Volume,
}

#[derive(Clone)]
struct ScrubDrag(Slider);

impl Render for ScrubDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

struct Entry {
    id: u64,
    track: Track,
    file: Option<PathBuf>,
}

pub struct MusicPlayer {
    player: Option<Player>,
    queue: Vec<Entry>,
    next_id: u64,
    current: Option<u64>,
    playing: bool,
    loading: bool,
    resolving: bool,
    error: Option<SharedString>,
    loop_mode: LoopMode,
    volume: f32,
    muted: bool,
    failures: u32,
    scrub: Option<f32>,
    popup: Popup<()>,
    queue_open: bool,
    art: Option<(u64, Arc<RenderImage>)>,
    input: Entity<ComposerInput>,
    queue_scroll: ScrollHandle,
    seek_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    volume_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    cache_dir: PathBuf,
    events: Option<Task<()>>,
    tick: Option<Task<()>>,
    resolve_task: Option<Task<()>>,
    fetch_task: Option<Task<()>>,
    prefetch_task: Option<Task<()>>,
    art_task: Option<Task<()>>,
}

impl MusicPlayer {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            ComposerInput::new("Link or search", cx)
                .with_single_line()
                .with_accessibility_role(gpui::Role::SearchInput)
                .with_text_metrics(12.5, 18.0)
        });
        cx.subscribe(&input, |this: &mut Self, input, event, cx| match event {
            ComposerInputEvent::Submitted | ComposerInputEvent::ModifiedSubmitted => {
                let text = input.read(cx).text().trim().to_string();
                if !text.is_empty() {
                    input.update(cx, |input, cx| input.set_text("", cx));
                    this.submit(text, cx);
                }
            }
            ComposerInputEvent::MentionDismiss => this.close(cx),
            _ => {}
        })
        .detach();
        cx.observe_global::<crate::settings::SettingsStore>(|this, cx| {
            if !crate::settings::music_player_enabled(cx) && this.player.is_some() {
                this.clear(cx);
                this.player = None;
                this.events = None;
                this.popup = Popup::default();
            }
        })
        .detach();
        let root = std::env::temp_dir().join("zeron-music");
        let cache_dir = root.join(std::process::id().to_string());
        let own = cache_dir.clone();
        cx.background_spawn(async move {
            for entry in std::fs::read_dir(&root).into_iter().flatten().flatten() {
                if entry.path() != own {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        })
        .detach();
        Self {
            player: None,
            queue: Vec::new(),
            next_id: 0,
            current: None,
            playing: false,
            loading: false,
            resolving: false,
            error: None,
            loop_mode: LoopMode::Off,
            volume: 0.8,
            muted: false,
            failures: 0,
            scrub: None,
            popup: Popup::default(),
            queue_open: false,
            art: None,
            input,
            queue_scroll: ScrollHandle::new(),
            seek_bounds: Rc::default(),
            volume_bounds: Rc::default(),
            cache_dir,
            events: None,
            tick: None,
            resolve_task: None,
            fetch_task: None,
            prefetch_task: None,
            art_task: None,
        }
    }

    fn player(&mut self, cx: &mut Context<Self>) -> &Player {
        if self.player.is_none() {
            let (player, mut events) = Player::new();
            player.set_volume(self.effective_volume());
            self.events = Some(cx.spawn(async move |this, cx| {
                while let Some(event) = events.next().await {
                    if this
                        .update(cx, |this, cx| this.on_event(event, cx))
                        .is_err()
                    {
                        break;
                    }
                }
            }));
            self.player = Some(player);
        }
        self.player.as_ref().expect("player")
    }

    fn effective_volume(&self) -> f32 {
        if self.muted { 0.0 } else { self.volume }
    }

    fn index_of(&self, id: u64) -> Option<usize> {
        self.queue.iter().position(|entry| entry.id == id)
    }

    fn current_index(&self) -> Option<usize> {
        self.current.and_then(|id| self.index_of(id))
    }

    fn submit(&mut self, text: String, cx: &mut Context<Self>) {
        self.resolving = true;
        self.error = None;
        let task = gpui_tokio::Tokio::spawn_result(cx, async move { source::resolve(&text).await });
        self.resolve_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.resolving = false;
                match result {
                    Ok(tracks) => this.append(tracks, cx),
                    Err(err) => this.error = Some(err.to_string().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn pick_files(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update(cx, |this, cx| {
                let tracks: Vec<_> = paths
                    .into_iter()
                    .filter(|path| source::is_local_audio(path))
                    .map(source::local_track)
                    .collect();
                if tracks.is_empty() {
                    this.error = Some("Unsupported file".into());
                    cx.notify();
                } else {
                    this.append(tracks, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn append(&mut self, tracks: Vec<Track>, cx: &mut Context<Self>) {
        let first = self.queue.len();
        for track in tracks {
            let file = match &track.origin {
                Origin::Local(path) => Some(path.clone()),
                Origin::Remote(_) => None,
            };
            self.queue.push(Entry {
                id: self.next_id,
                track,
                file,
            });
            self.next_id += 1;
        }
        let idle = self.current.is_none() || (!self.playing && !self.loading);
        if idle && let Some(entry) = self.queue.get(first) {
            self.play_id(entry.id, cx);
        } else {
            self.prefetch_next(cx);
        }
        cx.notify();
    }

    fn remove(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.index_of(id) else {
            return;
        };
        let entry = self.queue.remove(ix);
        self.discard_file(&entry);
        if self.current == Some(id) {
            match self
                .queue
                .get(ix)
                .or_else(|| self.queue.last())
                .map(|entry| entry.id)
            {
                Some(next) if self.playing || self.loading => self.play_id(next, cx),
                _ => self.stop(cx),
            }
        }
        cx.notify();
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.stop(cx);
        self.queue.clear();
        self.error = None;
        let dir = self.cache_dir.clone();
        cx.background_spawn(async move {
            let _ = std::fs::remove_dir_all(dir);
        })
        .detach();
        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        self.fetch_task = None;
        self.prefetch_task = None;
        self.current = None;
        self.playing = false;
        self.loading = false;
        self.drop_art(cx);
        if let Some(player) = &self.player {
            player.stop();
        }
        cx.notify();
    }

    fn load_art(
        &mut self,
        id: u64,
        thumbnail: Option<String>,
        file: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        if thumbnail.is_none() && file.is_none() {
            return;
        }
        let task = gpui_tokio::Tokio::spawn_result(cx, async move {
            let mut bytes = match thumbnail {
                Some(url) => source::fetch_art(&url).await.ok(),
                None => None,
            };
            if bytes.is_none() {
                bytes = file.as_deref().and_then(source::embedded_art);
            }
            bytes
                .as_deref()
                .and_then(cover)
                .ok_or_else(|| anyhow::anyhow!("No cover"))
        });
        self.art_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.art_task = None;
                if let Ok(image) = result
                    && this.current == Some(id)
                {
                    this.art = Some((id, image));
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn drop_art(&mut self, cx: &mut Context<Self>) {
        self.art_task = None;
        if let Some((_, image)) = self.art.take() {
            cx.defer(move |cx| gpui::ImageSource::Render(image).evict(None, cx));
        }
    }

    fn discard_file(&self, entry: &Entry) {
        if matches!(entry.track.origin, Origin::Remote(_))
            && let Some(file) = &entry.file
        {
            let _ = std::fs::remove_file(file);
        }
    }

    fn evict_cache(&mut self) {
        let current = self.current_index();
        let keep = |ix: usize| current.is_some_and(|c| ix == c || ix == c + 1);
        for ix in 0..self.queue.len() {
            if !keep(ix) && matches!(self.queue[ix].track.origin, Origin::Remote(_)) {
                self.discard_file(&self.queue[ix]);
                self.queue[ix].file = None;
            }
        }
    }

    fn play_id(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(ix) = self.index_of(id) else {
            return;
        };
        if self.art.as_ref().is_none_or(|(art_id, _)| *art_id != id) {
            self.drop_art(cx);
            let entry = &self.queue[ix];
            let (thumbnail, file) = (entry.track.thumbnail.clone(), entry.file.clone());
            self.load_art(id, thumbnail, file, cx);
        }
        self.current = Some(id);
        self.error = None;
        self.playing = false;
        self.loading = true;
        self.scrub = None;
        self.evict_cache();
        if let Some(file) = self.queue[ix].file.clone() {
            self.fetch_task = None;
            self.player(cx).load(file);
        } else {
            let track = self.queue[ix].track.clone();
            let dir = self.cache_dir.clone();
            let task =
                gpui_tokio::Tokio::spawn_result(
                    cx,
                    async move { source::fetch(&track, &dir).await },
                );
            self.fetch_task = Some(cx.spawn(async move |this, cx| {
                let result = task.await;
                this.update(cx, |this, cx| this.fetched(id, result, true, cx))
                    .ok();
            }));
        }
        cx.notify();
    }

    fn fetched(
        &mut self,
        id: u64,
        result: anyhow::Result<PathBuf>,
        play: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = self.index_of(id) else {
            return;
        };
        match result {
            Ok(path) => {
                self.queue[ix].file = Some(path.clone());
                if play && self.current == Some(id) {
                    if self.art.is_none() && self.art_task.is_none() {
                        self.load_art(id, None, Some(path.clone()), cx);
                    }
                    self.player(cx).load(path);
                }
            }
            Err(err) if play && self.current == Some(id) => {
                self.loading = false;
                self.fail(err.to_string(), cx);
            }
            Err(_) => {}
        }
        cx.notify();
    }

    fn prefetch_next(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.current_index().and_then(|ix| self.queue.get(ix + 1)) else {
            return;
        };
        if next.file.is_some() {
            return;
        }
        let (id, track, dir) = (next.id, next.track.clone(), self.cache_dir.clone());
        let task =
            gpui_tokio::Tokio::spawn_result(cx, async move { source::fetch(&track, &dir).await });
        self.prefetch_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| this.fetched(id, result, false, cx))
                .ok();
        }));
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Started { .. } => {
                self.loading = false;
                self.playing = true;
                self.failures = 0;
                self.prefetch_next(cx);
                self.sync_tick(cx);
            }
            Event::Ended => {
                self.playing = false;
                self.advance(true, cx);
            }
            Event::Failed(message) => {
                self.loading = false;
                self.playing = false;
                self.fail(message, cx);
            }
        }
        cx.notify();
    }

    fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        self.failures += 1;
        if self.failures < MAX_AUTO_SKIPS && self.loop_mode != LoopMode::One {
            self.advance(true, cx);
        } else {
            self.playing = false;
        }
    }

    fn advance(&mut self, auto: bool, cx: &mut Context<Self>) {
        let Some(ix) = self.current_index() else {
            return;
        };
        let next = if auto && self.loop_mode == LoopMode::One {
            Some(ix)
        } else if ix + 1 < self.queue.len() {
            Some(ix + 1)
        } else if self.loop_mode == LoopMode::All || !auto {
            Some(0)
        } else {
            None
        };
        match next.map(|ix| self.queue[ix].id) {
            Some(id) => self.play_id(id, cx),
            None => {
                self.playing = false;
                if let Some(player) = &self.player {
                    player.seek(0.0);
                }
            }
        }
    }

    fn previous(&mut self, cx: &mut Context<Self>) {
        let position = self.player.as_ref().map_or(0.0, Player::position);
        match self.current_index() {
            Some(ix) if position < RESTART_THRESHOLD && ix > 0 => {
                let id = self.queue[ix - 1].id;
                self.play_id(id, cx);
            }
            Some(_) => {
                if let Some(player) = &self.player {
                    player.seek(0.0);
                }
            }
            None => {}
        }
    }

    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        match (self.current, self.playing) {
            (None, _) => {
                if let Some(id) = self.queue.first().map(|entry| entry.id) {
                    self.play_id(id, cx);
                }
            }
            (Some(_), true) => {
                self.playing = false;
                self.player(cx).pause();
            }
            (Some(_), false) => {
                self.playing = true;
                self.player(cx).play();
                self.sync_tick(cx);
            }
        }
        cx.notify();
    }

    fn cycle_loop(&mut self, cx: &mut Context<Self>) {
        self.loop_mode = match self.loop_mode {
            LoopMode::Off => LoopMode::All,
            LoopMode::All => LoopMode::One,
            LoopMode::One => LoopMode::Off,
        };
        cx.notify();
    }

    fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.muted = !self.muted;
        let volume = self.effective_volume();
        if let Some(player) = &self.player {
            player.set_volume(volume);
        }
        cx.notify();
    }

    fn fraction(bounds: &Rc<Cell<Option<Bounds<Pixels>>>>, x: Pixels) -> Option<f32> {
        let bounds = bounds.get()?;
        let width = f32::from(bounds.size.width).max(1.0);
        Some((f32::from(x - bounds.left()) / width).clamp(0.0, 1.0))
    }

    fn scrub_to(&mut self, slider: Slider, x: Pixels, cx: &mut Context<Self>) {
        match slider {
            Slider::Seek => {
                if self.duration().is_some() {
                    self.scrub = Self::fraction(&self.seek_bounds, x);
                }
            }
            Slider::Volume => {
                if let Some(fraction) = Self::fraction(&self.volume_bounds, x) {
                    self.volume = fraction;
                    self.muted = fraction == 0.0;
                    if let Some(player) = &self.player {
                        player.set_volume(fraction);
                    }
                }
            }
        }
        cx.notify();
    }

    fn commit_scrub(&mut self, cx: &mut Context<Self>) {
        let (Some(fraction), Some(duration)) = (self.scrub.take(), self.duration()) else {
            return;
        };
        if let Some(player) = &self.player {
            player.seek(fraction as f64 * duration);
        }
        cx.notify();
    }

    fn duration(&self) -> Option<f64> {
        let ix = self.current_index()?;
        self.player
            .as_ref()
            .and_then(Player::duration)
            .filter(|_| !self.loading)
            .or(self.queue[ix].track.duration)
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.popup.open(());
        window.focus(&self.input.focus_handle(cx), cx);
        self.sync_tick(cx);
        cx.notify();
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.commit_scrub(cx);
        if self.popup.begin_close() {
            popover::reap_popup(cx, |this: &mut Self| &mut this.popup);
        }
        cx.notify();
    }

    fn sync_tick(&mut self, cx: &mut Context<Self>) {
        if self.tick.is_some() || !self.popup.is_open() || !self.playing {
            return;
        }
        self.tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let keep = this
                    .update(cx, |this, cx| {
                        let keep = this.popup.is_open() && this.playing;
                        if keep {
                            cx.notify();
                        } else {
                            this.tick = None;
                        }
                        keep
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        }));
    }

    fn render_card(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let current = self.current_index().map(|ix| &self.queue[ix]);
        let title: SharedString = current
            .map(|entry| entry.track.title.clone().into())
            .unwrap_or_else(|| "Nothing playing".into());
        let has_track = current.is_some();
        let duration = self.duration();
        let position = if self.loading {
            0.0
        } else {
            self.player.as_ref().map_or(0.0, Player::position)
        };
        let progress = match (self.scrub, duration) {
            (Some(scrub), _) => scrub,
            (None, Some(duration)) if duration > 0.0 => {
                (position / duration).clamp(0.0, 1.0) as f32
            }
            _ => 0.0,
        };
        let shown_position = match (self.scrub, duration) {
            (Some(scrub), Some(duration)) => scrub as f64 * duration,
            _ => position,
        };
        let subtitle: Option<(SharedString, gpui::Hsla)> = if let Some(error) = &self.error {
            Some((error.clone(), theme.danger))
        } else if self.loading {
            Some(("Loading".into(), theme.text_muted))
        } else {
            self.current_index().map(|ix| {
                (
                    format!("{} of {}", ix + 1, self.queue.len()).into(),
                    theme.text_muted,
                )
            })
        };

        let cover = self
            .art
            .as_ref()
            .filter(|(id, _)| self.current == Some(*id))
            .map(|(_, image)| image.clone());
        let art = div()
            .size(px(40.0))
            .flex_none()
            .rounded(px(10.0))
            .overflow_hidden()
            .bg(ink(0.06))
            .border_1()
            .border_color(hairline(0.06))
            .flex()
            .items_center()
            .justify_center()
            .child(if let Some(cover) = cover {
                gpui::img(cover)
                    .size_full()
                    .rounded(px(9.0))
                    .object_fit(gpui::ObjectFit::Cover)
                    .into_any_element()
            } else if self.loading || self.resolving {
                crate::loaders::mini_mono_spinner(
                    "music-art-spinner",
                    3.0,
                    theme.text_muted,
                    cx.entity_id(),
                    cx,
                )
                .into_any_element()
            } else {
                icon(icons::MUSIC_NOTE)
                    .size(px(16.0))
                    .text_color(if self.playing {
                        theme.accent
                    } else {
                        theme.text_muted
                    })
                    .into_any_element()
            });

        let now_playing = div().flex().items_center().gap(px(10.0)).child(art).child(
            div()
                .min_w_0()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .truncate()
                        .text_size(crate::typography::ui_rems(13.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(if has_track {
                            theme.text
                        } else {
                            theme.text_muted
                        })
                        .child(title),
                )
                .children(subtitle.map(|(text, color)| {
                    div()
                        .truncate()
                        .text_size(crate::typography::ui_rems(11.5))
                        .text_color(color)
                        .child(text)
                })),
        );

        let seek_bounds = self.seek_bounds.clone();
        let seekable = duration.is_some() && has_track && !self.loading;
        let seek = div()
            .id("music-seek")
            .group("music-slider")
            .relative()
            .h(px(14.0))
            .when(seekable, |el| el.cursor_pointer())
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.scrub_to(Slider::Seek, event.position.x, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.commit_scrub(cx)),
            )
            .on_drag(ScrubDrag(Slider::Seek), |drag, _, _, cx| {
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            })
            .child(
                gpui::canvas(
                    move |rect, _, _| seek_bounds.set(Some(rect)),
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .child(slider_track(theme, progress, self.scrub.is_some()));

        let times = div()
            .flex()
            .justify_between()
            .font_family(theme.font_mono.clone())
            .text_size(crate::typography::ui_rems(10.5))
            .text_color(theme.text_muted)
            .child(SharedString::from(format_time(shown_position)))
            .child(SharedString::from(
                duration.map(format_time).unwrap_or_else(|| "--:--".into()),
            ));

        let queue_len = self.queue.len();
        let queue_open = self.queue_open && queue_len > 0;
        let loop_icon = if self.loop_mode == LoopMode::One {
            icons::MUSIC_REPEAT_ONE
        } else {
            icons::MUSIC_REPEAT
        };
        let loop_label = match self.loop_mode {
            LoopMode::Off => "Loop off",
            LoopMode::All => "Loop queue",
            LoopMode::One => "Loop track",
        };
        let volume_bounds = self.volume_bounds.clone();
        let volume_fraction = self.effective_volume();
        let controls = div()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .w(px(CONTROL_GROUP_WIDTH))
                    .flex()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        control_button(
                            "music-loop",
                            loop_icon,
                            14.0,
                            theme,
                            true,
                            self.loop_mode != LoopMode::Off,
                        )
                        .tooltip(text_tooltip(loop_label))
                        .on_click(cx.listener(|this, _, _, cx| this.cycle_loop(cx))),
                    )
                    .child(
                        control_button("music-files", icons::FOLDER, 14.0, theme, true, false)
                            .tooltip(text_tooltip("Add files"))
                            .on_click(cx.listener(|this, _, _, cx| this.pick_files(cx))),
                    )
                    .child(
                        control_button(
                            "music-queue-toggle",
                            icons::LIST,
                            14.0,
                            theme,
                            queue_len > 0,
                            queue_open,
                        )
                        .tooltip(text_tooltip("Queue"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            if !this.queue.is_empty() {
                                this.queue_open = !this.queue_open;
                                cx.notify();
                            }
                        })),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        control_button(
                            "music-previous",
                            icons::MUSIC_PREVIOUS,
                            13.0,
                            theme,
                            has_track,
                            false,
                        )
                        .tooltip(text_tooltip("Previous"))
                        .on_click(cx.listener(|this, _, _, cx| this.previous(cx))),
                    )
                    .child(
                        play_button(theme, self.playing, queue_len > 0)
                            .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))),
                    )
                    .child(
                        control_button(
                            "music-next",
                            icons::MUSIC_NEXT,
                            13.0,
                            theme,
                            has_track && queue_len > 1,
                            false,
                        )
                        .tooltip(text_tooltip("Next"))
                        .on_click(cx.listener(|this, _, _, cx| this.advance(false, cx))),
                    ),
            )
            .child(
                div()
                    .w(px(CONTROL_GROUP_WIDTH))
                    .flex()
                    .items_center()
                    .justify_end()
                    .gap(px(4.0))
                    .child(
                        control_button(
                            "music-mute",
                            if volume_fraction == 0.0 {
                                icons::VOLUME_CROSS
                            } else {
                                icons::VOLUME_LOUD
                            },
                            14.0,
                            theme,
                            true,
                            false,
                        )
                        .tooltip(text_tooltip(if self.muted { "Unmute" } else { "Mute" }))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))),
                    )
                    .child(
                        div()
                            .id("music-volume")
                            .group("music-slider")
                            .relative()
                            .w(px(CONTROL_GROUP_WIDTH - BUTTON_SIZE - 4.0))
                            .h(px(14.0))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    this.scrub_to(Slider::Volume, event.position.x, cx);
                                }),
                            )
                            .on_drag(ScrubDrag(Slider::Volume), |drag, _, _, cx| {
                                cx.stop_propagation();
                                cx.new(|_| drag.clone())
                            })
                            .child(
                                gpui::canvas(
                                    move |rect, _, _| volume_bounds.set(Some(rect)),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(slider_track(theme, volume_fraction, false)),
                    ),
            );

        let input = div()
            .px(px(10.0))
            .py(px(6.0))
            .rounded(px(popover::MENU_ITEM_RADIUS))
            .bg(ink(0.04))
            .text_size(crate::typography::ui_rems(13.0))
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                icon(icons::MAGNIFER)
                    .size(px(13.0))
                    .text_color(theme.text_muted),
            )
            .child(div().flex_1().min_w_0().child(self.input.clone()));

        let player = div()
            .w(px(PLAYER_WIDTH))
            .flex_none()
            .flex()
            .flex_col()
            .gap(px(popover::MENU_GAP))
            .child(
                div()
                    .p(px(8.0))
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(now_playing)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(seek)
                            .child(times),
                    )
                    .child(controls),
            )
            .child(input);

        popover::popover_card(theme)
            .flex()
            .flex_row()
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close(cx)))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.commit_scrub(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.commit_scrub(cx)),
            )
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ScrubDrag>, _, cx| {
                    let slider = event.drag(cx).0;
                    this.scrub_to(slider, event.event.position.x, cx);
                }),
            )
            .child(player)
            .when(queue_open, |card| card.child(self.render_queue(theme, cx)))
            .into_any_element()
    }

    fn render_queue(&mut self, theme: &Theme, cx: &mut Context<Self>) -> AnyElement {
        let current = self.current;
        let playing = self.playing;
        let rows: Vec<AnyElement> = self
            .queue
            .iter()
            .enumerate()
            .map(|(ix, entry)| {
                let id = entry.id;
                let active = current == Some(id);
                let group: SharedString = format!("music-row-{id}").into();
                let lead = if active {
                    icon(if playing {
                        icons::MUSIC_NOTE
                    } else {
                        icons::MUSIC_PAUSE
                    })
                    .size(px(11.0))
                    .text_color(theme.accent)
                    .into_any_element()
                } else {
                    div()
                        .font_family(theme.font_mono.clone())
                        .text_size(crate::typography::ui_rems(10.5))
                        .text_color(theme.text_muted)
                        .child(SharedString::from((ix + 1).to_string()))
                        .into_any_element()
                };
                popover::menu_row(theme, active, format!("music-row-fade-{id}"))
                    .id(("music-row", id))
                    .group(group.clone())
                    .gap(px(8.0))
                    .py(px(5.0))
                    .on_click(cx.listener(move |this, _, _, cx| this.play_id(id, cx)))
                    .child(
                        div()
                            .w(px(16.0))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .child(lead),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.5))
                            .child(SharedString::from(entry.track.title.clone())),
                    )
                    .child(
                        div()
                            .relative()
                            .w(px(36.0))
                            .h(px(16.0))
                            .flex_none()
                            .child(
                                div()
                                    .absolute()
                                    .right_0()
                                    .top_0()
                                    .h_full()
                                    .flex()
                                    .items_center()
                                    .font_family(theme.font_mono.clone())
                                    .text_size(crate::typography::ui_rems(10.5))
                                    .text_color(theme.text_muted)
                                    .group_hover(group.clone(), |style| style.opacity(0.0))
                                    .child(SharedString::from(
                                        entry.track.duration.map(format_time).unwrap_or_default(),
                                    )),
                            )
                            .child(
                                div()
                                    .id(("music-remove", id))
                                    .absolute()
                                    .right(px(-2.0))
                                    .top_0()
                                    .size(px(16.0))
                                    .rounded(px(4.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .opacity(0.0)
                                    .group_hover(group, |style| style.opacity(1.0))
                                    .hover(|style| style.bg(wash(0.1)))
                                    .tooltip(text_tooltip("Remove"))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.remove(id, cx);
                                    }))
                                    .child(
                                        icon(icons::CLOSE)
                                            .size(px(9.0))
                                            .text_color(theme.text_muted),
                                    ),
                            ),
                    )
                    .into_any_element()
            })
            .collect();
        let column = div()
            .absolute()
            .inset_0()
            .pl(px(4.0))
            .flex()
            .flex_col()
            .gap(px(popover::MENU_GAP))
            .child(
                div()
                    .pl(px(8.0))
                    .pt(px(2.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from(format!("Queue · {}", self.queue.len())))
                    .child(
                        control_button(
                            "music-clear",
                            icons::TRASH_BIN_MINIMALISTIC,
                            12.0,
                            theme,
                            true,
                            false,
                        )
                        .size(px(22.0))
                        .tooltip(text_tooltip("Clear queue"))
                        .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
                    ),
            )
            .child(popover::faded_menu_list(
                &self.queue_scroll,
                div()
                    .id("music-queue")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.queue_scroll)
                    .flex()
                    .flex_col()
                    .gap(px(popover::MENU_GAP))
                    .children(rows),
            ));
        div()
            .relative()
            .w(px(QUEUE_WIDTH))
            .flex_none()
            .ml(px(2.0))
            .border_l_1()
            .border_color(hairline(0.06))
            .child(column)
            .into_any_element()
    }
}

impl Drop for MusicPlayer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.cache_dir);
    }
}

impl Render for MusicPlayer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_popup();
        let open = self.popup.is_open();
        let lit = open || self.playing;
        let mut trigger = div()
            .id("music-trigger")
            .role(gpui::Role::Button)
            .aria_label("Music")
            .aria_expanded(open)
            .tab_index(0)
            .focus_visible(|s| s.border_2().border_color(theme.accent))
            .relative()
            .size(px(BUTTON_SIZE))
            .flex_none()
            .rounded(px(8.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(if open {
                theme.glass_hover()
            } else {
                motion::hover_blend(
                    "music-trigger",
                    theme.glass_hover().opacity(0.0),
                    theme.glass_hover(),
                )
            })
            .on_hover(motion::hover_listener("music-trigger"))
            .when(!open, |el| el.tooltip(text_tooltip("Music")))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, _| this.popup.note_trigger_press()),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                if this.popup.take_press_was_open() {
                    this.close(cx);
                } else {
                    this.open(window, cx);
                }
            }))
            .child(icon(icons::MUSIC_NOTE).size(px(15.0)).text_color(if lit {
                theme.text
            } else {
                motion::hover_blend("music-trigger", theme.text_muted, theme.text)
            }))
            .when(self.playing, |el| {
                el.child(
                    div()
                        .absolute()
                        .top(px(5.0))
                        .right(px(5.0))
                        .size(px(5.0))
                        .rounded_full()
                        .bg(theme.accent),
                )
            });
        if self.popup.get().is_some() {
            let closing = self.popup.closing_since();
            let card = self.render_card(&theme, cx);
            trigger = trigger.child(popover::anchored_menu_above("music-popover", card, closing));
        }
        trigger
    }
}

fn slider_track(theme: &Theme, fraction: f32, active: bool) -> gpui::Div {
    let fraction = fraction.clamp(0.0, 1.0);
    div()
        .absolute()
        .left_0()
        .right_0()
        .top(px(5.5))
        .h(px(3.0))
        .rounded_full()
        .bg(hairline(0.12))
        .child(
            div()
                .h_full()
                .w(gpui::relative(fraction))
                .rounded_full()
                .bg(theme.accent),
        )
        .child(
            div()
                .absolute()
                .left(gpui::relative(fraction))
                .ml(px(-4.5))
                .top(px(-3.0))
                .size(px(9.0))
                .rounded_full()
                .bg(theme.accent)
                .when(!active, |el| el.opacity(0.0))
                .group_hover("music-slider", |style| style.opacity(1.0)),
        )
}

fn control_button(
    id: &'static str,
    path: &'static str,
    size: f32,
    theme: &Theme,
    enabled: bool,
    active: bool,
) -> gpui::Stateful<gpui::Div> {
    let tint = if active {
        theme.accent
    } else {
        theme.text_muted
    };
    div()
        .id(id)
        .role(gpui::Role::Button)
        .size(px(BUTTON_SIZE))
        .flex_none()
        .rounded(px(8.0))
        .flex()
        .items_center()
        .justify_center()
        .when(!enabled, |el| el.opacity(0.35))
        .when(enabled, |el| {
            el.cursor_pointer()
                .bg(motion::hover_blend(id, wash(0.0), wash(0.08)))
                .on_hover(motion::hover_listener(id))
        })
        .child(icon(path).size(px(size)).text_color(if enabled && !active {
            motion::hover_blend(id, tint, theme.text)
        } else {
            tint
        }))
}

fn play_button(theme: &Theme, playing: bool, enabled: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id("music-play")
        .role(gpui::Role::Button)
        .aria_label(if playing { "Pause" } else { "Play" })
        .size(px(34.0))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(theme.text)
        .when(!enabled, |el| el.opacity(0.35))
        .when(enabled, |el| {
            el.cursor_pointer().hover(|style| style.opacity(0.88))
        })
        .child(
            icon(if playing {
                icons::MUSIC_PAUSE
            } else {
                icons::MUSIC_PLAY
            })
            .size(px(14.0))
            .text_color(theme.bg),
        )
}

fn cover(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(ART_MAX_SOURCE_PX);
    limits.max_image_height = Some(ART_MAX_SOURCE_PX);
    reader.limits(limits);
    let mut pixels = reader
        .decode()
        .ok()?
        .resize_to_fill(ART_PX, ART_PX, image::imageops::FilterType::Triangle)
        .into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new([image::Frame::new(pixels)])))
}

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0).floor() as u64;
    let (hours, minutes, secs) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{secs:02}")
    } else {
        format!("{minutes}:{secs:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::format_time;

    #[test]
    fn times_drop_the_hour_until_needed() {
        assert_eq!(format_time(0.0), "0:00");
        assert_eq!(format_time(61.9), "1:01");
        assert_eq!(format_time(3_725.0), "1:02:05");
        assert_eq!(format_time(-4.0), "0:00");
    }
}
