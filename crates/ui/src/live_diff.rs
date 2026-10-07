//! The row above the composer: the "N files changed +A −D" pill while the
//! selected chat's agent is working (clicking it lists each file with its own
//! counts), and the "Scroll to bottom" chip, which folds into a bare ↓ beside
//! the pill. The row has no layout height: both float just above the composer
//! (like the chip always did), so the composer never moves when the pill
//! arrives — the dock springs the composer's top edge, so any height added
//! above the input would make it dip and settle. The transcript instead gets
//! the pill's height through [`LiveDiff::clearance`].
//!
//! The pill's numbers are the engine's capture since the agent started
//! working (`GetCheckoutDiff` in `run` mode), so edits already in the working
//! tree before the run don't count and steering mid-run doesn't reset them
//! (`turn` mode would: a steer starts a new turn). The capture is refreshed
//! whenever the checkout's `WatchCheckoutDiffs` checksum moves, and nothing is
//! subscribed while the agent is idle.

use std::cell::Cell;
use std::collections::HashSet;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use gpui::{
    Context, Entity, IntoElement, Render, ScrollHandle, SharedString, Subscription, Task, Window,
    div, prelude::*, px,
};
use zeron_proto::{CheckoutDiff, DiffFileSummary};
use zeron_rpc::methods;

use crate::changes::{apply_diff_frame, resolve_diff};
use crate::composer::QUEUE_SIDE_INSET;
use crate::composer_dock::stage;
use crate::motion;
use crate::popover::{self, MenuScrollbarState, ScrollRailHost};
use crate::shell::{ChipFold, Shell};
use crate::state::{AppState, Indicator};
use crate::theme::Theme;
use crate::transcript::Transcript;

/// Element id and hover-animation key of the pill (global per key).
const PILL_KEY: &str = "live-diff-pill";

/// Pause before re-subscribing after the diff stream fails or ends.
const WATCH_RETRY: Duration = Duration::from_secs(2);

const PILL_HEIGHT: f32 = 30.0;
/// How far below its resting place the pill starts when it rises in.
const PILL_RISE: f32 = 12.0;
/// The file list scrolls past this height (about nine rows).
const LIST_MAX_HEIGHT: f32 = 264.0;
/// The folded scroll-to-bottom button's size.
const JUMP_SIZE: f32 = 30.0;
/// Space between the pill and the folded button.
const ROW_GAP: f32 = 8.0;

/// A 0..1 value gliding toward a target over [`motion::RESIZE`], retargeting
/// from wherever it currently is (so a reversal doesn't jump).
#[derive(Clone, Copy)]
struct Glide {
    from: f32,
    to: f32,
    started: Instant,
}

impl Glide {
    fn settled(at: f32) -> Self {
        Self {
            from: at,
            to: at,
            started: Instant::now(),
        }
    }

    fn value(&self, reduced: bool) -> f32 {
        let total = motion::RESIZE.total().mul_f32(motion::speed_scale());
        let raw = self.started.elapsed().as_secs_f32() / total.as_secs_f32();
        if reduced || raw >= 1.0 {
            self.to
        } else {
            motion::lerp(self.from, self.to, motion::RESIZE.progress(raw))
        }
    }

    /// The value now, heading to `target`, and whether it is still moving.
    fn eval(&mut self, target: f32, reduced: bool) -> (f32, bool) {
        if self.to != target {
            *self = Self {
                from: self.value(reduced),
                to: target,
                started: Instant::now(),
            };
        }
        let value = self.value(reduced);
        (value, value != self.to)
    }
}

/// The chat being followed, and the device hosting its checkout when that
/// isn't this one (diffs are produced where the checkout lives).
#[derive(Clone, PartialEq)]
struct Follow {
    chat_id: String,
    target: Option<String>,
}

