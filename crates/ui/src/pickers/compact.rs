//! Alternate presentation of the same model and option mutations.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub(super) enum CompactControl {
    #[default]
    Model,
    Fast,
    Effort,
    Option(ModelSetting),
}

#[derive(Clone)]
pub(super) struct CatalogStatus {
    pub harness: HarnessId,
    pub name: String,
    pub error: Option<String>,
}

pub(super) const COMPACT_WIDTH: f32 = 256.0;
// The rail is a 24px capsule holding a 20px thumb 2px from its edge, so
// their radii stay concentric (12 = 10 + 2). Stops and pointer input share
// the thumb's centre inset, and endpoint dots stay inside the caps.
const SLIDER_HEIGHT: f32 = 32.0;
const RAIL_HEIGHT: f32 = 24.0;
const RAIL_TOP: f32 = (SLIDER_HEIGHT - RAIL_HEIGHT) / 2.0;
const THUMB_SIZE: f32 = 20.0;
const THUMB_INSET: f32 = RAIL_HEIGHT / 2.0;
/// Header (32) inside the card inset, the effort label and slider, and the
/// space above the option rows.
const HEADER_BLOCK: f32 = 40.0;
const EFFORT_BLOCK: f32 = 6.0 + 18.0 + 4.0 + SLIDER_HEIGHT;
const OPTIONS_GAP: f32 = 8.0;

#[derive(Default)]
pub(super) struct CompactMotion {
    effort: Option<ScalarTransition>,
    press: Option<ScalarTransition>,
    drag_fraction: Option<f32>,
    energy: Option<ScalarTransition>,
    fast: Option<ScalarTransition>,
    height: Option<ScalarTransition>,
    page: Option<bool>,
    reveal: Option<ScalarTransition>,
    pub frame_height: f32,
}

struct ScalarTransition {
    from: f32,
    to: f32,
    started: std::time::Instant,
}
impl ScalarTransition {
    fn value(&self, now: std::time::Instant) -> f32 {
        let progress = motion::RESIZE.progress(
            now.duration_since(self.started).as_secs_f32()
                / motion::RESIZE
                    .total()
                    .mul_f32(motion::speed_scale())
                    .as_secs_f32(),
        );
        motion::lerp(self.from, self.to, progress)
    }
    fn sample(
        state: &mut Option<Self>,
        target: f32,
        now: std::time::Instant,
        reduced: bool,
    ) -> (f32, bool) {
        let Some(transition) = state else {
            *state = Some(Self {
                from: target,
                to: target,
                started: now,
            });
            return (target, false);
        };
        if reduced {
            transition.from = target;
            transition.to = target;
            return (target, false);
        }
        if transition.to != target {
            transition.from = transition.value(now);
            transition.to = target;
            transition.started = now;
        }
        let value = transition.value(now);
        (value, (value - target).abs() > 0.001)
    }
}

