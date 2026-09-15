//! GPUI framebuffer presentation. The same transform paints and maps input.
use super::profiles::ViewMode;
use gpui::{prelude::*, *};
use std::sync::Arc;
use zeron_rdp::{Frame, RemoteCursor};

#[derive(Clone, Copy, Debug, Default)]
pub struct Transform {
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub width: u16,
    pub height: u16,
}
impl Transform {
    pub fn new(
        viewport: (f32, f32),
        desktop: (u16, u16),
        mode: ViewMode,
        dpi: f32,
        pan: (f32, f32),
    ) -> Self {
        let (w, h) = (desktop.0 as f32, desktop.1 as f32);
        if w == 0. || h == 0. || viewport.0 <= 0. || viewport.1 <= 0. {
            return Self::default();
        }
        let scale = match mode {
            ViewMode::Fit => (viewport.0 / w).min(viewport.1 / h),
            ViewMode::ActualSize => 1. / dpi.max(1.),
        };
        let x = ((viewport.0 - w * scale) / 2.).max(0.)
            - pan.0.clamp(0., (w - viewport.0 / scale).max(0.)) * scale;
        let y = ((viewport.1 - h * scale) / 2.).max(0.)
            - pan.1.clamp(0., (h - viewport.1 / scale).max(0.)) * scale;
        Self {
            x,
            y,
            scale,
            width: desktop.0,
            height: desktop.1,
        }
    }
    pub fn remote(&self, x: f32, y: f32, clamp: bool) -> Option<(u16, u16)> {
        if self.scale <= 0. || self.width == 0 || self.height == 0 {
            return None;
        }
        let (x, y) = ((x - self.x) / self.scale, (y - self.y) / self.scale);
        if !clamp && (x < 0. || y < 0. || x >= self.width as f32 || y >= self.height as f32) {
            return None;
        }
        Some((
            x.clamp(0., self.width as f32 - 1.) as u16,
            y.clamp(0., self.height as f32 - 1.) as u16,
        ))
    }
    pub fn bounds(&self, origin: Point<Pixels>) -> Bounds<Pixels> {
        Bounds::new(
            origin + point(px(self.x), px(self.y)),
            size(
                px(self.width as f32 * self.scale),
                px(self.height as f32 * self.scale),
            ),
        )
    }
}