pub struct LiveDiff {
    state: Entity<AppState>,
    /// `Some` only while the selected chat is working.
    following: Option<Follow>,
    /// The watch stream; only its checksums matter here.
    diffs: Vec<CheckoutDiff>,
    watch_task: Option<Task<()>>,
    /// What the pill draws: the run's capture without its patch (the pill
    /// needs only the counts, and this is cloned each frame). Outlives
    /// `following` so the pill can fold away after the run ends; the next run
    /// replaces it.
    capture: Option<Arc<CheckoutDiff>>,
    /// Watch checksum `capture` was captured at.
    capture_for: Option<String>,
    /// Watch checksum being captured, and its request.
    fetching: Option<String>,
    fetch_task: Option<Task<()>>,
    popup: popover::Popup<()>,
    /// The file list's scroll position, its rail, and the files that were
    /// already listed when it opened (so only later arrivals animate in).
    list_scroll: ScrollHandle,
    list_bar: MenuScrollbarState,
    list_seen: HashSet<String>,
    /// Set by the shell (so only the main conversation's row is live): the
    /// transcript whose scroll-to-bottom chip this row carries.
    transcript: Option<Entity<Transcript>>,
    _transcript: Option<Subscription>,
    /// 0 = no pill, 1 = pill: drives the arrow folding and sliding aside, the
    /// pill rising in and fading, and the transcript's clearance (and the
    /// reverse).
    pill: Glide,
    /// Height the transcript should leave above the composer for the pill,
    /// as of the last render.
    clearance: f32,
    /// 0 = no arrow, 1 = arrow: slides the pill over to make room for it.
    jump_slot: Glide,
    /// The pill's laid-out width, read back from last frame so the arrow
    /// knows where to land.
    pill_width: Rc<Cell<f32>>,
    _state: Subscription,
    _settings: Subscription,
}

impl ScrollRailHost for LiveDiff {
    fn rail_bar(&mut self) -> &mut MenuScrollbarState {
        &mut self.list_bar
    }

    fn rail_scroll(&self) -> Option<ScrollHandle> {
        Some(self.list_scroll.clone())
    }
}

