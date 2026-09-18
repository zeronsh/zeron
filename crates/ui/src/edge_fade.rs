//! [`edge_faded`] — wraps a child so its whole subtree paints inside a
//! [`gpui::EdgeFade`] scope: primitives fade by vertical distance to the
//! wrapper's own top/bottom edges (per-pixel text opacity — a true static
//! gradient, unlike whole-row opacity). Built for the GLASS sidebar's scroll
//! fade: over a see-through blurred backdrop no painted overlay can fade
//! content out, because "what is behind the window" is not a paintable color.

use std::cell::RefCell;

use gpui::{
    AnyElement, App, Bounds, EdgeFade, Element, Global, GlobalElementId, InspectorElementId,
    IntoElement, LayoutId, Pixels, ScrollHandle, Window, WindowId, px,
};

// GPUI replaces rather than composes nested EdgeFade scopes. Keep the active
// wrapper scopes during painting so a label can retain its scroll container's
// vertical fade. Window IDs prevent a nested draw of another window inheriting it.
#[derive(Default)]
struct PaintFades(RefCell<Vec<(WindowId, EdgeFade)>>);

impl Global for PaintFades {}

fn active_fade(window: &Window, cx: &App) -> Option<EdgeFade> {
    cx.try_global::<PaintFades>()?
        .0
        .borrow()
        .iter()
        .rev()
        .find(|(id, _)| *id == window.window_handle().window_id())
        .map(|(_, fade)| *fade)
}

fn inherit_vertical_fade(mut label: EdgeFade, parent: EdgeFade) -> EdgeFade {
    if !label.top && !label.bottom && (parent.top || parent.bottom) {
        label.bounds.origin.y = parent.bounds.origin.y;
        label.bounds.size.height = parent.bounds.size.height;
        label.top = parent.top;
        label.bottom = parent.bottom;
        // Keep the label's horizontal band independent of the container's
        // vertical band, including asymmetric top/bottom overrides.
        label.band_top = Some(parent.band_top.unwrap_or(parent.band));
        label.band_bottom = Some(parent.band_bottom.unwrap_or(parent.band));
    }
    label
}

/// Fade the child's content at its own edges: `top`/`bottom` select which
/// edges (pass the "is there hidden overflow" flags), `band` is the ramp
/// height in px. Horizontal edges via [`EdgeFaded::fade_left`] /
/// [`EdgeFaded::fade_right`].
pub fn edge_faded(band: f32, top: bool, bottom: bool, child: impl IntoElement) -> EdgeFaded {
    EdgeFaded {
        band,
        band_top: None,
        band_bottom: None,
        inset_top: 0.0,
        outset_bottom: 0.0,
        top,
        bottom,
        left: false,
        right: false,
        scroll_y: None,
        overflow_y: None,
        scroll_x: None,
        smooth_overflow_x: false,
        child: child.into_any_element(),
    }
}

pub struct EdgeFaded {
    band: f32,
    band_top: Option<f32>,
    band_bottom: Option<f32>,
    inset_top: f32,
    outset_bottom: f32,
    top: bool,
    bottom: bool,
    left: bool,
    right: bool,
    scroll_y: Option<ScrollHandle>,
    overflow_y: Option<Box<dyn Fn(&App) -> (bool, bool)>>,
    scroll_x: Option<ScrollHandle>,
    smooth_overflow_x: bool,
    child: AnyElement,
}

impl EdgeFaded {
    pub fn fade_left(mut self, on: bool) -> Self {
        self.left = on;
        self
    }

    pub fn fade_right(mut self, on: bool) -> Self {
        self.right = on;
        self
    }

    /// Override the ramp height at the TOP edge only. Asymmetric bands let
    /// content fade across chrome of different heights — a short titlebar
    /// above vs a tall composer stack below.
    pub fn band_top(mut self, px: f32) -> Self {
        self.band_top = Some(px);
        self
    }

    /// [`Self::band_top`], for the bottom edge.
    pub fn band_bottom(mut self, px: f32) -> Self {
        self.band_bottom = Some(px);
        self
    }

    /// Gate the vertical fades on the handle's overflow, read at PAINT time —
    /// after the tracked div's prepaint has clamped the offset for this frame.
    /// Render-time gating rides the LAST frame's offset, which goes stale on
    /// the final frame of a content shrink (rows removed while scrolled):
    /// prepaint clamps the offset to fit, nothing re-renders, and a fade with
    /// no overflow sticks on screen (user report). `top`/`bottom` become
    /// enables; the handle decides per frame.
    pub fn fade_overflow_y(mut self, handle: &ScrollHandle) -> Self {
        self.scroll_y = Some(handle.clone());
        self
    }

