//! Same-frame space shared by the composer, terminal and transcript underlay.
//!
//! The composer resolves the budget during request_layout, before the terminal
//! requests its layout. The underlay consumes it in prepaint, so neither uses
//! last frame's composer size when a draft wraps or gains attachments.

use std::{cell::Cell, rc::Rc};

use gpui::{
    AnyElement, App, AvailableSpace, Bounds, Element, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, Window, div, prelude::*, px,
};

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Geometry {
    /// The composer's layout destination uses this reservation, never the
    /// collision-limited height; otherwise the spring and terminal deadlock.
    pub reserved_height: f32,
    pub height: f32,
    pub content_height: f32,
    pub limit: f32,
}

pub(crate) type SharedGeometry = Rc<Cell<Geometry>>;

impl Geometry {
    pub fn new(height: f32, content_height: f32, limit: f32) -> Self {
        Self {
            reserved_height: height.max(0.0).min(limit),
            height: height.max(0.0).min(limit),
            content_height: content_height.max(0.0).min(limit),
            limit,
        }
    }
}

/// Build the terminal only after its preceding composer has measured itself.
pub(crate) fn terminal(
    geometry: SharedGeometry,
    measured: SharedGeometry,
    build: impl FnOnce(Geometry) -> AnyElement + 'static,
) -> impl IntoElement {
    Terminal {
        geometry,
        measured,
        build: Some(Box::new(build)),
    }
}

struct Terminal {
    geometry: SharedGeometry,
    measured: SharedGeometry,
    build: Option<Box<dyn FnOnce(Geometry) -> AnyElement>>,
}

impl IntoElement for Terminal {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Terminal {
    type RequestLayoutState = AnyElement;
    type PrepaintState = ();
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, AnyElement) {
        let geometry = self.geometry.get();
        if (self.measured.get().height - geometry.height).abs() > 0.5 {
            // Clearance is consumed by the shell on its next render. Even
            // when composer growth trades space with the terminal and leaves
            // total stack height unchanged, that clearance must be refreshed.
            window.request_animation_frame();
        }
        self.measured.set(geometry);
        let child = self.build.take().unwrap()(geometry);
        let mut child = div()
            .w_full()
            .h(px(geometry.reserved_height))
            .flex_none()
            .relative()
            .child(div().absolute().bottom_0().w_full().child(child))
            .into_any_element();
        (child.request_layout(window, cx), child)
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        child: &mut AnyElement,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
    }
}

/// Keep transcript layout and hitboxes above the very same terminal height.
pub(crate) fn above_terminal(
    child: impl IntoElement,
    geometry: SharedGeometry,
) -> impl IntoElement {
    AboveTerminal {
        child: Some(child.into_any_element()),
        geometry,
    }
}

struct AboveTerminal {
    child: Option<AnyElement>,
    geometry: SharedGeometry,
}

impl IntoElement for AboveTerminal {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for AboveTerminal {
    type RequestLayoutState = ();
    type PrepaintState = AnyElement;
    fn id(&self) -> Option<gpui::ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (
            div()
                .size_full()
                .into_any_element()
                .request_layout(window, cx),
            (),
        )
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let height = (f32::from(bounds.size.height) - self.geometry.get().height).max(0.0);
        let mut child = div()
            .w(bounds.size.width)
            .h(px(height))
            .child(self.child.take().unwrap())
            .into_any_element();
        child.prepaint_as_root(
            bounds.origin,
            bounds.size.map(AvailableSpace::Definite),
            window,
            cx,
        );
        child
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        child: &mut AnyElement,
        window: &mut Window,
        cx: &mut App,
    ) {
        child.paint(window, cx);
    }
}
