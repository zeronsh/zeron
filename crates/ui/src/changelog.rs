//! "What's new": a one-time window shown on the first launch after an update,
//! built from the project's GitHub release notes ([`zeron_update::release_notes`]).
//!
//! Launch flow ([`Changelog::init`]): compare the running version with the
//! last one whose notes were seen (`last_seen_changelog_version`). A fresh
//! install records the running version silently; an upgrade fetches the notes
//! for every release since the last one seen (capped, newest first) and opens
//! the window once they arrive. Failures stay silent and retry next launch —
//! the version is only recorded once the window has actually opened.
//!
//! The window is an in-app modal like the other dialogs, so it behaves the
//! same on every platform. It can be reopened from the app menu ("Show Update
//! Log…") or the account menu, and Escape, Enter or Space skips it.
//!
//! Motion: scrim fade, card rise, a once-only light sweep and logo ring on the
//! header, a slowly drifting accent glow, and per-row staggered entrances. All
//! of it is `with_animation`/pulse-clock driven, so reduced motion snaps to the
//! settled state and the loop parks when the window closes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnimationExt as _, AnyElement, App, AppContext as _, Context, Entity, EntityId, Global, Hsla,
    IntoElement, ScrollHandle, SharedString, Task, Window, div, linear_color_stop, linear_gradient,
    prelude::*, px,
};
use gpui_tokio::Tokio;
use zeron_update::release_notes::{self, Change, ChangeKind, Item, Release};

use crate::icons::{self, icon};
use crate::markdown::{self, BlockTree, render};
use crate::motion::{self, EASE, EASE_OUT_EXPO, MotionSpec};
use crate::popover;
use crate::settings::{self, SavePolicy};
use crate::theme::Theme;

/// Time between the app finishing boot and the window opening, so the entrance
/// plays on a settled UI instead of behind the boot splash.
const OPEN_DELAY: Duration = Duration::from_millis(900);

/// Exit transition length.
const EXIT_MS: u64 = 170;

const CARD_WIDTH: f32 = 580.0;
const CARD_MAX_HEIGHT: f32 = 700.0;
const CARD_RADIUS: f32 = 20.0;
const HEADER_HEIGHT: f32 = 148.0;

/// Entrance for the card: a long expo-out settle.
const CARD_IN: MotionSpec = MotionSpec::new(560, EASE_OUT_EXPO);
const SCRIM_IN: MotionSpec = MotionSpec::new(260, EASE);
/// One light sweep across the header shortly after the card lands.
const SWEEP: MotionSpec = MotionSpec::new(1100, EASE_OUT_EXPO).with_delay(420);
/// Expanding ring behind the logo tile.
const RING: MotionSpec = MotionSpec::new(1300, EASE_OUT_EXPO).with_delay(280);
/// Slow drift of the header glow (a loop on the shared pulse clock).
const GLOW_LOOP: MotionSpec = MotionSpec::new(16_000, EASE);
/// Rows beyond this many share the last stagger slot.
const STAGGER_ROWS: usize = 14;

/// The release set the window shows, newest first.
struct Loaded {
    /// Flat, ready-to-paint rows (parsed markdown included).
    rows: Vec<Row>,
    /// Version on the header.
    version: String,
    published: Option<String>,
    /// "Full changelog" destination: GitHub's compare link, else the release.
    more_url: Option<String>,
    /// Older releases the cap left out.
    omitted: usize,
}

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Hidden,
    /// Waiting on the network (or the post-boot delay); nothing painted.
    Loading,
    Open,
    Closing(Instant),
}

pub struct Changelog {
    api_url: String,
    phase: Phase,
    loaded: Option<Loaded>,
    scroll: ScrollHandle,
    task: Option<Task<()>>,
}

struct GlobalChangelog(Entity<Changelog>);

impl Global for GlobalChangelog {}

impl Changelog {
    /// Start the launch check. Call once at boot after settings and
    /// `gpui_tokio` are initialized. `existing_install` is whether a settings
    /// file existed before this launch — the line between a fresh install
    /// (nothing to catch up on) and an upgrade from a build that predates this
    /// window.
    pub fn init(existing_install: bool, cx: &mut App) {
        let api_url = std::env::var("ZERON_CHANGELOG_API")
            .unwrap_or_else(|_| release_notes::RELEASES_API.to_owned());
        let entity = cx.new(|_| Self {
            api_url,
            phase: Phase::Hidden,
            loaded: None,
            scroll: ScrollHandle::new(),
            task: None,
        });
        cx.set_global(GlobalChangelog(entity.clone()));
        let current = zeron_update::current_version();
        let seen = settings::current(cx).last_seen_changelog_version;
        match launch_action(seen.as_deref(), existing_install, current, auto_enabled()) {
            LaunchAction::Nothing => {}
            LaunchAction::Record => record_seen(current, cx),
            LaunchAction::Fetch { since } => {
                entity.update(cx, |this, cx| this.fetch(since, false, cx));
            }
        }
    }

