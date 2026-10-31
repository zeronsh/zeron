//! Landing on a search hit: the page glides the hit's row into view, then the
//! row glows a moment so the eye finds it.
//!
//! A page renders its rows deep inside a plain scroll container, and gpui
//! scrolls a container to a descendant only for `List`
//! (`Window::request_autoscroll`). So the row and its page meet halfway, in
//! one frame: the row's [`SearchAnchor`] reports where it laid out during
//! prepaint, and the page's [`PageReveal`] (placed after the scroll
//! container, so it prepaints later) scrolls the container there. One reveal
//! runs at a time, kept in a thread-local like the motion hover clock and
//! bound to the window that asked, so pages carry no state, listeners or
//! timers for it.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, Pixels, Position, ScrollHandle, Style, Window, WindowId, fill, point, px, size,
};

use super::Target;
use crate::motion::{self, MotionSpec};
use crate::settings::widgets;
use crate::theme::Theme;

/// How long a reveal waits for its row: a page still loading its data lays
/// the row out late, and a row that never shows lets the reveal go.
const SEEK_LIMIT: Duration = Duration::from_millis(1500);
/// Room a landed row keeps from the page's edges: the titlebar band and the
/// edge fade above, the fade below.
const TOP_CLEARANCE: f32 = Theme::TITLEBAR_HEIGHT + 24.0;
const BOTTOM_CLEARANCE: f32 = 24.0;
/// How far down the page a row that has to move lands.
const LANDING: f32 = 0.3;
/// The wash: in quickly, held long enough to find, out gently.
const WASH_IN: MotionSpec = motion::FADE_QUICK;
const WASH_HOLD: Duration = Duration::from_millis(900);
const WASH_OUT: MotionSpec = MotionSpec::new(600, motion::EASE);
/// How far a row's wash reaches past its sides, into its block's inset, and
/// its corners. A block's wash keeps to the block, under its own corners.
const WASH_BLEED: f32 = 8.0;
const WASH_RADIUS: f32 = 8.0;

thread_local! {
    static REVEAL: RefCell<Option<(WindowId, Reveal)>> = const { RefCell::new(None) };
}

#[cfg(test)]
thread_local! {
    /// Every target anchored in frames drawn since the last [`take_anchored`].
    static ANCHORED: RefCell<Vec<Target>> = const { RefCell::new(Vec::new()) };
}

/// Land on `target` in `window`: its page glides it into view and it glows a
/// moment. The caller opens the page and repaints; the reveal waits for the
/// row. A page has nothing to land on: it opens as its nav row does.
pub fn reveal(target: Target, window: &Window) {
    let reveal = match target {
        Target::Page(_) => None,
        _ => Some((
            window.window_handle().window_id(),
            Reveal::new(target, Instant::now()),
        )),
    };
    REVEAL.set(reveal);
}

/// Drop the reveal under way, if any: the route changed under it.
pub fn clear_reveal() {
    REVEAL.set(None);
}

/// `f` on the reveal under way in `window`, if there is one.
fn with_reveal<R>(window: &Window, f: impl FnOnce(&mut Reveal) -> R) -> Option<R> {
    let id = window.window_handle().window_id();
    REVEAL.with_borrow_mut(|slot| match slot {
        Some((asked_in, reveal)) if *asked_in == id => Some(f(reveal)),
        _ => None,
    })
}

/// The targets anchored in frames drawn since the last call.
#[cfg(test)]
pub(crate) fn take_anchored() -> Vec<Target> {
    ANCHORED.take()
}

/// The row the reveal under way landed on and lights, once it has.
#[cfg(test)]
pub(crate) fn lit() -> Option<Target> {
    REVEAL.with_borrow(|slot| slot.as_ref().and_then(|(_, reveal)| reveal.phase_lit()))
}