pub struct Desktop {
    pub focus: FocusHandle,
    pub enabled: bool,
    pub(super) pressed: std::collections::HashMap<String, u16>,
    pub(super) buttons: std::collections::HashSet<MouseButton>,
    pub(super) modifiers: Modifiers,
    pub(super) composition: String,
    pub(super) frame: Option<Arc<RenderImage>>,
    pub(super) cursor: Option<Arc<RenderImage>>,
    pub(super) remote_cursor: RemoteCursor,
    pub(super) frame_key: Option<(u64, u64)>,
    pub(super) dimensions: (u16, u16),
    last_geometry: Option<(u16, u16)>,
    pub(super) bounds: Bounds<Pixels>,
    pub(super) transform: Transform,
    pub mode: ViewMode,
    pub pan: (f32, f32),
    pub(super) pointer: Option<(u16, u16)>,
}
impl Desktop {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        cx.on_blur(&focus, window, |this, _, cx| this.release_input(cx))
            .detach();
        cx.on_release(|view, cx| {
            for image in [view.frame.take(), view.cursor.take()]
                .into_iter()
                .flatten()
            {
                ImageSource::Render(image).evict(None, cx);
            }
        })
        .detach();
        Self {
            focus,
            enabled: false,
            pressed: Default::default(),
            buttons: Default::default(),
            modifiers: Default::default(),
            composition: String::new(),
            frame: None,
            cursor: None,
            remote_cursor: RemoteCursor::Default,
            frame_key: None,
            dimensions: (0, 0),
            last_geometry: None,
            bounds: Bounds::default(),
            transform: Transform::default(),
            mode: ViewMode::Fit,
            pan: (0., 0.),
            pointer: None,
        }
    }
    pub fn update_frame(&mut self, frame: &Frame, window: &mut Window, cx: &mut Context<Self>) {
        if self.frame_key == Some((frame.generation, frame.sequence)) {
            return;
        }
        if zeron_rdp::validate_size(frame.width, frame.height).ok() != Some(frame.bgra.len()) {
            return;
        }
        let Some(pixels) = image::RgbaImage::from_raw(
            frame.width.into(),
            frame.height.into(),
            frame.bgra.to_vec(),
        ) else {
            return;
        };
        let image = Arc::new(RenderImage::new([image::Frame::new(pixels)]));
        if let Some(old) = self.frame.replace(image) {
            let _ = window.drop_image(old);
        }
        self.frame_key = Some((frame.generation, frame.sequence));
        self.dimensions = (frame.width, frame.height);
        cx.notify();
    }
    pub fn update_cursor(
        &mut self,
        cursor: RemoteCursor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Cursor snapshots usually share the same bytes across frame updates.
        if let (RemoteCursor::Bitmap { rgba: a, .. }, RemoteCursor::Bitmap { rgba: b, .. }) =
            (&self.remote_cursor, &cursor)
            && Arc::ptr_eq(a, b)
        {
            return;
        }
        if let Some(old) = self.cursor.take() {
            let _ = window.drop_image(old);
        }
        if let RemoteCursor::Bitmap {
            width,
            height,
            rgba,
            ..
        } = &cursor
        {
            if usize::from(*width) * usize::from(*height) <= 384 * 384
                && rgba.len() == usize::from(*width) * usize::from(*height) * 4
            {
                let mut bytes = rgba.to_vec();
                for p in bytes.chunks_exact_mut(4) {
                    p.swap(0, 2);
                }
                if let Some(pixels) =
                    image::RgbaImage::from_raw((*width).into(), (*height).into(), bytes)
                {
                    self.cursor = Some(Arc::new(RenderImage::new([image::Frame::new(pixels)])));
                }
            }
        }
        self.remote_cursor = cursor;
        cx.notify();
    }
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for image in [self.frame.take(), self.cursor.take()]
            .into_iter()
            .flatten()
        {
            let _ = window.drop_image(image);
        }
        self.frame_key = None;
        self.dimensions = (0, 0);
        cx.notify();
    }
    fn paint(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.enabled {
            window.handle_input(
                &self.focus,
                super::input::WeakInputHandler {
                    view: cx.entity().downgrade(),
                    bounds,
                },
                cx,
            );
        }
        if self.enabled
            && bounds.contains(&window.mouse_position())
            && self.pointer.is_some()
            && !matches!(self.remote_cursor, RemoteCursor::Default)
        {
            super::cursor::hide(cx);
        }
        let geometry = (
            (f32::from(bounds.size.width) * window.scale_factor()).round() as u16,
            (f32::from(bounds.size.height) * window.scale_factor()).round() as u16,
        );
        if self.last_geometry != Some(geometry)
            && zeron_rdp::validate_size(geometry.0, geometry.1).is_ok()
        {
            self.last_geometry = Some(geometry);
            cx.emit(super::input::DesktopEvent::Geometry(geometry.0, geometry.1));
        }
        self.bounds = bounds;
        self.transform = Transform::new(
            (bounds.size.width.into(), bounds.size.height.into()),
            self.dimensions,
            self.mode,
            window.scale_factor(),
            self.pan,
        );
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            if let Some(image) = &self.frame {
                let _ = window.paint_image(
                    self.transform.bounds(bounds.origin),
                    Corners::default(),
                    image.clone(),
                    0,
                    false,
                );
            }
            if let (
                Some(image),
                Some((x, y)),
                RemoteCursor::Bitmap {
                    width,
                    height,
                    hotspot_x,
                    hotspot_y,
                    ..
                },
            ) = (&self.cursor, self.pointer, &self.remote_cursor)
            {
                let scale = self.transform.scale;
                let origin = bounds.origin
                    + point(
                        px(self.transform.x + (x as f32 - *hotspot_x as f32) * scale),
                        px(self.transform.y + (y as f32 - *hotspot_y as f32) * scale),
                    );
                let _ = window.paint_image(
                    Bounds::new(
                        origin,
                        size(px(*width as f32 * scale), px(*height as f32 * scale)),
                    ),
                    Corners::default(),
                    image.clone(),
                    0,
                    false,
                );
            }
        });
    }
}
impl Render for Desktop {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let mut element = div()
            .id("remote-desktop-canvas")
            .track_focus(&self.focus)
            .key_context("RemoteDesktop")
            .on_action(
                cx.listener(|this, _: &super::input::ReleaseCapture, w, cx| {
                    this.release_capture(w, cx)
                }),
            )
            .on_key_down(cx.listener(Self::key_down))
            .on_key_up(cx.listener(Self::key_up))
            .on_modifiers_changed(cx.listener(Self::modifiers_changed))
            .on_mouse_move(cx.listener(Self::mouse_move))
            .on_hover(cx.listener(|this, hovered, _, cx| {
                if !hovered {
                    this.pointer = None;
                    cx.notify();
                }
            }))
            .on_scroll_wheel(cx.listener(Self::scroll))
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(rgb(0x111318));
        for button in MouseButton::all() {
            element = element
                .on_mouse_down(button, cx.listener(Self::mouse_down))
                .on_mouse_up(button, cx.listener(Self::mouse_up))
                .on_mouse_up_out(button, cx.listener(Self::mouse_up));
        }
        element.child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, cx| {
                    let _ = view.update(cx, |view, cx| view.paint(bounds, window, cx));
                },
            )
            .size_full(),
        )
    }
}
impl Focusable for Desktop {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{Transform, ViewMode};
    #[test]
    fn remote_desktop_letterboxing_and_dpi_share_hit_transform() {
        let fit = Transform::new((520., 800.), (1280, 800), ViewMode::Fit, 2., (0., 0.));
        assert!(fit.remote(10., 10., false).is_none());
        assert_eq!(fit.remote(260., 400., false), Some((640, 400)));
        let actual = Transform::new(
            (360., 360.),
            (1920, 1080),
            ViewMode::ActualSize,
            2.,
            (400., 200.),
        );
        assert_eq!(actual.remote(100., 100., false), Some((600, 400)));
        assert_eq!(actual.remote(-1000., -1000., true), Some((0, 0)));
        assert!(Transform::default().remote(0., 0., true).is_none());
    }
}