    pub fn global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalChangelog>()
            .map(|global| global.0.clone())
    }

    /// Whether the window is on screen (including its exit).
    pub fn is_visible(&self) -> bool {
        matches!(self.phase, Phase::Open | Phase::Closing(_))
    }

    /// Fetch the notes between `since` and the running version and open the
    /// window. `manual` requests surface failures by opening the releases
    /// page instead of staying silent.
    fn fetch(&mut self, since: Option<String>, manual: bool, cx: &mut Context<Self>) {
        if self.phase != Phase::Hidden {
            return;
        }
        self.phase = Phase::Loading;
        let api_url = self.api_url.clone();
        let current = zeron_update::current_version();
        let request = Tokio::spawn(cx, async move {
            release_notes::fetch_release_notes(&api_url, since.as_deref(), current).await
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let outcome = match request.await {
                Ok(Ok(found)) => Ok(found),
                Ok(Err(err)) => Err(format!("{err:#}")),
                Err(join) => Err(join.to_string()),
            };
            if !manual {
                cx.background_executor().timer(OPEN_DELAY).await;
            }
            this.update(cx, |this, cx| {
                this.phase = Phase::Hidden;
                match outcome {
                    Ok((releases, omitted)) if !releases.is_empty() => {
                        this.open(&releases, omitted, cx);
                    }
                    Ok(_) => {
                        tracing::info!("no release notes published for this version yet");
                        if manual {
                            cx.open_url(zeron_update::RELEASES_PAGE);
                        }
                    }
                    Err(message) => {
                        tracing::info!(%message, "release notes unavailable");
                        if manual {
                            cx.open_url(zeron_update::RELEASES_PAGE);
                        }
                    }
                }
            })
            .ok();
        }));
    }

    fn open(&mut self, releases: &[Release], omitted: usize, cx: &mut Context<Self>) {
        let Some(newest) = releases.first() else {
            return;
        };
        self.loaded = Some(Loaded {
            rows: build_rows(releases),
            version: newest.version.clone(),
            published: newest.published_at.as_deref().and_then(format_date),
            more_url: newest
                .notes
                .full_changelog
                .clone()
                .or_else(|| Some(newest.url.clone()))
                .filter(|url| is_github_url(url)),
            omitted,
        });
        self.scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        self.phase = Phase::Open;
        // Recorded on show, not on dismiss: quitting with the window up must
        // not replay it next launch.
        record_seen(zeron_update::current_version(), cx);
        cx.notify();
    }

    /// Close with the exit transition (immediately under reduced motion).
    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.phase != Phase::Open {
            return;
        }
        if cx.reduce_motion() {
            self.phase = Phase::Hidden;
            cx.notify();
            return;
        }
        self.phase = Phase::Closing(Instant::now());
        self.task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(EXIT_MS))
                .await;
            this.update(cx, |this, cx| {
                if matches!(this.phase, Phase::Closing(_)) {
                    this.phase = Phase::Hidden;
                    cx.notify();
                }
            })
            .ok();
        }));
        cx.notify();
    }

    /// "What's New…": show the running version's notes again. Offline or
    /// unpublished notes fall back to the releases page.
    pub fn show_current(&mut self, cx: &mut Context<Self>) {
        match self.phase {
            Phase::Hidden => self.fetch(None, true, cx),
            Phase::Open | Phase::Closing(_) | Phase::Loading => {}
        }
    }
}

fn auto_enabled() -> bool {
    !matches!(
        std::env::var("ZERON_CHANGELOG").ok().as_deref(),
        Some("0" | "off" | "false")
    )
}

#[derive(Debug, PartialEq, Eq)]
enum LaunchAction {
    Nothing,
    /// Remember the running version without showing anything.
    Record,
    Fetch {
        since: Option<String>,
    },
}

/// What a launch on `current` should do given the last version seen. Pure so
/// the install/upgrade/downgrade matrix is testable.
fn launch_action(
    seen: Option<&str>,
    existing_install: bool,
    current: &str,
    enabled: bool,
) -> LaunchAction {
    match seen {
        Some(seen) if seen.trim_start_matches('v') == current => LaunchAction::Nothing,
        // A downgrade (or a build older than the notes already read) shows
        // nothing and keeps the high-water mark.
        Some(seen) if zeron_update::version_newer(seen, current) => LaunchAction::Nothing,
        _ if !enabled => LaunchAction::Nothing,
        Some(seen) => LaunchAction::Fetch {
            since: Some(seen.trim_start_matches('v').to_owned()),
        },
        None if existing_install => LaunchAction::Fetch { since: None },
        None => LaunchAction::Record,
    }
}

fn record_seen(version: &str, cx: &mut App) {
    let version = version.to_owned();
    settings::update(SavePolicy::Immediate, cx, move |settings| {
        settings.last_seen_changelog_version = Some(version);
    });
}

/// App-menu / account-menu entry point.
pub fn show(cx: &mut App) {
    crate::activate_main_window(cx);
    if let Some(changelog) = Changelog::global(cx) {
        changelog.update(cx, |changelog, cx| changelog.show_current(cx));
    }
}

/// Keys that skip the window: Escape, Enter and Space all dismiss it.
pub fn is_skip_key(key: &str) -> bool {
    matches!(key, "escape" | "enter" | "space")
}

