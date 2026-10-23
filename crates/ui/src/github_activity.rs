use std::time::{Duration, Instant};

use gpui::{Context, Entity, IntoElement, Render, SharedString, Window, div, prelude::*, px};
use zeron_proto::{GitHubActivitySnapshot, GitHubActivityWeek};
use zeron_rpc::methods;

use crate::state::AppState;
use crate::theme::Theme;

const MIN_WIDTH: f32 = 600.0;
const PANEL_CHROME: f32 = 146.0;
const REFRESH_AFTER: Duration = Duration::from_secs(5 * 60);
const RETRY_AFTER: Duration = Duration::from_secs(60);
const IDLE_REFRESH: Duration = Duration::from_secs(30);
const SKELETON_AFTER: Duration = Duration::from_millis(200);
const SKELETON_WEEKS: usize = 53;
const REVEAL_STEP: f32 = 0.01;
const REVEAL_FADE: f32 = 0.3;

fn grouped(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in digits.bytes().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            out.push(',');
        }
        out.push(byte as char);
    }
    out.chars().rev().collect()
}

fn format_delta(delta: i32) -> String {
    let body = grouped(delta.unsigned_abs());
    if delta > 0 {
        format!("+{body}")
    } else if delta < 0 {
        format!("-{body}")
    } else {
        body
    }
}

pub struct GitHubActivity {
    state: Entity<AppState>,
    shown: bool,
    room: f32,
    width: f32,
    snapshot: Option<GitHubActivitySnapshot>,
    fetched_at: Option<Instant>,
    failed_at: Option<Instant>,
    hidden_at: Option<Instant>,
    loading_since: Option<Instant>,
    revealed_at: Option<Instant>,
}

