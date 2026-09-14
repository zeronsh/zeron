//! Per-range hit targets share the text's shaped geometry, including wrapped links.
use super::{
    links::*,
    render::{LinkUi, activate_link, range_rects},
};
use crate::{icons, popover, theme::Theme};
use gpui::{
    AnyElement, App, AvailableSpace, Bounds, ClickEvent, DispatchPhase, Element, ElementId,
    FocusHandle, GlobalElementId, InspectorElementId, LayoutId, MouseButton, Pixels, Point, Role,
    ScrollWheelEvent, SharedString, TextLayout, Window, div, prelude::*, px,
};
use std::{
    cell::{Cell, RefCell},
    ops::Range,
    rc::Rc,
};

#[cfg(feature = "browser-fixture")]
thread_local! {
    static FIXTURE_LINKS: RefCell<std::collections::HashMap<String, (Point<Pixels>, FocusHandle)>> = RefCell::default();
}
#[cfg(feature = "browser-fixture")]
pub(crate) fn fixture_link(target: &str) -> Option<(Point<Pixels>, FocusHandle)> {
    FIXTURE_LINKS.with(|links| links.borrow().get(target).cloned())
}

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
    menu_focus: [FocusHandle; 4],
    menu_focus_pending: Rc<Cell<bool>>,
    menu: Rc<RefCell<Option<(usize, Point<Pixels>)>>>,
    bounds: Bounds<Pixels>,
    epoch: Rc<Cell<u64>>,
    dismissed: Rc<Cell<bool>>,
    tooltip_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    focused: Option<usize>,
}
impl IntoElement for LinkRanges {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}
impl Element for LinkRanges {
    type RequestLayoutState = ();
    type PrepaintState = (
        Vec<AnyElement>,
        Rc<RefCell<Option<(usize, Point<Pixels>)>>>,
        Rc<Cell<u64>>,
        Rc<Cell<bool>>,
        Rc<Cell<Option<Bounds<Pixels>>>>,
    );
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
                    menu_focus_pending: Rc::default(),
                    menu: Rc::default(),
                    bounds,
                    epoch: Rc::default(),
                    dismissed: Rc::default(),
                    tooltip_bounds: Rc::default(),
                    focused: None,
                });
            if state.bounds != bounds {
                state.menu.borrow_mut().take();
                state.epoch.set(state.epoch.get().wrapping_add(1));
                state.dismissed.set(true);
            }
            let focused = state
                .focus
                .iter()
                .position(|focus| focus.is_focused(window));
            if focused != state.focused {
                state.dismissed.set(false);
            }
            state.focused = focused;
            state.bounds = bounds;
            state.tooltip_bounds.set(None);
            #[cfg(all(test, target_os = "linux"))]
            rendered_tests::TOOLTIP_BOUNDS.with(|bounds| {
                *bounds.borrow_mut() = Some(state.tooltip_bounds.clone());
            });
            let theme = Theme::of(cx).clone();
            let mut overlays = Vec::new();
            for (index, (range, target)) in self.links.iter().enumerate() {
                for (part, rect) in range_rects(&self.layout, range, 0., 0.)
                    .into_iter()
                    .enumerate()
                {
                    #[cfg(feature = "browser-fixture")]
                    if part == 0 {
                        FIXTURE_LINKS.with(|links| {
                            links.borrow_mut().insert(
                                target.original.clone(),
                                (rect.center(), state.focus[index].clone()),
                            )
                        });
                    }
                    let menu = state.menu.clone();
                    let keyboard_menu = menu.clone();
                    let focus = state.focus[index].clone();
                    let click_target = target.clone();
                    let destination = target.original.clone();
                    let tooltip_bounds = state.tooltip_bounds.clone();
                    let click_ui = self.ui.clone();
                    let menu_focus_pending = state.menu_focus_pending.clone();
                    let pointer_focus_pending = menu_focus_pending.clone();
                    let hit = div()
                        .id(format!("link-{index}-{part}-{}", state.epoch.get()))
                        // Removing the builder cancels both visible tooltips
                        // and GPUI's delayed show task while the menu owns input.
                        .when(state.menu.borrow().is_none(), |hit| {
                            hit.hoverable_tooltip(move |_, cx| {
                                let url = destination.clone();
                                let bounds = tooltip_bounds.clone();
                                cx.new(|_| super::link_destination::Destination(url, bounds))
                                    .into()
                            })
                        })
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
                                    LinkAction::Primary,
                                    click_ui.as_ref(),
                                    window,
                                    cx,
                                );
                            }
                        })
                        .on_mouse_down(MouseButton::Right, move |event, window, cx| {
                            *menu.borrow_mut() = Some((index, event.position));
                            pointer_focus_pending.set(true);
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
                                    menu_focus_pending.set(true);
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
            if state.menu.borrow().is_none() && !state.dismissed.get() {
                if let Some(index) = focused {
                    if let Some(rect) =
                        range_rects(&self.layout, &self.links[index].0, 0., 0.).first()
                    {
                        let card = super::link_destination::destination_card(
                            &state.targets[index].original,
                            state.tooltip_bounds.clone(),
                            window,
                            cx,
                        );
                        let mut popup = crate::popover::menu_at(
                            "focused-link-destination",
                            rect.bottom_left(),
                            card,
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
                }
            }
            if let Some((index, position)) = *state.menu.borrow() {
                let menu = state.menu.clone();
                let dismiss_menu = state.menu.clone();
                let return_focus = state.focus[index].clone();
                let menu_focus = state.menu_focus.clone();
                let pending = state.menu_focus_pending.clone();
                let initial_focus = state.menu_focus[0].clone();
                let mut card = crate::popover::popover_card(&theme)
                    .child(
                        gpui::canvas(
                            move |_, window, cx| {
                                // Claim focus only when the deferred menu mounts. The
                                // shell recovers focus from handles that are not mounted.
                                if pending.replace(false) {
                                    window.focus(&initial_focus, cx);
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .w(px(0.))
                        .h(px(0.)),
                    )
                    .id("link-actions")
                    .on_key_down(move |event, window, cx| {
                        match event.keystroke.key.as_str() {
                            "escape" => {
                                dismiss_menu.borrow_mut().take();
                                window.focus(&return_focus, cx);
                                window.refresh();
                            }
                            "tab" | "down" | "up" => {
                                let current = menu_focus
                                    .iter()
                                    .position(|focus| focus.is_focused(window))
                                    .unwrap_or(0);
                                let backwards = event.keystroke.key == "up"
                                    || (event.keystroke.key == "tab"
                                        && event.keystroke.modifiers.shift);
                                let next = if backwards {
                                    (current + menu_focus.len() - 1) % menu_focus.len()
                                } else {
                                    (current + 1) % menu_focus.len()
                                };
                                window.focus(&menu_focus[next], cx);
                            }
                            _ => return,
                        }
                        cx.stop_propagation();
                    })
                    .w(px(260.))
                    .flex()
                    .flex_col()
                    .on_mouse_down_out(move |_, window, _| {
                        menu.borrow_mut().take();
                        window.refresh();
                    });
                for (action_ix, (action, label, icon)) in [
                    (LinkAction::Internal, "Open in Zeron", icons::GLOBE),
                    (
                        LinkAction::External,
                        "Open in external browser",
                        icons::ARROW_UP_RIGHT,
                    ),
                    (LinkAction::Copy, "Copy link address", icons::COPY),
                ]
                .into_iter()
                .enumerate()
                {
                    let target = state.targets[index].clone();
                    let ui = self.ui.clone();
                    let menu = state.menu.clone();
                    let enabled = action == LinkAction::Copy || target.navigation.is_ok();
                    card = card.child(
                        popover::menu_row(
                            &theme,
                            false,
                            format!("{}-link-{index}-action-{action_ix}", self.id),
                        )
                        .id(label)
                        .child(icons::icon(icon).size(px(16.)).text_color(theme.text_muted))
                        .child(label)
                        .track_focus(&state.menu_focus[action_ix])
                        .role(Role::Button)
                        .aria_label(label)
                        .when(!enabled, |el| el.opacity(0.45))
                        .focus_visible(|s| s.bg(crate::theme::card_selected_bg()))
                        .on_click(move |_, window, cx| {
                            if enabled {
                                activate_link(target.clone(), action, ui.as_ref(), window, cx);
                            }
                            menu.borrow_mut().take();
                            window.refresh();
                        }),
                    );
                }
                let open_in_zeron = crate::settings::current(cx).open_web_links_in_zeron;
                let menu = state.menu.clone();
                card = card.child(popover::menu_separator()).child(
                    popover::menu_row(
                        &theme,
                        false,
                        format!("{}-link-{index}-default-destination", self.id),
                    )
                    .id("Open links in Zeron")
                    .child(div().w(px(16.)).flex_none().when(open_in_zeron, |el| {
                        el.child(
                            icons::icon(icons::CHECK)
                                .size(px(16.))
                                .text_color(theme.text_muted),
                        )
                    }))
                    .child("Open links in Zeron")
                    .track_focus(&state.menu_focus[3])
                    .role(Role::Button)
                    .aria_label(if open_in_zeron {
                        "Open links in Zeron, checked"
                    } else {
                        "Open links in Zeron, unchecked"
                    })
                    .focus_visible(|s| s.bg(crate::theme::card_selected_bg()))
                    .on_click(move |_, window, cx| {
                        crate::settings::update(
                            crate::settings::SavePolicy::Immediate,
                            cx,
                            |settings| {
                                settings.open_web_links_in_zeron = !open_in_zeron;
                            },
                        );
                        menu.borrow_mut().take();
                        cx.refresh_windows();
                        window.refresh();
                    }),
                );
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
            (
                (
                    overlays,
                    state.menu.clone(),
                    state.epoch.clone(),
                    state.dismissed.clone(),
                    state.tooltip_bounds.clone(),
                ),
                state,
            )
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
        let epoch = paint.2.clone();
        let dismissed = paint.3.clone();
        let tooltip_bounds = paint.4.clone();
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, _| {
            if phase == DispatchPhase::Capture
                && !tooltip_bounds
                    .get()
                    .is_some_and(|rect| rect.contains(&event.position))
            {
                menu.borrow_mut().take();
                epoch.set(epoch.get().wrapping_add(1));
                dismissed.set(true);
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
    thread_local! {
        pub(super) static TOOLTIP_BOUNDS: RefCell<Option<Rc<Cell<Option<Bounds<Pixels>>>>>> = RefCell::default();
    }
    fn draw_has_tooltip(window: &mut Window, cx: &mut App) -> bool {
        TOOLTIP_BOUNDS.with(|bounds| {
            if let Some(bounds) = bounds.borrow().as_ref() {
                bounds.set(None);
            }
        });
        window.refresh();
        let _ = window.draw(cx);
        TOOLTIP_BOUNDS.with(|bounds| bounds.borrow().as_ref().is_some_and(|b| b.get().is_some()))
    }
    fn key(window: &mut Window, key: &str, cx: &mut App) {
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
    struct Fixture {
        markdown: String,
        width: f32,
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
            let tree = super::super::parser::parse_full(&self.markdown);
            div()
                .w(px(self.width))
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
    fn context_menu_cancels_visible_and_pending_hover_tooltips() {
        let dir = tempfile::tempdir().unwrap();
        gpui_platform::headless().run(move |cx| {
            cx.set_global(Theme::dark());
            crate::settings::init(crate::settings::UiSettings::default(), dir.path(), cx);
            // Keep the headless event loop alive between scenario windows.
            cx.open_window(Default::default(), |_, cx| cx.new(|_| gpui::Empty))
                .unwrap();
            cx.spawn(async move |cx| {
                for visible in [true, false] {
                    for keyboard in [false, true] {
                        let activated = Rc::new(RefCell::new(Vec::new()));
                        let window = cx.update(|cx| {
                            cx.open_window(Default::default(), |_, cx| {
                                cx.new(|_| Fixture {
                                    markdown: "[Docs](https://example.com/docs)".into(),
                                    width: 320.,
                                    activated: activated.clone(),
                                })
                            })
                            .unwrap()
                        });
                        let position = cx
                            .update_window(window.into(), |_, window, cx| {
                                draw_has_tooltip(window, cx);
                                let (_, layout, _) =
                                    super::super::render::selection_test_snapshot("link-fixture:0");
                                let position = range_rects(&layout, &(0..4), 0., 0.)[0].center();
                                window.dispatch_event(
                                    gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                                        position,
                                        ..Default::default()
                                    }),
                                    cx,
                                );
                                draw_has_tooltip(window, cx);
                                position
                            })
                            .unwrap();
                        if visible {
                            cx.background_executor()
                                .timer(std::time::Duration::from_millis(650))
                                .await;
                            cx.update_window(window.into(), |_, window, cx| {
                                assert!(
                                    draw_has_tooltip(window, cx),
                                    "hover should show the destination"
                                );
                            })
                            .unwrap();
                        }
                        cx.update_window(window.into(), |_, window, cx| {
                            if keyboard {
                                window.focus_next(cx);
                                draw_has_tooltip(window, cx);
                                key(window, "shift-f10", cx);
                            } else {
                                window.dispatch_event(
                                    gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                                        button: MouseButton::Right,
                                        position,
                                        click_count: 1,
                                        ..Default::default()
                                    }),
                                    cx,
                                );
                                window.dispatch_event(
                                    gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                                        button: MouseButton::Right,
                                        position,
                                        click_count: 1,
                                        ..Default::default()
                                    }),
                                    cx,
                                );
                            }
                            assert!(
                                !draw_has_tooltip(window, cx),
                                "menu must hide an already visible tooltip"
                            );
                        })
                        .unwrap();
                        // Leave the pointer over the link beyond the show delay.
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(650))
                            .await;
                        cx.update_window(window.into(), |_, window, cx| {
                            assert!(
                                !draw_has_tooltip(window, cx),
                                "pending hover must not appear over the menu"
                            );
                            let before = crate::settings::current(cx).open_web_links_in_zeron;
                            key(window, "down", cx);
                            key(window, "down", cx);
                            key(window, "down", cx);
                            key(window, "enter", cx);
                            assert_ne!(
                                crate::settings::current(cx).open_web_links_in_zeron,
                                before,
                                "the fourth menu row toggles the default destination"
                            );
                            assert!(activated.borrow().is_empty());
                            window.remove_window();
                        })
                        .unwrap();
                    }
                }
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
    #[test]
    fn keyboard_visits_each_range_and_opens_the_link_menu() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let activated = Rc::new(RefCell::new(Vec::new()));
            let log = activated.clone();
            let window = cx
                .open_window(Default::default(), |_, cx| {
                    cx.new(|_| Fixture {
                        activated,
                        width: 320.,
                        markdown:
                            "[first](https://example.com/one) and [second](https://example.org/two)"
                                .into(),
                    })
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
    #[test]
    fn rendered_truncation_resizes_and_selects_the_original_url() {
        gpui_platform::headless().run(|cx| {
            cx.set_global(Theme::dark());
            let url = format!("https://example.com/{}", "long-segment-🙂/".repeat(20));
            let markdown = format!("[{url}]({url})");
            let window = cx
                .open_window(Default::default(), |_, cx| {
                    cx.new(|_| Fixture {
                        activated: Rc::default(),
                        width: 180.,
                        markdown,
                    })
                })
                .unwrap();
            let view = window.entity(cx).unwrap();
            for width in [180., 420., 100.] {
                view.update(cx, |view, cx| {
                    view.width = width;
                    cx.notify();
                });
                cx.update_window(window.into(), |_, window, cx| {
                    window.refresh();
                    let _ = window.draw(cx);
                    let (original, layout, offsets) =
                        super::super::render::selection_test_snapshot("link-fixture:0");
                    assert_eq!(original.as_ref(), url);
                    let offsets = offsets.unwrap();
                    assert_eq!(offsets.omissions.len(), 1);
                    assert!(layout.bounds().size.width <= px(width));
                    assert!(layout.bounds().size.height <= px(23.));
                    let shown_end = offsets.displayed(url.len());
                    let start =
                        layout.position_for_index(0).unwrap() + gpui::point(px(0.1), px(8.));
                    let end = layout.position_for_index(shown_end).unwrap()
                        + gpui::point(px(0.1), px(8.));
                    window.dispatch_event(
                        gpui::PlatformInput::MouseDown(gpui::MouseDownEvent {
                            button: MouseButton::Left,
                            position: start,
                            click_count: 1,
                            ..Default::default()
                        }),
                        cx,
                    );
                    window.refresh();
                    let _ = window.draw(cx);
                    window.dispatch_event(
                        gpui::PlatformInput::MouseMove(gpui::MouseMoveEvent {
                            position: end,
                            pressed_button: Some(MouseButton::Left),
                            ..Default::default()
                        }),
                        cx,
                    );
                    window.dispatch_event(
                        gpui::PlatformInput::MouseUp(gpui::MouseUpEvent {
                            button: MouseButton::Left,
                            position: end,
                            click_count: 1,
                            ..Default::default()
                        }),
                        cx,
                    );
                    assert_eq!(
                        super::super::selection::selected_text().as_deref(),
                        Some(url.as_str())
                    );
                    assert!(view.read(cx).activated.borrow().is_empty());
                    super::super::selection::clear_if_owner("link-fixture:0");
                })
                .unwrap();
            }
            view.update(cx, |view, cx| {
                view.width = 220.;
                view.markdown =
                    format!("| [{url}]({url}) | Notes |\n| --- | --- |\n| short | cell |");
                cx.notify();
            });
            cx.update_window(window.into(), |_, window, cx| {
                window.refresh();
                let _ = window.draw(cx);
                let (original, layout, offsets) =
                    super::super::render::selection_test_snapshot("link-fixture:0");
                assert_eq!(original.as_ref(), url);
                assert_eq!(offsets.unwrap().omissions.len(), 1);
                assert!(layout.bounds().size.width < px(220.));
                assert!(layout.bounds().size.height <= px(23.));
            })
            .unwrap();
            cx.spawn(async move |cx| {
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    }
}