/// Scroll the open window's body to `offset` px from the top. Review
/// fixtures use it to capture content below the fold.
#[doc(hidden)]
pub fn scroll_to(offset: f32, cx: &mut App) {
    if let Some(changelog) = Changelog::global(cx) {
        changelog
            .read(cx)
            .scroll
            .set_offset(gpui::point(px(0.0), px(-offset)));
    }
    cx.refresh_windows();
}

/// Close the window if it is up; whether it consumed the key press.
pub fn dismiss_if_visible(cx: &mut App) -> bool {
    let Some(changelog) = Changelog::global(cx) else {
        return false;
    };
    let visible = changelog.read(cx).is_visible();
    if visible {
        changelog.update(cx, |changelog, cx| changelog.dismiss(cx));
    }
    visible
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

enum Row {
    Release {
        version: String,
        date: Option<String>,
        latest: bool,
    },
    Heading(String),
    Group {
        kind: ChangeKind,
        count: usize,
    },
    Change {
        change: Change,
        title: Arc<BlockTree>,
    },
    Note(Arc<BlockTree>),
    Contributors(Vec<String>),
}

fn kind_order(kind: ChangeKind) -> u8 {
    match kind {
        ChangeKind::New => 0,
        ChangeKind::Improvement => 1,
        ChangeKind::Fix => 2,
    }
}

fn kind_label(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::New => "New",
        ChangeKind::Improvement => "Improvements",
        ChangeKind::Fix => "Fixes",
    }
}

fn kind_color(kind: ChangeKind, theme: &Theme) -> Hsla {
    match kind {
        ChangeKind::New => theme.accent,
        ChangeKind::Improvement => theme.success,
        ChangeKind::Fix => theme.warning,
    }
}

/// Flatten the releases into paint-ready rows: one release header per version
/// when several are stacked, then each section's changes grouped New →
/// Improvements → Fixes, hand-written notes in place, contributors last.
fn build_rows(releases: &[Release]) -> Vec<Row> {
    let mut rows = Vec::new();
    let stacked = releases.len() > 1;
    for (index, release) in releases.iter().enumerate() {
        if stacked {
            rows.push(Row::Release {
                version: release.version.clone(),
                date: release.published_at.as_deref().and_then(format_date),
                latest: index == 0,
            });
        }
        if release.notes.is_empty() {
            rows.push(Row::Note(Arc::new(markdown::parse_full(
                "_No release notes were published for this version._",
            ))));
        }
        for section in &release.notes.sections {
            let changes: Vec<&Change> = section
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Change(change) => Some(change),
                    _ => None,
                })
                .collect();
            // GitHub's own "What's Changed" title adds nothing over the
            // grouped headings below it.
            let generic = section
                .title
                .as_deref()
                .is_some_and(|title| title.eq_ignore_ascii_case("what's changed"));
            if let Some(title) = &section.title
                && !(generic && !changes.is_empty())
                && !section.items.is_empty()
                && !section
                    .items
                    .iter()
                    .all(|item| matches!(item, Item::Contributor { .. }))
            {
                rows.push(Row::Heading(title.clone()));
            }
            let mut kinds: Vec<ChangeKind> = changes.iter().map(|change| change.kind).collect();
            kinds.sort_by_key(|kind| kind_order(*kind));
            kinds.dedup();
            for kind in kinds {
                let group: Vec<&&Change> = changes.iter().filter(|c| c.kind == kind).collect();
                rows.push(Row::Group {
                    kind,
                    count: group.len(),
                });
                for change in group {
                    rows.push(Row::Change {
                        title: Arc::new(markdown::parse_full(&change.title)),
                        change: (*change).clone(),
                    });
                }
            }
            let handles: Vec<String> = section
                .items
                .iter()
                .filter_map(|item| match item {
                    Item::Contributor { handle, .. } => Some(handle.clone()),
                    _ => None,
                })
                .collect();
            if !handles.is_empty() {
                rows.push(Row::Heading("New contributors".into()));
                rows.push(Row::Contributors(handles));
            }
            for item in &section.items {
                if let Item::Note(text) = item {
                    rows.push(Row::Note(Arc::new(markdown::parse_full(text))));
                }
            }
        }
    }
    rows
}

/// `2026-10-01T01:04:21Z` → `Oct 1, 2026`.
fn format_date(timestamp: &str) -> Option<String> {
    let date = timestamp.split('T').next()?;
    let mut parts = date.split('-');
    let year: u32 = parts.next()?.parse().ok()?;
    let month: usize = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month = MONTHS.get(month.checked_sub(1)?)?;
    (1..=31)
        .contains(&day)
        .then(|| format!("{month} {day}, {year}"))
}

