//! Context occupancy is read from the replicated chat snapshot, never local CLI state.
use crate::theme::Theme;
use gpui::{IntoElement, SharedString, canvas, div, prelude::*, px};
use zeron_proto::ContextUsage;

/// The context ring's trigger chip; the footer ([`crate::account_usage`])
/// opens [`card`] from it on click.
pub fn chip(usage: Option<ContextUsage>, open: bool, theme: &Theme) -> gpui::Stateful<gpui::Div> {
    let fraction = usage.and_then(ContextUsage::fraction);
    let color = match fraction {
        Some(f) if f >= 0.9 => theme.danger,
        Some(f) if f >= 0.75 => theme.warning,
        Some(_) => theme.text_muted,
        None => theme.text_faint,
    };
    let label = fraction
        .map(|f| format!("{:.0}%", f * 100.0))
        .unwrap_or_else(|| "—".into());
    ring_chip(
        "context-usage",
        fraction.unwrap_or(0.0) as f32,
        color,
        color,
        label,
        open,
        theme,
    )
}

/// One footer ring indicator: ring + percent, identical geometry for every
/// ring so they sit side by side as equals. `arc` colours the ring's fill,
/// `text` the label; `open` holds the hover wash while its popover is up.
pub(crate) fn ring_chip(
    id: &'static str,
    fraction: f32,
    arc: gpui::Hsla,
    text: gpui::Hsla,
    label: String,
    open: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .gap(px(5.0))
        .h(px(24.0))
        .px(px(6.0))
        .rounded(px(6.0))
        .text_size(px(11.0))
        .text_color(text)
        .cursor_pointer()
        .when(open, |s| s.bg(crate::theme::ink(0.05)))
        .hover(|s| s.bg(crate::theme::ink(0.05)))
        .child(ring(fraction, arc, theme))
        .child(SharedString::from(label))
}

/// The footer's 16px progress ring: a faint full track under a `fraction` arc
/// starting at twelve o'clock. Shared with the account usage indicator.
///
/// Painted as a cached image, never a path. GPUI rasterizes each batch of
/// paths in its own drawable-sized, 4x-multisampled pass and reloads the
/// frame around it, so these 16px rings — on screen under every chat, in a
/// window that redraws on every scroll frame — cost ~2ms of GPU per frame on
/// a Retina window (measured: 5 render passes per frame instead of 1).
pub(crate) fn ring(fraction: f32, color: gpui::Hsla, theme: &Theme) -> impl IntoElement {
    let track = theme.text_faint.opacity(0.25);
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let image = ring_image(fraction, color, track, window);
            let _ = window.paint_image(bounds, gpui::Corners::default(), image, 0, false);
        },
    )
    .size(px(RING_SIZE))
}

const RING_SIZE: f32 = 16.0;
const RING_RADIUS: f32 = 6.0;
const RING_STROKE: f32 = 1.8;
/// Distinct rings kept before the cache is released (they're tiny; the bound
/// only guards against unbounded theme/scale churn).
const RING_CACHE_LIMIT: usize = 256;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct RingKey {
    /// Arc length in tenths of a percent: far below what 16px can show.
    permille: u16,
    arc: u32,
    track: u32,
    side_px: u16,
}

fn rgba_key(color: gpui::Hsla) -> u32 {
    let c = gpui::Rgba::from(color);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    byte(c.r) << 24 | byte(c.g) << 16 | byte(c.b) << 8 | byte(c.a)
}