impl GitHubActivity {
    pub fn new(state: Entity<AppState>, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            shown: false,
            room: 0.0,
            width: 0.0,
            snapshot: None,
            fetched_at: None,
            failed_at: None,
            hidden_at: None,
            loading_since: None,
            revealed_at: None,
        }
    }

    pub fn set_frame(&mut self, shown: bool, room: f32, width: f32, cx: &mut Context<Self>) {
        let resumed = shown && !self.shown;
        self.shown = shown;
        self.room = room;
        self.width = width;
        if !shown {
            self.hidden_at.get_or_insert_with(Instant::now);
            return;
        }
        let idle = resumed
            && self
                .hidden_at
                .take()
                .is_some_and(|at| at.elapsed() >= IDLE_REFRESH);
        self.ensure(idle, cx);
    }

    pub fn visible(&self) -> bool {
        self.cell().is_some()
    }

    #[cfg(test)]
    pub(crate) fn set_snapshot(&mut self, snapshot: GitHubActivitySnapshot) {
        self.snapshot = Some(snapshot);
        self.fetched_at = Some(Instant::now());
    }

    fn cell(&self) -> Option<f32> {
        let columns = match &self.snapshot {
            Some(snapshot) => snapshot.weeks.len(),
            None if self.failed_at.is_none()
                && self
                    .loading_since
                    .is_some_and(|at| at.elapsed() >= SKELETON_AFTER) =>
            {
                SKELETON_WEEKS
            }
            None => return None,
        };
        self.shown
            .then(|| activity_cell(self.width, columns, self.room))
            .flatten()
    }

    fn ensure(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.loading_since.is_some() {
            return;
        }
        let fresh = self
            .fetched_at
            .is_some_and(|at| at.elapsed() < REFRESH_AFTER);
        let failed = self.failed_at.is_some_and(|at| at.elapsed() < RETRY_AFTER);
        if !force && (fresh || failed) {
            return;
        }
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        let cached = self.snapshot.is_none();
        let refresh = force || !cached;
        self.loading_since = Some(Instant::now());
        if cached {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(SKELETON_AFTER).await;
                this.update(cx, |_, cx| cx.notify()).ok();
            })
            .detach();
        }
        cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(
                    methods::GET_GITHUB_ACTIVITY,
                    serde_json::json!({ "refresh": refresh, "cached": cached }),
                )
                .await
                .ok()
                .and_then(|value| serde_json::from_value::<GitHubActivitySnapshot>(value).ok());
            this.update(cx, |this, cx| {
                this.loading_since = None;
                match result {
                    Some(snapshot) => {
                        if this.snapshot.is_none() {
                            this.revealed_at = Some(Instant::now());
                        }
                        this.snapshot = Some(snapshot);
                        this.fetched_at = (!cached).then(Instant::now);
                        this.failed_at = None;
                    }
                    None => this.failed_at = Some(Instant::now()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

fn delta_color(theme: &Theme, delta: i32) -> gpui::Hsla {
    if delta > 0 {
        theme.success
    } else if delta < 0 {
        theme.danger
    } else {
        theme.text_muted
    }
}

fn stat_column(theme: &Theme, label: &str, count: gpui::Div, delta: i32) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .whitespace_nowrap()
                .text_size(crate::typography::ui_rems(12.0))
                .line_height(px(16.0))
                .text_color(theme.text_muted)
                .child(SharedString::from(label.to_owned())),
        )
        .child(div().h(px(26.0)).flex().items_center().child(count))
        .child(
            div()
                .h(px(16.0))
                .whitespace_nowrap()
                .text_size(crate::typography::ui_rems(12.0))
                .line_height(px(16.0))
                .text_color(delta_color(theme, delta))
                .when(delta != 0, |row| {
                    row.child(SharedString::from(format_delta(delta)))
                }),
        )
}

fn heat_fill(theme: &Theme, level: u8) -> gpui::Rgba {
    let colors: [u32; 5] = if matches!(theme.appearance, crate::theme::Appearance::Dark) {
        [0x161B22, 0x0E4429, 0x006D32, 0x26A641, 0x39D353]
    } else {
        [0xEBEDF0, 0x9BE9A8, 0x40C463, 0x30A14E, 0x216E39]
    };
    gpui::rgb(colors[usize::from(level.min(4))])
}

fn heat_cell(width: f32, columns: usize) -> f32 {
    let columns = columns.max(1) as f32;
    let gaps = (columns - 1.0).max(0.0) * 2.0;
    ((width - gaps) / columns).max(2.0)
}

fn activity_cell(width: f32, columns: usize, room: f32) -> Option<f32> {
    if width < MIN_WIDTH || columns == 0 {
        return None;
    }
    let cell = heat_cell(width, columns);
    (PANEL_CHROME + 7.0 * cell <= room).then_some(cell)
}

fn month_abbr(date: &str) -> Option<&'static str> {
    let month = date.get(5..7)?.parse::<usize>().ok()?;
    [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ]
    .get(month.checked_sub(1)?)
    .copied()
}

fn reveal(elapsed: f32, column: usize) -> f32 {
    crate::motion::EASE_OUT
        .eval(((elapsed - column as f32 * REVEAL_STEP) / REVEAL_FADE).clamp(0.0, 1.0))
}

fn heatmap(
    weeks: Option<&[GitHubActivityWeek]>,
    theme: &Theme,
    cell: f32,
    elapsed: f32,
) -> gpui::Div {
    let empty: gpui::Hsla = heat_fill(theme, 0).into();
    let weeks: Vec<Option<&GitHubActivityWeek>> = match weeks {
        Some(weeks) => weeks.iter().map(Some).collect(),
        None => vec![None; SKELETON_WEEKS],
    };
    let month_marks = weeks.iter().map(|week| {
        week.and_then(|week| week.days.iter().find(|day| day.date.ends_with("-01")))
            .and_then(|day| month_abbr(&day.date))
    });
    div()
        .mt(px(14.0))
        .w_full()
        .child(
            div()
                .flex()
                .gap(px(2.0))
                .mb(px(2.0))
                .children(month_marks.enumerate().map(|(column, mark)| {
                    div()
                        .opacity(reveal(elapsed, column))
                        .relative()
                        .w(px(cell))
                        .h(px(14.0))
                        .children(mark.map(|text| {
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .whitespace_nowrap()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(SharedString::from(text))
                        }))
                })),
        )
        .child(
            div()
                .flex()
                .gap(px(2.0))
                .children(weeks.iter().enumerate().map(|(column, week)| {
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .children((0..7).map(|row| {
                            let tile = div().w(px(cell)).h(px(cell)).rounded(px(2.0));
                            let level = match week {
                                Some(week) => week
                                    .days
                                    .iter()
                                    .find(|day| day.weekday == row)
                                    .map(|day| day.level),
                                None => Some(0),
                            };
                            match level {
                                Some(level) => tile.bg(crate::motion::mix(
                                    empty,
                                    heat_fill(theme, level).into(),
                                    reveal(elapsed, column),
                                )),
                                None => tile,
                            }
                        }))
                })),
        )
}

impl Render for GitHubActivity {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(cell) = self.cell() else {
            return div();
        };
        let theme = Theme::of(cx).clone();
        let view = cx.entity_id();
        let snapshot = self.snapshot.as_ref();
        let elapsed = match self.revealed_at {
            Some(at) if !cx.reduce_motion() => at.elapsed().as_secs_f32(),
            _ => f32::INFINITY,
        };
        if elapsed < SKELETON_WEEKS as f32 * REVEAL_STEP + REVEAL_FADE {
            window.request_animation_frame();
        }
        let pulse = match snapshot {
            Some(_) => 1.0,
            None => {
                let delta = crate::motion::pulse_delta(&crate::motion::ZERON_PULSE, view, cx);
                let phase = crate::motion::staggered_phase(delta, 4, 0.08);
                0.45 + 0.4 * crate::motion::pulse_wave(phase)
            }
        };
        let stats = [
            (
                "Contributions",
                snapshot.map(|snapshot| snapshot.contributions),
            ),
            (
                "PRs opened",
                snapshot.map(|snapshot| snapshot.pull_requests_opened),
            ),
            (
                "PRs merged",
                snapshot.map(|snapshot| snapshot.pull_requests_merged),
            ),
            ("Reviews", snapshot.map(|snapshot| snapshot.reviews)),
        ];
        div()
            .debug_selector(|| "github-activity".into())
            .w_full()
            .child(
                div()
                    .mb(px(4.0))
                    .whitespace_nowrap()
                    .text_size(crate::typography::ui_rems(14.0))
                    .line_height(px(18.0))
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(theme.text)
                    .child(SharedString::from("Last 30 days")),
            )
            .child(div().w_full().flex().flex_nowrap().gap(px(16.0)).children(
                stats.into_iter().enumerate().map(|(index, (label, stat))| {
                    let count = match stat {
                        Some(stat) => div()
                            .whitespace_nowrap()
                            .text_size(crate::typography::ui_rems(22.0))
                            .line_height(px(26.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .opacity(reveal(elapsed, 0))
                            .child(SharedString::from(grouped(stat.count))),
                        None => crate::pull_request_skeleton::bar(
                            px(56.0),
                            18.0,
                            index,
                            view,
                            &theme,
                            cx,
                        ),
                    };
                    stat_column(&theme, label, count, stat.map_or(0, |stat| stat.delta))
                }),
            ))
            .child(
                heatmap(
                    snapshot.map(|snapshot| snapshot.weeks.as_slice()),
                    &theme,
                    cell,
                    elapsed,
                )
                .opacity(pulse),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PANEL_CHROME, activity_cell, format_delta, grouped, heat_cell, month_abbr, reveal,
    };

    #[test]
    fn github_activity_groups_counts() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(8368), "8,368");
        assert_eq!(format_delta(0), "0");
        assert_eq!(format_delta(12), "+12");
        assert_eq!(format_delta(-25), "-25");
        assert_eq!(format_delta(1200), "+1,200");
    }

    #[test]
    fn github_activity_shows_only_when_it_fits() {
        let cell = activity_cell(736.0, 53, 400.0).unwrap();
        assert!(PANEL_CHROME + 7.0 * cell <= 400.0);
        assert!(activity_cell(736.0, 53, 200.0).is_none());
        assert!(activity_cell(480.0, 53, 400.0).is_none());
        assert!(activity_cell(736.0, 0, 400.0).is_none());
    }

    #[test]
    fn github_activity_fills_the_heatmap_from_left_to_right() {
        assert_eq!(reveal(0.0, 0), 0.0);
        assert_eq!(reveal(0.3, 0), 1.0);
        assert_eq!(reveal(0.82, 52), 1.0);
        assert_eq!(reveal(f32::INFINITY, 52), 1.0);
        assert!(reveal(0.15, 0) > reveal(0.15, 10));
        assert_eq!(reveal(0.05, 10), 0.0);
    }

    #[test]
    fn github_activity_heatmap_uses_the_panel_width() {
        let cell = heat_cell(736.0, 53);
        let used = 53.0 * cell + 52.0 * 2.0;
        assert!((used - 736.0).abs() < 0.5, "{used}");
        assert!(cell > 10.0);
    }

    #[test]
    fn github_activity_month_labels_follow_the_date() {
        assert_eq!(month_abbr("2026-01-02"), Some("Jan"));
        assert_eq!(month_abbr("2026-06-01"), Some("Jun"));
        assert_eq!(month_abbr("2026-12-31"), Some("Dec"));
        assert_eq!(month_abbr("2026-13-01"), None);
        assert_eq!(month_abbr("2026"), None);
    }
}