/// Links open in the browser; only GitHub URLs from the feed are trusted.
fn is_github_url(url: &str) -> bool {
    url.starts_with("https://github.com/")
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

/// The window for the current phase, or `None` while hidden. `view` is the
/// entity whose render calls this (the shell) — it is re-rendered by the pulse
/// clock for the glow drift and the exit.
pub fn render(
    viewport: gpui::Size<gpui::Pixels>,
    window: &Window,
    view: EntityId,
    cx: &mut App,
) -> Option<AnyElement> {
    let changelog = Changelog::global(cx)?;
    let (closing, scroll) = {
        let this = changelog.read(cx);
        let closing = match this.phase {
            Phase::Open => None,
            Phase::Closing(at) => Some(at),
            Phase::Hidden | Phase::Loading => return None,
        };
        this.loaded.as_ref()?;
        (closing, this.scroll.clone())
    };
    let theme = Theme::of(cx).for_popup();
    // Exit progress is wall-clock derived (see `motion::menu_out`): the
    // entrance animations hold their ids, so the exit must not swap any.
    let exit = closing
        .map(|at| EASE.eval((at.elapsed().as_millis() as f32 / EXIT_MS as f32).min(1.0)))
        .unwrap_or(0.0);
    if closing.is_some() {
        motion::pulse_lease(view, cx);
    }
    let glow = motion::pulse_delta(&GLOW_LOOP, view, cx);

    let this = changelog.read(cx);
    let loaded = this.loaded.as_ref()?;
    let (viewport_w, viewport_h) = (f32::from(viewport.width), f32::from(viewport.height));
    let width = CARD_WIDTH.min((viewport_w - 48.0).max(320.0));
    let height = CARD_MAX_HEIGHT.min((viewport_h - 72.0).max(320.0));
    let compact = viewport_w < 520.0;

    let header = render_header(loaded, &theme, glow, compact);
    let body = render_body(loaded, &theme, window, &scroll);
    let footer = render_footer(loaded, &theme);

    let card = div()
        .id("changelog-card")
        .occlude()
        .w(px(width))
        .h(px(height))
        .rounded(px(CARD_RADIUS))
        .bg(popover::surface_bg(&theme))
        .border_1()
        .border_color(crate::theme::hairline(0.12))
        .when(!theme.is_frost(), |el| el.shadow_lg())
        .flex()
        .flex_col()
        .overflow_hidden()
        .text_color(theme.text)
        .child(header)
        .child(body)
        .child(footer);
    let card = crate::frost::frosted(CARD_RADIUS, crate::frost::MENU_BLUR, card);

    let scrim_alpha = 0.42;
    let scrim = div()
        .id("changelog-scrim")
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(crate::theme::scrim(scrim_alpha))
        .on_click(|_, _, cx| {
            dismiss_if_visible(cx);
        })
        .with_animation("changelog-scrim-in", SCRIM_IN.animation(), move |el, t| {
            el.opacity(t * (1.0 - exit))
        });

    let layer = div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .child(div().child(card).with_animation(
            "changelog-card-in",
            CARD_IN.animation(),
            move |el, t| {
                el.relative()
                    .opacity(t * (1.0 - exit))
                    .top(px(26.0 * (1.0 - t) + 10.0 * exit))
            },
        ));

    Some(
        gpui::deferred(
            gpui::anchored()
                .position(gpui::point(px(0.0), px(0.0)))
                .child(
                    div()
                        .occlude()
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .relative()
                        .w(viewport.width)
                        .h(viewport.height)
                        .child(scrim)
                        .child(layer),
                ),
        )
        .priority(3)
        .into_any_element(),
    )
}

/// Entrance for one staggered piece of content: fade + rise.
fn appear<E>(id: impl Into<gpui::ElementId>, delay_ms: u64, rise: f32, element: E) -> AnyElement
where
    E: gpui::Styled + IntoElement + 'static,
{
    let spec = MotionSpec::new(620, EASE_OUT_EXPO).with_delay(delay_ms);
    element
        .with_animation(id, spec.animation(), move |el, t| {
            el.relative().opacity(t).top(px(rise * (1.0 - t)))
        })
        .into_any_element()
}

/// A soft radial glow from stacked translucent discs (gpui has no blur).
fn glow_disc(color: Hsla, diameter: f32, peak: f32) -> gpui::Div {
    const LAYERS: usize = 9;
    let mut disc = div().relative().w(px(diameter)).h(px(diameter));
    for layer in 0..LAYERS {
        let fraction = 1.0 - layer as f32 / LAYERS as f32;
        let size = diameter * fraction;
        let inset = (diameter - size) / 2.0;
        disc = disc.child(
            div()
                .absolute()
                .left(px(inset))
                .top(px(inset))
                .w(px(size))
                .h(px(size))
                .rounded_full()
                .bg(color.opacity(peak / LAYERS as f32)),
        );
    }
    disc
}

fn render_header(loaded: &Loaded, theme: &Theme, glow: f32, compact: bool) -> AnyElement {
    let phase = glow * std::f32::consts::TAU;
    let (drift_x, drift_y) = (phase.sin() * 10.0, phase.cos() * 5.0);
    let subtitle: SharedString = match &loaded.published {
        Some(date) => format!("Zeron {} · {date}", loaded.version).into(),
        None => format!("Zeron {}", loaded.version).into(),
    };

    // The app icon, with a ring that expands from it once.
    const TILE: f32 = 64.0;
    const TILE_RADIUS: f32 = 14.0;
    let tile = div()
        .relative()
        .flex_none()
        .w(px(TILE))
        .h(px(TILE))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .w(px(TILE))
                .h(px(TILE))
                .rounded(px(TILE_RADIUS))
                .border_2()
                .border_color(theme.accent)
                .with_animation("changelog-ring", RING.animation(), |el, t| {
                    let grow = 44.0 * t;
                    el.relative()
                        .left(px(-grow / 2.0))
                        .top(px(-grow / 2.0))
                        .w(px(TILE + grow))
                        .h(px(TILE + grow))
                        .opacity(0.55 * (1.0 - t))
                }),
        )
        .child(
            gpui::img(icons::APP_ICON)
                .absolute()
                .top_0()
                .left_0()
                .w(px(TILE))
                .h(px(TILE))
                .rounded(px(TILE_RADIUS))
                .shadow_md(),
        );

    let mut text = div().flex().flex_col().gap(px(3.0)).min_w_0();
    text = text
        .child(appear(
            "changelog-title",
            140,
            10.0,
            div()
                .text_size(crate::typography::ui_rems(if compact {
                    20.0
                } else {
                    24.0
                }))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text)
                .child("What’s new"),
        ))
        .child(appear(
            "changelog-subtitle",
            230,
            8.0,
            div()
                .text_size(crate::typography::ui_rems(13.0))
                .text_color(theme.text_muted)
                .child(subtitle),
        ));

    let close = div()
        .id("changelog-close")
        .absolute()
        .top(px(14.0))
        .right(px(14.0))
        .w(px(28.0))
        .h(px(28.0))
        .rounded(px(8.0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .text_color(motion::hover_blend(
            "changelog-close",
            theme.text_muted,
            theme.text,
        ))
        .bg(motion::hover_blend(
            "changelog-close",
            crate::theme::wash(0.0),
            crate::theme::ink(0.08),
        ))
        .on_hover(motion::hover_listener("changelog-close"))
        .on_click(|_, _, cx| {
            dismiss_if_visible(cx);
        })
        .child(icon(icons::CLOSE).size(px(14.0)));

    div()
        .relative()
        .flex_none()
        .h(px(HEADER_HEIGHT))
        .overflow_hidden()
        // Matches the card's inner radius (its 1px border takes the rest).
        .rounded_t(px(CARD_RADIUS - 1.0))
        .border_b_1()
        .border_color(crate::theme::hairline(0.08))
        .bg(linear_gradient(
            180.0,
            linear_color_stop(theme.accent.opacity(0.16), 0.0),
            linear_color_stop(theme.accent.opacity(0.0), 1.0),
        ))
        // gpui clips children to rectangles, so nothing may paint in the
        // rounded corner zones: the glows are sized and drifted to stay
        // clear of them, and the sweep runs in an inset strip.
        .child(
            div()
                .absolute()
                .left(px(20.0 + drift_x))
                .top(px(-25.0 + drift_y))
                .child(glow_disc(theme.accent, 210.0, 0.42)),
        )
        .child(
            div()
                .absolute()
                .right(px(20.0 - drift_x))
                .top(px(-18.0 - drift_y))
                .child(glow_disc(theme.glyph.light, 180.0, 0.26)),
        )
        // One light sweep across the header.
        .child(
            div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(px(CARD_RADIUS + 4.0))
                .right(px(CARD_RADIUS + 4.0))
                .overflow_hidden()
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(px(140.0))
                        .flex()
                        .flex_row()
                        // Two-stop gradients only: rise, then fall.
                        .child(div().h_full().w(px(70.0)).bg(linear_gradient(
                            90.0,
                            linear_color_stop(gpui::white().opacity(0.0), 0.0),
                            linear_color_stop(gpui::white().opacity(0.13), 1.0),
                        )))
                        .child(div().h_full().w(px(70.0)).bg(linear_gradient(
                            90.0,
                            linear_color_stop(gpui::white().opacity(0.13), 0.0),
                            linear_color_stop(gpui::white().opacity(0.0), 1.0),
                        )))
                        .with_animation("changelog-sweep", SWEEP.animation(), |el, t| {
                            el.relative()
                                .left(px(-140.0 + 700.0 * t))
                                .opacity((1.0 - t).min(1.0))
                        }),
                ),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .px(px(28.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(18.0))
                .child(appear("changelog-tile", 60, 14.0, tile))
                .child(text),
        )
        .child(close)
        .into_any_element()
}

fn render_body(
    loaded: &Loaded,
    theme: &Theme,
    window: &Window,
    scroll: &ScrollHandle,
) -> AnyElement {
    let mut column = div()
        .flex()
        .flex_col()
        .px(px(14.0))
        .pt(px(16.0))
        .pb(px(22.0));
    for (index, row) in loaded.rows.iter().enumerate() {
        let delay = 300 + 48 * index.min(STAGGER_ROWS) as u64;
        column = column.child(render_row(index, row, delay, theme, window));
    }
    if loaded.omitted > 0 {
        column = column.child(
            div()
                .id("changelog-older")
                .mt(px(10.0))
                .px(px(10.0))
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text_muted)
                .child(format!(
                    "{} earlier {} not shown.",
                    loaded.omitted,
                    if loaded.omitted == 1 {
                        "release is"
                    } else {
                        "releases are"
                    }
                )),
        );
    }
    // The wrapper gives the scroll region the card's remaining height.
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(
            crate::edge_fade::edge_faded(
                Theme::TRANSCRIPT_FADE_BAND,
                true,
                true,
                div()
                    .id("changelog-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(scroll)
                    .child(column),
            )
            .fade_overflow_y(scroll),
        )
        .into_any_element()
}

fn render_row(index: usize, row: &Row, delay: u64, theme: &Theme, window: &Window) -> AnyElement {
    let id = SharedString::from(format!("changelog-row-{index}"));
    match row {
        Row::Release {
            version,
            date,
            latest,
        } => {
            let mut line = div()
                .mt(px(if index == 0 { 0.0 } else { 18.0 }))
                .px(px(10.0))
                .pb(px(6.0))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(icon(icons::TAG).size(px(14.0)).text_color(theme.accent))
                .child(
                    div()
                        .text_size(crate::typography::ui_rems(15.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .child(format!("v{version}")),
                );
            if let Some(date) = date {
                line = line.child(
                    div()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted)
                        .child(date.clone()),
                );
            }
            if *latest {
                line = line.child(chip("Latest", theme.accent, theme));
            }
            appear(id, delay, 10.0, line)
        }
        Row::Heading(title) => appear(
            id,
            delay,
            8.0,
            div()
                .mt(px(14.0))
                .px(px(10.0))
                .pb(px(4.0))
                .text_size(crate::typography::ui_rems(11.0))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.text_muted)
                .child(SharedString::from(title.to_uppercase())),
        ),
        Row::Group { kind, count } => {
            let color = kind_color(*kind, theme);
            appear(
                id,
                delay,
                8.0,
                div()
                    .mt(px(if index == 0 { 0.0 } else { 14.0 }))
                    .px(px(10.0))
                    .pb(px(4.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(color))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(color)
                            .child(kind_label(*kind)),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(count.to_string()),
                    ),
            )
        }
        Row::Change { change, title } => {
            let key = format!("changelog-change-{index}");
            let mut meta = div()
                .mt(px(3.0))
                .flex()
                .flex_row()
                .flex_wrap()
                .items_center()
                .gap(px(6.0))
                .text_size(crate::typography::ui_rems(11.5))
                .text_color(theme.text_muted);
            if let Some(area) = &change.area {
                meta = meta.child(chip(area, theme.text_muted, theme));
            }
            if let Some(pull) = change.pull {
                meta = meta.child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(3.0))
                        .child(
                            icon(icons::PULL_REQUEST)
                                .size(px(12.0))
                                .text_color(theme.text_faint),
                        )
                        .child(format!("#{pull}")),
                );
            }
            if let Some(author) = &change.author {
                meta = meta.child(format!("@{author}"));
            }
            let url = change.url.clone().filter(|url| is_github_url(url));
            let mut row = div()
                .id(SharedString::from(key.clone()))
                .px(px(10.0))
                .py(px(7.0))
                .rounded(px(10.0))
                .flex()
                .flex_row()
                .items_start()
                .gap(px(10.0))
                .bg(motion::hover_blend(
                    &key,
                    crate::theme::wash(0.0),
                    crate::theme::card_selected_bg(),
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .child(markdown_block(title, &key, theme, window))
                        .child(meta),
                )
                .child(
                    div()
                        .mt(px(4.0))
                        .child(icon(icons::ARROW_UP_RIGHT).size(px(14.0)).text_color(
                            motion::hover_blend(
                                &key,
                                theme.text_faint.opacity(0.0),
                                theme.text_muted,
                            ),
                        )),
                );
            row.interactivity()
                .on_hover(motion::hover_listener(key.clone()));
            if let Some(url) = url {
                row = row.cursor_pointer().on_click(move |_, _, cx| {
                    cx.open_url(&url);
                });
            }
            appear(id, delay, 12.0, row)
        }
        Row::Note(tree) => appear(
            id.clone(),
            delay,
            10.0,
            div()
                .px(px(10.0))
                .py(px(4.0))
                .text_color(theme.text_muted)
                .child(markdown_block(tree, id.as_ref(), theme, window)),
        ),
        Row::Contributors(handles) => {
            let mut wrap = div()
                .px(px(10.0))
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(px(8.0));
            for handle in handles {
                wrap = wrap.child(contributor_chip(handle, theme));
            }
            appear(id, delay, 10.0, wrap)
        }
    }
}

