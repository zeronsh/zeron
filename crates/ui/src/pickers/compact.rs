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
/// One-line model rows (plus the 2px gap) so about seven fit at once.
pub(super) const COMPACT_ROW_HEIGHT: f32 = 32.0;
pub(super) const COMPACT_LIST_ROWS: f32 = 7.0;
/// The list fits its rows, up to [`COMPACT_LIST_ROWS`]; empty and loading
/// states keep room for a few skeleton rows.
pub(super) fn compact_list_height(rows: usize) -> f32 {
    let rows = if rows == 0 {
        4.0
    } else {
        (rows as f32).min(COMPACT_LIST_ROWS)
    };
    rows * (COMPACT_ROW_HEIGHT + popover::MENU_GAP) + 2.0 * popover::CARD_INSET
}
// A 28px rail under a 44x32 pill thumb that overhangs it by 2px, so the
// handle reads as the thing to grab. Stops and pointer input share the
// thumb's centre inset; the fill runs a rail radius past the centre, always
// hidden under the thumb.
const SLIDER_HEIGHT: f32 = 36.0;
const RAIL_HEIGHT: f32 = 28.0;
const RAIL_TOP: f32 = (SLIDER_HEIGHT - RAIL_HEIGHT) / 2.0;
const THUMB_WIDTH: f32 = 44.0;
const THUMB_HEIGHT: f32 = 32.0;
const THUMB_INSET: f32 = THUMB_WIDTH / 2.0;
const FILL_PAST: f32 = RAIL_HEIGHT / 2.0;
/// Both header buttons fill the header's height, so they sit one card inset
/// from the card's top and sides and their radius is concentric with its
/// corners. 8pt of padding around the two-line title puts its text 12pt
/// from the card's top and leading edges.
const HEADER_HEIGHT: f32 = 48.0;
/// Flush with the card inset, a 15pt glyph centred in 32pt ends 12pt from
/// the card edge: the slider rail's end.
const FAST_BUTTON_WIDTH: f32 = 32.0;
/// The header inside the card inset, then the slider block and the space
/// above the option rows. The slider's 4pt below its box leaves the rail
/// 12pt from the card's bottom when no options follow.
const HEADER_BLOCK: f32 = 2.0 * popover::CARD_INSET + HEADER_HEIGHT;
const SLIDER_TOP: f32 = 2.0;
const SLIDER_BOTTOM: f32 = 4.0;
const EFFORT_BLOCK: f32 = SLIDER_TOP + SLIDER_HEIGHT + SLIDER_BOTTOM;
const OPTIONS_GAP: f32 = 4.0;

#[derive(Default)]
pub(super) struct CompactMotion {
    effort: Option<ScalarTransition>,
    press: Option<ScalarTransition>,
    drag_fraction: Option<f32>,
    energy: Option<ScalarTransition>,
    /// Shimmer clock for fast mode and the top effort levels, integrated
    /// per frame so changing intensity never jumps the band.
    shimmer_phase: f32,
    shimmer_last_frame: Option<std::time::Instant>,
    fast: Option<ScalarTransition>,
    height: Option<ScalarTransition>,
    page: Option<bool>,
    reveal: Option<ScalarTransition>,
    pub frame_height: f32,
}

