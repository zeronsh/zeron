//! Rolling label transitions — a port of Scritto's `<scritto-text>` roll
//! (scrit.to/playground) for chip labels.
//!
//! When a keyed label changes, the characters it shares with the old value at
//! either end stay put (the trailing run slides to its new place), while the
//! changed middle rolls: old glyphs rise out and shrink away as new ones rise
//! in from below — each softened by Scritto's 0.1em blur and 2° tilt while
//! in motion — staggered left to right, and the label's width glides between
//! the two. The glyphs ease out without Scritto's spring overshoot so the
//! label lands dead still (user request).
//!
//! Steady state renders the ordinary truncating label, so layout, ellipsis and
//! hit-testing are unchanged whenever nothing is moving; the custom element
//! only exists for the ~0.7s a roll takes. Glyphs are painted from the shaped
//! lines themselves, so kerning matches the plain label and the hand-off at
//! the end is invisible.

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, ContentMask, Element, ElementId, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, ParentElement as _, Pixels, Radians, ScaledPixels, ShapedLine,
    SharedString, Style, Styled as _, TextRun, TransformationMatrix, Window, div, point, px,
};

use crate::motion::{CubicBezier, EASE_OUT_QUINT, RESIZE, lerp, speed_scale};

/// Scritto's default roll duration.
const DURATION: Duration = Duration::from_millis(550);
/// Per-glyph travel (em), resting scale, tilt (degrees), blur (em) and the
/// stagger spread (fraction of the duration) — Scritto's
/// `{y: .35, scale: .6, rotate: 2, blur: .1, stagger: .3}`.
const TRAVEL_EM: f32 = 0.35;
const REST_SCALE: f32 = 0.6;
const TILT_DEG: f32 = 2.0;
const BLUR_EM: f32 = 0.1;
const STAGGER: f32 = 0.3;
/// The glyph roll: Scritto's `cubic-bezier(.22,1,.36,1)`, a long settle with
/// no overshoot.
const GLIDE: CubicBezier = EASE_OUT_QUINT;

/// Width and the shared trailing run settle on the shell's 200ms resize — the
/// same clock the model chip's own width glide runs on — so whatever follows
/// the label (the chip's effort text) reaches its final place together with
/// the chip, early, and never moves again while the glyphs finish rolling.
fn layout_progress(elapsed: f32) -> f32 {
    RESIZE.progress(elapsed / RESIZE.total().as_secs_f32())
}


#[derive(Clone)]
struct Roll {
    from: SharedString,
    to: SharedString,
    started: Instant,
}

/// Bound on remembered labels (see [`rolling`]).
const MAX_KEYS: usize = 256;

thread_local! {
    /// Last label per key, plus the roll in flight (if any).
    static ROLLS: RefCell<HashMap<SharedString, (SharedString, Option<Roll>)>> =
        RefCell::new(HashMap::new());
}

/// The whole roll in unscaled seconds: the stagger pushes the last glyph's
/// start out by up to STAGGER of the duration.
fn span() -> f32 {
    DURATION.as_secs_f32() * (1.0 + STAGGER)
}

fn total() -> Duration {
    Duration::from_secs_f32(span() * speed_scale())
}

/// The label for `key`: plain at rest, rolling for a moment after `text`
/// changes. The first sight of a key never rolls.
pub fn roll_text(key: impl Into<SharedString>, text: SharedString, reduced: bool) -> gpui::AnyElement {
    rolling(key, text.clone(), reduced)
        .unwrap_or_else(|| div().min_w_0().truncate().child(text).into_any_element())
}