impl Pickers {
    pub(super) fn render_compact_model_row(
        &mut self,
        ix: usize,
        row: &ModelRowData,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let selected = self.effective_harness(cx) == Some(row.harness)
            && self
                .selected_model(cx)
                .is_some_and(|model| model.id == row.model.id);
        let favorite = self.defaults.is_favorite(row.harness, &row.model.id);
        let (icon, tint) = harness_brand_icon(row.harness);
        let harness = row.harness;
        let model = row.model.id.clone();
        let label: SharedString = row.model.label.clone().into();
        let subtitle: SharedString = row.harness_name.clone();
        let details: SharedString = match row.model.description.as_deref() {
            Some(description) => format!(
                "{} · {}\n{}",
                row.model.label, row.harness_name, description
            )
            .into(),
            None => format!("{} · {}", row.model.label, row.harness_name).into(),
        };
        let accessible_name = details.clone();
        let favorite_hint: SharedString = format!(
            "{} {} {}",
            if favorite { "Remove" } else { "Add" },
            row.model.label,
            if favorite {
                "from favorites"
            } else {
                "to favorites"
            }
        )
        .into();
        let favorite_tooltip: SharedString =
            format!("{}\n⌘⇧F on the highlighted model", favorite_hint).into();
        let item = div()
            .id(("model-row", ix))
            .role(gpui::Role::ListBoxOption)
            .aria_label(accessible_name)
            .aria_selected(selected)
            .tooltip(move |_, cx| cx.new(|_| PickerHint(details.clone())).into())
            .tooltip_show_delay(std::time::Duration::from_millis(500))
            .h(px(48.0))
            .px(px(8.0))
            .py(px(6.0))
            .rounded(px(popover::MENU_ITEM_RADIUS))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .text_color(theme.text)
            // Every row reserves the glass rim so selection doesn't shift content.
            .border_1()
            .border_color(gpui::transparent_black())
            .when(selected, |el| crate::glass::light(el, &theme, 1.0))
            .when(!selected && self.active == ix, |el| {
                el.bg(crate::theme::ink(0.05))
            })
            .when(self.compact_keyboard && self.active == ix, |el| {
                el.aria_active_descendant().shadow(focus_outline(&theme))
            })
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered && (this.active != ix || this.compact_keyboard) {
                    this.compact_keyboard = false;
                    this.active = ix;
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| this.activate_model_index(ix, cx)))
            .child(
                crate::icons::icon(icon)
                    .size(px(17.0))
                    .text_color(tint.unwrap_or(theme.text_muted)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(label),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(subtitle),
                    ),
            )
            .child(div().size(px(14.0)).when(selected, |el| {
                el.child(
                    crate::icons::icon(crate::icons::CHECK)
                        .size(px(14.0))
                        .text_color(theme.accent),
                )
            }))
            .child(
                div()
                    .id(("model-star", ix))
                    .role(gpui::Role::Button)
                    .aria_label(favorite_hint)
                    .aria_toggled(if favorite {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .aria_keyshortcuts("Meta+Shift+F")
                    .tooltip(move |_, cx| cx.new(|_| PickerHint(favorite_tooltip.clone())).into())
                    .size(px(24.0))
                    .rounded(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(|s| s.bg(crate::theme::ink(0.08)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.toggle_model_favorite(harness, &model, cx);
                    }))
                    .child(
                        crate::icons::icon(if favorite {
                            crate::icons::STAR_BOLD
                        } else {
                            crate::icons::STAR
                        })
                        .size(px(13.0))
                        .text_color(if favorite {
                            theme.accent
                        } else {
                            theme.text_muted
                        }),
                    ),
            );
        div()
            .pb(px(popover::MENU_GAP))
            .child(item)
            .into_any_element()
    }

    pub(super) fn compact_model_picker(&self, cx: &App) -> bool {
        self.title.is_none() && crate::settings::compact_model_picker(cx)
    }

    pub(super) fn show_compact_models(&mut self, cx: &mut Context<Self>) {
        self.setting_menu = None;
        self.compact_model_list = true;
        self.model_rail = ModelRail::All;
        // Open at the favorites group, even when the current model is further down.
        self.active = 0;
        self.model_scroll
            .scroll_to_item(0, gpui::ScrollStrategy::Top);
        self.focus_on_mount = true;
        cx.notify();
    }

    pub(super) fn compact_model_back_header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = Theme::of(cx).for_popup();
        div()
            .h(px(40.0))
            .flex_none()
            .px(px(popover::CARD_INSET))
            .flex()
            .items_center()
            .child(
                popover::menu_row(&theme, false, "compact-model-back")
                    .id("compact-model-back")
                    .role(gpui::Role::Button)
                    .aria_label("Back to effort")
                    .w_full()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.compact_control = CompactControl::Model;
                        this.compact_model_list = false;
                        this.focus_on_mount = true;
                        cx.notify();
                    }))
                    .child(
                        crate::icons::icon(crate::icons::ALT_ARROW_LEFT)
                            .size(px(14.0))
                            .text_color(theme.text_muted),
                    )
                    .child("Select model"),
            )
    }

    pub(super) fn compact_fast_choice(&self, cx: &App) -> Option<(String, String, bool, bool)> {
        let option = self
            .selected_model(cx)?
            .options
            .iter()
            .find(|o| o.id == "serviceTier")?;
        if !option.choices.iter().any(|c| c.id == "fast") || option.default_choice == "fast" {
            return None;
        }
        let options = self.explicit_options(cx);
        let fast = options
            .get(&option.id)
            .and_then(|v| v.as_str())
            .unwrap_or(&option.default_choice)
            == "fast";
        Some((
            option.id.clone(),
            if fast {
                option.default_choice.clone()
            } else {
                "fast".into()
            },
            fast,
            fast,
        ))
    }

    pub(super) fn pick_effort_at(&mut self, x: gpui::Pixels, cx: &mut Context<Self>) {
        let Some(bounds) = self.effort_bounds else {
            return;
        };
        let levels = self.trait_ladder(cx);
        let Some(index) = effort_index(
            f32::from(x - bounds.left()),
            f32::from(bounds.size.width),
            levels.len(),
        ) else {
            return;
        };
        if self.effort_dragging {
            let fraction =
                effort_fraction(f32::from(x - bounds.left()), f32::from(bounds.size.width));
            self.compact_motion.drag_fraction = Some(if levels.len() > 1 { fraction } else { 0.0 });
            cx.notify();
        }
        self.pick_compact_effort(levels[index], cx);
    }

    pub(super) fn compact_option_visible(id: &ModelSetting) -> bool {
        !matches!(id, ModelSetting::Reasoning)
            && !matches!(id, ModelSetting::Option(id) if id == "serviceTier")
    }

    fn compact_controls(&self, cx: &App) -> Vec<CompactControl> {
        let mut controls = vec![CompactControl::Model];
        if self.compact_fast_choice(cx).is_some() {
            controls.push(CompactControl::Fast);
        }
        if !self.trait_ladder(cx).is_empty() {
            controls.push(CompactControl::Effort);
        }
        controls.extend(
            self.setting_groups(cx)
                .into_iter()
                .filter(|g| Self::compact_option_visible(&g.id))
                .map(|g| CompactControl::Option(g.id)),
        );
        controls
    }

    fn pick_compact_effort(&mut self, level: ReasoningLevel, cx: &mut Context<Self>) {
        if self.effective_reasoning(cx) != Some(level) {
            self.pick_reasoning(level, cx);
            crate::haptics::selection_step();
        }
    }

    pub(super) fn render_compact_menu(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let options = self
            .setting_groups(cx)
            .iter()
            .filter(|g| Self::compact_option_visible(&g.id))
            .count();
        let panel_height = HEADER_BLOCK
            + if self.trait_ladder(cx).is_empty() {
                0.0
            } else {
                EFFORT_BLOCK
            }
            + if options == 0 {
                0.0
            } else {
                OPTIONS_GAP + (6.0 + options as f32 * 28.0).min(64.0)
            };
        let target = self.menu_geometry().height.min(if self.compact_model_list {
            302.0
        } else {
            panel_height
        });
        let (height, moving) = ScalarTransition::sample(
            &mut self.compact_motion.height,
            target,
            std::time::Instant::now(),
            cx.reduce_motion(),
        );
        self.compact_motion.frame_height = height;
        if moving {
            window.request_animation_frame();
        }
        let content = if self.compact_model_list {
            self.render_harness_model_popover(cx)
        } else {
            self.render_compact_model_panel(window, cx)
        };
        let now = std::time::Instant::now();
        if self
            .compact_motion
            .page
            .is_some_and(|page| page != self.compact_model_list)
        {
            self.compact_motion.reveal = Some(ScalarTransition {
                from: 0.0,
                to: 1.0,
                started: now,
            });
        }
        self.compact_motion.page = Some(self.compact_model_list);
        let (reveal, revealing) = ScalarTransition::sample(
            &mut self.compact_motion.reveal,
            1.0,
            now,
            cx.reduce_motion(),
        );
        if revealing {
            window.request_animation_frame();
        }
        // One short directional entrance, coordinated with the card resize.
        // Initial opening uses the shared popover entrance only.
        let direction = if self.compact_model_list { 1.0 } else { -1.0 };
        let content = div()
            .relative()
            .left(px(direction * 6.0 * (1.0 - reveal)))
            .opacity(0.65 + reveal * 0.35)
            .child(content)
            .into_any_element();
        self.popover_frame_flush(COMPACT_WIDTH, content, cx)
    }

    pub(super) fn compact_panel_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        self.compact_keyboard = true;
        let controls = self.compact_controls(cx);
        if !controls.contains(&self.compact_control) {
            self.compact_control = CompactControl::Model;
        }
        match event.keystroke.key.as_str() {
            "escape" => self.animate_close(cx),
            "up" | "down" | "tab" => {
                let current = controls
                    .iter()
                    .position(|control| *control == self.compact_control);
                let backwards = event.keystroke.key == "up"
                    || (event.keystroke.key == "tab" && event.keystroke.modifiers.shift);
                let next =
                    popover::menu_step(current, controls.len(), if backwards { -1 } else { 1 })
                        .unwrap_or(0);
                self.compact_control = controls[next].clone();
                self.active = match &self.compact_control {
                    CompactControl::Option(id) => self
                        .setting_groups(cx)
                        .iter()
                        .position(|g| &g.id == id)
                        .map(|i| self.model_rows_len(cx) + i)
                        .unwrap_or(NO_ACTIVE_ROW),
                    _ => NO_ACTIVE_ROW,
                };
                if let CompactControl::Option(id) = &self.compact_control {
                    let index = self
                        .setting_groups(cx)
                        .iter()
                        .filter(|g| Self::compact_option_visible(&g.id))
                        .position(|g| &g.id == id)
                        .unwrap_or(0);
                    self.menu_scroll
                        .set_offset(gpui::point(px(0.0), px(-(index as f32 * 28.0))));
                }
            }
            "enter" | "space" => match self.compact_control.clone() {
                CompactControl::Model => self.show_compact_models(cx),
                CompactControl::Fast => {
                    if let Some((option, choice, default, _)) = self.compact_fast_choice(cx) {
                        self.pick_option(option, choice, default, cx);
                    }
                }
                CompactControl::Option(id) => self.open_setting(SettingScope::Tray, id, cx),
                CompactControl::Effort => {}
            },
            "right" if matches!(self.compact_control, CompactControl::Option(_)) => {
                if let CompactControl::Option(id) = self.compact_control.clone() {
                    self.open_setting(SettingScope::Tray, id, cx);
                }
            }
            "left" | "right" | "home" | "end" if self.compact_control == CompactControl::Effort => {
                let levels = self.trait_ladder(cx);
                if !levels.is_empty() {
                    let index = levels
                        .iter()
                        .position(|level| Some(*level) == self.effective_reasoning(cx))
                        .unwrap_or(0);
                    let next = match event.keystroke.key.as_str() {
                        "left" => index.saturating_sub(1),
                        "right" => (index + 1).min(levels.len() - 1),
                        "home" => 0,
                        _ => levels.len() - 1,
                    };
                    self.pick_compact_effort(levels[next], cx);
                }
            }
            _ => return,
        }
        cx.notify();
        cx.stop_propagation();
    }

    pub(super) fn compact_catalog_statuses(&self, cx: &App) -> Vec<CatalogStatus> {
        self.rail_descriptors(cx)
            .into_iter()
            .filter_map(|descriptor| {
                let error = match self.models.get(&descriptor.id) {
                    Some(Loadable::Ready(_)) => return None,
                    Some(Loadable::Error(error)) => Some(error.clone()),
                    _ => None,
                };
                Some(CatalogStatus {
                    harness: descriptor.id,
                    name: descriptor.name,
                    error,
                })
            })
            .collect()
    }

    pub(super) fn render_compact_catalog_status(
        &self,
        ix: usize,
        status: &CatalogStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let harness = status.harness;
        let failed = status.error.is_some();
        let title: SharedString = format!(
            "{} — {}",
            status.name,
            if failed {
                "Models unavailable"
            } else {
                "Loading models…"
            }
        )
        .into();
        let mut row = popover::menu_row(&theme, self.active == ix, format!("catalog-status-{ix}"))
            .id(("catalog-status", ix))
            .h(px(48.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().child(title))
                    .when_some(status.error.clone(), |el, error| {
                        el.child(
                            div()
                                .truncate()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(error),
                        )
                    }),
            );
        if failed {
            let error: SharedString = status.error.clone().unwrap_or_default().into();
            row = row
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(format!("Retry {} models", status.name)))
                .aria_description(error.clone())
                .tooltip(move |_, cx| cx.new(|_| PickerHint(error.clone())).into())
                .on_click(cx.listener(move |this, _, _, cx| this.ensure_models(harness, true, cx)))
                .child(div().text_color(theme.accent).child("Retry"));
        }
        div()
            .pb(px(popover::MENU_GAP))
            .child(row)
            .into_any_element()
    }

    pub(super) fn render_compact_model_panel(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = Theme::of(cx).for_popup();
        let levels = self.trait_ladder(cx);
        let selected = levels
            .iter()
            .position(|l| Some(*l) == self.effective_reasoning(cx))
            .unwrap_or(0);
        let label: SharedString = self
            .selected_model(cx)
            .map(|m| m.label.clone())
            .unwrap_or_else(|| "Select model".into())
            .into();
        let effort: SharedString = levels
            .get(selected)
            .copied()
            .map(reasoning_label)
            .unwrap_or("Default")
            .into();
        let model_hint: SharedString = format!("Change model\n{}", label).into();
        let model_accessible: SharedString =
            format!("{} · {} · Change model", effort, label).into();
        let model_focus = self.compact_keyboard && self.compact_control == CompactControl::Model;
        let fast_focus = self.compact_keyboard && self.compact_control == CompactControl::Fast;
        let effort_focus = self.compact_keyboard && self.compact_control == CompactControl::Effort;
        let brand = self.effective_harness(cx).map(harness_brand_icon);
        let now = std::time::Instant::now();
        let reduced = cx.reduce_motion();
        // Leading: the current model, which opens the list. Trailing: fast
        // mode, whose accent glass fades in like the composer's Stop.
        let mut header = div()
            .h(px(32.0))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                div()
                    .id("compact-select-model")
                    .role(gpui::Role::Button)
                    .aria_label(model_accessible)
                    .tooltip(move |_, cx| cx.new(|_| PickerHint(model_hint.clone())).into())
                    .h(px(28.0))
                    .flex_1()
                    .min_w_0()
                    // The trailing chevron sits optically closer to the edge.
                    .pl(px(8.0))
                    .pr(px(6.0))
                    .rounded(px(popover::MENU_ITEM_RADIUS))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .cursor_pointer()
                    .when(model_focus, |el| {
                        el.aria_active_descendant().shadow(focus_outline(&theme))
                    })
                    .hover(|s| s.bg(crate::theme::ink(0.05)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.compact_keyboard = false;
                        this.show_compact_models(cx);
                    }))
                    .when_some(brand, |el, (icon, tint)| {
                        el.child(
                            crate::icons::icon(icon)
                                .size(px(14.0))
                                .flex_none()
                                .text_color(tint.unwrap_or(theme.text_muted)),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(crate::typography::ui_rems(12.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(label.clone()),
                    )
                    .child(
                        crate::icons::icon(crate::icons::ALT_ARROW_RIGHT)
                            .size(px(11.0))
                            .flex_none()
                            .text_color(theme.text_muted),
                    ),
            );
        if let Some((option, choice, default, fast)) = self.compact_fast_choice(cx) {
            let (fast_t, fading) = ScalarTransition::sample(
                &mut self.compact_motion.fast,
                if fast { 1.0 } else { 0.0 },
                now,
                reduced,
            );
            if fading {
                window.request_animation_frame();
            }
            header = header.child(
                div()
                    .id("compact-fast")
                    .role(gpui::Role::Button)
                    .aria_label("Fast mode")
                    .aria_toggled(if fast {
                        gpui::Toggled::True
                    } else {
                        gpui::Toggled::False
                    })
                    .tooltip(move |_, cx| {
                        cx.new(|_| {
                            PickerHint(
                                if fast {
                                    "Fast mode on · Turn off"
                                } else {
                                    "Fast mode off · Turn on"
                                }
                                .into(),
                            )
                        })
                        .into()
                    })
                    .relative()
                    .size(px(28.0))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .when(!fast, |el| el.hover(|s| s.bg(crate::theme::ink(0.06))))
                    .when(fast_focus, |el| {
                        el.aria_active_descendant().shadow(focus_outline(&theme))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.compact_keyboard = false;
                        this.pick_option(option.clone(), choice.clone(), default, cx);
                    }))
                    .when(fast_t > 0.001, |el| {
                        el.child(crate::glass::accent(
                            div().absolute().inset_0().rounded_full(),
                            &theme,
                            fast_t,
                            0.0,
                        ))
                    })
                    .child(
                        crate::icons::icon(crate::icons::FAST_TIER)
                            .size(px(14.0))
                            .text_color(motion::mix(theme.text_muted, theme.on_accent, fast_t)),
                    ),
            );
        }
        let mut panel = div()
            .p(px(popover::CARD_INSET))
            .flex()
            .flex_col()
            .child(header);
        if !levels.is_empty() {
            let target = if levels.len() > 1 {
                selected as f32 / (levels.len() - 1) as f32
            } else {
                0.0
            };
            let (mut fraction, moving) =
                ScalarTransition::sample(&mut self.compact_motion.effort, target, now, reduced);
            if self.effort_dragging {
                fraction = self.compact_motion.drag_fraction.unwrap_or(target);
                // Keep the release animation rooted at the actual pointer, not the previous stop.
                self.compact_motion.effort = Some(ScalarTransition {
                    from: fraction,
                    to: fraction,
                    started: now,
                });
            }
            if moving {
                window.request_animation_frame();
            }
            let fast = self
                .compact_fast_choice(cx)
                .is_some_and(|(_, _, _, fast)| fast);
            // Fast mode spreads the fill's glow, as the voice level does for
            // Stop; a static glow rather than a running animation.
            let (energy, energizing) = ScalarTransition::sample(
                &mut self.compact_motion.energy,
                fast_energy(fraction, fast),
                now,
                reduced,
            );
            if energizing {
                window.request_animation_frame();
            }
            let (press, pressing) = ScalarTransition::sample(
                &mut self.compact_motion.press,
                if self.effort_dragging { 1.0 } else { 0.0 },
                now,
                reduced,
            );
            if pressing {
                window.request_animation_frame();
            }
            let thumb_size = THUMB_SIZE * (1.0 - 0.04 * press);
            let entity = cx.entity().downgrade();
            let drag_entity = entity.clone();
            let slider = div()
                .id("compact-effort-slider")
                .role(gpui::Role::Slider)
                .when(effort_focus, |el| el.aria_active_descendant())
                .aria_label("Reasoning effort")
                .aria_value(effort.clone())
                .aria_numeric_value(selected as f64)
                .aria_min_numeric_value(0.0)
                .aria_max_numeric_value(levels.len().saturating_sub(1) as f64)
                .aria_numeric_value_step(1.0)
                .aria_orientation(gpui::Orientation::Horizontal)
                .aria_description("Use Left and Right to adjust reasoning; Home and End select the first and last levels")
                .relative()
                .h(px(SLIDER_HEIGHT))
                .cursor_pointer()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                        window.focus(&this.focus, cx);
                        this.compact_keyboard = false;
                        this.compact_control = CompactControl::Effort;
                        this.active = NO_ACTIVE_ROW;
                        this.effort_dragging = true;
                        this.pick_effort_at(event.position.x, cx);
                        cx.notify();
                        cx.stop_propagation();
                    }),
                )
                .child(
                    gpui::canvas(
                        move |bounds, _, cx| {
                            let _ = entity.update(cx, |this, _| this.effort_bounds = Some(bounds));
                        },
                        move |_, _, window, _| {
                            let release = drag_entity.clone();
                            window.on_mouse_event(move |_: &gpui::MouseUpEvent, phase, _, cx| {
                                if phase == gpui::DispatchPhase::Bubble {
                                    let _ = release.update(cx, |this, cx| {
                                        if this.effort_dragging {
                                            this.effort_dragging = false;
                                            this.compact_motion.drag_fraction = None;
                                            cx.notify();
                                        }
                                    });
                                }
                            });
                            let entity = drag_entity.clone();
                            window.on_mouse_event(
                                move |event: &gpui::MouseMoveEvent, phase, _, cx| {
                                    if phase == gpui::DispatchPhase::Bubble
                                        && event.pressed_button == Some(gpui::MouseButton::Left)
                                    {
                                        let _ = entity.update(cx, |this, cx| {
                                            if this.effort_dragging {
                                                this.pick_effort_at(event.position.x, cx);
                                            }
                                        });
                                    }
                                },
                            );
                        },
                    )
                    .absolute()
                    .inset_0(),
                )
                .child(crate::glass::light(
                    div()
                        .absolute()
                        .left_0()
                        .right_0()
                        .top(px(RAIL_TOP))
                        .h(px(RAIL_HEIGHT))
                        .rounded_full(),
                    &theme,
                    1.0,
                ))
                // A contained glow: it must not wash over the option rows.
                .child(effort_fill(&theme, fraction, levels.len(), energy * 0.35))
                .child(
                    div()
                        .absolute()
                        .left(px(THUMB_INSET))
                        .right(px(THUMB_INSET))
                        .top(px((SLIDER_HEIGHT - thumb_size) / 2.0))
                        .child(
                            crate::glass::thumb(
                                div()
                                    .absolute()
                                    .left(gpui::relative(fraction))
                                    .ml(px(-thumb_size / 2.0))
                                    .size(px(thumb_size))
                                    .rounded_full(),
                                &theme,
                            )
                            .when(effort_focus, |el| el.border_2().border_color(theme.text)),
                        ),
                );
            // The value sits with its control rather than in the header.
            panel = panel
                .child(
                    div()
                        .mt(px(6.0))
                        .h(px(18.0))
                        .px(px(8.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .text_size(crate::typography::ui_rems(11.0))
                        .child(div().text_color(theme.text_muted).child("Reasoning"))
                        .child(
                            div()
                                .text_color(theme.text)
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(effort.clone()),
                        ),
                )
                .child(div().px(px(8.0)).pt(px(4.0)).child(slider));
        }
        let option_count = self
            .setting_groups(cx)
            .iter()
            .filter(|g| Self::compact_option_visible(&g.id))
            .count();
        if option_count > 0 {
            let options = self.render_traits_sections(cx);
            let scrollbar = popover::rail(self, "compact-options-scrollbar", &theme, cx);
            let chrome =
                HEADER_BLOCK + if levels.is_empty() { 0.0 } else { EFFORT_BLOCK } + OPTIONS_GAP;
            // Space, not a rule, separates the options from the effort group.
            panel = panel.child(
                div().mt(px(OPTIONS_GAP)).child(
                    popover::menu_scroll_host("compact-options-host")
                        .on_hover(cx.listener(Self::on_menu_list_hover))
                        .child(popover::faded_menu_list(
                            &self.menu_scroll,
                            popover::menu_scroll_list("compact-options", &self.menu_scroll)
                                .max_h(px(
                                    64.0_f32.min((self.menu_geometry().height - chrome).max(0.0))
                                ))
                                .child(options),
                        ))
                        .children(scrollbar),
                ),
            );
        }
        panel.into_any_element()
    }
}