struct Reveal {
    target: Target,
    /// Where to land while the target's row is hidden.
    fallback: Option<Target>,
    asked: Instant,
    /// Where the target's row, and the fallback's, laid out this frame.
    target_bounds: Option<Bounds<Pixels>>,
    fallback_bounds: Option<Bounds<Pixels>>,
    phase: Phase,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// Waiting for the row to lay out.
    Seeking,
    /// Scrolling the page to `lit`, the target's row or its fallback's; its
    /// wash starts once it lands.
    Gliding { lit: Target, glide: Glide },
    /// Landed: `lit` glows from `since`.
    Landed { lit: Target, since: Instant },
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Glide {
    from: f32,
    to: f32,
    started: Instant,
    /// Where the glide last put the page, to tell the user's own scrolling.
    last: f32,
}

/// What the page does this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Idle,
    /// Scroll to this distance and paint again.
    Scroll(f32),
    /// Paint again: the wash starts, or the user took the page back.
    Repaint,
    /// The reveal is done, or gave up on its row.
    Over,
}

/// A page's scroll container as the reveal sees it: window-space edges and
/// scroll distance, down positive.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Viewport {
    top: f32,
    height: f32,
    scrolled: f32,
    max: f32,
}

impl Viewport {
    fn of(scroll: &ScrollHandle) -> Self {
        let bounds = scroll.bounds();
        Self {
            top: f32::from(bounds.top()),
            height: f32::from(bounds.size.height),
            scrolled: -f32::from(scroll.offset().y),
            max: f32::from(scroll.max_offset().y),
        }
    }

    /// The scroll distance that shows a row spanning `top..bottom` (window
    /// space) clear of the page's edges, a third of the way down: `None`
    /// while it already shows clear, or when the page can come no closer.
    fn landing(&self, top: f32, bottom: f32) -> Option<f32> {
        let (top, bottom) = (top - self.top, bottom - self.top);
        let floor = self.height - BOTTOM_CLEARANCE;
        if top >= TOP_CLEARANCE && bottom <= floor {
            return None;
        }
        // A row taller than the room keeps its top in view.
        let land_at = (self.height * LANDING)
            .min(floor - (bottom - top))
            .max(TOP_CLEARANCE);
        let to = (self.scrolled + top - land_at).clamp(0.0, self.max);
        ((to - self.scrolled).abs() > 0.5).then_some(to)
    }
}

impl Reveal {
    /// The row lit once landed, if it has.
    #[cfg(test)]
    fn phase_lit(&self) -> Option<Target> {
        match self.phase {
            Phase::Landed { lit, .. } => Some(lit),
            Phase::Seeking | Phase::Gliding { .. } => None,
        }
    }

    fn new(target: Target, now: Instant) -> Self {
        Self {
            target,
            fallback: target.fallback(),
            asked: now,
            target_bounds: None,
            fallback_bounds: None,
            phase: Phase::Seeking,
        }
    }

    /// An anchor's turn, as it lays out: while seeking, note where the row
    /// sits; once landed, the wash strength if this is the lit row.
    fn anchor_at(
        &mut self,
        target: Target,
        bounds: Bounds<Pixels>,
        reduced: bool,
        now: Instant,
    ) -> Option<f32> {
        match self.phase {
            Phase::Seeking => {
                if target == self.target {
                    self.target_bounds = Some(bounds);
                } else if Some(target) == self.fallback {
                    self.fallback_bounds = Some(bounds);
                }
                None
            }
            Phase::Gliding { .. } => None,
            Phase::Landed { lit, since } => {
                if lit != target {
                    return None;
                }
                wash(now.saturating_duration_since(since), reduced)
            }
        }
    }