fn ring_image(
    fraction: f32,
    arc: gpui::Hsla,
    track: gpui::Hsla,
    window: &mut gpui::Window,
) -> std::sync::Arc<gpui::RenderImage> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static RINGS: RefCell<HashMap<RingKey, std::sync::Arc<gpui::RenderImage>>> =
            RefCell::default();
    }
    let side_px = (RING_SIZE * window.scale_factor()).round().max(1.0) as u16;
    let key = RingKey {
        permille: (fraction.clamp(0.0, 1.0) * 1000.0).round() as u16,
        arc: rgba_key(arc),
        track: rgba_key(track),
        side_px,
    };
    RINGS.with(|rings| {
        let mut rings = rings.borrow_mut();
        if let Some(image) = rings.get(&key) {
            return image.clone();
        }
        if rings.len() >= RING_CACHE_LIMIT {
            for (_, image) in rings.drain() {
                let _ = window.drop_image(image);
            }
        }
        let pixels = rasterize_ring(key.permille as f32 / 1000.0, arc, track, side_px as u32);
        let image = std::sync::Arc::new(gpui::RenderImage::new([image::Frame::new(pixels)]));
        rings.insert(key, image.clone());
        image
    })
}

/// Anti-aliased ring at `side` device pixels: the full `track` circle under
/// a butt-capped `fraction` arc running clockwise from twelve o'clock. BGRA,
/// straight alpha (what GPUI's sprite atlas samples).
fn rasterize_ring(
    fraction: f32,
    arc: gpui::Hsla,
    track: gpui::Hsla,
    side: u32,
) -> image::RgbaImage {
    let scale = side as f32 / RING_SIZE;
    let center = side as f32 / 2.0;
    let radius = RING_RADIUS * scale;
    let half_width = RING_STROKE * scale / 2.0;
    let sweep = std::f32::consts::TAU * fraction;
    let (arc, track) = (gpui::Rgba::from(arc), gpui::Rgba::from(track));
    let mut pixels = image::RgbaImage::new(side, side);
    for y in 0..side {
        for x in 0..side {
            let (dx, dy) = (x as f32 + 0.5 - center, y as f32 + 0.5 - center);
            let distance = dx.hypot(dy);
            let band = (half_width + 0.5 - (distance - radius).abs()).clamp(0.0, 1.0);
            if band <= 0.0 {
                continue;
            }
            // Clockwise angle from twelve o'clock, 0..TAU.
            let angle = dx.atan2(-dy).rem_euclid(std::f32::consts::TAU);
            // Signed distance (px) inside the nearer butt cap, as 1px AA. The
            // wrapped angle covers the outside edge of the twelve o'clock cap.
            let cover = |a: f32| (0.5 + (a * distance).min((sweep - a) * distance)).clamp(0.0, 1.0);
            let along = if fraction >= 1.0 {
                1.0
            } else if fraction <= 0.0 {
                0.0
            } else {
                cover(angle).max(cover(angle - std::f32::consts::TAU))
            };
            // Arc over track, source-over in straight alpha.
            let a_arc = arc.a * band * along;
            let a_track = track.a * band;
            let a = a_arc + a_track * (1.0 - a_arc);
            if a <= 0.0 {
                continue;
            }
            let mix = |s: f32, d: f32| (s * a_arc + d * a_track * (1.0 - a_arc)) / a;
            let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            pixels.put_pixel(
                x,
                y,
                image::Rgba([
                    byte(mix(arc.b, track.b)),
                    byte(mix(arc.g, track.g)),
                    byte(mix(arc.r, track.r)),
                    byte(a),
                ]),
            );
        }
    }
    pixels
}