fn markdown_block(tree: &Arc<BlockTree>, key: &str, theme: &Theme, window: &Window) -> AnyElement {
    let opts = render::RenderOptions::settled(SharedString::from(key.to_owned()));
    render::render_tree(tree, &opts, theme, window, &|_| None)
}

fn chip(label: &str, tone: Hsla, theme: &Theme) -> gpui::Div {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(5.0))
        .bg(tone.opacity(0.13))
        .text_size(crate::typography::ui_rems(11.0))
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(if tone == theme.text_muted {
            theme.text_muted
        } else {
            tone
        })
        .child(SharedString::from(label.to_owned()))
}

fn contributor_chip(handle: &str, theme: &Theme) -> gpui::Div {
    let initial = handle
        .chars()
        .next()
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_default();
    div()
        .pl(px(4.0))
        .pr(px(10.0))
        .py(px(4.0))
        .rounded_full()
        .bg(theme.accent_wash)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(7.0))
        .child(
            div()
                .w(px(20.0))
                .h(px(20.0))
                .rounded_full()
                .bg(theme.accent)
                .flex()
                .items_center()
                .justify_center()
                .text_size(crate::typography::ui_rems(10.5))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.on_accent)
                .child(initial),
        )
        .child(
            div()
                .text_size(crate::typography::ui_rems(12.0))
                .text_color(theme.text)
                .child(format!("@{handle}")),
        )
}

