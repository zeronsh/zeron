//! Two persistent explorer pages, with horizontal gestures and a settling slide.

use std::time::{Duration, Instant};

use gpui::{
    AnyElement, Context, FocusHandle, ScrollWheelEvent, Size, Task, TouchPhase, Window, canvas,
    div, prelude::*, px, relative,
};

use crate::{icons, motion, surface_chrome, theme::Theme};

use super::{FilesEvent, FilesSurface};

const SETTLE_DELAY: Duration = Duration::from_millis(160);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ExplorerPage {
    Agents,
    #[default]
    Files,
}

impl ExplorerPage {
    fn position(self) -> f32 {
        match self {
            Self::Agents => 0.0,
            Self::Files => 1.0,
        }
    }
}

#[derive(Clone, Copy)]
struct Slide {
    from: f32,
    started: Instant,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GestureAxis {
    Horizontal,
    Vertical,
}

pub(super) struct ExplorerPane {
    page: ExplorerPage,
    position: f32,
    slide: Option<Slide>,
    axis: Option<GestureAxis>,
    settle: Option<Task<()>>,
    pub(super) viewport: Size<gpui::Pixels>,
    pub(super) focus: FocusHandle,
}

impl ExplorerPane {
    pub(super) fn new(cx: &mut Context<FilesSurface>) -> Self {
        Self {
            page: ExplorerPage::Files,
            position: ExplorerPage::Files.position(),
            slide: None,
            axis: None,
            settle: None,
            viewport: Size::default(),
            focus: cx.focus_handle(),
        }
    }

    fn painted_position(&self) -> f32 {
        self.slide.map_or(self.position, |slide| {
            let t = slide.started.elapsed().as_secs_f32() / motion::RESIZE.total().as_secs_f32();
            motion::lerp(slide.from, self.page.position(), motion::RESIZE.progress(t))
        })
    }

    fn select(&mut self, page: ExplorerPage, reduced_motion: bool) {
        let from = self.painted_position();
        self.page = page;
        self.position = page.position();
        self.slide = (!reduced_motion && (from - self.position).abs() > 0.001).then(|| Slide {
            from,
            started: Instant::now(),
        });
    }

    /// Lock the axis for the whole gesture. A vertical list scroll with a
    /// little sideways drift must never become a page change midway through.
    fn scroll(&mut self, x: f32, y: f32, width: f32) -> bool {
        if !x.is_finite() || !y.is_finite() || width <= 0.0 {
            return false;
        }
        if self.axis.is_none() && x.abs().max(y.abs()) > 0.0 {
            self.axis = Some(if x.abs() > y.abs() * 1.25 {
                GestureAxis::Horizontal
            } else {
                GestureAxis::Vertical
            });
        }
        if self.axis != Some(GestureAxis::Horizontal) {
            return false;
        }
        self.position = (self.painted_position() - x / width).clamp(0.0, 1.0);
        self.slide = None;
        true
    }

    fn destination(&self) -> ExplorerPage {
        // A short, deliberate swipe is enough; tiny drifts return to the
        // current page. The threshold scales down with narrow panes.
        let threshold = (48.0 / f32::from(self.viewport.width).max(1.0)).min(0.22);
        match self.page {
            ExplorerPage::Agents if self.position >= threshold => ExplorerPage::Files,
            ExplorerPage::Files if self.position <= 1.0 - threshold => ExplorerPage::Agents,
            page => page,
        }
    }
}

impl FilesSurface {
    pub(crate) fn explorer_page(&self) -> ExplorerPage {
        // Expose the intended page during the gesture so the titlebar need
        // not wait for Ended or the idle timer. Keep the gesture's origin in
        // pane.page until settling, so reversing a swipe can still cancel it.
        if self.pane.axis == Some(GestureAxis::Horizontal) {
            self.pane.destination()
        } else {
            self.pane.page
        }
    }

    pub(crate) fn select_explorer_page(&mut self, page: ExplorerPage, cx: &mut Context<Self>) {
        let changed = self.explorer_page() != page;
        self.pane.settle = None;
        self.pane.axis = None;
        self.pane.select(page, motion::reduced_motion(cx));
        if changed {
            cx.emit(FilesEvent::ExplorerPageChanged);
        }
        cx.notify();
    }

    fn settle_explorer_page(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let page = self.pane.destination();
        self.select_explorer_page(page, cx);
        self.focus_explorer(window, cx);
    }