fn with_separators(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// whether the indicator has anything to measure against: harnesses that
/// never report a window (antigravity) get no indicator at all, rather than a
/// permanently empty ring.
pub fn has_window(usage: Option<ContextUsage>) -> bool {
    usage
        .and_then(|usage| usage.window)
        .is_some_and(|window| window > 0)
}

fn details(usage: Option<ContextUsage>) -> String {
    match usage.unwrap_or_default() {
        ContextUsage {
            tokens: Some(tokens),
            window: Some(window),
        } if window > 0 => {
            format!(
                "{} / {} tokens\n{} tokens remaining",
                with_separators(tokens),
                with_separators(window),
                with_separators(window.saturating_sub(tokens))
            )
        }
        ContextUsage {
            tokens: Some(tokens),
            ..
        } => format!(
            "{} tokens used\nContext limit not reported",
            with_separators(tokens)
        ),
        ContextUsage {
            window: Some(window),
            ..
        } if window > 0 => format!(
            "{} token capacity\nWaiting for context usage",
            with_separators(window)
        ),
        _ => "Context usage not reported by this harness yet".into(),
    }
}

/// The context ring's popover content.
pub fn card(usage: Option<ContextUsage>, theme: &Theme) -> gpui::Div {
    crate::popover::popover_card(theme)
        .flex()
        .flex_col()
        .child(crate::popover::menu_heading(theme, "Context window"))
        .child(
            div()
                .px(px(8.0))
                .pb(px(6.0))
                .text_size(px(12.0))
                .line_height(px(19.0))
                .whitespace_nowrap()
                .text_color(theme.text_muted)
                .child(SharedString::from(details(usage))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_raster_paints_the_arc_clockwise_from_twelve_over_its_track() {
        let arc = gpui::Hsla::from(gpui::Rgba {
            r: 1.0,
            g: 0.0,
            b: 0.0,
            a: 1.0,
        });
        let track = gpui::Hsla::from(gpui::Rgba {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 0.25,
        });
        // 2x scale: 32px square, centre (16,16), radius 12px, stroke 3.6px.
        let ring = rasterize_ring(0.5, arc, track, 32);
        let bgra = |x: u32, y: u32| ring.get_pixel(x, y).0;
        let (top, right, left) = (bgra(16, 4), bgra(28, 16), bgra(3, 16));
        // Twelve and three o'clock are on a half arc: opaque red.
        for px in [top, right] {
            assert_eq!(px[2], 255, "red channel: {px:?}");
            assert!(px[0] < 10 && px[1] < 10, "no blue/green: {px:?}");
            assert!(px[3] > 240, "opaque: {px:?}");
        }
        // Nine o'clock is past the sweep: only the faint white track.
        assert!(
            left[0] > 240 && left[1] > 240 && left[2] > 240,
            "white: {left:?}"
        );
        assert!((50..=80).contains(&left[3]), "25% alpha: {left:?}");
        // Inside and outside the band: nothing painted.
        assert_eq!(bgra(16, 16)[3], 0);
        assert_eq!(bgra(0, 0)[3], 0);
        // An empty or full fraction is the plain track or the full arc.
        assert_eq!(
            rasterize_ring(0.0, arc, track, 32).get_pixel(16, 4).0[2],
            255
        );
        assert!(rasterize_ring(0.0, arc, track, 32).get_pixel(16, 4).0[3] < 80);
        assert!(rasterize_ring(1.0, arc, track, 32).get_pixel(3, 16).0[3] > 240);
    }

    #[test]
    fn indicator_needs_a_reported_window() {
        assert!(!has_window(None));
        assert!(!has_window(Some(ContextUsage {
            tokens: Some(1_200),
            window: None,
        })));
        assert!(!has_window(Some(ContextUsage {
            tokens: Some(1_200),
            window: Some(0),
        })));
        assert!(has_window(Some(ContextUsage {
            tokens: None,
            window: Some(200_000),
        })));
    }

    #[test]
    fn missing_usage_is_distinct_from_zero_and_overflow() {
        assert!(details(None).contains("not reported"));
        assert!(
            details(Some(ContextUsage {
                tokens: Some(0),
                window: Some(200)
            }))
            .contains("200 tokens remaining")
        );
        assert!(
            details(Some(ContextUsage {
                tokens: Some(250),
                window: Some(200)
            }))
            .contains("0 tokens remaining")
        );
        assert!(
            details(Some(ContextUsage {
                tokens: Some(10),
                window: Some(0)
            }))
            .contains("limit not reported")
        );
    }

    #[test]
    fn token_counts_are_grouped_by_thousands() {
        assert_eq!(with_separators(0), "0");
        assert_eq!(with_separators(999), "999");
        assert_eq!(with_separators(5417), "5,417");
        assert_eq!(with_separators(1_048_576), "1,048,576");
        assert_eq!(
            details(Some(ContextUsage {
                tokens: Some(5417),
                window: Some(1_048_576)
            })),
            "5,417 / 1,048,576 tokens\n1,043,159 tokens remaining"
        );
    }
}