/// The rolling element while `key`'s label is mid-roll, else `None` — for
/// callers with their own at-rest presentation. Records `text` either way.
pub fn rolling(
    key: impl Into<SharedString>,
    text: SharedString,
    reduced: bool,
) -> Option<gpui::AnyElement> {
    let key = key.into();
    let now = Instant::now();
    let roll = ROLLS.with(|rolls| {
        let mut rolls = rolls.borrow_mut();
        // Keys can be per-turn (the working trailer), so bound the map: past
        // the cap, forget settled labels — they show plain next time.
        if rolls.len() >= MAX_KEYS && !rolls.contains_key(&key) {
            rolls.retain(|_, (_, roll)| roll.is_some());
        }
        let entry = rolls.entry(key).or_insert_with(|| (text.clone(), None));
        if entry.0 != text {
            let from = std::mem::replace(&mut entry.0, text.clone());
            entry.1 = (!reduced).then(|| Roll {
                from,
                to: text.clone(),
                started: now,
            });
        }
        // Finished — or reduced motion switched on mid-roll, where no frames
        // would come to finish it: settle on the plain label now.
        if reduced
            || entry
                .1
                .as_ref()
                .is_some_and(|roll| now.saturating_duration_since(roll.started) >= total())
        {
            entry.1 = None;
        }
        entry.1.clone()
    });
    roll.map(|roll| RollText { roll }.into_any_element())
}