impl LiveDiff {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        Self {
            _state: cx.observe(&state, |this, _, cx| this.sync(cx)),
            _settings: cx
                .observe_global::<crate::settings::SettingsStore>(|this, cx| this.sync(cx)),
            state,
            following: None,
            diffs: Vec::new(),
            watch_task: None,
            capture: None,
            capture_for: None,
            fetching: None,
            fetch_task: None,
            popup: popover::Popup::default(),
            list_scroll: ScrollHandle::new(),
            list_bar: MenuScrollbarState::default(),
            list_seen: HashSet::new(),
            transcript: None,
            _transcript: None,
            pill: Glide::settled(0.0),
            clearance: 0.0,
            jump_slot: Glide::settled(0.0),
            pill_width: Rc::default(),
        }
    }

    /// Room the transcript keeps above the composer for the pill (zero while
    /// there is none). Fed into the shell's bottom-stack measurement, so the
    /// transcript eases up to make room without the composer moving.
    pub(crate) fn clearance(&self) -> f32 {
        self.clearance
    }

    pub(crate) fn set_transcript(
        &mut self,
        transcript: Entity<Transcript>,
        cx: &mut Context<Self>,
    ) {
        self._transcript = Some(cx.observe(&transcript, |_, _, cx| cx.notify()));
        self.transcript = Some(transcript);
        self.sync(cx);
    }

    /// Whether the pill has files to show right now.
    fn visible(&self) -> bool {
        self.following.is_some() && self.capture.as_ref().is_some_and(|c| !c.files.is_empty())
    }

    /// Follow the selected chat while it works, and stop when it doesn't (or
    /// when the user turned the pill off). Runs on every state or settings
    /// change, so it only notifies on a real change. Inert until the shell
    /// hands over a transcript, so the composers of side chats never subscribe
    /// to anything.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let enabled = crate::settings::live_diff_pill(cx);
        let want = {
            let state = self.state.read(cx);
            state
                .selected_chat_row()
                .filter(|_| enabled && self.transcript.is_some())
                // The host's own report, not the send overlay: a request made
                // before the host has started the turn would be answered from
                // the previous turn's base.
                .filter(|chat| {
                    state.reported_indicator_for(&chat.id, Utc::now()) == Indicator::Working
                })
                .map(|chat| Follow {
                    chat_id: chat.id.clone(),
                    target: (state.local_device_id.as_deref() != Some(chat.device_id.as_str()))
                        .then(|| chat.device_id.clone()),
                })
        };
        if want != self.following {
            // A new run, another chat or another host: nothing carries over.
            self.diffs.clear();
            self.watch_task = None;
            self.capture_for = None;
            self.fetching = None;
            self.fetch_task = None;
            self.popup = popover::Popup::default();
            // A run that just ended keeps its capture so the pill can fold
            // away; the next run replaces it.
            if want.is_some() {
                self.capture = None;
            }
            self.following = want;
            cx.notify();
        }
        if self.following.is_some() && self.watch_task.is_none() {
            self.start_watch(cx);
        }
    }

    fn start_watch(&mut self, cx: &mut Context<Self>) {
        let Some(follow) = self.following.clone() else {
            return;
        };
        // Engine still booting: the next state change retries.
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        self.watch_task = Some(cx.spawn(async move |this, cx| {
            loop {
                let mut params = serde_json::Map::new();
                if let Some(target) = &follow.target {
                    params.insert("targetDeviceId".into(), target.clone().into());
                }
                let stream = engine
                    .client()
                    .subscribe(
                        methods::WATCH_CHECKOUT_DIFFS,
                        serde_json::Value::Object(params),
                    )
                    .await;
                if let Ok(mut rx) = stream {
                    while let Some(value) = rx.recv().await {
                        let alive = this.update(cx, |live, cx| {
                            if apply_diff_frame(&mut live.diffs, value) {
                                live.refresh_capture(cx);
                            }
                        });
                        if alive.is_err() {
                            return;
                        }
                    }
                }
                cx.background_executor().timer(WATCH_RETRY).await;
            }
        }));
    }

    /// Re-capture the run's diff when the checkout's checksum has moved. One
    /// capture at a time; the newest checksum is picked up when it lands.
    fn refresh_capture(&mut self, cx: &mut Context<Self>) {
        if self.fetching.is_some() {
            return;
        }
        let Some(follow) = self.following.clone() else {
            return;
        };
        let state = self.state.read(cx);
        let Some(chat) = state.selected_chat_row().filter(|c| c.id == follow.chat_id) else {
            return;
        };
        let Some(diff) = resolve_diff(&self.diffs, chat) else {
            return;
        };
        if self.capture_for.as_deref() == Some(diff.checksum.as_str()) {
            return;
        }
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let (cwd, checksum) = (diff.cwd.clone(), diff.checksum.clone());
        // `run` spans steers. A host that predates it would answer the plain
        // working tree for an unknown mode (counting files that were already
        // dirty), so there we settle for `turn` (since the last prompt).
        let mode = if state.chat_host_supports(
            &follow.chat_id,
            zeron_proto::capabilities::CHECKOUT_RUN_DIFF_V1,
        ) {
            "run"
        } else {
            "turn"
        };
        let mut params = serde_json::json!({
            "cwd": cwd,
            "chatId": follow.chat_id,
            "mode": mode,
        });
        if let Some(target) = follow.target {
            params["targetDeviceId"] = target.into();
        }
        self.fetching = Some(checksum.clone());
        self.fetch_task = Some(cx.spawn(async move |this, cx| {
            let result = engine
                .client()
                .call(methods::GET_CHECKOUT_DIFF, params)
                .await
                .ok()
                .and_then(|value| serde_json::from_value::<CheckoutDiff>(value).ok())
                // Only the counts are drawn, and this is cloned every frame.
                .map(|diff| {
                    Arc::new(CheckoutDiff {
                        patch: String::new(),
                        ..diff
                    })
                });
            this.update(cx, |live, cx| {
                if live.fetching.take().as_deref() != Some(checksum.as_str()) {
                    return; // reset by a new run while in flight
                }
                // A failed capture keeps what is showing rather than blanking it.
                live.capture = result.or(live.capture.take());
                live.capture_for = Some(checksum);
                live.refresh_capture(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// A trigger click: open the file list, or close it when the press found
    /// it open (the card's mouse-down-out already began that close).
    fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.popup.take_press_was_open() || self.popup.as_open().is_some() {
            self.dismiss(cx);
            return;
        }
        // Files already listed when it opens don't animate in; ones that
        // arrive while it is open do.
        self.list_seen = self
            .capture
            .iter()
            .flat_map(|capture| capture.files.iter().map(|file| file.path.clone()))
            .collect();
        popover::reset_menu_scroll(&self.list_scroll, &mut self.list_bar);
        self.popup.open(());
        cx.notify();
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        if self.popup.begin_close() {
            popover::reap_popup(cx, |live: &mut Self| &mut live.popup);
        }
        cx.notify();
    }

    /// `theme` is [`Theme::for_popup`].
    fn file_list(
        &mut self,
        capture: &CheckoutDiff,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let rows: Vec<_> = capture
            .files
            .iter()
            .map(|file| file_row(file, !self.list_seen.contains(&file.path), theme))
            .collect();
        popover::popover_card(theme)
            .w(px(320.0))
            .on_mouse_down_out(cx.listener(|live, _, _, cx| live.dismiss(cx)))
            .child(
                // Many files wheel-scroll inside a bounded list: the edges
                // fade only where more is clipped, and a floating rail tracks
                // the position — the composer's mention list treatment.
                popover::menu_scroll_host("live-diff-scroll-host")
                    .on_hover(cx.listener(|live, hovered: &bool, _, cx| {
                        if live.list_bar.set_list_hovered(*hovered) {
                            cx.notify();
                        }
                    }))
                    .child(popover::faded_menu_list(
                        &self.list_scroll,
                        popover::menu_scroll_list("live-diff-files", &self.list_scroll)
                            .max_h(px(LIST_MAX_HEIGHT))
                            .flex()
                            .flex_col()
                            .gap(px(popover::MENU_GAP))
                            .children(rows),
                    ))
                    .children(popover::rail(self, "live-diff-scrollbar", theme, cx)),
            )
    }

    /// The pill: "N files changed +A −D", with the file list opening above it.
    /// `fade` is how far it has faded in; `theme` is [`Theme::for_popup`].
    fn render_pill(
        &mut self,
        capture: &CheckoutDiff,
        fade: f32,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // The same floating glass chip as "Scroll to bottom": the transcript
        // scrolls under this, so it must blur and tint what it covers.
        let (base, wash) = popover::pill_fills(PILL_KEY, theme);
        let measured = self.pill_width.clone();
        let pill = div()
            .id(PILL_KEY)
            .relative()
            .top(px(PILL_RISE * (1.0 - fade)))
            .opacity(fade)
            .h(px(PILL_HEIGHT))
            .rounded_full()
            .border_1()
            .border_color(theme.border)
            .when(!theme.is_frost(), |el| el.shadow_md())
            .cursor_pointer()
            .bg(base)
            .on_hover(motion::hover_listener(PILL_KEY))
            .role(gpui::Role::Button)
            .aria_label(format!(
                "{}, +{} −{}",
                label(capture.files.len()),
                capture.additions,
                capture.deletions
            ))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|live, _, _, _| live.popup.note_trigger_press_matching(|_| true)),
            )
            .on_click(cx.listener(|live, _, _, cx| live.toggle(cx)))
            .child(
                // Reads back the laid-out width (plus the 1px border the
                // absolute box sits inside) for next frame's arrow target.
                gpui::canvas(
                    move |bounds, window, _| {
                        let width = f32::from(bounds.size.width) + 2.0;
                        if (measured.get() - width).abs() > 0.5 {
                            measured.set(width);
                            window.request_animation_frame();
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .inset_0(),
            )
            .child(
                // The hover wash rides an inner layer so it composites over
                // the tint (a div has one bg).
                div()
                    .h_full()
                    .px(px(13.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .bg(wash)
                    .text_size(crate::typography::ui_rems(13.0))
                    .text_color(theme.text)
                    .child(label(capture.files.len()))
                    .child(counts(capture.additions, capture.deletions, theme)),
            );
        // The list opens above the pill, centered on it.
        div()
            .relative()
            .child(crate::frost::frosted(
                15.0,
                crate::frost::MENU_BLUR * fade,
                pill,
            ))
            .when(self.popup.get().is_some(), |el| {
                el.child(popover::centered_menu_above(
                    "live-diff-menu",
                    self.file_list(capture, theme, cx).into_any_element(),
                    self.popup.closing_since(),
                ))
            })
            .into_any_element()
    }
}

/// One file of the list; `fresh` rows (not there when the list opened) fade
/// and rise in.
fn file_row(file: &DiffFileSummary, fresh: bool, theme: &Theme) -> gpui::AnyElement {
    let (name, dir) = split_path(&file.path);
    let row = popover::menu_row(theme, false, format!("live-diff-row-{}", file.path))
        .cursor_default()
        .child(
            crate::file_icons::icon(
                crate::file_icons::FileIconIdentity::file(&file.path),
                theme.appearance,
            )
            .size(px(14.0)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(px(6.0))
                .child(div().flex_none().max_w_full().truncate().child(name))
                .when(!dir.is_empty(), |el| {
                    el.child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_faint)
                            .child(dir),
                    )
                }),
        )
        .child(counts(file.additions, file.deletions, theme));
    if fresh {
        motion::fade_in(format!("live-diff-row-in-{}", file.path), row).into_any_element()
    } else {
        row.into_any_element()
    }
}

/// `+A −D`, the same glyphs and colors as the Changes pane.
fn counts(additions: u32, deletions: u32, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .flex()
        .gap(px(6.0))
        .font_family(theme.font_mono.clone())
        .text_size(px(11.0))
        .child(
            div()
                .text_color(theme.diff_add)
                .child(format!("+{additions}")),
        )
        .child(
            div()
                .text_color(theme.diff_del)
                .child(format!("−{deletions}")),
        )
}

fn label(files: usize) -> String {
    match files {
        1 => "1 file changed".to_string(),
        n => format!("{n} files changed"),
    }
}

/// `(file name, containing directory)` of a repo-relative path.
fn split_path(path: &str) -> (SharedString, SharedString) {
    match path.rsplit_once('/') {
        Some((dir, name)) => (name.to_string().into(), dir.to_string().into()),
        None => (path.to_string().into(), SharedString::default()),
    }
}

/// Natural width of the chip's label, so folding it away is a smooth narrowing.
fn chip_label_width(window: &Window, theme: &Theme) -> f32 {
    let text = "Scroll to bottom";
    let line = window.text_system().shape_line(
        SharedString::from(text),
        crate::typography::ui_rems(13.0).to_pixels(window.rem_size()),
        &[gpui::TextRun {
            len: text.len(),
            font: gpui::font(theme.font_sans.clone()),
            color: theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );
    f32::from(line.width()).ceil()
}

impl Render for LiveDiff {
    /// The row above the composer. It never has any height: the pill and the
    /// "Scroll to bottom" chip float above it, so the composer stays put. When
    /// the pill arrives one clock `g` moves everything: it rises into place and
    /// fades in, and with the chip present, the chip first folds to a bare ↓
    /// and slides to the pill's right so the two never overlap. The transcript
    /// leaves room via [`Self::clearance`]. The run ending plays it backwards.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let reduced = motion::reduced_motion(cx);
        let theme = Theme::of(cx).for_popup();
        let transcript = self
            .transcript
            .clone()
            .filter(|_| self.state.read(cx).selected_chat.is_some());
        let jump_wanted = transcript
            .as_ref()
            .is_some_and(|transcript| transcript.read(cx).jump_button_shown());

        let (g, g_moving) = self
            .pill
            .eval(if self.visible() { 1.0 } else { 0.0 }, reduced);
        let (j, j_moving) = self
            .jump_slot
            .eval(if jump_wanted { 1.0 } else { 0.0 }, reduced);
        if g_moving || j_moving {
            window.request_animation_frame();
        }
        self.clearance = (PILL_HEIGHT + ROW_GAP) * g;
        let folded = stage(g, 0.0, 0.55);
        let slide = stage(g, 0.25, 1.0);
        // Alone, the pill rises and fades over the whole glide; beside the
        // chip it waits until the chip has cleared its spot.
        let fade = stage(g, if jump_wanted { 0.7 } else { 0.0 }, 1.0);

        // The pill sits left of the arrow, so it shifts over as the arrow
        // appears; the arrow lands one gap right of the pill's far edge.
        let pill_center = -(JUMP_SIZE + ROW_GAP) * 0.5 * j;
        let arrow_center =
            slide * (pill_center + self.pill_width.get() * 0.5 + ROW_GAP + JUMP_SIZE * 0.5);
        let chip = transcript.filter(|_| jump_wanted).map(|transcript| {
            let fold = ChipFold {
                amount: folded,
                // Only needed while the label is narrowing.
                label_width: if folded > 0.0 {
                    chip_label_width(window, &theme)
                } else {
                    0.0
                },
            };
            Shell::jump_pill(
                "live-diff-jump",
                "live-diff-jump-pill",
                transcript,
                fold,
                cx,
            )
        });
        // Drawn while present or still folding away after the run.
        let capture = self.capture.clone().filter(|_| self.visible() || g > 0.0);
        div()
            .relative()
            .mx(px(QUEUE_SIDE_INSET))
            // No height, and the column gap that follows is cancelled: the
            // composer's top edge never moves, whatever floats above it.
            .h_0()
            .mb(px(-Theme::SPACE_SM))
            .children(capture.map(|capture| {
                div()
                    .absolute()
                    .bottom(px(ROW_GAP))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .pr(px((JUMP_SIZE + ROW_GAP) * j))
                    .child(self.render_pill(&capture, fade, &theme, cx))
            }))
            .children(chip.map(|chip| {
                // Alone it floats six pixels above the composer, as it always
                // has; with the pill it settles into the pill's row.
                div()
                    .absolute()
                    .bottom(px(motion::lerp(6.0, ROW_GAP, g)))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .pl(px(2.0 * arrow_center))
                    .child(chip)
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glide_snaps_when_reduced_and_retargets_from_where_it_is() {
        let mut glide = Glide::settled(0.0);
        assert_eq!(glide.eval(1.0, true), (1.0, false));
        assert_eq!(glide.eval(0.0, true), (0.0, false));

        let mut glide = Glide::settled(0.0);
        let (start, moving) = glide.eval(1.0, false);
        assert!(moving && start < 1.0, "starts at the beginning of a glide");
        // Reversing mid-way heads back to 0 from the current value.
        let (back, _) = glide.eval(0.0, false);
        assert!(back <= start + 0.05);
        assert_eq!(glide.to, 0.0);
    }

    #[test]
    fn label_pluralises() {
        assert_eq!(label(1), "1 file changed");
        assert_eq!(label(5), "5 files changed");
    }

    fn capture_with(files: usize) -> CheckoutDiff {
        serde_json::from_value(serde_json::json!({
            "checkoutId": "c", "deviceId": "d", "cwd": "/repo", "patch": "",
            "files": (0..files).map(|i| serde_json::json!({
                "path": format!("f{i}.rs"), "status": "M",
                "additions": 1, "deletions": 0, "binary": false,
            })).collect::<Vec<_>>(),
            "additions": files, "deletions": 0, "truncated": false,
            "checksum": "x", "updatedAt": "2026-01-01T00:00:00Z",
        }))
        .unwrap()
    }

    #[gpui::test]
    fn pill_shows_only_while_following_a_run_with_changes(cx: &mut gpui::TestAppContext) {
        let state = cx.new(|_| AppState::new());
        let live = cx.new(|cx| LiveDiff::new(state, cx));
        live.update(cx, |live, _| {
            // Idle chat: nothing is followed, so a kept capture stays hidden.
            live.capture = Some(Arc::new(capture_with(2)));
            assert!(!live.visible());
            live.following = Some(Follow {
                chat_id: "chat".into(),
                target: None,
            });
            assert!(live.visible());
            // Working but nothing changed yet this run.
            live.capture = Some(Arc::new(capture_with(0)));
            assert!(!live.visible());
            live.capture = None;
            assert!(!live.visible());
        });
    }

    #[gpui::test]
    fn turning_the_setting_off_stops_following_a_working_chat(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::default());
            crate::settings::init(Default::default(), dir.path(), cx);
        });
        let state = cx.new(|_| {
            let now = Utc::now();
            let mut state = AppState::new();
            state.chats = vec![zeron_proto::Chat {
                id: "c".into(),
                device_id: "dev".into(),
                title: None,
                archived: false,
                cwd: None,
                branch: None,
                checkout_id: None,
                source_context: None,
                config: None,
                last_message_preview: None,
                last_message_at: None,
                created_at: now,
                harness_session_id: None,
                harness_session_cwd: None,
                parent_chat_id: None,
                space_id: None,
                last_seen_at: None,
                room_gen: None,
            }];
            state.selected_chat = Some("c".into());
            // The host reports the turn as started (a send in flight alone
            // would not count).
            state.sessions = vec![zeron_proto::Session {
                last_completed_turn: None,
                chat_id: "c".into(),
                device_id: "dev".into(),
                status: zeron_proto::SessionStatus::Working,
                started_at: Some(now),
                updated_at: now,
            }];
            state
        });
        let transcript = cx.new(|cx| Transcript::new(state.clone(), cx));
        let live = cx.new(|cx| LiveDiff::new(state, cx));
        live.update(cx, |live, cx| live.set_transcript(transcript, cx));
        assert!(live.read_with(cx, |live, _| live.following.is_some()));

        cx.update(|cx| {
            crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
                settings.live_diff_pill = false;
            })
        });
        cx.run_until_parked();
        assert!(live.read_with(cx, |live, _| live.following.is_none()));

        cx.update(|cx| {
            crate::settings::update(crate::settings::SavePolicy::Immediate, cx, |settings| {
                settings.live_diff_pill = true;
            })
        });
        cx.run_until_parked();
        assert!(live.read_with(cx, |live, _| live.following.is_some()));
    }

    #[test]
    fn split_path_separates_name_and_directory() {
        let (name, dir) = split_path("crates/ui/src/live_diff.rs");
        assert_eq!(
            (name.as_ref(), dir.as_ref()),
            ("live_diff.rs", "crates/ui/src")
        );
        let (name, dir) = split_path("README.md");
        assert_eq!((name.as_ref(), dir.as_ref()), ("README.md", ""));
    }
}
