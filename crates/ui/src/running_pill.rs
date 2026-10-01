//! How many subagents are running, drawn the same way wherever it turns up:
//! the "● 3" pill on the sidebar's chat rows (beside the row's own activity
//! indicator) and the Subagents header, and the pulsing button with a count
//! badge on the titlebar's explorer toggle.

use gpui::{
    AnyElement, App, AppContext as _, Bounds, Context, Entity, Hsla, IntoElement, Render,
    RenderOnce, SharedString, Styled as _, TextAlign, TextRun, Window, canvas, div, fill, point,
    prelude::*, px, size,
};

use crate::loaders;
use crate::motion::{self, ZERON_PULSE};
use crate::theme::Theme;

/// Pill height. The sidebar's status slot is 13px and its rows 29px; 16px
/// matches the sidebar's pull-request badge so the two never disagree.
const HEIGHT: f32 = 16.0;
const DOT: f32 = 5.0;
/// Pill padding either side of the content, and the gap between dot and digits.
const PAD: f32 = 6.0;
const GAP: f32 = 4.0;
/// A capped label ends in "+", whose thin arms leave the eye a wider gap
/// than the same padding after a digit; the padding after it is trimmed by
/// this much so the two ends look balanced.
const PLUS_TRIM: f32 = 1.0;
/// Where the middle of Geist Mono's digits sits above the baseline, as a
/// fraction of the font size: (726 + -16) / 2 units of 1000 for the round
/// digits, 710 / 2 for the flat ones.
const DIGIT_MID: f32 = 0.355;
/// Height of the count badge on the titlebar button.
const BADGE: f32 = 13.0;
/// Corner radius of the titlebar's icon buttons.
const BUTTON_RADIUS: f32 = 6.0;
/// Past this the count would outgrow the pill; it reads "99+".
const COUNT_CAP: u32 = 99;

/// The count as drawn: exact up to [`COUNT_CAP`], then "99+".
pub fn count_label(count: u32) -> SharedString {
    if count > COUNT_CAP {
        format!("{COUNT_CAP}+").into()
    } else {
        count.to_string().into()
    }
}

fn is_capped(count: u32) -> bool {
    count > COUNT_CAP
}

/// The pill for `count` running subagents; callers draw nothing at zero.
/// `key` scopes the pulse's animation state — one per placement.
pub fn running_pill(key: impl Into<SharedString>, count: u32, theme: &Theme) -> AnyElement {
    RunningPill {
        key: key.into(),
        count,
        tone: theme.busy,
        family: theme.font_mono.clone(),
    }
    .into_any_element()
}

#[derive(IntoElement)]
struct RunningPill {
    key: SharedString,
    count: u32,
    tone: Hsla,
    family: SharedString,
}

impl RenderOnce for RunningPill {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let spec = PillSpec {
            count: self.count,
            tone: self.tone,
            family: self.family,
        };
        // The pill's width follows its label, and a cached view needs that up
        // front; shaping is cached by the text system.
        let font_size = pill_font_size(window);
        let width = spec.shape(window, font_size).width;
        let total = px(PAD + DOT + GAP + spec.right_pad()) + width;
        // Own the pulse in a cached view so the surrounding list stays still.
        let view = window.with_global_id(self.key.into(), |id, window| {
            window.with_element_state(id, |previous: Option<Entity<PillView>>, _| {
                let view = previous.unwrap_or_else(|| cx.new(|_| PillView { spec: spec.clone() }));
                view.update(cx, |view, cx| {
                    if view.spec != spec {
                        view.spec = spec.clone();
                        cx.notify();
                    }
                });
                (view.clone(), view)
            })
        });
        view.cached(
            gpui::StyleRefinement::default()
                .w(total)
                .h(px(HEIGHT))
                .flex_none(),
        )
    }
}

fn pill_font_size(window: &Window) -> gpui::Pixels {
    crate::typography::ui_rems(10.0).to_pixels(window.rem_size())
}