    /// The page's turn, after its rows laid out.
    fn place_at(&mut self, page: Viewport, reduced: bool, now: Instant) -> Step {
        match self.phase {
            Phase::Seeking => {
                // Nothing may have drawn since the row was asked for: one
                // that shows only past the limit is let be, the user has
                // moved on.
                if now.saturating_duration_since(self.asked) > SEEK_LIMIT {
                    return Step::Over;
                }
                let (lit, row) = if let Some(row) = self.target_bounds.take() {
                    (self.target, row)
                } else if let (Some(fallback), Some(row)) =
                    (self.fallback, self.fallback_bounds.take())
                {
                    (fallback, row)
                } else {
                    return Step::Idle;
                };
                match page.landing(row.top().into(), row.bottom().into()) {
                    Some(to) if !reduced => {
                        let glide = Glide {
                            from: page.scrolled,
                            to,
                            started: now,
                            last: page.scrolled,
                        };
                        self.phase = Phase::Gliding { lit, glide };
                        Step::Repaint
                    }
                    Some(to) => {
                        self.phase = Phase::Landed { lit, since: now };
                        Step::Scroll(to)
                    }
                    None => {
                        self.phase = Phase::Landed { lit, since: now };
                        Step::Repaint
                    }
                }
            }
            Phase::Gliding { lit, mut glide } => {
                // The user scrolled mid-glide: the page is theirs, and the
                // row glows wherever it is.
                if (page.scrolled - glide.last).abs() > 0.5 {
                    self.phase = Phase::Landed { lit, since: now };
                    return Step::Repaint;
                }
                let duration = motion::SCROLL_GLIDE
                    .total()
                    .mul_f32(motion::speed_scale())
                    .as_secs_f32();
                let elapsed = now.saturating_duration_since(glide.started).as_secs_f32();
                let t = (elapsed / duration).min(1.0);
                glide.last = motion::lerp(glide.from, glide.to, motion::SCROLL_GLIDE.progress(t));
                self.phase = if t < 1.0 {
                    Phase::Gliding { lit, glide }
                } else {
                    Phase::Landed { lit, since: now }
                };
                Step::Scroll(glide.last)
            }
            Phase::Landed { since, .. } => {
                if wash(now.saturating_duration_since(since), reduced).is_some() {
                    Step::Idle
                } else {
                    Step::Over
                }
            }
        }
    }
}

/// The wash strength `elapsed` after landing; `None` once it has faded.
/// Reduced motion holds it as long, without the fades.
fn wash(elapsed: Duration, reduced: bool) -> Option<f32> {
    let scale = motion::speed_scale();
    let fade_in = WASH_IN.total().mul_f32(scale);
    let hold = WASH_HOLD.mul_f32(scale);
    let fade_out = WASH_OUT.total().mul_f32(scale);
    if elapsed >= fade_in + hold + fade_out {
        return None;
    }
    if reduced || (fade_in..fade_in + hold).contains(&elapsed) {
        return Some(1.0);
    }
    Some(if elapsed < fade_in {
        WASH_IN.progress(elapsed.as_secs_f32() / fade_in.as_secs_f32())
    } else {
        let out = elapsed - fade_in - hold;
        1.0 - WASH_OUT.progress(out.as_secs_f32() / fade_out.as_secs_f32())
    })
}

/// Marks an element — a settings row, or a whole block — as where a search
/// hit for `target` lands. It lays out exactly as the element it wraps.
pub trait SearchAnchor: IntoElement + Sized {
    fn search_anchor(self, target: impl Into<Target>) -> AnchoredElement {
        AnchoredElement {
            target: target.into(),
            block: false,
            child: self.into_any_element(),
        }
    }
}

impl<E: IntoElement> SearchAnchor for E {}

pub struct AnchoredElement {
    target: Target,
    block: bool,
    child: AnyElement,
}

impl AnchoredElement {
    /// The anchor is a whole block (a [`widgets::section_card`], a row of
    /// cards), not one of its rows: its wash keeps to its edges and corners.
    pub fn block(mut self) -> Self {
        self.block = true;
        self
    }
}

impl Element for AnchoredElement {
    type RequestLayoutState = ();
    /// The wash strength to paint under the row this frame.
    type PrepaintState = Option<f32>;

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
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Option<f32> {
        self.child.prepaint(window, cx);
        #[cfg(test)]
        ANCHORED.with_borrow_mut(|anchored| anchored.push(self.target));
        let reduced = motion::reduced_motion(cx);
        with_reveal(window, |reveal| {
            reveal.anchor_at(self.target, bounds, reduced, Instant::now())
        })
        .flatten()
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        wash: &mut Option<f32>,
        window: &mut Window,
        cx: &mut App,
    ) {
        if let Some(strength) = *wash {
            let mut color = Theme::of(cx).accent_wash;
            color.a *= strength;
            let (bleed, radius) = if self.block {
                (px(0.0), px(widgets::BLOCK_RADIUS))
            } else {
                (px(WASH_BLEED), px(WASH_RADIUS))
            };
            window.paint_quad(
                fill(
                    Bounds::new(
                        point(bounds.left() - bleed, bounds.top()),
                        size(bounds.size.width + bleed * 2.0, bounds.size.height),
                    ),
                    color,
                )
                .corner_radii(radius),
            );
            window.request_animation_frame();
        }
        self.child.paint(window, cx);
    }
}