    /// Paint-time overflow for custom scrollable elements that do not use a
    /// ScrollHandle. Called after the child's prepaint has clamped scrolling.
    pub fn fade_overflow_y_with(
        mut self,
        overflow: impl Fn(&App) -> (bool, bool) + 'static,
    ) -> Self {
        self.overflow_y = Some(Box::new(overflow));
        self
    }

    /// [`Self::fade_overflow_y`] for the HORIZONTAL edges — gates
    /// [`Self::fade_left`]/[`Self::fade_right`] on the handle's x overflow at
    /// paint time (the right-pane surface-tab strip).
    pub fn fade_overflow_x(mut self, handle: &ScrollHandle) -> Self {
        self.scroll_x = Some(handle.clone());
        self
    }

    /// Ease a right-edge label fade into view as overflow grows from zero to
    /// one band. Unlike scroll chrome, a label should not suddenly dim its
    /// final characters as soon as it becomes a fraction too wide.
    pub fn fade_label_overflow(mut self, handle: &ScrollHandle) -> Self {
        self.scroll_x = Some(handle.clone());
        self.smooth_overflow_x = true;
        self
    }

    /// Pull the fade's TOP edge `px` inside the wrapper: gpui clamps the ramp
    /// to 0 past an active edge, so content between the wrapper's real top
    /// and the inset edge paints fully transparent — content under opaque-ish
    /// chrome (titlebar TEXT) vanishes before it can overlap.
    pub fn inset_top(mut self, px: f32) -> Self {
        self.inset_top = px;
        self
    }

    pub fn outset_bottom(mut self, px: f32) -> Self {
        self.outset_bottom = px;
        self
    }
}

impl Element for EdgeFaded {
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
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
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
        let (mut top, mut bottom) = (self.top, self.bottom);
        if let Some(scroll) = &self.scroll_y {
            let scrolled = -f32::from(scroll.offset().y);
            let max_scroll = f32::from(scroll.max_offset().y);
            top &= scrolled > 1.0;
            bottom &= scrolled < max_scroll - 1.0;
        }
        if let Some(overflow) = &self.overflow_y {
            let (overflow_top, overflow_bottom) = overflow(cx);
            top &= overflow_top;
            bottom &= overflow_bottom;
        }
        let (mut left, mut right) = (self.left, self.right);
        let mut outset_right = 0.0;
        if let Some(scroll) = &self.scroll_x {
            let scrolled = -f32::from(scroll.offset().x);
            let max_scroll = f32::from(scroll.max_offset().x);
            left &= scrolled > 1.0;
            if self.smooth_overflow_x {
                let overflow = (max_scroll - scrolled).max(0.0);
                right &= overflow > 0.0;
                outset_right = label_fade_outset(overflow, self.band);
            } else {
                right &= scrolled < max_scroll - 1.0;
            }
        }
        let fade = (top || bottom || left || right).then(|| {
            let mut bounds = bounds;
            bounds.size.width += px(outset_right);
            let inset = px(self.inset_top).min(bounds.size.height);
            bounds.origin.y += inset;
            bounds.size.height -= inset;
            bounds.size.height += px(self.outset_bottom);
            bounds.size.height = bounds.size.height.max(px(0.0));
            EdgeFade {
                bounds,
                band: px(self.band),
                band_top: self.band_top.map(px),
                band_bottom: self.band_bottom.map(px),
                top,
                bottom,
                left,
                right,
            }
        });
        let Some(mut fade) = fade else {
            self.child.paint(window, cx);
            return;
        };
        if self.smooth_overflow_x
            && let Some(parent) = active_fade(window, cx)
        {
            fade = inherit_vertical_fade(fade, parent);
        }
        if !cx.has_global::<PaintFades>() {
            cx.set_global(PaintFades::default());
        }
        cx.global::<PaintFades>()
            .0
            .borrow_mut()
            .push((window.window_handle().window_id(), fade));
        window.with_edge_fade(Some(fade), |window| self.child.paint(window, cx));
        cx.global::<PaintFades>().0.borrow_mut().pop();
    }
}

