//! Paint-time source-alpha feather. Resizing changes only GPU parameters, not
//! the image identity, pixels, atlas entry, or an asynchronous raster job.
use crate::settings::NewThreadBackgroundAdjustment;
use gpui::{Bounds, Corners, ImageAlphaMask, Pixels, Point, RenderImage, Window, point, px, size};
use std::{cell::Cell, rc::Rc, sync::Arc};

// Reveal half-strength artwork through the cutout's darkest area. The main
// masked pass preserves its contour, while this underlay softens its contrast.
pub(crate) const CUTOUT_REVEAL_OPACITY: f32 = 0.5;

pub(crate) type SurfaceBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

#[derive(Clone, Copy)]
struct FittedGeometry {
    bounds: Bounds<Pixels>,
    width: f32,
    height: f32,
    overflow_x: f32,
    overflow_y: f32,
}

fn fitted_geometry(
    source_width: f32,
    source_height: f32,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
) -> Option<FittedGeometry> {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0
        || height <= 0.0
        || !width.is_finite()
        || !height.is_finite()
        || source_width <= 0.0
        || source_height <= 0.0
        || !source_width.is_finite()
        || !source_height.is_finite()
    {
        return None;
    }

    let adjustment = adjustment.normalized();
    let cover = (width / source_width).max(height / source_height);
    let fitted_width = (source_width * cover * adjustment.zoom).max(width);
    let fitted_height = (source_height * cover * adjustment.zoom).max(height);
    let overflow_x = (fitted_width - width).max(0.0);
    let overflow_y = (fitted_height - height).max(0.0);
    let fitted = Bounds::new(
        point(
            bounds.left() - px(overflow_x * adjustment.focal_x),
            bounds.top() - px(overflow_y * adjustment.focal_y),
        ),
        size(px(fitted_width), px(fitted_height)),
    );
    Some(FittedGeometry {
        bounds: fitted,
        width: fitted_width,
        height: fitted_height,
        overflow_x,
        overflow_y,
    })
}

fn source_geometry(
    source: &RenderImage,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
) -> Option<FittedGeometry> {
    let source_size = source.size(0);
    fitted_geometry(
        source_size.width.0 as f32,
        source_size.height.0 as f32,
        bounds,
        adjustment,
    )
}

/// Translate a direct-manipulation drag into viewport-independent framing.
pub(crate) fn pan_adjustment(
    source: &RenderImage,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    delta: Point<Pixels>,
) -> NewThreadBackgroundAdjustment {
    let adjustment = adjustment.normalized();
    let Some(geometry) = source_geometry(source, bounds, adjustment) else {
        return adjustment;
    };
    let mut next = adjustment;
    if geometry.overflow_x > 0.0 {
        next.focal_x -= f32::from(delta.x) / geometry.overflow_x;
    }
    if geometry.overflow_y > 0.0 {
        next.focal_y -= f32::from(delta.y) / geometry.overflow_y;
    }
    next.normalized()
}

/// Preserve the source pixel beneath `anchor` while changing magnification.
pub(crate) fn zoom_adjustment_around(
    source: &RenderImage,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    zoom: f32,
    anchor: Point<Pixels>,
) -> NewThreadBackgroundAdjustment {
    let adjustment = adjustment.normalized();
    let Some(previous) = source_geometry(source, bounds, adjustment) else {
        return adjustment;
    };
    let mut next = NewThreadBackgroundAdjustment { zoom, ..adjustment }.normalized();
    let Some(fitted) = source_geometry(source, bounds, next) else {
        return adjustment;
    };

    let source_x = f32::from(anchor.x - previous.bounds.left()) / previous.width;
    let source_y = f32::from(anchor.y - previous.bounds.top()) / previous.height;
    if fitted.overflow_x > 0.0 {
        let desired_left = f32::from(anchor.x) - source_x * fitted.width;
        next.focal_x = (f32::from(bounds.left()) - desired_left) / fitted.overflow_x;
    }
    if fitted.overflow_y > 0.0 {
        let desired_top = f32::from(anchor.y) - source_y * fitted.height;
        next.focal_y = (f32::from(bounds.top()) - desired_top) / fitted.overflow_y;
    }
    next.normalized()
}