fn render_footer(loaded: &Loaded, theme: &Theme) -> AnyElement {
    let mut left = div().flex().flex_row().items_center().gap(px(4.0));
    if let Some(url) = loaded.more_url.clone() {
        left = left.child(
            popover::btn_ghost(theme, "Full changelog on GitHub", "changelog-more")
                .id("changelog-more")
                .on_click(move |_, _, cx| cx.open_url(&url)),
        );
    }
    div()
        .flex_none()
        .px(px(14.0))
        .py(px(12.0))
        .border_t_1()
        .border_color(crate::theme::hairline(0.08))
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .child(left)
        .child(
            popover::btn_primary(theme, "Continue")
                .id("changelog-continue")
                .on_click(|_, _, cx| {
                    dismiss_if_visible(cx);
                }),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_update::release_notes::{Notes, Section};

    fn change(title: &str, kind: ChangeKind) -> Item {
        Item::Change(Change {
            title: title.into(),
            area: None,
            kind,
            author: Some("a".into()),
            pull: Some(1),
            url: Some("https://github.com/zeronsh/zeron/pull/1".into()),
        })
    }

    fn release(version: &str, sections: Vec<Section>) -> Release {
        Release {
            version: version.into(),
            url: "https://github.com/zeronsh/zeron/releases/tag/v1".into(),
            published_at: Some("2026-10-01T01:04:21Z".into()),
            notes: Notes {
                sections,
                full_changelog: None,
            },
        }
    }

    fn shape(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Release { version, .. } => format!("release {version}"),
                Row::Heading(title) => format!("heading {title}"),
                Row::Group { kind, count } => format!("group {} {count}", kind_label(*kind)),
                Row::Change { change, .. } => format!("change {}", change.title),
                Row::Note(_) => "note".into(),
                Row::Contributors(handles) => format!("contributors {}", handles.join(",")),
            })
            .collect()
    }

    #[test]
    fn launch_matrix() {
        use LaunchAction::*;
        let fetch = |since: Option<&str>| Fetch {
            since: since.map(str::to_owned),
        };
        // Fresh install: remember, show nothing.
        assert_eq!(launch_action(None, false, "0.2.100", true), Record);
        // Upgrade from a build that predates the window.
        assert_eq!(launch_action(None, true, "0.2.100", true), fetch(None));
        // Normal upgrade, with or without a `v` prefix.
        assert_eq!(
            launch_action(Some("0.2.98"), true, "0.2.100", true),
            fetch(Some("0.2.98"))
        );
        assert_eq!(
            launch_action(Some("v0.2.98"), true, "0.2.100", true),
            fetch(Some("0.2.98"))
        );
        // Already seen, and downgrades.
        assert_eq!(
            launch_action(Some("0.2.100"), true, "0.2.100", true),
            Nothing
        );
        assert_eq!(
            launch_action(Some("0.2.101"), true, "0.2.100", true),
            Nothing
        );
        // Disabled by the environment.
        assert_eq!(
            launch_action(Some("0.2.98"), true, "0.2.100", false),
            Nothing
        );
        assert_eq!(launch_action(None, false, "0.2.100", false), Nothing);
    }

    #[test]
    fn rows_group_changes_by_kind_and_hide_the_generic_heading() {
        let rows = build_rows(&[release(
            "0.2.100",
            vec![
                Section {
                    title: Some("What's Changed".into()),
                    items: vec![
                        change("Fix a", ChangeKind::Fix),
                        change("Add b", ChangeKind::New),
                        change("Tune c", ChangeKind::Improvement),
                        change("Fix d", ChangeKind::Fix),
                    ],
                },
                Section {
                    title: Some("New Contributors".into()),
                    items: vec![Item::Contributor {
                        handle: "octo".into(),
                        url: None,
                    }],
                },
            ],
        )]);
        assert_eq!(
            shape(&rows),
            [
                "group New 1",
                "change Add b",
                "group Improvements 1",
                "change Tune c",
                "group Fixes 2",
                "change Fix a",
                "change Fix d",
                "heading New contributors",
                "contributors octo",
            ]
        );
    }

    #[test]
    fn stacked_releases_get_headers_and_empty_notes_a_placeholder() {
        let rows = build_rows(&[
            release(
                "0.2.100",
                vec![Section {
                    title: Some("Highlights".into()),
                    items: vec![Item::Note("- one".into())],
                }],
            ),
            release("0.2.99", vec![]),
        ]);
        assert_eq!(
            shape(&rows),
            [
                "release 0.2.100",
                "heading Highlights",
                "note",
                "release 0.2.99",
                "note"
            ]
        );
        assert!(matches!(rows[0], Row::Release { latest: true, .. }));
        assert!(matches!(rows[3], Row::Release { latest: false, .. }));
    }

    // ---- the real window, rendered and driven in a test window -------------

    use gpui::{Render, TestAppContext};

    /// Paints [`render`] full-window, as the shell does.
    struct Host;

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let viewport = window.viewport_size();
            div()
                .size_full()
                .children(super::render(viewport, window, cx.entity_id(), cx))
        }
    }

    fn many_releases() -> Vec<Release> {
        let items = (0..40)
            .map(|n| change(&format!("Change number {n}"), ChangeKind::Improvement))
            .collect();
        vec![release(
            zeron_update::current_version(),
            vec![Section {
                title: Some("What's Changed".into()),
                items,
            }],
        )]
    }

    fn setup(cx: &mut TestAppContext) -> Entity<Changelog> {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            settings::init(settings::UiSettings::default(), dir.keep(), cx);
            let entity = cx.new(|_| Changelog {
                api_url: String::new(),
                phase: Phase::Hidden,
                loaded: None,
                scroll: ScrollHandle::new(),
                task: None,
            });
            cx.set_global(GlobalChangelog(entity.clone()));
            entity
        })
    }

    #[gpui::test]
    fn opening_records_the_version_and_paints(cx: &mut TestAppContext) {
        let changelog = setup(cx);
        assert!(
            !cx.update(dismiss_if_visible),
            "hidden: nothing to dismiss"
        );
        cx.update(|cx| {
            changelog.update(cx, |this, cx| this.open(&many_releases(), 2, cx));
            assert_eq!(
                settings::current(cx).last_seen_changelog_version.as_deref(),
                Some(zeron_update::current_version())
            );
        });
        let (_, cx) = cx.add_window_view(|_, _| Host);
        cx.simulate_resize(gpui::size(px(1100.0), px(800.0)));
        cx.run_until_parked();
        assert!(cx.update(|_, cx| changelog.read(cx).is_visible()));
    }

    #[gpui::test]
    fn dismissing_plays_the_exit_then_hides(cx: &mut TestAppContext) {
        let changelog = setup(cx);
        cx.update(|cx| changelog.update(cx, |this, cx| this.open(&many_releases(), 0, cx)));
        let (_, cx) = cx.add_window_view(|_, _| Host);
        cx.simulate_resize(gpui::size(px(1100.0), px(800.0)));
        cx.run_until_parked();

        assert!(cx.update(|_, cx| dismiss_if_visible(cx)));
        cx.run_until_parked();
        // Mid-exit: still painted, still "visible" so the keys stay captured.
        assert!(matches!(
            cx.update(|_, cx| changelog.read(cx).phase),
            Phase::Closing(_)
        ));
        cx.executor()
            .advance_clock(Duration::from_millis(EXIT_MS + 20));
        cx.run_until_parked();
        assert!(matches!(
            cx.update(|_, cx| changelog.read(cx).phase),
            Phase::Hidden
        ));
        assert!(!cx.update(|_, cx| dismiss_if_visible(cx)));
    }

    #[gpui::test]
    fn reduced_motion_closes_immediately(cx: &mut TestAppContext) {
        let changelog = setup(cx);
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            changelog.update(cx, |this, cx| this.open(&many_releases(), 0, cx));
            assert!(dismiss_if_visible(cx));
            assert!(matches!(changelog.read(cx).phase, Phase::Hidden));
        });
    }

    #[gpui::test]
    fn the_body_scrolls_under_the_modal_layer(cx: &mut TestAppContext) {
        let changelog = setup(cx);
        cx.update(|cx| changelog.update(cx, |this, cx| this.open(&many_releases(), 0, cx)));
        let (_, cx) = cx.add_window_view(|_, _| Host);
        cx.simulate_resize(gpui::size(px(1100.0), px(800.0)));
        cx.run_until_parked();
        let scroll = cx.update(|_, cx| changelog.read(cx).scroll.clone());
        assert_eq!(scroll.offset().y, px(0.0));
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(px(550.0), px(400.0)),
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-300.0))),
            ..Default::default()
        });
        cx.run_until_parked();
        assert!(
            scroll.offset().y < px(-100.0),
            "wheel over the body should scroll it, got {:?}",
            scroll.offset()
        );
    }

    #[test]
    fn escape_enter_and_space_skip_the_window() {
        for key in ["escape", "enter", "space"] {
            assert!(is_skip_key(key), "{key}");
        }
        for key in ["a", "tab", "backspace", "left", "down"] {
            assert!(!is_skip_key(key), "{key}");
        }
    }

    #[test]
    fn dates_and_links() {
        assert_eq!(
            format_date("2026-10-01T01:04:21Z").as_deref(),
            Some("Oct 1, 2026")
        );
        assert_eq!(format_date("2026-13-01T00:00:00Z"), None);
        assert_eq!(format_date("garbage"), None);
        assert!(is_github_url("https://github.com/zeronsh/zeron"));
        assert!(!is_github_url("https://evil.example/github.com/"));
        assert!(!is_github_url("http://github.com/x"));
    }
}