    fn scroll_explorer_page(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_page = self.explorer_page();
        if event.touch_phase == TouchPhase::Started {
            self.pane.axis = None;
        }
        let delta = event.delta.pixel_delta(px(32.0));
        // Shift + mouse wheel provides the same navigation without a trackpad.
        let (x, y) = if event.modifiers.shift && delta.x == px(0.0) {
            (f32::from(delta.y), 0.0)
        } else {
            (f32::from(delta.x), f32::from(delta.y))
        };
        let horizontal = self.pane.scroll(x, y, self.pane.viewport.width.into());
        if horizontal {
            cx.stop_propagation();
            cx.notify();
        }
        if self.explorer_page() != previous_page {
            cx.emit(FilesEvent::ExplorerPageChanged);
        }
        if event.touch_phase == TouchPhase::Ended {
            self.pane.settle = None;
            if horizontal {
                self.settle_explorer_page(window, cx);
            } else {
                self.pane.axis = None;
            }
            return;
        }
        // Mouse wheels and some Linux touchpads only emit Moved. An idle
        // timeout settles those too and releases the axis lock.
        self.pane.settle = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SETTLE_DELAY).await;
            this.update_in(cx, |this, window, cx| {
                if this.pane.axis == Some(GestureAxis::Horizontal) {
                    this.settle_explorer_page(window, cx);
                } else {
                    this.pane.axis = None;
                }
            })
            .ok();
        }));
    }

    pub(super) fn render_explorer_pane(
        &mut self,
        window: &mut Window,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.pane.slide.is_some_and(|slide| {
            motion::reduced_motion(cx) || slide.started.elapsed() >= motion::RESIZE.total()
        }) {
            self.pane.slide = None;
        }
        if self.pane.slide.is_some() {
            cx.on_next_frame(window, |_, _, cx| cx.notify());
        }
        let position = self.pane.painted_position();
        // Keep both pages mounted: virtual list offsets, expanded directories,
        // the search query, and disclosure state survive a trip to the other page.
        let files = div()
            .id("explorer-files-page")
            .debug_selector(|| "explorer-files-page".into())
            .absolute()
            .left(relative(ExplorerPage::Files.position() - position))
            .top_0()
            .size_full()
            .flex()
            .flex_col()
            .child(self.render_explorer_header(theme, cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_explorer(theme, cx)),
            );
        let activity_header = surface_chrome::toolbar(theme)
            .child(
                icons::icon(icons::BOT)
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            )
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_size(px(11.5))
                    .text_color(theme.text_muted)
                    .child("Subagents & sidechats"),
            );
        let agents = div()
            .id("explorer-agents-page")
            .debug_selector(|| "explorer-agents-page".into())
            .absolute()
            .left(relative(ExplorerPage::Agents.position() - position))
            .top_0()
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.pane.focus)
            .child(activity_header)
            .child(
                div()
                    .id("explorer-activity")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(self.render_sections(theme, cx)),
            );
        let weak = cx.entity().downgrade();
        div()
            .id("explorer-pages")
            .debug_selector(|| "explorer-pages".into())
            .size_full()
            .relative()
            .overflow_hidden()
            .child(agents)
            .child(files)
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                    move |bounds, hitbox, window, cx| {
                        weak.update(cx, |this, cx| {
                            if this.pane.viewport != bounds.size {
                                this.pane.viewport = bounds.size;
                                cx.on_next_frame(window, |_, _, cx| cx.notify());
                            }
                        })
                        .ok();
                        // Capture before the virtual tree and section scrollers:
                        // they otherwise interpret horizontal wheels as vertical.
                        window.on_mouse_event(
                            move |event: &ScrollWheelEvent, phase, window, cx| {
                                if phase == gpui::DispatchPhase::Capture
                                    && hitbox.should_handle_scroll(window)
                                {
                                    weak.update(cx, |this, cx| {
                                        this.scroll_explorer_page(event, window, cx)
                                    })
                                    .ok();
                                }
                            },
                        );
                    },
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, ScrollDelta, TestAppContext, point, size};

    use crate::state::AppState;

    #[gpui::test]
    fn gestures_lock_the_axis_clamp_and_snap(cx: &mut TestAppContext) {
        let files = cx.new(|cx| {
            let state = cx.new(|_| AppState::new());
            FilesSurface::new_explorer(state, "chat".into(), false, cx)
        });
        files.update(cx, |files, _| {
            let pane = &mut files.pane;
            pane.viewport = size(px(286.0), px(700.0));
            assert!(!pane.scroll(2.0, -30.0, 286.0));
            assert!(!pane.scroll(-100.0, -1.0, 286.0));
            assert_eq!(pane.position, 1.0, "vertical gestures stay vertical");
            pane.axis = None;
            assert!(pane.scroll(12.0, 1.0, 286.0));
            assert_eq!(
                pane.destination(),
                ExplorerPage::Files,
                "small drift snaps back"
            );
            assert!(pane.scroll(60.0, 2.0, 286.0));
            assert_eq!(pane.destination(), ExplorerPage::Agents);
            assert!(pane.scroll(1000.0, 0.0, 286.0));
            assert_eq!(pane.position, 0.0);
            pane.select(ExplorerPage::Agents, true);
            assert!(pane.slide.is_none(), "reduced motion snaps immediately");
            pane.axis = None;
            assert!(pane.scroll(-65.0, 0.0, 286.0));
            assert_eq!(pane.destination(), ExplorerPage::Files);
            pane.select(ExplorerPage::Files, false);
            let before_reversal = pane.painted_position();
            pane.select(ExplorerPage::Agents, false);
            assert!((pane.painted_position() - before_reversal).abs() < 0.02);
        });
    }

    #[gpui::test]
    fn horizontal_wheel_pages_over_lists_and_preserves_search(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            motion::set_reduced_motion(cx, true);
        });
        let (files, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| AppState::new());
            FilesSurface::new_explorer(state, "chat".into(), false, cx)
        });
        cx.simulate_resize(size(px(286.0), px(650.0)));
        cx.update(|window, cx| {
            window.activate_window();
            window.draw(cx).clear();
        });
        let search = cx.debug_bounds("files-search").unwrap().center();
        cx.simulate_click(search, Default::default());
        cx.simulate_input("src/main.rs");
        let bounds = cx.debug_bounds("explorer-pages").unwrap();
        let position = bounds.center();
        for (touch_phase, x, y) in [
            (TouchPhase::Started, 0.0, -40.0),
            (TouchPhase::Moved, 90.0, -2.0),
            (TouchPhase::Ended, 0.0, 0.0),
        ] {
            cx.simulate_event(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(x), px(y))),
                touch_phase,
                ..Default::default()
            });
        }
        files.read_with(cx, |files, _| {
            assert_eq!(files.explorer_page(), ExplorerPage::Files)
        });
        let selected_pages = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let observed_pages = selected_pages.clone();
        let _subscription = cx.update(|_, cx| {
            cx.subscribe(&files, move |files, event, cx| {
                if matches!(event, FilesEvent::ExplorerPageChanged) {
                    observed_pages
                        .borrow_mut()
                        .push(files.read(cx).explorer_page());
                }
            })
        });
        // Reversing before release cancels the intended page immediately,
        // including the notification that repaints the shell's highlight.
        for (touch_phase, x, expected) in [
            (TouchPhase::Started, 90.0, ExplorerPage::Agents),
            (TouchPhase::Moved, -90.0, ExplorerPage::Files),
        ] {
            cx.simulate_event(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(x), px(0.0))),
                touch_phase,
                ..Default::default()
            });
            files.read_with(cx, |files, _| assert_eq!(files.explorer_page(), expected));
            assert_eq!(selected_pages.borrow().last(), Some(&expected));
        }
        cx.simulate_event(ScrollWheelEvent {
            position,
            touch_phase: TouchPhase::Ended,
            ..Default::default()
        });
        for x in [90.0, -90.0] {
            let expected = if x > 0.0 {
                ExplorerPage::Agents
            } else {
                ExplorerPage::Files
            };
            cx.simulate_event(ScrollWheelEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(x), px(0.0))),
                touch_phase: TouchPhase::Started,
                ..Default::default()
            });
            files.read_with(cx, |files, _| assert_eq!(files.explorer_page(), expected));
            assert_eq!(selected_pages.borrow().last(), Some(&expected));
            cx.simulate_event(ScrollWheelEvent {
                position,
                touch_phase: TouchPhase::Ended,
                ..Default::default()
            });
            cx.update(|window, cx| window.draw(cx).clear());
            files.read_with(cx, |files, cx| {
                assert_eq!(files.explorer_page(), expected);
                assert_eq!(files.search.read(cx).text(), "src/main.rs");
            });
            let selector = if x > 0.0 {
                "explorer-agents-page"
            } else {
                "explorer-files-page"
            };
            assert_eq!(cx.debug_bounds(selector).unwrap(), bounds);
        }
        cx.update(|window, cx| {
            use gpui::Focusable;
            assert!(files.read(cx).search.focus_handle(cx).is_focused(window));
        });
    }
}
