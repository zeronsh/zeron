//! Per-range hit targets share the text's shaped geometry, including wrapped links.
use super::{
    links::*,
    render::{LinkUi, activate_link, range_rects},
};
use crate::theme::Theme;
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ClickEvent, DispatchPhase, Element, ElementId,
    FocusHandle, GlobalElementId, InspectorElementId, LayoutId, MouseButton, Pixels, Point, Role,
    ScrollWheelEvent, SharedString, TextLayout, Window, div, prelude::*, px,
};
use std::{cell::RefCell, ops::Range, rc::Rc};

pub struct LinkRanges {
    pub id: SharedString,
    pub child: AnyElement,
    pub layout: TextLayout,
    pub links: Vec<(Range<usize>, LinkTarget)>,
    pub ui: Option<LinkUi>,
}
struct Interaction {
    targets: Vec<LinkTarget>,
    focus: Vec<FocusHandle>,
    menu_focus: [FocusHandle; 3],
    menu: Rc<RefCell<Option<(usize, Point<Pixels>)>>>,
    bounds: Bounds<Pixels>,
}
impl IntoElement for LinkRanges {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for LinkRanges {
    type RequestLayoutState = ();
    type PrepaintState = (Vec<AnyElement>, Rc<RefCell<Option<(usize, Point<Pixels>)>>>);
    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone().into())
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
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        id: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        self.child.prepaint(window, cx);
        window.with_element_state::<Interaction, _>(id.unwrap(), |previous, window| {
            let targets: Vec<_> = self.links.iter().map(|(_, t)| t.clone()).collect();
            let mut state = previous
                .filter(|s| s.targets == targets)
                .unwrap_or_else(|| Interaction {
                    focus: targets
                        .iter()
                        .map(|_| cx.focus_handle().tab_stop(true))
                        .collect(),
                    targets,
                    menu_focus: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
                    menu: Rc::default(),
                    bounds,
                });
            if state.bounds != bounds {
                state.menu.borrow_mut().take();
            }
            state.bounds = bounds;
            let theme = Theme::of(cx).clone();
            let mut overlays = Vec::new();
            for (index, (range, target)) in self.links.iter().enumerate() {
                for (part, rect) in range_rects(&self.layout, range, 0., 0.)
                    .into_iter()
                    .enumerate()
                {
                    let menu = state.menu.clone();
                    let keyboard_menu = menu.clone();
                    let focus = state.focus[index].clone();
                    let click_target = target.clone();
                    let click_ui = self.ui.clone();
                    let menu_focus = state.menu_focus[0].clone();
                    let hit = div()
                        .id(format!("link-{index}-{part}"))
                        .w(rect.size.width)
                        .h(rect.size.height)
                        .cursor_pointer()
                        .role(Role::Link)
                        .aria_label(target.label.clone())
                        .when(part == 0, |el| el.track_focus(&focus))
                        .focus_visible(|s| {
                            s.bg(theme.selection).border_1().border_color(theme.accent)
                        })
                        .on_click(move |event, window, cx| {
                            if click_is_activation(event)
                                && (matches!(event, ClickEvent::Keyboard(_))
                                    || super::selection::selected_text().is_none())
                            {
                                activate_link(
                                    click_target.clone(),
                                    LinkAction::Internal,
                                    click_ui.as_ref(),
                                    window,
                                    cx,
                                );
                            }
                        })
                        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                            *menu.borrow_mut() = Some((index, event.position));
                            cx.stop_propagation();
                            window.refresh();
                        })
                        .on_key_down(move |event, window, cx| {
                            match event.keystroke.key.as_str() {
                                "tab" => {
                                    if event.keystroke.modifiers.shift {
                                        window.focus_prev(cx);
                                    } else {
                                        window.focus_next(cx);
                                    }
                                }
                                "f10" if event.keystroke.modifiers.shift => {
                                    *keyboard_menu.borrow_mut() = Some((index, rect.bottom_left()));
                                    window.focus(&menu_focus, cx);
                                    window.refresh();
                                }
                                "escape" => {
                                    keyboard_menu.borrow_mut().take();
                                    window.refresh();
                                }
                                _ => {
                                    cx.propagate();
                                    return;
                                }
                            }
                            cx.stop_propagation();
                        });
                    let mut hit = hit.into_any_element();
                    hit.prepaint_as_root(
                        rect.origin,
                        rect.size.map(AvailableSpace::Definite),
                        window,
                        cx,
                    );
                    overlays.push(hit);
                }
            }
            if let Some((index, position)) = *state.menu.borrow() {
                let menu = state.menu.clone();
                let dismiss_menu = state.menu.clone();
                let return_focus = state.focus[index].clone();
                let mut card = crate::popover::popover_card(&theme)
                    .id("link-actions")
                    .on_key_down(move |event, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => {
                                dismiss_menu.borrow_mut().take();
                                window.focus(&return_focus, cx);
                                window.refresh();
                            }
                            "tab" if event.keystroke.modifiers.shift => window.focus_prev(cx),
                            "tab" | "down" => window.focus_next(cx),
                            "up" => window.focus_prev(cx),
                            _ => return,
                        }
                        cx.stop_propagation();
                    })
                    .w(px(230.))
                    .flex()
                    .flex_col()
                    .on_mouse_down_out(move |_, window, _| {
                        menu.borrow_mut().take();
                        window.refresh();
                    });
                for (action_ix, (action, label)) in [
                    (LinkAction::Internal, "Open in Zeron"),
                    (LinkAction::External, "Open in external browser"),
                    (LinkAction::Copy, "Copy link address"),
                ]
                .into_iter()
                .enumerate()
                {
                    let target = state.targets[index].clone();
                    let ui = self.ui.clone();
                    let menu = state.menu.clone();
                    let enabled = action == LinkAction::Copy || target.navigation.is_ok();
                    card = card.child(
                        div()
                            .id(label)
                            .px(px(10.))
                            .py(px(7.))
                            .child(label)
                            .track_focus(&state.menu_focus[action_ix])
                            .role(Role::Button)
                            .aria_label(label)
                            .when(!enabled, |el| el.opacity(0.45))
                            .hover(|s| s.bg(theme.selection))
                            .focus_visible(|s| s.bg(theme.selection))
                            .on_click(move |_, window, cx| {
                                if enabled {
                                    activate_link(target.clone(), action, ui.as_ref(), window, cx);
                                }
                                menu.borrow_mut().take();
                                window.refresh();
                            }),
                    );
                }
                let mut popup = crate::popover::menu_at(
                    "transcript-link-actions",
                    position,
                    card.into_any_element(),
                    None,
                );
                popup.prepaint_as_root(
                    bounds.origin,
                    window.viewport_size().map(AvailableSpace::Definite),
                    window,
                    cx,
                );
                overlays.push(popup);
            }
            ((overlays, state.menu.clone()), state)
        })
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        paint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let menu = paint.1.clone();
        window.on_mouse_event(move |_: &ScrollWheelEvent, phase, window, _| {
            if phase == DispatchPhase::Capture && menu.borrow_mut().take().is_some() {
                window.refresh();
            }
        });
        self.child.paint(window, cx);
        for overlay in &mut paint.0 {
            overlay.paint(window, cx);
        }
    }
}
fn click_is_activation(event: &ClickEvent) -> bool {
    match event {
        ClickEvent::Mouse(event) => {
            event.down.button == MouseButton::Left
                && event.down.click_count == 1
                && (event.up.position - event.down.position).magnitude() <= 4.
        }
        ClickEvent::Keyboard(_) | ClickEvent::Touch(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{MouseClickEvent, MouseDownEvent, MouseUpEvent, TestAppContext, point};
    #[test]
    fn selection_drags_and_secondary_clicks_do_not_navigate() {
        let mut event = MouseClickEvent {
            down: MouseDownEvent {
                button: MouseButton::Left,
                click_count: 1,
                ..Default::default()
            },
            up: MouseUpEvent::default(),
        };
        assert!(click_is_activation(&ClickEvent::Mouse(event.clone())));
        event.up.position = point(px(20.), px(0.));
        assert!(!click_is_activation(&ClickEvent::Mouse(event.clone())));
        event.up.position = event.down.position;
        event.down.button = MouseButton::Right;
        assert!(!click_is_activation(&ClickEvent::Mouse(event.clone())));
        event.down.button = MouseButton::Left;
        event.down.click_count = 2;
        assert!(!click_is_activation(&ClickEvent::Mouse(event)));
    }
    #[gpui::test]
    fn all_actions_keep_the_destination(cx: &mut TestAppContext) {
        let window = cx.add_window(|_, _| gpui::Empty);
        window
            .update(cx, |_, window, cx| {
                let target = LinkTarget::new(
                    "A misleading label",
                    "https://example.com/full?query=yes#fragment",
                );
                activate_link(target.clone(), LinkAction::Copy, None, window, cx);
                assert_eq!(
                    cx.read_from_clipboard().unwrap().text().as_deref(),
                    Some(target.original.as_str())
                );
                for action in [LinkAction::Internal, LinkAction::External] {
                    let seen = Rc::new(RefCell::new(None));
                    let captured = seen.clone();
                    let ui = LinkUi {
                        source_session: Some("parent".into()),
                        handler: Rc::new(move |a, _, _| {
                            *captured.borrow_mut() =
                                Some((a.target.clone(), a.action, a.source_session.clone()));
                            LinkOutcome::Rejected
                        }),
                    };
                    activate_link(target.clone(), action, Some(&ui), window, cx);
                    assert_eq!(
                        *seen.borrow(),
                        Some((target.clone(), action, Some("parent".into())))
                    );
                }
            })
            .unwrap();
    }
}

#[cfg(all(test, target_os = "linux"))]
mod rendered_tests {
    use super::*;
    use gpui::{Context, Render};
    struct Fixture {
        activated: Rc<RefCell<Vec<LinkActivation>>>,
    }
    impl Render for Fixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let mut opts = super::super::render::RenderOptions::settled("link-fixture".into());
            let activated = self.activated.clone();
            opts.link = Some(LinkUi {
                source_session: Some("session".into()),
                handler: Rc::new(move |a, _, _| {
                    activated.borrow_mut().push(a.clone());
                    LinkOutcome::Rejected
                }),
            });
            let tree = super::super::parser::parse_full(
                "[first](https://example.com/one) and [second](https://example.org/two)",
            );
            div()
                .w(px(320.))
                .child(super::super::render::selection_frame_reset())
                .child(super::super::render::render_tree(
                    &tree,
                    &opts,
                    &Theme::of(cx).clone(),
                    window,
                    &|_| None,
                ))
        }
    }
    #[test]
    fn keyboard_visits_each_range_and_opens_the_link_menu() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let activated = Rc::new(RefCell::new(Vec::new()));
            let log = activated.clone();
            let window = cx
                .open_window(Default::default(), |_, cx| {
                    cx.new(|_| Fixture { activated })
                })
                .unwrap();
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
                window.focus_next(cx);
                window.refresh();
                let _ = window.draw(cx);
                for key in ["enter", "tab", "enter", "shift-f10"] {
                    window.dispatch_event(
                        gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                            keystroke: gpui::Keystroke::parse(key).unwrap(),
                            is_held: false,
                            prefer_character_input: false,
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                            keystroke: gpui::Keystroke::parse(key).unwrap(),
                        }),
                        cx,
                    );
                    window.refresh();
                    let _ = window.draw(cx);
                }
                assert_eq!(log.borrow().len(), 2);
                assert_eq!(log.borrow()[0].target.original, "https://example.com/one");
                assert_eq!(log.borrow()[1].target.original, "https://example.org/two");
                // Menu starts on Open in Zeron; choose the external action.
                for key in ["down", "enter"] {
                    window.dispatch_event(
                        gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                            keystroke: gpui::Keystroke::parse(key).unwrap(),
                            is_held: false,
                            prefer_character_input: false,
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        gpui::PlatformInput::KeyUp(gpui::KeyUpEvent {
                            keystroke: gpui::Keystroke::parse(key).unwrap(),
                        }),
                        cx,
                    );
                    window.refresh();
                    let _ = window.draw(cx);
                }
                assert_eq!(log.borrow().len(), 3);
                assert_eq!(log.borrow()[2].action, LinkAction::External);
                assert_eq!(log.borrow()[2].target.original, "https://example.org/two");
            })
            .unwrap();
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
}