impl IntoElement for AnchoredElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The reveal's hand on a page's scroll container. Place it after the
/// container inside the page's `.relative()` host; it takes no room.
pub fn page_reveal(scroll: &ScrollHandle) -> PageReveal {
    PageReveal {
        scroll: scroll.clone(),
    }
}

pub struct PageReveal {
    scroll: ScrollHandle,
}

impl Element for PageReveal {
    type RequestLayoutState = ();
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
    ) -> (LayoutId, ()) {
        let style = Style {
            position: Position::Absolute,
            ..Style::default()
        };
        (window.request_layout(style, None, cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let reduced = motion::reduced_motion(cx);
        let step = with_reveal(window, |reveal| {
            reveal.place_at(Viewport::of(&self.scroll), reduced, Instant::now())
        });
        match step {
            Some(Step::Scroll(to)) => {
                let offset = self.scroll.offset();
                self.scroll.set_offset(point(offset.x, px(-to)));
                window.request_animation_frame();
            }
            Some(Step::Repaint) => window.request_animation_frame(),
            Some(Step::Over) => clear_reveal(),
            Some(Step::Idle) | None => {}
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut (),
        _prepaint: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }
}

impl IntoElement for PageReveal {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Entity, Render, TestAppContext, VisualTestContext, div, prelude::*};

    use super::*;
    use crate::settings::ShortcutId;
    use crate::settings::search::SettingId;

    const ROW: Target = Target::Setting(SettingId::WordWrap);

    /// A 600px page scrolled to the top, with 2000px to travel.
    fn page() -> Viewport {
        Viewport {
            top: 0.0,
            height: 600.0,
            scrolled: 0.0,
            max: 2000.0,
        }
    }

    fn row(top: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(40.0), px(top)), size(px(600.0), px(60.0)))
    }

    fn at(start: Instant, ms: u64) -> Instant {
        start + Duration::from_millis(ms)
    }

    #[test]
    fn a_row_already_in_view_lights_where_it_is() {
        let start = Instant::now();
        let mut reveal = Reveal::new(ROW, start);
        assert_eq!(reveal.anchor_at(ROW, row(200.0), false, start), None);
        assert_eq!(reveal.place_at(page(), false, start), Step::Repaint);
        // Fading in, then fully lit, then gone.
        let fading = reveal
            .anchor_at(ROW, row(200.0), false, at(start, 50))
            .unwrap();
        assert!(fading > 0.0 && fading < 1.0, "{fading}");
        assert_eq!(
            reveal.anchor_at(ROW, row(200.0), false, at(start, 600)),
            Some(1.0)
        );
        assert_eq!(
            reveal.anchor_at(ROW, row(200.0), false, at(start, 5000)),
            None
        );
        assert_eq!(reveal.place_at(page(), false, at(start, 5000)), Step::Over);
    }

    #[test]
    fn a_row_below_the_fold_glides_to_a_third_down_then_lights() {
        let start = Instant::now();
        let mut reveal = Reveal::new(ROW, start);
        reveal.anchor_at(ROW, row(1500.0), false, start);
        assert_eq!(reveal.place_at(page(), false, start), Step::Repaint);
        // No wash while the page moves.
        assert_eq!(
            reveal.anchor_at(ROW, row(1500.0), false, at(start, 100)),
            None
        );
        let mut scrolled = 0.0;
        let mut ms = 0;
        loop {
            ms += 16;
            let page = Viewport { scrolled, ..page() };
            match reveal.place_at(page, false, at(start, ms)) {
                Step::Scroll(to) => {
                    assert!(to >= scrolled, "the glide only moves toward the row");
                    scrolled = to;
                }
                step => panic!("unexpected {step:?} at {ms}ms"),
            }
            if matches!(reveal.phase, Phase::Landed { .. }) {
                break;
            }
        }
        // The row's top lands 30% down the page.
        assert_eq!(scrolled, 1500.0 - 600.0 * LANDING);
        assert!(ms >= 500, "the glide takes its full timeline: {ms}ms");
        assert_eq!(
            reveal.anchor_at(ROW, row(180.0), false, at(start, ms + 400)),
            Some(1.0)
        );
    }

    #[test]
    fn scrolling_mid_glide_hands_the_page_back() {
        let start = Instant::now();
        let mut reveal = Reveal::new(ROW, start);
        reveal.anchor_at(ROW, row(1500.0), false, start);
        reveal.place_at(page(), false, start);
        let Step::Scroll(first) = reveal.place_at(page(), false, at(start, 100)) else {
            panic!("the glide moves the page");
        };
        // The wheel moved the page somewhere else meanwhile.
        let wheeled = Viewport {
            scrolled: first + 300.0,
            ..page()
        };
        assert_eq!(
            reveal.place_at(wheeled, false, at(start, 116)),
            Step::Repaint
        );
        assert!(matches!(reveal.phase, Phase::Landed { lit: ROW, .. }));
    }

    #[test]
    fn reduced_motion_jumps_and_snaps_the_wash() {
        let start = Instant::now();
        let mut reveal = Reveal::new(ROW, start);
        reveal.anchor_at(ROW, row(1500.0), true, start);
        assert_eq!(reveal.place_at(page(), true, start), Step::Scroll(1320.0));
        assert_eq!(
            reveal.anchor_at(ROW, row(180.0), true, at(start, 1)),
            Some(1.0)
        );
        assert_eq!(
            reveal.anchor_at(ROW, row(180.0), true, at(start, 1600)),
            Some(1.0)
        );
        assert_eq!(
            reveal.anchor_at(ROW, row(180.0), true, at(start, 1700)),
            None
        );
    }

    #[test]
    fn landings_stay_clear_of_the_edges_and_inside_the_page() {
        let page = page();
        // Under the titlebar band, or past the bottom fade: moved.
        assert_eq!(
            page.landing(10.0, 70.0),
            None,
            "the page cannot rise above its top"
        );
        let scrolled = Viewport {
            scrolled: 500.0,
            ..page
        };
        assert_eq!(scrolled.landing(10.0, 70.0), Some(500.0 + 10.0 - 180.0));
        assert_eq!(page.landing(560.0, 620.0), Some(560.0 - 180.0));
        // Clear of both: left alone.
        assert_eq!(page.landing(TOP_CLEARANCE, 600.0 - BOTTOM_CLEARANCE), None);
        // A row taller than the room keeps its top under the band.
        assert_eq!(page.landing(900.0, 1900.0), Some(900.0 - TOP_CLEARANCE));
        // Near the end of the page, as close as it can come.
        assert_eq!(page.landing(2550.0, 2610.0), Some(2000.0));
    }

    #[test]
    fn a_hidden_row_lands_on_its_parent() {
        let start = Instant::now();
        let delay = Target::Setting(SettingId::AutosaveDelay);
        let autosave = Target::Setting(SettingId::Autosave);
        let mut reveal = Reveal::new(delay, start);
        // Only the parent laid out: autosave is off, so its delay is hidden.
        reveal.anchor_at(autosave, row(200.0), false, start);
        assert_eq!(reveal.place_at(page(), false, start), Step::Repaint);
        assert_eq!(
            reveal.anchor_at(autosave, row(200.0), false, at(start, 600)),
            Some(1.0)
        );
        assert_eq!(
            reveal.anchor_at(delay, row(260.0), false, at(start, 600)),
            None
        );
    }

    #[test]
    fn a_row_that_never_lays_out_in_time_lets_go() {
        let start = Instant::now();
        let shortcut = Target::Shortcut(ShortcutId::SaveFile);
        let mut reveal = Reveal::new(shortcut, start);
        // A page still loading lays the row out late, and still lands.
        assert_eq!(reveal.place_at(page(), false, at(start, 400)), Step::Idle);
        reveal.anchor_at(shortcut, row(200.0), false, at(start, 800));
        assert_eq!(
            reveal.place_at(page(), false, at(start, 800)),
            Step::Repaint
        );
        // One that never comes is given up.
        let mut reveal = Reveal::new(shortcut, start);
        assert_eq!(reveal.place_at(page(), false, at(start, 1600)), Step::Over);
        // So is one that comes past the limit, at the first frame anything
        // draws: the page stays where the user has it.
        let mut reveal = Reveal::new(shortcut, start);
        reveal.anchor_at(shortcut, row(1500.0), false, at(start, 4000));
        assert_eq!(reveal.place_at(page(), false, at(start, 4000)), Step::Over);
    }

    const ROWS: usize = 40;
    const ROW_HEIGHT: f32 = 60.0;
    /// The anchored row on [`TestPage`], far below its fold.
    const ANCHORED_AT: usize = 30;

    /// A page built as the settings pages are: a scroll container of rows,
    /// one of them anchored, and the page's reveal after the container.
    struct TestPage {
        scroll: ScrollHandle,
    }

    impl Render for TestPage {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .relative()
                .size_full()
                .child(
                    div()
                        .id("test-page")
                        .size_full()
                        .overflow_y_scroll()
                        .track_scroll(&self.scroll)
                        .children((0..ROWS).map(|ix| {
                            let row = div().flex_none().h(px(ROW_HEIGHT));
                            if ix == ANCHORED_AT {
                                row.search_anchor(ROW).into_any_element()
                            } else {
                                row.into_any_element()
                            }
                        })),
                )
                .child(page_reveal(&self.scroll))
        }
    }

    /// The page in a 600px-tall window, drawn once, motion reduced.
    fn open_page(cx: &mut TestAppContext) -> (Entity<TestPage>, &mut VisualTestContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            motion::set_reduced_motion(cx, true);
        });
        let (page, cx) = cx.add_window_view(|_, _| TestPage {
            scroll: ScrollHandle::new(),
        });
        cx.simulate_resize(size(px(800.0), px(600.0)));
        draw(cx);
        (page, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
    }

    fn scrolled(page: &Entity<TestPage>, cx: &mut VisualTestContext) -> f32 {
        page.read_with(cx, |page, _| -f32::from(page.scroll.offset().y))
    }

    #[gpui::test]
    fn a_reveal_scrolls_its_page_to_the_row_and_lights_it(cx: &mut TestAppContext) {
        let (page, cx) = open_page(cx);
        assert_eq!(scrolled(&page, cx), 0.0);
        cx.update(|window, _| reveal(ROW, window));
        // The row reports where it laid out; the page jumps to put its top a
        // third of the way down.
        draw(cx);
        assert_eq!(
            scrolled(&page, cx),
            ANCHORED_AT as f32 * ROW_HEIGHT - 600.0 * LANDING
        );
        assert_eq!(lit(), Some(ROW));
        // Drawn there, lit, and the page stays put.
        draw(cx);
        assert_eq!(
            scrolled(&page, cx),
            ANCHORED_AT as f32 * ROW_HEIGHT - 600.0 * LANDING
        );
        assert_eq!(lit(), Some(ROW));
        clear_reveal();
    }

    #[gpui::test]
    fn a_reveal_asked_in_another_window_is_left_alone(cx: &mut TestAppContext) {
        let (page, cx) = open_page(cx);
        let elsewhere = WindowId::from(u64::MAX);
        REVEAL.set(Some((elsewhere, Reveal::new(ROW, Instant::now()))));
        draw(cx);
        draw(cx);
        assert_eq!(scrolled(&page, cx), 0.0);
        assert!(REVEAL.with_borrow(|slot| {
            matches!(slot, Some((id, reveal)) if *id == elsewhere && reveal.phase == Phase::Seeking)
        }));
        clear_reveal();
    }

    #[gpui::test]
    fn a_page_target_clears_the_reveal(cx: &mut TestAppContext) {
        let (_, cx) = open_page(cx);
        cx.update(|window, _| {
            reveal(ROW, window);
            reveal(Target::Page(crate::shell::SettingsSection::Files), window);
        });
        assert!(REVEAL.with_borrow(Option::is_none));
    }
}