/// Byte lengths of the shared leading and trailing character runs, never
/// overlapping in either string.
fn shared_ends(from: &str, to: &str) -> (usize, usize) {
    let prefix: usize = from
        .chars()
        .zip(to.chars())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .sum();
    let room = (from.len() - prefix).min(to.len() - prefix);
    let suffix: usize = from[prefix..]
        .chars()
        .rev()
        .zip(to[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(c, _)| c.len_utf8())
        .scan(0, |sum, len| {
            *sum += len;
            Some(*sum)
        })
        .take_while(|sum| *sum <= room)
        .last()
        .unwrap_or(0);
    (prefix, suffix)
}

struct RollText {
    roll: Roll,
}

/// One shaped glyph: its x and advance in the line, and its byte index.
struct Glyph {
    font: gpui::FontId,
    id: gpui::GlyphId,
    index: usize,
    x: f32,
    /// The shaper's vertical offset, which gpui adds below the baseline.
    y: f32,
    advance: f32,
    emoji: bool,
}

struct Shaped {
    from: ShapedLine,
    to: ShapedLine,
    font_size: Pixels,
    line_height: Pixels,
}

impl IntoElement for RollText {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl RollText {
    fn elapsed(&self) -> f32 {
        Instant::now()
            .saturating_duration_since(self.roll.started)
            .as_secs_f32()
            / speed_scale()
    }
}

impl Element for RollText {
    type RequestLayoutState = Shaped;
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Shaped) {
        let text_style = window.text_style();
        let font_size = text_style.font_size.to_pixels(window.rem_size());
        // Exactly gpui's text element: line height snapped to the device
        // pixel grid (not rounded to a logical pixel), so the line box and
        // baseline match the plain label it hands off to.
        let line_height = window.pixel_snap(
            text_style
                .line_height
                .to_pixels(font_size.into(), window.rem_size()),
        );
        let shape = |text: &SharedString| {
            let run = TextRun {
                len: text.len(),
                font: text_style.font(),
                color: text_style.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            window
                .text_system()
                .shape_line(text.clone(), font_size, &[run], None)
        };
        let from = shape(&self.roll.from);
        let to = shape(&self.roll.to);
        let progress = layout_progress(self.elapsed());
        // Rounded up like gpui's text element sizes the plain label, so the
        // settled width equals the label that replaces this element.
        let width = lerp(f32::from(from.width), f32::from(to.width), progress).ceil();
        let mut style = Style::default();
        style.size.width = gpui::Length::Definite(px(width).into());
        style.size.height = gpui::Length::Definite(line_height.into());
        style.min_size.width = px(0.0).into();
        style.flex_shrink = 1.0;
        let layout_id = window.request_layout(style, [], cx);
        (
            layout_id,
            Shaped {
                from,
                to,
                font_size,
                line_height,
            },
        )
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Shaped,
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        shaped: &mut Shaped,
        _prepaint: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let elapsed = self.elapsed();
        let duration = DURATION.as_secs_f32();
        let color = window.text_style().color;
        let font_size = shaped.font_size;
        let travel = f32::from(font_size) * TRAVEL_EM;
        let (prefix, suffix) = shared_ends(&self.roll.from, &self.roll.to);
        let from_len = self.roll.from.len();
        let to_len = self.roll.to.len();
        // The trailing run slides from its old x to its new one with the width.
        let slide = layout_progress(elapsed);
        let suffix_dx = f32::from(
            shaped.from.x_for_index(from_len - suffix) - shaped.to.x_for_index(to_len - suffix),
        ) * (1.0 - slide);

        // Glyphs of one line with their x extents and role.
        let glyphs = |line: &ShapedLine| {
            let mut out = Vec::new();
            for run in line.runs.iter() {
                for glyph in &run.glyphs {
                    out.push((
                        run.font_id,
                        glyph.id,
                        glyph.index,
                        f32::from(glyph.position.x),
                        f32::from(glyph.position.y),
                        glyph.is_emoji,
                    ));
                }
            }
            let width = f32::from(line.width);
            let ends: Vec<f32> = out
                .iter()
                .skip(1)
                .map(|g| g.3)
                .chain(std::iter::once(width))
                .collect();
            out.into_iter()
                .zip(ends)
                .map(|((font, id, index, x, y, emoji), end)| Glyph {
                    font,
                    id,
                    index,
                    x,
                    y,
                    advance: (end - x).max(0.0),
                    emoji,
                })
                .collect::<Vec<_>>()
        };
        let from_glyphs = glyphs(&shaped.from);
        let to_glyphs = glyphs(&shaped.to);
        let changed = |index: usize, len: usize| index >= prefix && index < len - suffix;
        // Stagger sweeps left to right across each side's changed glyphs.
        let sweep = |list: &[Glyph], len: usize| {
            let xs: Vec<f32> = list
                .iter()
                .filter(|g| changed(g.index, len))
                .map(|g| g.x)
                .collect();
            let count = xs.len();
            let min = xs.iter().copied().fold(f32::INFINITY, f32::min);
            let max = xs.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let spread = duration * STAGGER * count.saturating_sub(1) as f32 / count.max(1) as f32;
            move |x: f32| {
                if max > min {
                    spread * ((x - min) / (max - min)).clamp(0.0, 1.0)
                } else {
                    0.0
                }
            }
        };
        let from_delay = sweep(&from_glyphs, from_len);
        let to_delay = sweep(&to_glyphs, to_len);

        let padding_top = (shaped.line_height - shaped.to.ascent - shaped.to.descent) / 2.0;
        let baseline = bounds.origin.y + padding_top + shaped.to.ascent;
        let center_y = f32::from(bounds.origin.y + shaped.line_height / 2.0);
        let scale_factor = window.scale_factor();
        // Vertical room for glyphs rolling past the line box, and a small
        // horizontal allowance for the blur halo; the width otherwise clips.
        let room = px(travel * 2.0);
        let halo = px((f32::from(font_size) * BLUR_EM * 3.0).ceil());
        let mask = Bounds {
            origin: point(bounds.origin.x - halo, bounds.origin.y - room),
            size: gpui::size(bounds.size.width + halo * 2.0, bounds.size.height + room * 2.0),
        };
        // `motion` = 1 − progress: Scritto's resting-state distance. Tilt and
        // blur fade with it; scale and translate are the caller's.
        let paint = |window: &mut Window,
                     glyph: &Glyph,
                     dx: f32,
                     dy: f32,
                     scale: f32,
                     motion: f32,
                     alpha: f32| {
            if alpha <= 0.0 {
                return;
            }
            let left = f32::from(bounds.origin.x) + glyph.x;
            let center_x = left + glyph.advance / 2.0;
            let rest_origin = point(px(left), px(f32::from(baseline) + glyph.y));
            if glyph.emoji {
                // Emoji can be neither tinted nor transformed; they swap at
                // the midpoint and ride the translation alone.
                if alpha < 0.5 {
                    return;
                }
                let origin = point(px(left + dx), px(f32::from(baseline) + glyph.y + dy));
                let _ = window.paint_emoji(origin, glyph.font, glyph.id, font_size);
                return;
            }
            if motion <= 0.0 && dx == 0.0 && dy == 0.0 {
                // At rest (the shared runs): the ordinary pipeline, so these
                // glyphs antialias exactly like the label they hand off to.
                let _ = window.paint_glyph(
                    rest_origin,
                    glyph.font,
                    glyph.id,
                    font_size,
                    color.opacity(alpha.min(1.0)),
                );
                return;
            }
            if scale >= 1.0 && motion <= 0.0 {
                // The sliding tail: a pure translation, same pipeline.
                let origin = point(px(left + dx), px(f32::from(baseline) + glyph.y + dy));
                let _ = window.paint_glyph(
                    origin,
                    glyph.font,
                    glyph.id,
                    font_size,
                    color.opacity(alpha.min(1.0)),
                );
                return;
            }
            // Mid-roll: one full-size raster, scaled and tilted about the
            // glyph's center at composite time, softened by Scritto's blur.
            let dev = |value: f32| ScaledPixels(value * scale_factor);
            let transformation = TransformationMatrix::unit()
                .translate(point(dev(center_x + dx), dev(center_y + dy)))
                .rotate(Radians((TILT_DEG * motion).to_radians()))
                .scale(gpui::size(scale, scale))
                .translate(point(dev(-center_x), dev(-center_y)));
            let _ = window.paint_glyph_transformed(
                rest_origin,
                glyph.font,
                glyph.id,
                font_size,
                color.opacity(alpha.min(1.0)),
                transformation,
                px(f32::from(font_size) * BLUR_EM * motion),
            );
        };
        window.with_content_mask(Some(ContentMask { bounds: mask }), |window| {
            for glyph in &from_glyphs {
                if !changed(glyph.index, from_len) {
                    continue; // the new line paints the shared runs
                }
                let progress = GLIDE.eval(((elapsed - from_delay(glyph.x)) / duration).clamp(0.0, 1.0));
                paint(
                    window,
                    glyph,
                    0.0,
                    -travel * progress,
                    lerp(1.0, REST_SCALE, progress),
                    progress,
                    1.0 - progress,
                );
            }
            for glyph in &to_glyphs {
                if glyph.index < prefix {
                    paint(window, glyph, 0.0, 0.0, 1.0, 0.0, 1.0);
                } else if glyph.index >= to_len - suffix {
                    paint(window, glyph, suffix_dx, 0.0, 1.0, 0.0, 1.0);
                } else {
                    let progress =
                        GLIDE.eval(((elapsed - to_delay(glyph.x)) / duration).clamp(0.0, 1.0));
                    paint(
                        window,
                        glyph,
                        0.0,
                        travel * (1.0 - progress),
                        lerp(REST_SCALE, 1.0, progress),
                        1.0 - progress,
                        progress,
                    );
                }
            }
        });
        // Keep rolling on the shared ~30Hz pulse clock rather than at the
        // display's refresh rate: the working timer rolls every second, and
        // full-rate frames for most of every agent turn were exactly the
        // battery drain the pulse clock exists to prevent. The lease lapses
        // on its own once the roll stops painting.
        if Instant::now().saturating_duration_since(self.roll.started) < total() {
            crate::motion::pulse_lease(window.current_view(), cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_ends_keep_common_runs() {
        assert_eq!(shared_ends("Opus 5.5", "Opus 5.1"), (7, 0));
        assert_eq!(shared_ends("feature/a", "fix/a"), (1, 2));
        assert_eq!(shared_ends("main", "main-2"), (4, 0));
        // Never overlapping: "aa" → "aaa" shares the two leading chars only.
        assert_eq!(shared_ends("aa", "aaa"), (2, 0));
        assert_eq!(shared_ends("ab", "b"), (0, 1));
    }

    #[test]
    fn glyphs_settle_without_overshoot() {
        assert_eq!(GLIDE.eval(0.0), 0.0);
        assert_eq!(GLIDE.eval(1.0), 1.0);
        let mut last = 0.0;
        for i in 0..=200 {
            let value = GLIDE.eval(i as f32 / 200.0);
            assert!(value <= 1.0, "overshoot at {i}: {value}");
            assert!(value >= last, "not monotonic at {i}");
            last = value;
        }
    }
}
