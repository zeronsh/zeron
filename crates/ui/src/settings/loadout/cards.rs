use std::cell::Cell;

use super::*;

#[derive(Clone)]
pub(super) struct LoadoutSlotDrag {
    pub(super) from: usize,
    label: SharedString,
    target: Cell<usize>,
}

/// Convert a hovered card half into the final slot index after removing the
/// dragged item. Empty slots are placeholders, so they resolve to the end of
/// the filled portion of the loadout.
fn reorder_destination(
    from: usize,
    over: usize,
    after: bool,
    first_empty: Option<usize>,
    slot_count: usize,
) -> usize {
    let last_filled = first_empty
        .unwrap_or(slot_count)
        .saturating_sub(1)
        .min(slot_count.saturating_sub(1));
    if first_empty.is_some_and(|first_empty| over >= first_empty) {
        return last_filled;
    }

    let insertion_point = over.saturating_add(usize::from(after));
    insertion_point
        .saturating_sub(usize::from(from < insertion_point))
        .min(last_filled)
}

fn slot_pill(theme: &Theme) -> gpui::Div {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .bg(ink(0.12))
        .text_size(px(9.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.text_muted)
}

fn fast_pill(theme: &Theme) -> gpui::Div {
    div()
        .px(px(6.0))
        .py(px(1.0))
        .rounded(px(4.0))
        .bg(theme.warning.opacity(0.16))
        .text_size(px(9.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme.warning)
}

impl LoadoutPage {
    pub(super) fn render_slot(
        &mut self,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let filled = self.loadout.slot(index).cloned();
        let first_empty = self.loadout.first_empty() == Some(index);
        let hovered = self.hover_slot == Some(index);
        let drag_over = self.drag_over == Some(index);
        let menu_open = self.slot_menu.slot() == Some(index);
        let resolved_combos = self.loadout.resolved_combos();
        let reorder_target = self.slot_drag_over.is_some_and(|(at, _)| at == index);
        let insert_after = self
            .slot_drag_over
            .is_some_and(|(at, after)| at == index && after);
        let mut card = div()
            .id(("loadout-slot", index))
            .debug_selector(move || format!("loadout-slot-{index}"))
            .role(gpui::Role::Button)
            .when(filled.is_some(), |el| {
                el.track_focus(&self.slot_focus[index])
                    .tab_index(index as isize)
            })
            .relative()
            .w(px(164.0))
            .flex_shrink_0()
            .h(px(148.0))
            .min_w(px(112.0))
            .rounded(px(10.0))
            .border_1()
            .flex()
            .flex_col()
            .px(px(12.0))
            .pt(px(12.0))
            .pb(px(10.0));

        if filled.is_some() {
            card = card
                .border_color(if reorder_target {
                    theme.accent
                } else if menu_open {
                    theme.border_strong
                } else {
                    theme.border
                })
                .bg(ink(0.03))
                .cursor_pointer()
                .hover(|s| s.bg(ink(0.05)));
        } else {
            card = card
                .border_dashed()
                .border_color(if drag_over || first_empty {
                    theme.text.opacity(0.22)
                } else {
                    theme.border
                })
                .bg(if drag_over { ink(0.04) } else { ink(0.015) });
        }

        if drag_over && self.slot_drag_over.is_none() {
            let phase =
                crate::motion::pulse_delta(&crate::motion::GRADIENT_SPIN, cx.entity_id(), cx);
            let wave = crate::motion::pulse_wave(phase);
            card = card
                .border_color(theme.accent.opacity(0.5 + 0.35 * wave))
                .bg(theme.accent.opacity(0.045 + 0.045 * wave));
        }

        card = card
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover_slot = hovered.then_some(index);
                cx.notify();
            }))
            .on_drag_move::<LoadoutModelDrag>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<LoadoutModelDrag>, _, cx| {
                    if !event.bounds.contains(&event.event.position) {
                        if this.drag_over == Some(index) {
                            this.drag_over = None;
                            cx.notify();
                        }
                        return;
                    }
                    if this.drag_over != Some(index) || this.slot_drag_over.is_some() {
                        this.slot_drag_over = None;
                        this.drag_over = Some(index);
                        cx.notify();
                    }
                },
            ))
            .on_drop::<LoadoutModelDrag>(cx.listener(
                move |this, drag: &LoadoutModelDrag, _, cx| {
                    this.drop_model(index, drag, cx);
                },
            ))
            .on_drag_move::<LoadoutSlotDrag>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<LoadoutSlotDrag>, _, cx| {
                    if !event.bounds.contains(&event.event.position) {
                        if this.drag_over == Some(index) {
                            this.drag_over = None;
                            this.slot_drag_over = None;
                            cx.notify();
                        }
                        return;
                    }
                    let payload = event.drag(cx);
                    let after = event.event.position.x
                        >= event.bounds.left() + event.bounds.size.width / 2.0;
                    let target = reorder_destination(
                        payload.from,
                        index,
                        after,
                        this.loadout.first_empty(),
                        LOADOUT_SLOTS,
                    );
                    payload.target.set(target);
                    if this.drag_over != Some(index) || this.slot_drag_over != Some((index, after))
                    {
                        this.drag_over = Some(index);
                        this.slot_drag_over = Some((index, after));
                        cx.notify();
                    }
                },
            ))
            .on_drop::<LoadoutSlotDrag>(cx.listener(
                move |this, payload: &LoadoutSlotDrag, _, cx| {
                    this.drag_over = None;
                    this.slot_drag_over = None;
                    let target = payload.target.get();
                    this.loadout.reorder(payload.from, target);
                    this.close_menus(cx);
                    this.commit(cx);
                },
            ));

        if let Some(slot) = filled {
            let (icon_path, tint) = harness_brand_icon(slot.harness);
            let drag = LoadoutSlotDrag {
                from: index,
                label: slot.label.clone().into(),
                target: Cell::new(index),
            };
            let model = self.model_for(slot.harness, &slot.model);
            let fast_label = slot_speed_enabled(&slot, model).then(|| {
                SharedString::from(
                    model
                        .and_then(speed_option)
                        .map(speed_option_label)
                        .unwrap_or("Fast"),
                )
            });
            let shortcut_badge = slot
                .shortcut
                .as_deref()
                .filter(|shortcut| !shortcut.is_empty())
                .and_then(|_| resolved_combos.get(index))
                .filter(|combo| !combo.is_empty())
                .map(|combo| SharedString::from(crate::settings::badge_combo(combo)));
            let effort = slot
                .reasoning
                .map(reasoning_label)
                .unwrap_or("High")
                .to_ascii_uppercase();
            let show_clear = hovered || menu_open;
            let page = cx.entity().downgrade();
            card = card
                .on_drag(drag, move |payload, cursor_offset, window, cx| {
                    let _ = page.update(cx, |page, cx| {
                        if page.recording {
                            page.set_recording(false, window, cx);
                        }
                        page.close_menus(cx);
                    });
                    cx.stop_propagation();
                    let label = payload.label.clone();
                    cx.new(|_| LoadoutDragGhost {
                        label,
                        cursor_offset,
                    })
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    if cx.has_active_drag() {
                        cx.stop_propagation();
                        return;
                    }
                    this.gear_open = false;
                    this.open_slot_menu(index, window, cx);
                }))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if this.recording || this.slot_menu.slot().is_some() {
                        return;
                    }
                    let key = event.keystroke.key.as_str();
                    let destination = match key {
                        "left" if index > 0 && this.loadout.slot(index - 1).is_some() => {
                            Some(index - 1)
                        }
                        "right"
                            if index + 1 < LOADOUT_SLOTS
                                && this.loadout.slot(index + 1).is_some() =>
                        {
                            Some(index + 1)
                        }
                        "enter" | "space" => {
                            cx.stop_propagation();
                            this.open_slot_menu(index, window, cx);
                            return;
                        }
                        _ => None,
                    };
                    if let Some(destination) = destination {
                        this.loadout.reorder(index, destination);
                        window.focus(&this.slot_focus[destination], cx);
                        this.close_menus(cx);
                        this.commit(cx);
                        cx.stop_propagation();
                    }
                }))
                .when(shortcut_badge.is_some() || fast_label.is_some(), |el| {
                    el.child(
                        div()
                            .absolute()
                            .bottom(px(7.0))
                            .left(px(28.0))
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .when_some(shortcut_badge, |el, badge| {
                                el.child(
                                    slot_pill(theme)
                                        .id(("loadout-shortcut-badge", index))
                                        .debug_selector(move || {
                                            format!("loadout-shortcut-badge-{index}")
                                        })
                                        .child(badge),
                                )
                            })
                            .when_some(fast_label, |el, label| {
                                el.child(
                                    fast_pill(theme)
                                        .id(("loadout-fast", index))
                                        .debug_selector(move || format!("loadout-fast-{index}"))
                                        .child(label),
                                )
                            }),
                    )
                })
                .when(index == 0, |el| {
                    el.child(
                        slot_pill(theme)
                            .absolute()
                            .top(px(8.0))
                            .left(px(8.0))
                            .child(SharedString::from("DEFAULT")),
                    )
                })
                .when(show_clear, |el| {
                    el.child(
                        div()
                            .id(("loadout-clear", index))
                            .absolute()
                            .top(px(6.0))
                            .right(px(6.0))
                            .size(px(18.0))
                            .rounded(px(4.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor_pointer()
                            .hover(|s| s.bg(ink(0.1)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.remove_slot(index, cx);
                            }))
                            .child(
                                icon(icons::CLOSE)
                                    .size(px(11.0))
                                    .text_color(theme.text_muted),
                            ),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .child(
                            icon(icon_path)
                                .size(px(22.0))
                                .text_color(tint.unwrap_or(theme.text)),
                        )
                        .child(
                            div()
                                .mt(px(10.0))
                                .text_size(crate::typography::ui_rems(13.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text)
                                .child(SharedString::from(slot.label.clone())),
                        )
                        .child(
                            div()
                                .mt(px(2.0))
                                .text_size(px(10.0))
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(theme.text_muted.opacity(0.7))
                                .child(SharedString::from(effort)),
                        ),
                );
            card = card.child(
                div().absolute().bottom(px(8.0)).left(px(10.0)).child(
                    icon(icons::DRAG_HANDLE)
                        .size(px(12.0))
                        .text_color(theme.text_muted.opacity(0.45)),
                ),
            );
            if reorder_target {
                card = card.child(
                    div()
                        .absolute()
                        .debug_selector(move || format!("loadout-slot-insertion-{index}"))
                        .when(insert_after, |el| el.right(px(-4.0)))
                        .when(!insert_after, |el| el.left(px(-4.0)))
                        .top(px(10.0))
                        .bottom(px(10.0))
                        .w(px(2.0))
                        .rounded(px(1.0))
                        .bg(theme.accent),
                );
            }
        } else {
            card = card.child(indicator::vacant_indicator(
                index,
                first_empty,
                drag_over,
                theme,
                cx,
            ));
        }

        card = card.child(
            div()
                .absolute()
                .bottom(px(8.0))
                .right(px(10.0))
                .text_size(px(10.0))
                .text_color(theme.text_muted.opacity(0.45))
                .child(SharedString::from(format!("{}", index + 1))),
        );

        if menu_open {
            let menu = self.render_slot_menu(index, theme, cx);
            card = card.child(popover::anchored_menu_below_split(
                format!("loadout-slot-menu-{index}"),
                menu,
                None,
            ));
        }
        card
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_page(cx: &mut Context<LoadoutPage>) -> LoadoutPage {
        let state = cx.new(|_| AppState::new());
        let mut config = LoadoutConfig::default();
        for (index, (model, label, reasoning, shortcut, temperature)) in [
            ("one", "One", ReasoningLevel::Low, "mod-a", 0.1),
            ("two", "Two", ReasoningLevel::High, "mod-b", 0.2),
            ("three", "Three", ReasoningLevel::Low, "mod-c", 0.3),
        ]
        .into_iter()
        .enumerate()
        {
            let mut model_options = serde_json::Map::new();
            model_options.insert("temperature".into(), serde_json::json!(temperature));
            config.slots[index] = Some(LoadoutSlot {
                harness: HarnessId::Codex,
                model: model.into(),
                label: label.into(),
                reasoning: Some(reasoning),
                model_options,
                shortcut: Some(shortcut.into()),
            });
        }
        LoadoutPage::new(state, config, KeymapConfig::default(), cx)
    }

    #[gpui::test]
    fn focused_card_reorders_full_slot_and_click_opens_menu(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let page = fixture_page(cx);
            window.focus(&page.focus, cx);
            page
        });
        cx.refresh().unwrap();

        page.update_in(cx, |page, window, cx| window.focus(&page.slot_focus[0], cx));
        cx.simulate_keystrokes("right");
        cx.refresh().unwrap();
        page.read_with(cx, |page, _| {
            let moved = page.loadout.slot(1).unwrap();
            assert_eq!(moved.model, "one");
            assert_eq!(moved.reasoning, Some(ReasoningLevel::Low));
            assert_eq!(moved.shortcut.as_deref(), Some("mod-a"));
            assert_eq!(
                moved.model_options.get("temperature"),
                Some(&serde_json::json!(0.1))
            );
            assert_eq!(page.loadout.slot(0).unwrap().model, "two");
        });

        assert!(cx.debug_bounds("loadout-shortcut-badge-1").is_some());
        let card = cx.debug_bounds("loadout-slot-1").unwrap();
        cx.simulate_click(card.center(), gpui::Modifiers::default());
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-menu-root").is_some());
    }

    #[gpui::test]
    fn fast_pill_sits_right_of_the_shortcut_badge(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (_page, cx) = cx.add_window_view(|_, cx| {
            let mut page = fixture_page(cx);
            let model = Model {
                id: "one".into(),
                label: "One".into(),
                description: None,
                reasoning_levels: vec![],
                options: vec![zeron_proto::ModelOption {
                    id: "serviceTier".into(),
                    label: "Service tier".into(),
                    default_choice: "default".into(),
                    choices: vec![
                        zeron_proto::ModelOptionChoice {
                            id: "default".into(),
                            label: "Standard".into(),
                        },
                        zeron_proto::ModelOptionChoice {
                            id: "fast".into(),
                            label: "Fast".into(),
                        },
                    ],
                }],
            };
            set_slot_speed(page.loadout.slots[0].as_mut().unwrap(), Some(&model), true);
            page.models
                .insert(HarnessId::Codex, Loadable::Ready(vec![model]));
            page
        });
        cx.refresh().unwrap();
        let shortcut = cx.debug_bounds("loadout-shortcut-badge-0").unwrap();
        let fast = cx.debug_bounds("loadout-fast-0").unwrap();
        assert!(fast.origin.x >= shortcut.origin.x + shortcut.size.width);
    }

    #[gpui::test]
    fn card_drag_tracks_pointer_side_and_clears_when_leaving(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|_, cx| fixture_page(cx));
        cx.refresh().unwrap();
        let first = cx.debug_bounds("loadout-slot-0").unwrap();
        let second = cx.debug_bounds("loadout-slot-1").unwrap();
        let source = first.center();
        let target = gpui::point(second.right() - px(15.0), second.center().y);
        cx.simulate_mouse_down(source, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            source + gpui::point(px(10.0), px(0.0)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            target,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.refresh().unwrap();
        page.read_with(cx, |page, _| {
            assert_eq!(page.slot_drag_over, Some((1, true)))
        });
        assert!(
            cx.debug_bounds("loadout-slot-insertion-1")
                .unwrap()
                .origin
                .x
                >= second.right()
        );
        cx.simulate_mouse_move(
            gpui::point(px(10.0), px(10.0)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        page.read_with(cx, |page, _| {
            assert!(page.drag_over.is_none());
            assert!(page.slot_drag_over.is_none());
        });
        cx.simulate_mouse_move(
            target,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(target, gpui::MouseButton::Left, gpui::Modifiers::default());
        page.read_with(cx, |page, _| {
            assert_eq!(page.loadout.slot(1).unwrap().model, "one");
            assert_eq!(page.loadout.slot(0).unwrap().model, "two");
        });
    }

    #[gpui::test]
    fn dropping_a_filled_slot_away_removes_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|_, cx| fixture_page(cx));
        cx.refresh().unwrap();
        let first = cx.debug_bounds("loadout-slot-0").unwrap();
        let page_bounds = cx.debug_bounds("loadout-page").unwrap();
        let source = first.center();
        let away = gpui::point(page_bounds.origin.x + px(24.0), first.origin.y - px(28.0));
        cx.simulate_mouse_down(source, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            source + gpui::point(px(10.0), px(0.0)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            away,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_up(away, gpui::MouseButton::Left, gpui::Modifiers::default());
        page.read_with(cx, |page, _| {
            assert_eq!(page.loadout.slot(0).unwrap().model, "two");
            assert_eq!(page.loadout.slot(1).unwrap().model, "three");
            assert!(page.loadout.slot(2).is_none());
        });
    }

    #[test]
    fn destination_accounts_for_removing_the_source() {
        assert_eq!(reorder_destination(0, 1, false, Some(3), 5), 0);
        assert_eq!(reorder_destination(0, 1, true, Some(3), 5), 1);
        assert_eq!(reorder_destination(2, 0, false, Some(3), 5), 0);
        assert_eq!(reorder_destination(2, 1, true, Some(3), 5), 2);
    }

    #[test]
    fn empty_target_places_item_at_end_of_filled_slots() {
        assert_eq!(reorder_destination(0, 3, false, Some(3), 5), 2);
        assert_eq!(reorder_destination(2, 4, true, Some(3), 5), 2);
    }

    #[test]
    fn full_loadout_clamps_to_last_slot() {
        assert_eq!(reorder_destination(0, 4, true, None, 5), 4);
        assert_eq!(reorder_destination(4, 0, false, None, 5), 0);
    }
}