/// The accent glass fill from the rail's start to just past the thumb, with
/// the stops painted over rail and fill in one coordinate space.
fn effort_fill(theme: &Theme, fraction: f32, count: usize, energy: f32) -> AnyElement {
    let fill = crate::glass::accent_plate(theme, 1.0, energy);
    let filled_stop = gpui::white().opacity(0.7);
    let open_stop = theme.text_muted.opacity(0.45);
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let width = f32::from(bounds.size.width);
            let run = (width - 2.0 * THUMB_INSET).max(0.0);
            let center = THUMB_INSET + fraction * run;
            let rect = |x: f32, y: f32, w: f32, h: f32| {
                gpui::Bounds::new(
                    bounds.origin + gpui::point(px(x), px(y)),
                    gpui::size(px(w), px(h)),
                )
            };
            fill.paint(
                window,
                rect(
                    0.0,
                    RAIL_TOP,
                    (center + THUMB_INSET).min(width),
                    RAIL_HEIGHT,
                ),
                RAIL_HEIGHT / 2.0,
            );
            for i in 0..count {
                let stop = if count > 1 {
                    i as f32 / (count - 1) as f32
                } else {
                    0.0
                };
                let x = THUMB_INSET + stop * run;
                // Filled stops follow the animated front, never jump ahead of it.
                let color = if stop <= fraction {
                    filled_stop
                } else {
                    open_stop
                };
                window.paint_quad(gpui::quad(
                    rect(x - 2.0, SLIDER_HEIGHT / 2.0 - 2.0, 4.0, 4.0),
                    px(2.0),
                    color,
                    px(0.0),
                    gpui::transparent_black(),
                    gpui::BorderStyle::default(),
                ));
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}

fn fast_energy(fraction: f32, fast: bool) -> f32 {
    if fast {
        0.25 + 0.75 * fraction.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn effort_fraction(x: f32, width: f32) -> f32 {
    ((x - THUMB_INSET) / (width - THUMB_INSET * 2.0).max(1.0)).clamp(0.0, 1.0)
}

/// Slider stops include both endpoints and only advertised reasoning levels.
fn effort_index(x: f32, width: f32, count: usize) -> Option<usize> {
    (count > 0)
        .then(|| (effort_fraction(x, width) * count.saturating_sub(1) as f32).round() as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_transitions_start_settled_and_retarget_without_jumping() {
        let start = std::time::Instant::now();
        let mut state = None;
        assert_eq!(
            ScalarTransition::sample(&mut state, 0.5, start, false),
            (0.5, false)
        );
        assert_eq!(
            ScalarTransition::sample(&mut state, 1.0, start, false).0,
            0.5
        );
        let middle = start + motion::RESIZE.total().mul_f32(motion::speed_scale() * 0.5);
        let before = state.as_ref().unwrap().value(middle);
        let retargeted = ScalarTransition::sample(&mut state, 0.0, middle, false).0;
        assert!((before - retargeted).abs() < 0.0001);
        assert_eq!(
            ScalarTransition::sample(&mut state, 0.0, middle, true),
            (0.0, false)
        );
    }

    #[test]
    fn glow_requires_fast_mode_at_every_effort() {
        for fraction in [0.0, 0.2, 0.5, 0.8, 1.0] {
            assert_eq!(fast_energy(fraction, false), 0.0);
            assert!(fast_energy(fraction, true) > 0.0);
        }
        assert!(fast_energy(1.0, true) > fast_energy(0.0, true));
    }

    #[test]
    fn continuous_drag_and_stops_share_inset_geometry() {
        let width = 228.0;
        for count in 2..=8 {
            for index in 0..count {
                let fraction = index as f32 / (count - 1) as f32;
                let x = THUMB_INSET + fraction * (width - THUMB_INSET * 2.0);
                assert!((effort_fraction(x, width) - fraction).abs() < 0.00001);
                assert_eq!(effort_index(x, width, count), Some(index));
                assert!(x - 2.0 >= 0.0 && x + 2.0 <= width);
            }
        }
        assert!((effort_fraction(63.0, width) - 0.25).abs() < 0.00001);
        assert_eq!(effort_fraction(-100.0, width), 0.0);
        assert_eq!(effort_fraction(500.0, width), 1.0);
    }

    #[test]
    fn effort_stops_clamp_and_round_to_advertised_levels() {
        assert_eq!(effort_index(100.0, 200.0, 0), None);
        assert_eq!(effort_index(100.0, 200.0, 1), Some(0));
        assert_eq!(effort_index(-20.0, 200.0, 5), Some(0));
        assert_eq!(effort_index(100.0, 200.0, 5), Some(2));
        assert_eq!(effort_index(250.0, 200.0, 5), Some(4));
    }
}

fn focus_outline(theme: &Theme) -> Vec<gpui::BoxShadow> {
    vec![gpui::BoxShadow {
        color: theme.text.opacity(0.8),
        offset: gpui::point(px(0.0), px(0.0)),
        blur_radius: px(0.0),
        spread_radius: px(2.0),
        inset: true,
    }]
}

struct PickerHint(SharedString);
impl Render for PickerHint {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).for_popup();
        let card = popover::popover_card(&theme)
            .max_w(px(320.0))
            .p(px(8.0))
            .text_size(crate::typography::ui_rems(12.0))
            .line_height(px(17.0))
            .text_color(theme.text)
            .child(self.0.clone());
        crate::frost::frosted(popover::CARD_RADIUS, crate::frost::MENU_BLUR, card)
    }
}
