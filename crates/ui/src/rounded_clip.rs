//! [`rounded_clip`] — clips a child's whole subtree to its own bounds with
//! rounded corners ([`Window::with_rounded_clip`]): quads, text and images
//! are cut per pixel along the arcs, with an antialiased edge. Built for
//! scrolling content inside a rounded card on glass, where no painted
//! corner cover can hide what scrolls past the corners.

use gpui::{
    AnyElement, App, Bounds, Element, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Window, px,
};

/// Clip `child` to its own bounds with `radius` corners. Takes the child's
/// layout as is, so it can wrap a flex item without changing its sizing.
pub fn rounded_clip(radius: f32, child: impl IntoElement) -> RoundedClip {
    RoundedClip {
        radius,
        child: child.into_any_element(),
    }
}

pub struct RoundedClip {
    radius: f32,
    child: AnyElement,
}

impl Element for RoundedClip {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<gpui::ElementId> {
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
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        // Hitboxes past the bounds are clipped like the paint.
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            self.child.prepaint(window, cx)
        });
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        _prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        window.with_rounded_clip(bounds, px(self.radius), |window| {
            self.child.paint(window, cx)
        });
    }
}

impl IntoElement for RoundedClip {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