fn mask(bounds: Bounds<Pixels>, composer: Bounds<Pixels>, cutout: bool) -> ImageAlphaMask {
    let height = f32::from(bounds.size.height);
    // Keep the cleared area open through the hero's bottom. A taller image
    // must not fade back in beneath the composer's rounded lower edge.
    let cleared = Bounds::new(
        composer.origin,
        size(
            composer.size.width,
            composer.bottom().max(bounds.bottom()) - composer.top(),
        ),
    );
    ImageAlphaMask {
        // The reveal pass has only the shared bottom fade. Its exclusion sits
        // below the image, so it fills the cutout without changing its shape.
        bounds: if cutout {
            cleared
        } else {
            Bounds::new(point(bounds.left(), bounds.bottom() + px(1.0)), bounds.size)
        },
        radius: if cutout {
            px(crate::composer::COMPOSER_RADIUS)
        } else {
            px(0.0)
        },
        feather: if cutout {
            px((height * 0.52).clamp(120.0, 280.0))
        } else {
            px(1.0)
        },
        clearance: if cutout { px(8.0) } else { px(0.0) },
        // Start fading at the image's top, rather than holding full opacity
        // through its first 40% and compressing the transition near the bottom.
        // Both passes use the full height, independently of the softer cutout.
        bottom_fade: Some((bounds.bottom(), px(height.max(1.0)))),
    }
}

fn adjusted_image_paint(
    source: &RenderImage,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    corner_radii: Corners<Pixels>,
) -> Option<(Bounds<Pixels>, Bounds<Pixels>, Corners<Pixels>)> {
    let fitted = source_geometry(source, bounds, adjustment)?;
    Some((bounds, fitted.bounds, corner_radii))
}

fn paint_adjusted_with_mask(
    source: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    corner_radii: Corners<Pixels>,
    mask: Option<ImageAlphaMask>,
    window: &mut Window,
) {
    let Some((visible, fitted, corner_radii)) =
        adjusted_image_paint(&source, bounds, adjustment, corner_radii)
    else {
        return;
    };
    let _ = window.paint_image_fitted_masked(visible, fitted, corner_radii, source, 0, false, mask);
}

/// Paint the exact responsive crop used by the new-thread hero, without its
/// composer mask. Appearance uses this for a faithful adjustment preview.
pub(crate) fn paint_adjusted(
    source: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    corner_radii: Corners<Pixels>,
    window: &mut Window,
) {
    paint_adjusted_with_mask(source, bounds, adjustment, corner_radii, None, window);
}

