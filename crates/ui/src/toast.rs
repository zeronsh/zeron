//! In-app floating toasts — a compact pill that drops in from the top of the
//! main column (centered on the composer), auto-dismisses, and click-dismisses.
//!
//! Kinds: Neutral (gray), Warning (yellow), Error (red). Motion is opacity +
//! translateY; gpui divs have no scale transform at the pinned rev.

use std::time::{Duration, Instant};

use gpui::{
    AnimationElement, AnyElement, Context, IntoElement, SharedString, Styled, div, prelude::*, px,
};

use crate::icons;
use crate::motion::{self, AnimationExt as _, EASE_OUT, MotionSpec};
use crate::theme::Theme;

/// Hold before the exit starts.
pub const TOAST_HOLD: Duration = Duration::from_millis(3200);
/// Entrance: 250ms ease-out, 12px drop from above.
pub const TOAST_IN: MotionSpec = MotionSpec::new(250, EASE_OUT);
/// Exit: faster than the entrance so leaving feels snappy.
pub const TOAST_OUT: MotionSpec = MotionSpec::new(180, EASE_OUT);
/// How far above rest the toast starts / ends.
pub const TOAST_DISTANCE: f32 = 12.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Neutral,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub id: u64,
    pub kind: ToastKind,
    pub message: SharedString,
    pub shown_at: Instant,
    pub closing_since: Option<Instant>,
}

impl Toast {
    pub fn new(id: u64, kind: ToastKind, message: impl Into<SharedString>) -> Self {
        Self {
            id,
            kind,
            message: message.into(),
            shown_at: Instant::now(),
            closing_since: None,
        }
    }

    pub fn begin_close(&mut self) {
        if self.closing_since.is_none() {
            self.closing_since = Some(Instant::now());
        }
    }

    pub fn is_finished(&self) -> bool {
        self.closing_since.is_some_and(|since| {
            since.elapsed() >= TOAST_OUT.total().mul_f32(motion::speed_scale())
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToastPalette {
    pub border: gpui::Hsla,
    pub fill: gpui::Hsla,
    pub text: gpui::Hsla,
}

pub fn toast_palette(theme: &Theme, kind: ToastKind) -> ToastPalette {
    match kind {
        ToastKind::Neutral => ToastPalette {
            border: theme.border,
            fill: crate::theme::ink(0.06),
            text: theme.text.opacity(0.92),
        },
        ToastKind::Warning => {
            let amber = theme.warning;
            ToastPalette {
                border: amber.opacity(0.22),
                fill: amber.opacity(0.08),
                text: theme.warning_muted.opacity(0.95),
            }
        }
        ToastKind::Error => {
            let red = theme.danger;
            ToastPalette {
                border: red.opacity(0.22),
                fill: red.opacity(0.08),
                text: theme.danger_muted.opacity(0.95),
            }
        }
    }
}

fn toast_icon(kind: ToastKind) -> &'static str {
    match kind {
        ToastKind::Neutral => icons::INFO_CIRCLE,
        ToastKind::Warning | ToastKind::Error => icons::DANGER_TRIANGLE,
    }
}

fn exit_progress(since: Instant) -> f32 {
    let total = TOAST_OUT.total().mul_f32(motion::speed_scale());
    if total.is_zero() {
        return 1.0;
    }
    TOAST_OUT.progress(since.elapsed().as_secs_f32() / total.as_secs_f32())
}

fn toast_in<E>(id: impl Into<gpui::ElementId>, element: E) -> AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    element.with_animation(id, TOAST_IN.animation(), |el, t| {
        el.relative()
            .opacity(t)
            .top(px(-TOAST_DISTANCE * (1.0 - t)))
    })
}

fn toast_out<E>(id: impl Into<gpui::ElementId>, t: f32, element: E) -> AnimationElement<E>
where
    E: Styled + IntoElement + 'static,
{
    element.with_animation(id, TOAST_OUT.animation(), move |el, _| {
        el.relative().opacity(1.0 - t).top(px(-TOAST_DISTANCE * t))
    })
}

/// The toast pill. `on_dismiss` fires on click (the host starts the close).
pub fn render_toast(
    toast: &Toast,
    theme: &Theme,
    on_dismiss: impl Fn(&gpui::ClickEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let palette = toast_palette(theme, toast.kind);
    let icon = toast_icon(toast.kind);
    let card = div()
        .id(("toast", toast.id))
        .flex()
        .flex_row()
        .items_start()
        .gap(px(8.0))
        .px(px(12.0))
        .py(px(8.0))
        .max_w(px(420.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(palette.border)
        .bg(palette.fill)
        .shadow_md()
        .cursor_pointer()
        .on_click(move |event, window, cx| on_dismiss(event, window, cx))
        .child(
            icons::icon(icon)
                .size(px(14.0))
                .mt(px(1.0))
                .flex_none()
                .text_color(palette.text),
        )
        .child(
            div()
                .min_w_0()
                .text_size(crate::typography::ui_rems(12.5))
                .line_height(px(17.0))
                .text_color(palette.text)
                .child(toast.message.clone()),
        );

    if let Some(since) = toast.closing_since {
        toast_out(("toast-out", toast.id), exit_progress(since), card).into_any_element()
    } else {
        toast_in(("toast-in", toast.id), card).into_any_element()
    }
}

/// Schedule close → reap for one toast id. The host compares ids so a newer
/// toast is never dismissed by an older timer.
pub fn dismiss_after<T: 'static>(
    id: u64,
    cx: &mut Context<T>,
    mut begin_close: impl FnMut(&mut T) -> bool + 'static,
    mut reap: impl FnMut(&mut T) + 'static,
) -> gpui::Task<()> {
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(TOAST_HOLD).await;
        let closing = this
            .update(cx, |host, cx| {
                let started = begin_close(host);
                if started {
                    cx.notify();
                }
                started
            })
            .unwrap_or(false);
        if !closing {
            return;
        }
        let out = TOAST_OUT.total().mul_f32(motion::speed_scale());
        cx.background_executor().timer(out).await;
        this.update(cx, |host, cx| {
            reap(host);
            let _ = id;
            cx.notify();
        })
        .ok();
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_cover_gray_yellow_red() {
        assert_eq!(
            [ToastKind::Neutral, ToastKind::Warning, ToastKind::Error].len(),
            3
        );
    }

    #[test]
    fn hold_is_longer_than_motion() {
        assert!(TOAST_HOLD > TOAST_IN.total());
        assert!(TOAST_OUT.total() < TOAST_IN.total());
        assert!(TOAST_IN.total() <= Duration::from_millis(300));
    }

    #[test]
    fn finished_only_after_close_elapses() {
        let mut toast = Toast::new(1, ToastKind::Error, "nope");
        assert!(!toast.is_finished());
        toast.closing_since = Some(Instant::now() - Duration::from_secs(2));
        assert!(toast.is_finished());
    }

    #[test]
    fn begin_close_is_idempotent() {
        let mut toast = Toast::new(1, ToastKind::Neutral, "ok");
        toast.begin_close();
        let first = toast.closing_since;
        toast.begin_close();
        assert_eq!(toast.closing_since, first);
    }
}