pub(super) struct ScalarTransition {
    from: f32,
    to: f32,
    started: std::time::Instant,
}
impl ScalarTransition {
    pub(super) fn value(&self, now: std::time::Instant) -> f32 {
        let progress = motion::RESIZE.progress(
            now.duration_since(self.started).as_secs_f32()
                / motion::RESIZE
                    .total()
                    .mul_f32(motion::speed_scale())
                    .as_secs_f32(),
        );
        motion::lerp(self.from, self.to, progress)
    }
    pub(super) fn sample(
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
        // The brand mark names the provider; a description appears only to
        // tell identically named rows apart.
        let attribution: Option<SharedString> = row
            .model
            .description
            .as_deref()
            .map(str::trim)
            .filter(|d| row.ambiguous && !d.is_empty())
            .map(|d| SharedString::from(d.to_owned()));
        let hovered = self.active == ix;
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
            .h(px(COMPACT_ROW_HEIGHT))
            .pl(px(8.0))
            .pr(px(4.0))
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
                    .size(px(14.0))
                    .flex_none()
                    .text_color(tint.unwrap_or(theme.text_muted)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_baseline()
                    .gap(px(6.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .child(
                        div()
                            .flex_none()
                            .max_w_full()
                            .truncate()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(label),
                    )
                    .when_some(attribution, |el, attribution| {
                        el.child(
                            div()
                                .min_w_0()
                                .truncate()
                                .text_size(crate::typography::ui_rems(11.0))
                                .text_color(theme.text_muted)
                                .child(attribution),
                        )
                    }),
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
                    // Quiet until the row is hovered or the model is starred.
                    .when(!favorite && !hovered, |el| el.opacity(0.0))
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
        // Open on the current model: its provider's group at the top when
        // the model sits near the start of it, else centred on the model.
        let selected = self.selected_model_index(cx);
        self.active = selected;
        let start = self
            .compact_groups(cx)
            .into_iter()
            .rev()
            .find(|(_, start)| *start <= selected)
            .map_or(0, |(_, start)| start);
        if selected - start < COMPACT_LIST_ROWS as usize - 2 {
            // One row of the previous group stays above, under the edge
            // fade, so the group's first row is never washed out and the
            // list shows there is more above.
            self.model_scroll.scroll_to_item_strict_with_offset(
                start,
                gpui::ScrollStrategy::Top,
                usize::from(start > 0),
            );
        } else {
            self.model_scroll
                .scroll_to_item_strict(selected, gpui::ScrollStrategy::Center);
        }
        self.focus_on_mount = true;
        cx.notify();
    }

    /// Where each provider's group starts in the unsearched list: starred
    /// models first, then one run per provider.
    fn compact_groups(&self, cx: &App) -> Vec<(Option<HarnessId>, usize)> {
        let rows = self.model_rows(cx);
        let mut groups: Vec<(Option<HarnessId>, usize)> = Vec::new();
        for (ix, row) in rows.iter().enumerate() {
            let group =
                (!self.defaults.is_favorite(row.harness, &row.model.id)).then_some(row.harness);
            if groups.last().is_none_or(|(last, _)| *last != group) {
                groups.push((group, ix));
            }
        }
        groups
    }

    pub(super) fn compact_model_back_header(&self, cx: &mut Context<Self>) -> gpui::Div {
        let theme = Theme::of(cx).for_popup();
        let searching = !self.search.read(cx).text().trim().is_empty();
        let groups = if searching {
            Vec::new()
        } else {
            self.compact_groups(cx)
        };
        // The group whose rows sit at the top of the scroll lights its chip.
        // Rows share one pitch, so the offset names the top row directly.
        let top = (f32::from(-self.model_scroll_base().offset().y)
            / (COMPACT_ROW_HEIGHT + popover::MENU_GAP))
            .round()
            .max(0.0) as usize;
        let current = groups
            .iter()
            .rev()
            .find(|(_, start)| *start <= top)
            .map(|(group, _)| *group);
        let strip = (groups.len() > 1).then(|| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(2.0))
                .children(groups.into_iter().map(|(group, start)| {
                    let viewed = current == Some(group);
                    let (icon, tint, name): (&'static str, Option<gpui::Hsla>, SharedString) =
                        match group {
                            None => (crate::icons::STAR_BOLD, None, "Starred".into()),
                            Some(harness) => {
                                let (icon, tint) = harness_brand_icon(harness);
                                let name = self
                                    .rail_descriptors(cx)
                                    .into_iter()
                                    .find(|d| d.id == harness)
                                    .map(|d| d.name)
                                    .unwrap_or_default();
                                (icon, tint, name.into())
                            }
                        };
                    let hint = name.clone();
                    div()
                        .id(SharedString::from(format!("compact-group-{start}")))
                        .role(gpui::Role::Button)
                        .aria_label(SharedString::from(format!("Jump to {name}")))
                        .tooltip(move |_, cx| cx.new(|_| PickerHint(hint.clone())).into())
                        .size(px(26.0))
                        .rounded(px(7.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .when(viewed, |el| el.bg(crate::theme::ink(0.08)))
                        .when(!viewed, |el| {
                            el.opacity(0.7)
                                .hover(|s| s.bg(crate::theme::ink(0.05)).opacity(1.0))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.compact_keyboard = false;
                            this.active = start;
                            this.model_scroll
                                .scroll_to_item_strict(start, gpui::ScrollStrategy::Top);
                            cx.notify();
                        }))
                        .child(crate::icons::icon(icon).size(px(14.0)).text_color(
                            tint.unwrap_or(if viewed { theme.text } else { theme.text_muted }),
                        ))
                }))
        });
        div()
            .h(px(40.0))
            .flex_none()
            .px(px(popover::CARD_INSET))
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(
                popover::menu_row(&theme, false, "compact-model-back")
                    .id("compact-model-back")
                    .role(gpui::Role::Button)
                    .aria_label("Back to effort")
                    .flex_1()
                    .min_w_0()
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
                    .child(div().truncate().child("Models")),
            )
            .children(strip)
    }

    pub(super) fn compact_fast_choice(&self, cx: &App) -> Option<(String, String, bool, bool)> {
        let (option, on, off) = self
            .selected_model(cx)?
            .options
            .iter()
            .find_map(|o| fast_mode_values(o).map(|(on, off)| (o, on, off)))?;
        let options = self.explicit_options(cx);
        let fast = options
            .get(&option.id)
            .and_then(|v| v.as_str())
            .unwrap_or(&option.default_choice)
            == on;
        Some((
            option.id.clone(),
            if fast { off.to_owned() } else { on.to_owned() },
            fast,
            fast,
        ))
    }

    /// The selected model's fast-mode option id, whichever form it takes.
    pub(super) fn fast_option_id(&self, cx: &App) -> Option<String> {
        self.selected_model(cx)?
            .options
            .iter()
            .find(|o| fast_mode_values(o).is_some())
            .map(|o| o.id.clone())
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

    /// Effort and fast mode have their own controls in the compact card.
    pub(super) fn compact_option_visible(id: &ModelSetting, fast: Option<&str>) -> bool {
        !matches!(id, ModelSetting::Reasoning)
            && !matches!(id, ModelSetting::Option(id) if Some(id.as_str()) == fast)
    }

    fn compact_controls(&self, cx: &App) -> Vec<CompactControl> {
        let mut controls = vec![CompactControl::Model];
        if self.compact_fast_choice(cx).is_some() {
            controls.push(CompactControl::Fast);
        }
        if !self.trait_ladder(cx).is_empty() {
            controls.push(CompactControl::Effort);
        }
        let fast_id = self.fast_option_id(cx);
        controls.extend(
            self.setting_groups(cx)
                .into_iter()
                .filter(|g| Self::compact_option_visible(&g.id, fast_id.as_deref()))
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
        let fast_id = self.fast_option_id(cx);
        let options = self
            .setting_groups(cx)
            .iter()
            .filter(|g| Self::compact_option_visible(&g.id, fast_id.as_deref()))
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
            // Back header and search (40 each) above the list.
            80.0 + compact_list_height(
                self.model_rows_len(cx) + self.compact_catalog_statuses(cx).len(),
            ) + 2.0
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
        // The new page fades in while the card resizes to it: the swap
        // never shows at full strength and nothing slides sideways against
        // the vertical resize. Initial opening uses the popover entrance.
        let content = div().opacity(reveal).child(content).into_any_element();
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
                    let fast_id = self.fast_option_id(cx);
                    let index = self
                        .setting_groups(cx)
                        .iter()
                        .filter(|g| Self::compact_option_visible(&g.id, fast_id.as_deref()))
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
        let model_accessible: SharedString =
            format!("{} · {} · Change model", effort, label).into();
        let model_focus = self.compact_keyboard && self.compact_control == CompactControl::Model;
        let fast_focus = self.compact_keyboard && self.compact_control == CompactControl::Fast;
        let effort_focus = self.compact_keyboard && self.compact_control == CompactControl::Effort;
        let now = std::time::Instant::now();
        let reduced = cx.reduce_motion();
        // Fast mode alone brings the fill to life: a sheen and speed streaks
        // racing toward the thumb, and a breathing glow.
        let fast = self
            .compact_fast_choice(cx)
            .is_some_and(|(_, _, _, fast)| fast);
        let (intensity, powering) = ScalarTransition::sample(
            &mut self.compact_motion.energy,
            if fast { 1.0 } else { 0.0 },
            now,
            reduced,
        );
        if powering {
            window.request_animation_frame();
        }
        let dt = self
            .compact_motion
            .shimmer_last_frame
            .replace(now)
            .map_or(0.0, |last| now.duration_since(last).as_secs_f32().min(0.05));
        let alive =
            intensity > 0.001 && !reduced && window.is_window_active() && self.open.is_open();
        if alive {
            self.compact_motion.shimmer_phase =
                (self.compact_motion.shimmer_phase + dt * (0.32 + 0.22 * intensity)).fract();
            window.request_animation_frame();
        }
        let phase = self.compact_motion.shimmer_phase;
        let breath = if alive {
            0.5 + 0.5 * (std::f32::consts::TAU * phase).sin()
        } else {
            0.5
        };
        // The effort titles the model's name as one button filling the row
        // up to fast mode, its text on the rail's and option rows' edge; fast
        // mode is a small icon button whose glyph ends where the rail does.
        // The chevron leans toward the list on hover.
        let model_key: SharedString = format!("compact-model-link-{}", cx.entity_id()).into();
        let hover = motion::hover_t(&model_key);
        let lean = motion::EASE_OUT_QUINT.eval(hover);
        let title = div()
            .id("compact-select-model")
            .role(gpui::Role::Button)
            .aria_label(model_accessible)
            .flex_1()
            .min_w_0()
            .h_full()
            .px(px(8.0))
            .rounded(px(popover::MENU_ITEM_RADIUS))
            .bg(crate::theme::ink(0.05 * hover))
            .flex()
            .flex_col()
            .items_start()
            .justify_center()
            .cursor_pointer()
            .on_hover(motion::hover_listener(model_key))
            .when(model_focus, |el| {
                el.aria_active_descendant().shadow(focus_outline(&theme))
            })
            .on_click(cx.listener(|this, _, _, cx| {
                this.compact_keyboard = false;
                this.show_compact_models(cx);
            }))
            .when(!levels.is_empty(), |el| {
                el.child(
                    div()
                        .text_size(crate::typography::ui_rems(14.0))
                        .line_height(px(17.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .child(effort.clone()),
                )
            })
            .child(
                div()
                    .max_w_full()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .gap(px(3.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .line_height(px(15.0))
                    .text_color(motion::mix(theme.text_muted, theme.text, hover))
                    .child(div().min_w_0().truncate().child(label.clone()))
                    .child(
                        // Leans 3pt toward the list and firms up on hover.
                        div()
                            .flex_none()
                            .relative()
                            .left(px(3.0 * lean))
                            .opacity(0.55 + 0.45 * lean)
                            .child(
                                crate::icons::icon(crate::icons::ALT_ARROW_RIGHT)
                                    .size(px(10.0))
                                    .text_color(motion::mix(theme.text_muted, theme.text, lean)),
                            ),
                    ),
            );
        let mut fast_button = None;
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
            fast_button = Some(
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
                    // A faint neutral plate marks it as a control in both
                    // states; the glyph's colour is the state, and the
                    // slider's fill carries the accent.
                    .w(px(FAST_BUTTON_WIDTH))
                    .h_full()
                    .flex_none()
                    .rounded(px(popover::MENU_ITEM_RADIUS))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .bg(crate::theme::ink(0.04))
                    .hover(|s| s.bg(crate::theme::ink(0.07)))
                    .when(fast_focus, |el| {
                        el.aria_active_descendant().shadow(focus_outline(&theme))
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.compact_keyboard = false;
                        this.pick_option(option.clone(), choice.clone(), default, cx);
                    }))
                    .child(
                        crate::icons::icon(crate::icons::FAST_TIER)
                            .size(px(15.0))
                            .text_color(motion::mix(theme.text_muted, theme.accent, fast_t)),
                    ),
            );
        }
        // Flush with the card inset, like the option rows, so the title's
        // text and the rows' labels share one leading edge.
        let header = div()
            .h(px(HEADER_HEIGHT))
            .flex_none()
            .flex()
            .gap(px(popover::CARD_INSET))
            .child(title)
            .children(fast_button);
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
            let (press, pressing) = ScalarTransition::sample(
                &mut self.compact_motion.press,
                if self.effort_dragging { 1.0 } else { 0.0 },
                now,
                reduced,
            );
            if pressing {
                window.request_animation_frame();
            }
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
                .child(effort_fill(
                    &theme,
                    fraction,
                    levels.len(),
                    Shimmer {
                        intensity,
                        phase,
                        breath,
                    },
                ))
                .child({
                    // A plain handle that keeps its size; a press only lifts
                    // it on a softer, wider drop.
                    let (w, h) = (THUMB_WIDTH, THUMB_HEIGHT);
                    // Fast mode breathes a soft accent halo under the thumb.
                    let live = intensity * (0.55 + 0.45 * breath);
                    let place = |el: gpui::Div| {
                        el.absolute()
                            .left(gpui::relative(fraction))
                            .ml(px(-w / 2.0))
                            .w(px(w))
                            .h(px(h))
                            .rounded_full()
                    };
                    div()
                        .absolute()
                        .left(px(THUMB_INSET))
                        .right(px(THUMB_INSET))
                        .top(px((SLIDER_HEIGHT - h) / 2.0))
                        .when(intensity > 0.001, |el| {
                            el.child(place(div()).shadow(vec![gpui::BoxShadow {
                                color: theme.accent.opacity(0.35 * live),
                                offset: gpui::point(px(0.0), px(1.0)),
                                blur_radius: px(10.0),
                                spread_radius: px(0.0),
                                inset: false,
                            }]))
                        })
                        .when(press > 0.001, |el| {
                            el.child(place(div()).shadow(vec![gpui::BoxShadow {
                                color: gpui::black().opacity(0.12 * press),
                                offset: gpui::point(px(0.0), px(2.0 * press)),
                                blur_radius: px(8.0),
                                spread_radius: px(0.0),
                                inset: false,
                            }]))
                        })
                        .child(
                            crate::glass::thumb(place(div()), &theme)
                                .when(effort_focus, |el| el.border_2().border_color(theme.text)),
                        )
                });
            // The rail starts on the same edge as the title and row labels.
            panel = panel.child(
                div()
                    .px(px(8.0))
                    .pt(px(SLIDER_TOP))
                    .pb(px(SLIDER_BOTTOM))
                    .child(slider),
            );
        }
        let fast_id = self.fast_option_id(cx);
        let option_count = self
            .setting_groups(cx)
            .iter()
            .filter(|g| Self::compact_option_visible(&g.id, fast_id.as_deref()))
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
#[derive(Clone, Copy)]
struct Shimmer {
    /// 0 at rest, up to 1 for fast mode on a top level.
    intensity: f32,
    /// 0–1 position of the band's sweep.
    phase: f32,
    /// 0–1 breathing of the glow.
    breath: f32,
}

fn effort_fill(theme: &Theme, fraction: f32, count: usize, shimmer: Shimmer) -> AnyElement {
    let Shimmer {
        intensity,
        phase,
        breath,
    } = shimmer;
    // A contained glow that breathes; it must not wash over the option rows.
    let fill = crate::glass::accent_plate(theme, 1.0, intensity * (0.2 + 0.25 * breath));
    let accent = theme.accent;
    // Quiet stops: a hint of the ladder, not a second focal point.
    let filled_stop = gpui::white().opacity(0.4);
    let open_stop = theme.text_muted.opacity(0.28);
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
                rect(0.0, RAIL_TOP, (center + FILL_PAST).min(width), RAIL_HEIGHT),
                RAIL_HEIGHT / 2.0,
            );
            // Fast mode: a band of light flows through the whole fill toward
            // the thumb. Two slow waves drift at different speeds, shading
            // the fill between a lifted accent and a hue-shifted one, and the
            // light gathers just before the thumb. It is painted in thin
            // strips whose gradients join end to end; the end strips carry
            // the fill's corner radius, so it never leaves the rounded ends.
            let fill_end = (center + FILL_PAST).min(width);
            if intensity > 0.001 && fill_end > RAIL_HEIGHT {
                let inset = 1.0;
                let height = RAIL_HEIGHT - 2.0 * inset;
                let radius = height / 2.0;
                let (from, to) = (inset, fill_end - inset);
                let base = accent;
                let lifted = motion::mix(base, gpui::white(), 0.45);
                let shifted = gpui::hsla(
                    (base.h + 0.07).fract(),
                    base.s,
                    (base.l + 0.14).min(0.9),
                    1.0,
                );
                let tau = std::f32::consts::TAU;
                let flow = |x: f32| -> gpui::Hsla {
                    let slow = 0.5 + 0.5 * (tau * (x / 150.0 - 2.0 * phase)).sin();
                    let quick = 0.5 + 0.5 * (tau * (x / 64.0 - 3.0 * phase) + 1.7).sin();
                    let glow = (0.65 * slow + 0.35 * quick).powi(2);
                    let charge = (-(center - x).max(0.0) / 40.0).exp() * (0.6 + 0.4 * breath);
                    let lead = ((x - from) / 28.0).clamp(0.0, 1.0);
                    let alpha = intensity * ((0.08 + 0.34 * glow) * lead + 0.22 * charge);
                    motion::mix(shifted, lifted, slow).opacity(alpha.min(1.0))
                };
                let mut edges = vec![from, (from + radius).min(to)];
                let inner_end = (to - radius).max(edges[1]);
                let mut x = edges[1];
                while x + 4.0 < inner_end {
                    x += 4.0;
                    edges.push(x);
                }
                if inner_end > *edges.last().unwrap() {
                    edges.push(inner_end);
                }
                if to > *edges.last().unwrap() {
                    edges.push(to);
                }
                let last = edges.len().saturating_sub(2);
                for (i, pair) in edges.windows(2).enumerate() {
                    let (x0, x1) = (pair[0], pair[1]);
                    if x1 - x0 < 0.1 {
                        continue;
                    }
                    let r = px(radius.min(x1 - x0));
                    let zero = px(0.0);
                    let corners = gpui::Corners {
                        top_left: if i == 0 { r } else { zero },
                        bottom_left: if i == 0 { r } else { zero },
                        top_right: if i == last { r } else { zero },
                        bottom_right: if i == last { r } else { zero },
                    };
                    window.paint_quad(gpui::quad(
                        rect(x0, RAIL_TOP + inset, x1 - x0, height),
                        corners,
                        gpui::linear_gradient(
                            90.0,
                            gpui::linear_color_stop(flow(x0), 0.0),
                            gpui::linear_color_stop(flow(x1), 1.0),
                        ),
                        px(0.0),
                        gpui::transparent_black(),
                        gpui::BorderStyle::default(),
                    ));
                }
            }
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
        assert!((effort_fraction(68.0, width) - 0.25).abs() < 0.00001);
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
