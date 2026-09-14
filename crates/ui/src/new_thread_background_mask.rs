//! Paint-time source-alpha feather. Resizing changes only GPU parameters, not
//! the image identity, pixels, atlas entry, or an asynchronous raster job.
use gpui::{Bounds, ImageAlphaMask, Pixels, RenderImage, Window, point, px, size};
use std::{cell::Cell, rc::Rc, sync::Arc};

pub(crate) type SurfaceBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

fn mask(bounds: Bounds<Pixels>, composer: Bounds<Pixels>) -> ImageAlphaMask {
    let height = f32::from(bounds.size.height);
    ImageAlphaMask {
        bounds: composer,
        radius: px(crate::composer::COMPOSER_RADIUS),
        feather: px((height * 0.52).clamp(120.0, 220.0)),
        clearance: px(8.0),
        bottom_fade: Some((bounds.bottom(), px((height * 0.22).max(1.0)))),
    }
}

/// All elements have finished prepaint before this reads the measured surface,
/// so the first visible frame uses the current composer, including on sidebar
/// resize and right-panel handoffs. Object-fit cropping is independent.
pub(crate) fn paint(
    source: Arc<RenderImage>,
    bounds: Bounds<Pixels>,
    composer: Bounds<Pixels>,
    window: &mut Window,
) {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    let source_size = source.size(0);
    if width <= 0.0 || height <= 0.0 || source_size.width.0 <= 0 || source_size.height.0 <= 0 {
        return;
    }
    let scale = (width / source_size.width.0 as f32).max(height / source_size.height.0 as f32);
    let fitted_size = size(
        px(source_size.width.0 as f32 * scale),
        px(source_size.height.0 as f32 * scale),
    );
    let fitted = Bounds::new(
        bounds.center() - point(fitted_size.width * 0.5, fitted_size.height * 0.5),
        fitted_size,
    );
    let _ = window.paint_image_fitted_masked(
        bounds,
        fitted,
        Default::default(),
        source,
        0,
        false,
        Some(mask(bounds, composer)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, canvas, div, prelude::*};

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
                                    paint(source, bounds, composer, window);
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
                let mask = mask(hero, composer);
                assert_eq!(mask.bounds, composer);
                assert_eq!(mask.bottom_fade, Some((px(480.0), px(96.8))));
                assert_eq!(mask.feather, px(220.0));
                assert_eq!(mask.clearance, px(8.0));
            }
        }
    }
}