#[derive(Clone, PartialEq)]
struct PillSpec {
    count: u32,
    tone: Hsla,
    family: SharedString,
}

impl PillSpec {
    fn right_pad(&self) -> f32 {
        if is_capped(self.count) {
            PAD - PLUS_TRIM
        } else {
            PAD
        }
    }

    fn shape(&self, window: &Window, font_size: gpui::Pixels) -> gpui::ShapedLine {
        shape_count(
            window,
            self.count,
            &self.family,
            gpui::FontWeight::MEDIUM,
            self.tone,
            font_size,
        )
    }
}

fn shape_count(
    window: &Window,
    count: u32,
    family: &SharedString,
    weight: gpui::FontWeight,
    color: Hsla,
    font_size: gpui::Pixels,
) -> gpui::ShapedLine {
    let label = count_label(count);
    let run = TextRun {
        len: label.len(),
        font: gpui::Font {
            family: family.clone(),
            features: gpui::FontFeatures::default(),
            fallbacks: None,
            weight,
            style: gpui::FontStyle::Normal,
        },
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_line(label, font_size, &[run], None)
}

struct PillView {
    spec: PillSpec,
}

impl Render for PillView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let spec = self.spec.clone();
        let font_size = pill_font_size(window);
        let line = spec.shape(window, font_size);
        let delta = motion::pulse_delta_slow(&ZERON_PULSE, cx.entity_id(), cx);
        let wave = motion::lerp(0.4, 1.0, (motion::pulse_opacity(delta) - 0.08) / 0.92);
        canvas(
            |_, _, _| (),
            move |bounds: Bounds<gpui::Pixels>, _, window, cx| {
                paint_pill(bounds, &spec, &line, font_size, wave, window, cx);
            },
        )
        .size_full()
    }
}

/// Paint the pill: background, dot, digits. Everything hangs off the digits'
/// own baseline so the dot and the digits can never drift apart. Text
/// metrics and glyph rounding differ per font size and display scale, so the
/// baseline is snapped to a device pixel here (as gpui would on painting),
/// and the dot is centred on the digits' actual middle from that baseline.
/// Where to put a line of digits so their middle sits on the middle of
/// `bounds`: the digits' baseline, snapped to a device pixel (as gpui would on
/// painting), and the top of the line box that lands gpui's own centring of
/// ascent-plus-descent on that baseline. Returns `(line_top, baseline)`.
fn digits_baseline(
    bounds: Bounds<gpui::Pixels>,
    font_size: gpui::Pixels,
    line: &gpui::ShapedLine,
    scale: f32,
) -> (f32, f32) {
    let height = f32::from(bounds.size.height);
    let digit_mid = f32::from(font_size) * DIGIT_MID;
    let baseline =
        ((f32::from(bounds.origin.y) + height / 2.0 + digit_mid) * scale).round() / scale;
    let baseline_in_box =
        (height - f32::from(line.ascent) - f32::from(line.descent)) / 2.0 + f32::from(line.ascent);
    (baseline - baseline_in_box, baseline)
}

fn paint_pill(
    bounds: Bounds<gpui::Pixels>,
    spec: &PillSpec,
    line: &gpui::ShapedLine,
    font_size: gpui::Pixels,
    wave: f32,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = window.scale_factor();
    let snap = |value: f32| (value * scale).round() / scale;
    let height = bounds.size.height;
    window.paint_quad(fill(bounds, spec.tone.opacity(0.16)).corner_radii(height / 2.0));

    // Baseline: the digits' middle on the pill's middle, then onto a device
    // pixel. `line_top` is where gpui expects the line box so its own
    // centring of ascent+descent lands the baseline there.
    let digit_mid = f32::from(font_size) * DIGIT_MID;
    let baseline = snap(f32::from(bounds.origin.y) + f32::from(height) / 2.0 + digit_mid);
    let baseline_in_box = (f32::from(height) - f32::from(line.ascent) - f32::from(line.descent))
        / 2.0
        + f32::from(line.ascent);
    let text_origin = point(
        px(snap(f32::from(bounds.origin.x) + PAD + DOT + GAP)),
        px(baseline - baseline_in_box),
    );

    // Dot: whole device pixels, centred on the digits.
    let diameter = snap(DOT).max(1.0 / scale);
    let centre_y = baseline - digit_mid;
    let dot = Bounds::new(
        point(
            px(snap(f32::from(bounds.origin.x) + PAD)),
            px(snap(centre_y - diameter / 2.0)),
        ),
        size(px(diameter), px(diameter)),
    );
    window.paint_quad(fill(dot, spec.tone.opacity(wave)).corner_radii(px(diameter / 2.0)));

    let _ = line.paint(text_origin, height, TextAlign::Left, None, window, cx);
}