fn label_fade_outset(overflow: f32, band: f32) -> f32 {
    let band = band.max(1.0);
    let progress = (overflow / band).clamp(0.0, 1.0);
    let eased = progress * progress * (3.0 - 2.0 * progress);
    band * (1.0 - eased)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, Context, Render, Styled, div, prelude::*};
    use std::rc::Rc;

    #[gpui::test]
    fn painted_label_retains_scroll_fade_across_overflow_transitions(
        cx: &mut gpui::TestAppContext,
    ) {
        struct Fixture {
            width: f32,
            top: bool,
            bottom: bool,
            observed: Rc<RefCell<Vec<Option<EdgeFade>>>>,
        }
        impl Render for Fixture {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let probe = |width: f32| {
                    let observed = self.observed.clone();
                    gpui::canvas(
                        |_, _, _| (),
                        move |_, _, window, cx| {
                            observed.borrow_mut().push(active_fade(window, cx));
                        },
                    )
                    .w(px(width))
                    .h(px(20.0))
                    .flex_none()
                };
                let scroll = ScrollHandle::new();
                let label = edge_faded(
                    20.0,
                    false,
                    false,
                    div()
                        .id("label")
                        .w(px(self.width))
                        .overflow_hidden()
                        .track_scroll(&scroll)
                        .flex()
                        .child(probe(200.0)),
                )
                .fade_right(true)
                .fade_label_overflow(&scroll);
                div()
                    .child(
                        edge_faded(
                            24.0,
                            self.top,
                            self.bottom,
                            div()
                                .w(px(300.0))
                                .h(px(100.0))
                                .child(label)
                                .child(probe(20.0)),
                        )
                        .band_top(12.0)
                        .band_bottom(30.0)
                        .inset_top(7.0)
                        .outset_bottom(9.0),
                    )
                    .child(probe(20.0))
            }
        }
        let observed = Rc::new(RefCell::new(Vec::new()));
        let handle = cx.add_window(|_, _| Fixture {
            width: 80.0,
            top: true,
            bottom: true,
            observed: observed.clone(),
        });
        for (top, bottom) in [(true, true), (true, false), (false, true), (false, false)] {
            for width in [80.0, 240.0, 80.0] {
                handle
                    .update(cx, |view, _, cx| {
                        view.width = width;
                        view.top = top;
                        view.bottom = bottom;
                        cx.notify();
                    })
                    .unwrap();
                observed.borrow_mut().clear();
                cx.update_window(handle.into(), |_, window, cx| {
                    window.draw(cx).clear();
                    assert!(active_fade(window, cx).is_none(), "paint scope leaked");
                })
                .unwrap();
                let seen = observed.borrow();
                let [label, sibling, outside] = &seen[seen.len() - 3..] else {
                    unreachable!()
                };
                assert!(outside.is_none(), "fade leaked outside scroll container");
                let overflowing = width < 200.0;
                if top || bottom {
                    let label = label.unwrap();
                    let sibling = sibling.unwrap();
                    assert_eq!((label.top, label.bottom), (top, bottom));
                    assert_eq!(label.bounds.origin.y, sibling.bounds.origin.y);
                    assert_eq!(label.bounds.size.height, sibling.bounds.size.height);
                    assert_eq!(label.band_top, Some(px(12.0)));
                    assert_eq!(label.band_bottom, Some(px(30.0)));
                    assert_eq!(label.right, overflowing);
                    assert!(!sibling.right, "label fade leaked to a sibling");
                    if overflowing {
                        assert_eq!(label.band, px(20.0));
                        assert_eq!(label.bounds.size.width, px(width));
                    }
                } else {
                    assert!(sibling.is_none());
                    assert_eq!(label.is_some(), overflowing);
                    if let Some(label) = label {
                        assert!(!label.top && !label.bottom);
                        assert!(label.right);
                    }
                }
            }
        }
    }

    #[test]
    fn label_fade_enters_continuously_and_settles_at_one_band() {
        let band = 20.0;
        assert_eq!(label_fade_outset(0.0, band), band);
        assert_eq!(label_fade_outset(band, band), 0.0);
        assert_eq!(label_fade_outset(100.0, band), 0.0);
        // Simulate resizing in 0.1 px increments across the old 1 px cutoff.
        let mut previous = 1.0;
        for step in 0..=400 {
            let overflow = step as f32 / 10.0;
            let edge_alpha = (label_fade_outset(overflow, band) / band).powi(2);
            assert!(edge_alpha <= previous);
            assert!(previous - edge_alpha < 0.02);
            previous = edge_alpha;
        }
    }
}

impl IntoElement for EdgeFaded {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}