/// All elements have finished prepaint before this reads the measured surface,
/// so the first visible frame uses the current composer, including on sidebar
/// resize and right-panel handoffs. Object-fit cropping is independent.
pub(crate) fn paint(
    source: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
    composer: Bounds<Pixels>,
    adjustment: NewThreadBackgroundAdjustment,
    cutout: bool,
    window: &mut Window,
) {
    paint_adjusted_with_mask(
        source,
        bounds,
        adjustment,
        Default::default(),
        Some(mask(bounds, composer, cutout)),
        window,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, canvas, div, prelude::*};

    fn hero(width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(40.0), px(24.0)), size(px(width), px(height)))
    }

    fn fitted(
        source: (f32, f32),
        viewport: Bounds<Pixels>,
        adjustment: NewThreadBackgroundAdjustment,
    ) -> FittedGeometry {
        fitted_geometry(source.0, source.1, viewport, adjustment).unwrap()
    }

    fn assert_near(actual: Pixels, expected: Pixels) {
        assert!(
            (f32::from(actual - expected)).abs() < 0.001,
            "expected {expected:?}, got {actual:?}"
        );
    }

    fn source(width: u32, height: u32) -> RenderImage {
        RenderImage::new([image::Frame::new(image::RgbaImage::from_pixel(
            width,
            height,
            image::Rgba([79, 151, 233, 180]),
        ))])
    }

    #[test]
    fn default_adjustment_is_the_existing_centered_cover_crop() {
        let viewport = hero(1000.0, 500.0);
        let crop = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment::default(),
        );
        assert_eq!(crop.bounds.size.width, px(1000.0));
        assert_eq!(crop.bounds.size.height, px(562.5));
        assert_eq!(crop.bounds.left(), viewport.left());
        assert_eq!(crop.bounds.top(), viewport.top() - px(31.25));
        assert_eq!(crop.bounds.center(), viewport.center());
    }

    #[test]
    fn focal_extremes_align_the_overflowing_image_edges() {
        let viewport = hero(1000.0, 500.0);
        let start = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment {
                focal_x: 0.0,
                focal_y: 0.0,
                zoom: 2.0,
            },
        );
        assert_eq!(start.bounds.left(), viewport.left());
        assert_eq!(start.bounds.top(), viewport.top());

        let end = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment {
                focal_x: 1.0,
                focal_y: 1.0,
                zoom: 2.0,
            },
        );
        assert_near(end.bounds.right(), viewport.right());
        assert_near(end.bounds.bottom(), viewport.bottom());
    }

    #[test]
    fn zoom_multiplies_cover_before_positioning() {
        let viewport = hero(1000.0, 500.0);
        let base = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment::default(),
        );
        let zoomed = fitted(
            (1600.0, 900.0),
            viewport,
            NewThreadBackgroundAdjustment {
                zoom: 2.0,
                ..Default::default()
            },
        );
        assert_near(zoomed.bounds.size.width, base.bounds.size.width * 2.0);
        assert_near(zoomed.bounds.size.height, base.bounds.size.height * 2.0);
        assert_eq!(zoomed.bounds.center(), viewport.center());
    }

    #[test]
    fn resizing_preserves_the_normalized_focal_alignment() {
        let adjustment = NewThreadBackgroundAdjustment {
            focal_x: 0.23,
            focal_y: 0.71,
            zoom: 2.2,
        };
        for viewport in [hero(420.0, 300.0), hero(960.0, 420.0), hero(1200.0, 760.0)] {
            let crop = fitted((1600.0, 900.0), viewport, adjustment);
            let x = f32::from(viewport.left() - crop.bounds.left()) / crop.overflow_x;
            let y = f32::from(viewport.top() - crop.bounds.top()) / crop.overflow_y;
            assert!((x - adjustment.focal_x).abs() < 0.0001);
            assert!((y - adjustment.focal_y).abs() < 0.0001);
        }
    }

    #[test]
    fn every_supported_crop_covers_the_entire_viewport() {
        for source in [(900.0, 1600.0), (1600.0, 900.0), (1000.0, 1000.0)] {
            for viewport in [hero(360.0, 760.0), hero(1000.0, 500.0), hero(640.0, 640.0)] {
                for focal_x in [0.0, 0.37, 1.0] {
                    for focal_y in [0.0, 0.61, 1.0] {
                        for zoom in [1.0, 1.8, NewThreadBackgroundAdjustment::MAX_ZOOM] {
                            let crop = fitted(
                                source,
                                viewport,
                                NewThreadBackgroundAdjustment {
                                    focal_x,
                                    focal_y,
                                    zoom,
                                },
                            );
                            assert!(crop.bounds.left() <= viewport.left());
                            assert!(crop.bounds.top() <= viewport.top());
                            assert!(crop.bounds.right() >= viewport.right());
                            assert!(crop.bounds.bottom() >= viewport.bottom());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn masked_passes_share_identical_adjusted_geometry() {
        let viewport = hero(1000.0, 500.0);
        let composer = Bounds::new(point(px(180.0), px(320.0)), size(px(720.0), px(124.0)));
        let adjustment = NewThreadBackgroundAdjustment {
            focal_x: 0.17,
            focal_y: 0.82,
            zoom: 1.7,
        };
        let main = fitted((1600.0, 900.0), viewport, adjustment);
        let reveal = fitted((1600.0, 900.0), viewport, adjustment);
        assert_eq!(main.bounds, reveal.bounds);
        let main_mask = mask(viewport, composer, true);
        let reveal_mask = mask(viewport, composer, false);
        assert_ne!(main_mask.bounds, reveal_mask.bounds);
        assert_eq!(main_mask.bottom_fade, reveal_mask.bottom_fade);
    }

    #[test]
    fn pan_adjustment_tracks_direct_pointer_movement() {
        let source = source(16, 9);
        let viewport = hero(100.0, 50.0);
        let adjustment = NewThreadBackgroundAdjustment {
            zoom: 2.0,
            ..Default::default()
        };
        let before = source_geometry(&source, viewport, adjustment).unwrap();
        let next = pan_adjustment(&source, viewport, adjustment, point(px(10.0), px(-5.0)));
        let after = source_geometry(&source, viewport, next).unwrap();
        assert_near(after.bounds.left(), before.bounds.left() + px(10.0));
        assert_near(after.bounds.top(), before.bounds.top() - px(5.0));
    }

    #[test]
    fn pointer_anchored_zoom_keeps_the_same_source_pixel_underneath() {
        let source = source(16, 9);
        let viewport = hero(100.0, 50.0);
        let adjustment = NewThreadBackgroundAdjustment {
            zoom: 1.5,
            ..Default::default()
        };
        let anchor = point(viewport.left() + px(60.0), viewport.top() + px(30.0));
        let before = source_geometry(&source, viewport, adjustment).unwrap();
        let source_x = f32::from(anchor.x - before.bounds.left()) / before.width;
        let source_y = f32::from(anchor.y - before.bounds.top()) / before.height;

        let next = zoom_adjustment_around(&source, viewport, adjustment, 2.4, anchor);
        let after = source_geometry(&source, viewport, next).unwrap();
        let next_source_x = f32::from(anchor.x - after.bounds.left()) / after.width;
        let next_source_y = f32::from(anchor.y - after.bounds.top()) / after.height;
        assert!((next_source_x - source_x).abs() < 0.0001);
        assert!((next_source_y - source_y).abs() < 0.0001);
    }

    #[test]
    fn preview_paint_keeps_rounded_viewport_corners_when_panning_and_zooming() {
        let source = source(16, 9);
        let viewport = hero(100.0, 50.0);
        let rounded = Corners::all(px(11.0));
        let initial = NewThreadBackgroundAdjustment::default();
        let zoomed = zoom_adjustment_around(&source, viewport, initial, 2.0, viewport.center());
        let panned = pan_adjustment(&source, viewport, zoomed, point(px(10.0), px(-5.0)));

        for adjustment in [initial, zoomed, panned] {
            let (visible, fitted, corners) =
                adjusted_image_paint(&source, viewport, adjustment, rounded).unwrap();
            let (hero_visible, hero_fitted, hero_corners) =
                adjusted_image_paint(&source, viewport, adjustment, Default::default()).unwrap();
            assert_eq!(visible, viewport);
            assert_eq!(corners, rounded);
            assert_eq!(hero_corners, Corners::all(px(0.0)));
            assert_eq!((visible, fitted), (hero_visible, hero_fitted));
            assert_eq!(
                fitted,
                source_geometry(&source, viewport, adjustment)
                    .unwrap()
                    .bounds
            );
        }
    }

    #[gpui::test]
    fn background_paint_sees_same_frame_composer_bounds_even_when_painted_first(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Fixture {
            surface: SurfaceBounds,
            painted: SurfaceBounds,
            source: Arc<RenderImage>,
            left: f32,
            width: f32,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let surface = self.surface.clone();
                let painted = self.painted.clone();
                let measured = self.surface.clone();
                let source = self.source.clone();
                div()
                    .relative()
                    .size_full()
                    .child(
                        canvas(
                            |_, _, _| {},
                            move |bounds, _, window, _| {
                                painted.set(surface.get());
                                if let Some(composer) = surface.get() {
                                    paint(
                                        source,
                                        bounds,
                                        composer,
                                        NewThreadBackgroundAdjustment::default(),
                                        true,
                                        window,
                                    );
                                }
                            },
                        )
                        .absolute()
                        .inset_0(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left(px(self.left))
                            .top(px(360.25))
                            .w(px(self.width))
                            .h(px(124.0))
                            .child(
                                canvas(
                                    move |bounds, _, _| measured.set(Some(bounds)),
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .inset_0(),
                            ),
                    )
            }
        }
        let surface: SurfaceBounds = Default::default();
        let painted: SurfaceBounds = Default::default();
        let handle = cx.add_window(|_, _| Fixture {
            surface: surface.clone(),
            painted: painted.clone(),
            source: Arc::new(RenderImage::new([image::Frame::new(
                image::RgbaImage::from_pixel(2, 2, image::Rgba([79, 151, 233, 180])),
            )])),
            left: 40.0,
            width: 768.0,
        });
        for (left, width) in [
            (40.0, 768.0),
            (264.0, 544.0),
            (152.25, 656.0),
            (40.0, 408.0),
            (40.0, 768.0),
        ] {
            handle
                .update(cx, |fixture, _, cx| {
                    fixture.left = left;
                    fixture.width = width;
                    cx.notify();
                })
                .unwrap();
            cx.update_window(handle.into(), |_, window, cx| {
                window.draw(cx).clear();
            })
            .unwrap();
            let actual = painted.get().expect("no cold first-frame geometry");
            assert_eq!(Some(actual), surface.get());
            // GPUI layout snaps to physical pixels; the mask must consume
            // that exact measured surface, not the unrounded layout request.
            assert!((f32::from(actual.left()) - left).abs() <= 0.5);
            assert_eq!(actual.size.width, px(width));
        }
    }

    #[test]
    fn mask_tracks_current_surface_in_window_space_without_rounding() {
        for sidebar in [0.0, 112.25, 224.0] {
            for right_panel in [0.0, 360.0] {
                let hero = Bounds::new(
                    point(px(sidebar), px(40.0)),
                    size(px(1200.0 - sidebar), px(440.0)),
                );
                let composer = Bounds::new(
                    point(px(sidebar + 40.5), px(360.25)),
                    size(px(900.0 - sidebar - right_panel), px(124.0)),
                );
                let mask = mask(hero, composer, true);
                assert_eq!(mask.bounds, composer);
                assert_eq!(mask.bottom_fade, Some((px(480.0), px(440.0))));
                assert_eq!(mask.feather, px(440.0 * 0.52));
                assert_eq!(mask.clearance, px(8.0));
            }
        }
    }

    #[test]
    fn taller_background_stays_cleared_below_the_composer() {
        let hero = Bounds::new(point(px(0.0), px(0.0)), size(px(1440.0), px(691.2)));
        let composer = Bounds::new(point(px(352.0), px(406.0)), size(px(736.0), px(124.0)));
        let mask = mask(hero, composer, true);
        assert_eq!(mask.bounds.origin, composer.origin);
        assert_eq!(mask.bounds.size.width, composer.size.width);
        assert_eq!(mask.bounds.bottom(), hero.bottom());
        assert_eq!(mask.feather, px(280.0));
    }
    #[test]
    fn new_thread_cutout_reveal_preserves_the_bottom_fade_and_image_extent() {
        let hero = Bounds::new(point(px(0.0), px(0.0)), size(px(1440.0), px(691.2)));
        let composer = Bounds::new(point(px(352.0), px(406.0)), size(px(736.0), px(124.0)));
        let cutout = mask(hero, composer, true);
        let reveal = mask(hero, composer, false);
        assert_eq!(cutout.bottom_fade, reveal.bottom_fade);
        assert!(reveal.bounds.top() - reveal.feather >= hero.bottom());
        assert_eq!(reveal.radius, px(0.0));
        assert_eq!(reveal.clearance, px(0.0));
        assert_eq!(CUTOUT_REVEAL_OPACITY, 0.5);
    }

    #[test]
    fn new_thread_main_fade_uses_the_full_height_at_every_window_size() {
        for height in [288.0, 489.6, 691.2, 760.0] {
            let hero = Bounds::new(point(px(224.25), px(40.5)), size(px(1000.0), px(height)));
            let composer = Bounds::new(point(px(352.0), px(406.0)), size(px(736.0), px(124.0)));
            for cutout in [false, true] {
                let (end, feather) = mask(hero, composer, cutout).bottom_fade.unwrap();
                assert!((f32::from(end - feather - hero.top())).abs() < 0.0001);
                assert_eq!(end, hero.bottom());
            }
        }
    }
}