/// Marks the titlebar's explorer button while subagents run, so they can be
/// found with the panel closed: the button's face breathes in the activity
/// tone, and a small count badge sits on its corner. Both are drawn over the
/// button, so it keeps its size and position among the titlebar controls.
pub fn mark_files_button(
    button: gpui::Stateful<gpui::Div>,
    count: u32,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let tone = theme.busy;
    button
        .relative()
        .child(div().absolute().inset_0().child(loaders::pulse_glow(
            "files-button-glow",
            BUTTON_RADIUS,
            tone,
            0.10,
            0.30,
        )))
        .child(
            div()
                .absolute()
                .top(px(-3.0))
                .right(px(-3.0))
                .child(CountBadge {
                    count,
                    tone,
                    family: theme.font_mono.clone(),
                }),
        )
}

/// The count badge on the titlebar button, painted rather than laid out so
/// the digits can be centred exactly: a laid-out text box lands on a rounded
/// device pixel, which read as the digit sitting left of centre.
#[derive(IntoElement)]
struct CountBadge {
    count: u32,
    tone: Hsla,
    family: SharedString,
}

/// Padding either side of the badge's digits.
const BADGE_PAD: f32 = 3.0;

impl RenderOnce for CountBadge {
    fn render(self, window: &mut Window, _cx: &mut App) -> impl IntoElement {
        let font_size = crate::typography::ui_rems(9.0).to_pixels(window.rem_size());
        let line = shape_count(
            window,
            self.count,
            &self.family,
            gpui::FontWeight::SEMIBOLD,
            gpui::white(),
            font_size,
        );
        let capped = is_capped(self.count);
        let text_width = f32::from(line.width);
        // Capped labels trim the padding after the "+" (see [`PLUS_TRIM`]).
        let right_pad = if capped {
            BADGE_PAD - PLUS_TRIM
        } else {
            BADGE_PAD
        };
        let width = (BADGE_PAD + text_width + right_pad).max(BADGE);
        let tone = self.tone;
        canvas(
            |_, _, _| (),
            move |bounds: Bounds<gpui::Pixels>, _, window, cx| {
                let scale = window.scale_factor();
                let snap = |value: f32| (value * scale).round() / scale;
                window.paint_quad(fill(bounds, tone).corner_radii(bounds.size.height / 2.0));
                let (line_top, _) = digits_baseline(bounds, font_size, &line, scale);
                // Single digits sit centred in the badge; a capped label sits
                // at the padding, its trimmed side toward the "+".
                let left = f32::from(bounds.origin.x);
                let x = if capped {
                    left + BADGE_PAD
                } else {
                    left + (f32::from(bounds.size.width) - text_width) / 2.0
                };
                let origin = point(px(snap(x)), px(line_top));
                let _ = line.paint(
                    origin,
                    bounds.size.height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            },
        )
        .w(px(width))
        .h(px(BADGE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_label_caps_at_ninety_nine() {
        assert_eq!(count_label(1).as_ref(), "1");
        assert_eq!(count_label(99).as_ref(), "99");
        assert_eq!(count_label(100).as_ref(), "99+");
    }
}
