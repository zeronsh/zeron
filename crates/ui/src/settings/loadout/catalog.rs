use std::collections::{HashMap, HashSet};

use gpui::AnimationExt;

use super::*;

const CATALOG_COLUMN_WIDTH: f32 = 176.0;
const CATALOG_COLUMN_GAP: f32 = 20.0;
const CATALOG_SCROLL_EDGE: f32 = 48.0;
const CATALOG_FADE_BAND: f32 = 24.0;
const CATALOG_ROW_STAGGER_MS: u64 = 28;
const CATALOG_ROW_STAGGER_CAP: u64 = 12;
const CATALOG_SCROLL_STEP: f32 = 120.0;
const CATALOG_SCROLLBAR_HIT_HEIGHT: f32 = 10.0;

/// The two OpenCode partitions are separate provider columns. The suffix is
/// part of the persisted identity, so the OpenRouter column can be moved
/// independently of the native OpenCode column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CatalogPartition {
    Native,
    OpenRouter,
}

impl CatalogPartition {
    fn is_openrouter(self) -> bool {
        matches!(self, Self::OpenRouter)
    }

    fn key(self) -> &'static str {
        match self {
            Self::Native => "default",
            Self::OpenRouter => "openrouter",
        }
    }
}

#[derive(Debug, Clone)]
struct CatalogColumn {
    descriptor: HarnessDescriptor,
    partition: CatalogPartition,
}

impl CatalogColumn {
    fn key(&self) -> String {
        provider_key(self.descriptor.id, self.partition)
    }

    fn name(&self) -> &'static str {
        if self.partition.is_openrouter() {
            "OpenRouter"
        } else {
            harness_display_name(self.descriptor.id)
        }
    }
}

/// Payload carried by a provider-column header drag. Model rows keep using
/// `LoadoutModelDrag`, so a provider reorder can never accidentally fill a
/// loadout slot.
#[derive(Debug, Clone)]
pub(super) struct ProviderColumnDrag {
    key: String,
    label: SharedString,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProviderColumnDragState {
    pub key: String,
    pub over: usize,
}

struct ProviderColumnDragGhost {
    label: SharedString,
}

struct CatalogScrollbarDrag;

impl Render for ProviderColumnDragGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .h(px(28.0))
            .max_w(px(CATALOG_COLUMN_WIDTH))
            .px(px(10.0))
            .flex()
            .items_center()
            .rounded(px(8.0))
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .text_size(px(12.0))
            .text_color(theme.text)
            .opacity(0.9)
            .child(div().min_w_0().truncate().child(self.label.clone()))
    }
}

impl Render for CatalogScrollbarDrag {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

pub(super) fn provider_key(harness: HarnessId, partition: CatalogPartition) -> String {
    let harness = serde_json::to_value(harness)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("{harness:?}").to_ascii_lowercase());
    format!("{harness}:{}", partition.key())
}

fn catalog_columns(list: &[HarnessDescriptor]) -> Vec<CatalogColumn> {
    visible_harnesses(list)
        .into_iter()
        .filter(|descriptor| {
            loadout_supports_harness(descriptor.id) && descriptor_enabled(descriptor)
        })
        .flat_map(|descriptor| {
            let native = CatalogColumn {
                descriptor: descriptor.clone(),
                partition: CatalogPartition::Native,
            };
            if descriptor.id == HarnessId::Opencode {
                vec![
                    native,
                    CatalogColumn {
                        descriptor,
                        partition: CatalogPartition::OpenRouter,
                    },
                ]
            } else {
                vec![native]
            }
        })
        .collect()
}

fn ordered_catalog_columns(
    list: &[HarnessDescriptor],
    provider_order: &[String],
) -> Vec<CatalogColumn> {
    let columns = catalog_columns(list);
    let mut by_key = columns
        .iter()
        .map(|column| (column.key(), column.clone()))
        .collect::<HashMap<_, _>>();
    let mut ordered = Vec::with_capacity(columns.len());
    let mut seen = HashSet::new();

    for key in provider_order {
        if seen.insert(key.clone()) {
            if let Some(column) = by_key.remove(key) {
                ordered.push(column);
            }
        }
    }
    for column in columns {
        if seen.insert(column.key()) {
            ordered.push(column);
        }
    }
    ordered
}

/// Reorder only visible provider keys while retaining unknown persisted keys.
/// Unknown keys can refer to a disabled or disconnected harness and must
/// survive a later reorder so user preferences are not silently erased.
pub(super) fn reorder_provider_order(
    stored: &[String],
    visible_keys: &[String],
    from: usize,
    over: usize,
) -> Vec<String> {
    if from >= visible_keys.len() || over >= visible_keys.len() || from == over {
        return stored.to_vec();
    }

    let mut visible = visible_keys.to_vec();
    let moved = visible.remove(from);
    visible.insert(over, moved);

    let visible_set: HashSet<&str> = visible_keys.iter().map(String::as_str).collect();
    let mut visible_iter = visible.iter();
    let mut result = Vec::with_capacity(stored.len().max(visible.len()));
    let mut seen_visible = HashSet::new();

    for key in stored {
        if visible_set.contains(key.as_str()) {
            if let Some(next) = visible_iter.next() {
                result.push(next.clone());
                seen_visible.insert(next.as_str());
            }
        } else {
            result.push(key.clone());
        }
    }
    result.extend(
        visible
            .iter()
            .filter(|key| !seen_visible.contains(key.as_str()))
            .cloned(),
    );
    result
}

fn is_openrouter_model(model: &Model) -> bool {
    model
        .id
        .split('/')
        .next()
        .is_some_and(|provider| provider.eq_ignore_ascii_case("openrouter"))
        || model.description.as_deref() == Some("OpenRouter")
}

fn canonical_model_id(model: &Model) -> String {
    model.id.clone()
}

fn filtered_catalog_models<'a>(
    models: &'a [Model],
    query: &str,
    partition: Option<bool>,
) -> Vec<&'a Model> {
    let query = query.trim();
    let mut seen = HashSet::new();
    let mut rows: Vec<(usize, usize, &'a Model)> = models
        .iter()
        .enumerate()
        .filter(|(_, model)| {
            partition.is_none_or(|openrouter| is_openrouter_model(model) == openrouter)
        })
        .filter(|(_, model)| seen.insert(canonical_model_id(model)))
        .filter_map(|(index, model)| {
            if query.is_empty() {
                return Some((0, index, model));
            }
            let haystack = format!(
                "{} {} {}",
                model.label,
                model.id,
                model.description.as_deref().unwrap_or("")
            );
            popover::match_rank(query, &haystack).map(|rank| (rank, index, model))
        })
        .collect();
    rows.sort_by_key(|(rank, index, _)| (*rank, *index));
    rows.into_iter().map(|(_, _, model)| model).collect()
}

fn catalog_drop_index(relative_x: f32, count: usize) -> usize {
    if count == 0 {
        return 0;
    }
    let slot = CATALOG_COLUMN_WIDTH + CATALOG_COLUMN_GAP;
    ((relative_x.max(0.0) / slot).floor() as usize).min(count - 1)
}

fn catalog_scroll_target(current: f32, max: f32, pointer_x: f32, left: f32, right: f32) -> f32 {
    if max <= 0.0 {
        return current.clamp(0.0, max.max(0.0));
    }
    let delta = if pointer_x < left + CATALOG_SCROLL_EDGE {
        -((left + CATALOG_SCROLL_EDGE - pointer_x) / CATALOG_SCROLL_EDGE).clamp(0.0, 1.0)
            * CATALOG_SCROLL_STEP
    } else if pointer_x > right - CATALOG_SCROLL_EDGE {
        ((pointer_x - (right - CATALOG_SCROLL_EDGE)) / CATALOG_SCROLL_EDGE).clamp(0.0, 1.0)
            * CATALOG_SCROLL_STEP
    } else {
        0.0
    };
    (current + delta).clamp(0.0, max)
}

fn catalog_column_entrance(
    store: &mut HashMap<String, std::time::Instant>,
    key: &str,
) -> std::time::Instant {
    *store
        .entry(key.to_string())
        .or_insert_with(std::time::Instant::now)
}

fn catalog_row_entrance(
    ix: usize,
    started: Option<std::time::Instant>,
    reduce: bool,
) -> Option<u64> {
    if reduce {
        return None;
    }
    let started = started?;
    let delay = (ix as u64).min(CATALOG_ROW_STAGGER_CAP) * CATALOG_ROW_STAGGER_MS;
    (started.elapsed().as_millis() < u128::from(crate::motion::FADE_IN.duration_ms + delay))
        .then_some(delay)
}

impl LoadoutPage {
    fn on_catalog_key_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let max = f32::from(self.catalog_scroll.max_offset().x).max(0.0);
        if max <= 0.0 {
            return;
        }
        let current = (-f32::from(self.catalog_scroll.offset().x)).clamp(0.0, max);
        let viewport = f32::from(self.catalog_scroll.bounds().size.width).max(CATALOG_SCROLL_STEP);
        let target = match event.keystroke.key.as_str() {
            "left" => Some(current - CATALOG_SCROLL_STEP),
            "right" => Some(current + CATALOG_SCROLL_STEP),
            "pageup" => Some(current - viewport),
            "pagedown" => Some(current + viewport),
            "home" => Some(0.0),
            "end" => Some(max),
            _ => None,
        };
        let Some(target) = target else {
            return;
        };
        self.catalog_scroll.set_offset(gpui::point(
            px(-target.clamp(0.0, max)),
            self.catalog_scroll.offset().y,
        ));
        window.prevent_default();
        cx.stop_propagation();
        cx.notify();
    }

    fn on_catalog_drag_move(
        &mut self,
        event: &gpui::DragMoveEvent<ProviderColumnDrag>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.bounds.contains(&event.event.position) {
            if self.catalog_drag.take().is_some() {
                cx.notify();
            }
            return;
        }
        let payload = event.drag(cx);
        let Some(list) = self.harnesses.ready() else {
            return;
        };
        let columns = ordered_catalog_columns(list, &self.loadout.provider_order);
        let visible_keys: Vec<String> = columns.iter().map(CatalogColumn::key).collect();
        let Some(_from) = visible_keys.iter().position(|key| key == &payload.key) else {
            return;
        };

        let left = f32::from(event.bounds.left());
        let right = f32::from(event.bounds.right());
        let current = (-f32::from(self.catalog_scroll.offset().x)).max(0.0);
        let max = f32::from(self.catalog_scroll.max_offset().x).max(0.0);
        let target =
            catalog_scroll_target(current, max, f32::from(event.event.position.x), left, right);
        if (target - current).abs() > f32::EPSILON {
            self.catalog_scroll
                .set_offset(gpui::point(px(-target), self.catalog_scroll.offset().y));
        }
        let relative_x = f32::from(event.event.position.x) - left - PAGE_GUTTER + target;
        let over = catalog_drop_index(relative_x, visible_keys.len());
        let next = ProviderColumnDragState {
            key: payload.key.clone(),
            over,
        };
        if self.catalog_drag.as_ref() != Some(&next) {
            self.catalog_drag = Some(next);
            cx.notify();
        }
    }

    fn commit_catalog_reorder(&mut self, payload: &ProviderColumnDrag, cx: &mut Context<Self>) {
        let Some(list) = self.harnesses.ready() else {
            self.catalog_drag = None;
            return;
        };
        let columns = ordered_catalog_columns(list, &self.loadout.provider_order);
        let visible_keys: Vec<String> = columns.iter().map(CatalogColumn::key).collect();
        let Some(from) = visible_keys.iter().position(|key| key == &payload.key) else {
            self.catalog_drag = None;
            cx.notify();
            return;
        };
        let over = self
            .catalog_drag
            .as_ref()
            .filter(|drag| drag.key == payload.key)
            .map(|drag| drag.over)
            .unwrap_or(from);
        let next = reorder_provider_order(&self.loadout.provider_order, &visible_keys, from, over);
        self.catalog_drag = None;
        if next != self.loadout.provider_order {
            self.loadout.provider_order = next;
            self.commit(cx);
        } else {
            cx.notify();
        }
    }

    fn render_catalog_scrollbar(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !self.catalog_scrollbar.visible() {
            return None;
        }
        let metrics = self.catalog_scrollbar.metrics(&self.catalog_scroll)?;
        let active = self.catalog_scrollbar.active();
        let thumb_height = if active {
            popover::MENU_SCROLLBAR_HOVER_THUMB_WIDTH
        } else {
            popover::MENU_SCROLLBAR_THUMB_WIDTH
        };
        Some(
            div()
                .id("loadout-catalog-scrollbar")
                .absolute()
                .left_0()
                .right_0()
                .bottom_0()
                .h(px(CATALOG_SCROLLBAR_HIT_HEIGHT))
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    if this.catalog_scrollbar.set_bar_hovered(*hovered) {
                        cx.notify();
                    }
                }))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                        window.prevent_default();
                        if this
                            .catalog_scrollbar
                            .begin_press(&this.catalog_scroll, event.position.x)
                        {
                            cx.stop_propagation();
                            cx.notify();
                        }
                    }),
                )
                .on_drag(CatalogScrollbarDrag, |_, _, _, cx| {
                    cx.stop_propagation();
                    cx.new(|_| CatalogScrollbarDrag)
                })
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if this.catalog_scrollbar.end_press() {
                            cx.notify();
                        }
                    }),
                )
                .on_mouse_up_out(
                    gpui::MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        if this.catalog_scrollbar.end_press() {
                            cx.notify();
                        }
                    }),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(popover::MENU_SCROLLBAR_TRACK_INSET + metrics.thumb_left))
                        .bottom(px(2.0))
                        .w(px(metrics.thumb_width))
                        .h(px(thumb_height))
                        .rounded(px(thumb_height / 2.0))
                        .bg(theme.text_faint.opacity(if active { 0.68 } else { 0.5 })),
                )
                .into_any_element(),
        )
    }

    fn render_provider_column(
        &mut self,
        column: &CatalogColumn,
        index: usize,
        total_columns: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let descriptor = &column.descriptor;
        let name = column.name();
        let key = column.key();
        let column_selector = format!("loadout-catalog-column-{key}");
        let handle_selector = format!("loadout-catalog-handle-{key}");
        let (icon_path, tint) = harness_brand_icon(descriptor.id);
        let drag_indicator = self.catalog_drag.as_ref().and_then(|drag| {
            (drag.over == index && drag.key != key).then(|| {
                let from = self
                    .harnesses
                    .ready()
                    .map(|list| ordered_catalog_columns(list, &self.loadout.provider_order))
                    .and_then(|columns| columns.iter().position(|column| column.key() == drag.key))
                    .unwrap_or(index);
                let place_after = from < index;
                let indicator_selector = format!("loadout-catalog-drop-indicator-{index}");
                div()
                    .debug_selector(move || indicator_selector.clone().into())
                    .absolute()
                    .top(px(2.0))
                    .bottom(px(2.0))
                    .w(px(2.0))
                    .rounded_full()
                    .bg(theme.accent)
                    .when(place_after, |line| line.right(px(-11.0)))
                    .when(!place_after, |line| line.left(px(-11.0)))
            })
        });
        let header_label: SharedString = name.into();
        let mut column_element = div()
            .id(SharedString::from(format!("loadout-provider-{key}")))
            .debug_selector(move || column_selector.clone().into())
            .relative()
            .flex()
            .flex_col()
            .w(px(CATALOG_COLUMN_WIDTH))
            .flex_none()
            .aria_column_index(index + 1)
            .aria_column_count(total_columns)
            .child(
                div()
                    .h(px(28.0))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .mb(px(8.0))
                    .child(
                        div()
                            .id(SharedString::from(format!("loadout-provider-handle-{key}")))
                            .debug_selector(move || handle_selector.clone().into())
                            .px(px(2.0))
                            .cursor_pointer()
                            .aria_label(format!("Reorder {name} provider column"))
                            .on_drag(
                                ProviderColumnDrag {
                                    key: key.clone(),
                                    label: header_label.clone(),
                                },
                                |payload, _, _, cx| {
                                    cx.stop_propagation();
                                    cx.new(|_| ProviderColumnDragGhost {
                                        label: payload.label.clone(),
                                    })
                                },
                            )
                            .child(
                                icon(icons::DRAG_HANDLE)
                                    .size(px(12.0))
                                    .text_color(theme.text_muted.opacity(0.65)),
                            ),
                    )
                    .child(
                        icon(icon_path)
                            .size(px(14.0))
                            .text_color(tint.unwrap_or(theme.text)),
                    )
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(13.0))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.text)
                            .child(header_label),
                    ),
            )
            .children(drag_indicator);

        if !descriptor.installed {
            let label = format!("Configure {name}");
            return column_element.child(
                div()
                    .id(SharedString::from(format!("loadout-configure-{key}")))
                    .h(px(36.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_dashed()
                    .border_color(theme.border)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(ink(0.04)))
                    .on_click(cx.listener(|this, _, _, cx| {
                        cx.emit(LoadoutEvent::OpenAgents);
                        this.close_menus(cx);
                    }))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(label)),
                    ),
            );
        }

        match self.models.get(&descriptor.id).cloned() {
            Some(Loadable::Ready(models)) => {
                let entrance = Some(catalog_column_entrance(&mut self.catalog_entrance, &key));
                let query = self.search.read(cx).text();
                let filtered = filtered_catalog_models(
                    &models,
                    query,
                    (descriptor.id == HarnessId::Opencode)
                        .then_some(column.partition.is_openrouter()),
                );
                let reduce = cx.reduce_motion();
                let total = filtered.len();
                let visible = total.min(MODEL_COLUMN_LIMIT);
                let rows: Vec<_> = filtered
                    .into_iter()
                    .take(visible)
                    .enumerate()
                    .map(|(ix, model)| {
                        let harness = descriptor.id;
                        let drag = LoadoutModelDrag {
                            harness,
                            model_id: model.id.clone(),
                            label: model.label.clone(),
                        };
                        let in_loadout = self.loadout.slots.iter().any(|slot| {
                            slot.as_ref().is_some_and(|slot| {
                                slot.harness == harness && slot.model == model.id
                            })
                        });
                        let row_selector = format!("loadout-catalog-row-{key}-{ix}");
                        let row = div()
                            .id(SharedString::from(format!("loadout-source-{key}-{ix}")))
                            .debug_selector(move || row_selector.clone().into())
                            .mb(px(6.0))
                            .h(px(36.0))
                            .px(px(10.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme.border)
                            .bg(ink(0.03))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .cursor_pointer()
                            .hover(|s| s.bg(ink(0.05)))
                            .aria_label(format!("Add {} from {name}", model.label))
                            .on_drag(drag, |payload, cursor_offset, _, cx| {
                                cx.stop_propagation();
                                let label = payload.label.clone().into();
                                cx.new(|_| LoadoutDragGhost {
                                    label,
                                    cursor_offset,
                                })
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(crate::typography::ui_rems(12.5))
                                    .text_color(theme.text)
                                    .child(SharedString::from(model.label.clone())),
                            )
                            .when(in_loadout, |el| {
                                el.child(
                                    icon(icons::CHECK)
                                        .size(px(12.0))
                                        .text_color(theme.text_muted),
                                )
                            })
                            .child(
                                icon(icons::DRAG_HANDLE)
                                    .size(px(12.0))
                                    .text_color(theme.text_muted.opacity(0.55)),
                            );
                        if let Some(delay) = catalog_row_entrance(ix, entrance, reduce) {
                            row.with_animation(
                                SharedString::from(format!("loadout-catalog-enter-{key}-{ix}")),
                                crate::motion::FADE_IN.with_delay(delay).animation(),
                                |el, t| el.opacity(t),
                            )
                            .into_any_element()
                        } else {
                            row.into_any_element()
                        }
                    })
                    .collect();
                column_element = column_element.children(rows);
                if visible < total {
                    column_element = column_element.child(
                        div()
                            .pt(px(4.0))
                            .text_size(crate::typography::ui_rems(11.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(format!(
                                "Showing {visible} of {total}. Search to narrow."
                            ))),
                    );
                }
            }
            Some(Loadable::Error(err)) => {
                column_element = column_element.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.danger_muted)
                        .child(SharedString::from(err)),
                );
            }
            _ => {
                column_element = column_element.child(
                    div()
                        .h(px(36.0))
                        .rounded(px(8.0))
                        .bg(ink(0.04))
                        .border_1()
                        .border_color(theme.border),
                );
            }
        }
        column_element
    }

    pub(super) fn render_catalog(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        match &self.harnesses {
            Loadable::Error(err) => widgets::error_strip(&theme, err.clone()).into_any_element(),
            Loadable::Idle | Loadable::Loading => div()
                .mt(px(24.0))
                .text_color(theme.text_muted)
                .child(SharedString::from("Loading models…"))
                .into_any_element(),
            Loadable::Ready(list) => {
                let columns = ordered_catalog_columns(list, &self.loadout.provider_order);
                let total = columns.len();
                let elements: Vec<_> = columns
                    .iter()
                    .enumerate()
                    .map(|(index, column)| {
                        self.render_provider_column(column, index, total, theme, cx)
                    })
                    .collect();
                let scrollbar = self.render_catalog_scrollbar(theme, cx);
                let mut scroller = div()
                    .id("loadout-catalog-scroll")
                    .debug_selector(|| "loadout-catalog-scroll".into())
                    .px(px(PAGE_GUTTER))
                    .relative()
                    .w_full()
                    .min_w_0()
                    .mt(px(28.0))
                    .pb(px(14.0))
                    .flex()
                    .flex_row()
                    .items_start()
                    .gap(px(CATALOG_COLUMN_GAP))
                    .overflow_x_scroll()
                    .track_scroll(&self.catalog_scroll)
                    .track_focus(&self.catalog_focus)
                    .tab_index(0)
                    .tab_group()
                    .role(gpui::Role::Group)
                    .aria_label("Model providers")
                    .aria_description(
                        "Use horizontal scrolling or the arrow keys to reach every provider column.",
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.catalog_focus.focus(window, cx);
                        }),
                    )
                    .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                        if this.catalog_scrollbar.set_viewport_hovered(*hovered) {
                            cx.notify();
                        }
                    }))
                    .on_key_down(cx.listener(Self::on_catalog_key_down))
                    .on_drag_move::<ProviderColumnDrag>(cx.listener(Self::on_catalog_drag_move))
                    .on_drop::<ProviderColumnDrag>(cx.listener(
                        |this, payload: &ProviderColumnDrag, _, cx| {
                            this.commit_catalog_reorder(payload, cx);
                        },
                    ))
                    .children(elements);
                scroller.style().restrict_scroll_to_axis = Some(true);
                div()
                    .id("loadout-catalog-frame")
                    .relative()
                    .mx(px(-PAGE_GUTTER))
                    .min_w_0()
                    .on_drag_move::<CatalogScrollbarDrag>(cx.listener(
                        |this, event: &gpui::DragMoveEvent<CatalogScrollbarDrag>, _, cx| {
                            if this
                                .catalog_scrollbar
                                .drag_to(&this.catalog_scroll, event.event.position.x)
                            {
                                cx.stop_propagation();
                                cx.notify();
                            }
                        },
                    ))
                    .child(
                        crate::edge_fade::edge_faded(CATALOG_FADE_BAND, false, false, scroller)
                            .fade_left(true)
                            .fade_right(true)
                            .fade_overflow_x(&self.catalog_scroll),
                    )
                    .children(scrollbar)
                    .into_any_element()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, description: Option<&str>) -> Model {
        Model {
            id: id.into(),
            label: id.into(),
            description: description.map(str::to_owned),
            reasoning_levels: Vec::new(),
            options: Vec::new(),
        }
    }

    #[test]
    fn provider_keys_keep_native_and_openrouter_opencode_separate() {
        assert_eq!(
            provider_key(HarnessId::Opencode, CatalogPartition::Native),
            "opencode:default"
        );
        assert_eq!(
            provider_key(HarnessId::Opencode, CatalogPartition::OpenRouter),
            "opencode:openrouter"
        );
    }

    #[test]
    fn reorder_keeps_hidden_provider_keys_and_updates_visible_order() {
        let stored = vec![
            "claude-code:default".into(),
            "disabled-harness:default".into(),
            "opencode:default".into(),
            "opencode:openrouter".into(),
        ];
        let visible = vec![
            "claude-code:default".into(),
            "opencode:default".into(),
            "opencode:openrouter".into(),
        ];
        assert_eq!(
            reorder_provider_order(&stored, &visible, 0, 2),
            vec![
                "opencode:default",
                "disabled-harness:default",
                "opencode:openrouter",
                "claude-code:default",
            ]
        );
    }

    #[test]
    fn reorder_rejects_stale_indices_without_mutating_preferences() {
        let stored = vec!["a".into(), "hidden".into(), "b".into()];
        let visible = vec!["a".into(), "b".into()];
        assert_eq!(reorder_provider_order(&stored, &visible, 9, 0), stored);
        assert_eq!(reorder_provider_order(&stored, &visible, 0, 9), stored);
        assert_eq!(reorder_provider_order(&stored, &visible, 1, 1), stored);
    }

    #[test]
    fn model_partition_deduplicates_canonical_ids_and_searches_each_partition() {
        let models = vec![
            model("openrouter/z-ai/glm", Some("OpenRouter")),
            model("openrouter/z-ai/glm", Some("OpenRouter")),
            model("OPENROUTER/Z-AI/GLM", Some("OpenRouter")),
            model("anthropic/opus", Some("Anthropic")),
            model("local/opus", Some("Local")),
        ];
        let openrouter = filtered_catalog_models(&models, "glm", Some(true));
        assert_eq!(openrouter.len(), 2);
        assert_eq!(openrouter[0].id, "openrouter/z-ai/glm");
        assert_eq!(openrouter[1].id, "OPENROUTER/Z-AI/GLM");
        let native = filtered_catalog_models(&models, "opus", Some(false));
        assert_eq!(native.len(), 2);
        assert!(filtered_catalog_models(&models, "glm", Some(false)).is_empty());
    }

    #[test]
    fn non_opencode_catalog_keeps_openrouter_mentions() {
        let models = vec![
            model("provider/model", Some("Uses OpenRouter for routing")),
            model("provider/other", Some("Provider")),
        ];
        let models = filtered_catalog_models(&models, "", None);
        assert_eq!(models.len(), 2);
    }

    #[test]
    fn drag_autoscroll_clamps_and_scales_at_edges() {
        assert_eq!(catalog_scroll_target(0.0, 500.0, 0.0, 0.0, 400.0), 0.0);
        assert_eq!(
            catalog_scroll_target(500.0, 500.0, 400.0, 0.0, 400.0),
            500.0
        );
        assert!(catalog_scroll_target(100.0, 500.0, 395.0, 0.0, 400.0) > 100.0);
        assert!(catalog_scroll_target(100.0, 500.0, 5.0, 0.0, 400.0) < 100.0);
    }

    #[test]
    fn drop_index_reaches_the_last_provider() {
        assert_eq!(catalog_drop_index(0.0, 4), 0);
        assert_eq!(
            catalog_drop_index(CATALOG_COLUMN_WIDTH + CATALOG_COLUMN_GAP, 4),
            1
        );
        assert_eq!(catalog_drop_index(10_000.0, 4), 3);
        assert_eq!(catalog_drop_index(0.0, 0), 0);
    }

    #[test]
    fn catalog_row_entrance_staggers_then_expires() {
        let now = std::time::Instant::now();
        assert_eq!(catalog_row_entrance(0, Some(now), false), Some(0));
        assert_eq!(
            catalog_row_entrance(3, Some(now), false),
            Some(3 * CATALOG_ROW_STAGGER_MS)
        );
        assert_eq!(
            catalog_row_entrance(20, Some(now), false),
            Some(CATALOG_ROW_STAGGER_CAP * CATALOG_ROW_STAGGER_MS)
        );
        assert_eq!(catalog_row_entrance(0, Some(now), true), None);
        assert_eq!(catalog_row_entrance(0, None, false), None);
        let expired = now - std::time::Duration::from_secs(2);
        assert_eq!(catalog_row_entrance(0, Some(expired), false), None);
    }

    #[test]
    fn late_column_starts_its_own_fade() {
        let mut store = HashMap::new();
        store.insert(
            "claude-code:default".into(),
            std::time::Instant::now() - std::time::Duration::from_secs(2),
        );
        assert_eq!(
            catalog_row_entrance(0, store.get("claude-code:default").copied(), false),
            None
        );
        let cursor = catalog_column_entrance(&mut store, "cursor:default");
        assert_eq!(catalog_row_entrance(0, Some(cursor), false), Some(0));
        assert_eq!(
            catalog_column_entrance(&mut store, "cursor:default"),
            cursor
        );
    }
}

#[cfg(test)]
mod rendered_tests {
    use super::*;
    use gpui::{TestAppContext, size};

    fn descriptor(id: HarnessId, name: &str) -> HarnessDescriptor {
        HarnessDescriptor {
            id,
            name: name.into(),
            supports_steering: false,
            steering_mode: zeron_proto::SteeringMode::StepBoundary,
            reasoning_levels: Vec::new(),
            installed: true,
            enabled: Some(true),
        }
    }

    fn fixture_page(cx: &mut Context<LoadoutPage>) -> LoadoutPage {
        fixture_page_with_model_count(cx, 1)
    }

    fn fixture_page_with_model_count(
        cx: &mut Context<LoadoutPage>,
        model_count: usize,
    ) -> LoadoutPage {
        let state = cx.new(|_| AppState::new());
        let mut page =
            LoadoutPage::new(state, LoadoutConfig::default(), KeymapConfig::default(), cx);
        page.harnesses = Loadable::Ready(vec![
            descriptor(HarnessId::ClaudeCode, "Claude Code"),
            descriptor(HarnessId::Codex, "Codex"),
            descriptor(HarnessId::Cursor, "Cursor"),
            descriptor(HarnessId::Grok, "Grok"),
            descriptor(HarnessId::Opencode, "OpenCode"),
        ]);
        for harness in [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Cursor,
            HarnessId::Grok,
            HarnessId::Opencode,
        ] {
            let models = (0..model_count)
                .map(|index| Model {
                    id: format!("{harness:?}-model-{index}"),
                    label: format!("{harness:?} model {index}"),
                    description: None,
                    reasoning_levels: Vec::new(),
                    options: Vec::new(),
                })
                .collect();
            page.models.insert(harness, Loadable::Ready(models));
        }
        page
    }

    #[gpui::test]
    fn catalog_model_drag_follows_pointer_and_fills_next_slot(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|_, cx| fixture_page(cx));
        cx.simulate_resize(size(px(1000.0), px(800.0)));
        cx.refresh().unwrap();
        let source = cx
            .debug_bounds("loadout-catalog-row-claude-code:default-0")
            .unwrap();
        let target = cx.debug_bounds("loadout-slot-0").unwrap().center();
        // Grab near the far edge, where the old source-relative ghost offset was largest.
        let grab = gpui::point(source.right() - px(8.0), source.center().y);
        cx.simulate_mouse_down(grab, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.simulate_mouse_move(
            grab + gpui::point(px(12.0), px(0.0)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            target,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.refresh().unwrap();
        let ghost = cx
            .debug_bounds("loadout-drag-ghost")
            .expect("drag preview must render");
        assert!(
            (ghost.left() - target.x).abs() <= px(20.0),
            "preview should stay beside the cursor: {ghost:?}, {target:?}"
        );
        assert!((ghost.top() - target.y).abs() <= px(24.0));
        cx.simulate_mouse_up(target, gpui::MouseButton::Left, gpui::Modifiers::default());
        cx.refresh().unwrap();
        page.read_with(cx, |page, _| {
            let slot = page.loadout.slot(0).unwrap();
            assert_eq!(slot.harness, HarnessId::ClaudeCode);
            assert_eq!(slot.model, "ClaudeCode-model-0");
        });
        assert!(cx.debug_bounds("loadout-slot-1").is_some());
        assert!(cx.debug_bounds("loadout-slot-2").is_none());
    }

    #[gpui::test]
    fn rendered_loadout_page_catalog_reaches_final_provider_column(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let page = fixture_page(cx);
            window.focus(&page.catalog_focus, cx);
            page
        });
        cx.simulate_resize(size(px(360.0), px(800.0)));
        cx.refresh().unwrap();
        cx.run_until_parked();

        let viewport = cx
            .debug_bounds("loadout-catalog-scroll")
            .expect("the real LoadoutPage catalog scroller must render");
        assert_eq!(
            viewport.left(),
            px(0.0),
            "catalog must clip at the content pane edge"
        );
        assert_eq!(viewport.right(), px(360.0));
        let first_column = cx
            .debug_bounds("loadout-catalog-column-claude-code:default")
            .unwrap();
        assert_eq!(
            first_column.left(),
            px(28.0),
            "column alignment must stay unchanged"
        );
        let final_column = cx
            .debug_bounds("loadout-catalog-column-opencode:openrouter")
            .expect("the final OpenRouter provider column must render");
        assert!(final_column.origin.x >= viewport.origin.x + viewport.size.width);

        let (bounds, max_offset) = page.read_with(cx, |page, _| {
            (
                page.catalog_scroll.bounds(),
                page.catalog_scroll.max_offset(),
            )
        });
        assert!(bounds.size.width > px(0.0));
        assert!(
            max_offset.x > px(0.0),
            "provider columns must create a horizontal scroll range"
        );

        let wheel_position = viewport.origin + gpui::point(px(12.0), px(12.0));
        cx.simulate_mouse_move(wheel_position, None, gpui::Modifiers::default());
        cx.refresh().unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: wheel_position,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(-96.0), px(0.0))),
            touch_phase: gpui::TouchPhase::Moved,
            ..Default::default()
        });
        cx.run_until_parked();
        let wheel_offset = page.read_with(cx, |page, _| page.catalog_scroll.offset());
        assert!(
            wheel_offset.x < px(0.0),
            "a horizontal trackpad delta over the real catalog must move its scroll handle: offset={wheel_offset:?}, viewport={viewport:?}"
        );

        cx.simulate_keystrokes("end");
        cx.run_until_parked();
        let offset = page.read_with(cx, |page, _| page.catalog_scroll.offset());
        assert_eq!(offset.x, -max_offset.x);

        let final_column = cx
            .debug_bounds("loadout-catalog-column-opencode:openrouter")
            .expect("the final provider column must remain addressable after scrolling");
        let viewport_right = viewport.origin.x + viewport.size.width;
        assert!(final_column.origin.x >= viewport.origin.x);
        assert!(final_column.origin.x + final_column.size.width <= viewport_right + px(1.0));
    }

    #[gpui::test]
    fn rendered_loadout_page_catalog_rows_scroll_the_outer_page(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let page = fixture_page_with_model_count(cx, 24);
            window.focus(&page.catalog_focus, cx);
            page
        });
        cx.simulate_resize(size(px(520.0), px(800.0)));
        cx.refresh().unwrap();
        cx.run_until_parked();

        let first_row = cx
            .debug_bounds("loadout-catalog-row-claude-code:default-0")
            .expect("the real catalog must render its first model row");
        let lower_row = cx
            .debug_bounds("loadout-catalog-row-claude-code:default-20")
            .expect("the real catalog must render lower model rows");
        let (page_bounds, max_offset) = page.read_with(cx, |page, _| {
            (page.page_scroll.bounds(), page.page_scroll.max_offset())
        });
        assert!(first_row.origin.y < page_bounds.origin.y + page_bounds.size.height);
        assert!(
            lower_row.origin.y >= page_bounds.origin.y + page_bounds.size.height,
            "lower catalog rows should initially be below the page viewport: row={lower_row:?}, page={page_bounds:?}"
        );
        assert!(
            max_offset.y > px(0.0),
            "the outer LoadoutPage must have vertical overflow for the model rows"
        );

        let wheel_position = first_row.center();
        cx.simulate_mouse_move(wheel_position, None, gpui::Modifiers::default());
        cx.refresh().unwrap();
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: wheel_position,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-10_000.0))),
            touch_phase: gpui::TouchPhase::Moved,
            ..Default::default()
        });
        cx.run_until_parked();

        let offset = page.read_with(cx, |page, _| page.page_scroll.offset());
        assert!(
            offset.y < px(0.0),
            "vertical wheel input over a catalog row must move the outer page: offset={offset:?}"
        );
        cx.refresh().unwrap();
        let lower_row_after_scroll = cx
            .debug_bounds("loadout-catalog-row-claude-code:default-20")
            .expect("the lower catalog row must remain rendered after outer scrolling");
        assert!(
            lower_row_after_scroll.origin.y < page_bounds.origin.y + page_bounds.size.height,
            "outer scrolling must make lower model rows reachable: row={lower_row_after_scroll:?}, page={page_bounds:?}"
        );
    }

    #[gpui::test]
    fn rendered_loadout_page_catalog_headers_drag_and_persist_order(cx: &mut TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let page = fixture_page(cx);
            window.focus(&page.catalog_focus, cx);
            page
        });
        cx.simulate_resize(size(px(1600.0), px(800.0)));
        cx.refresh().unwrap();
        cx.run_until_parked();

        let source = cx
            .debug_bounds("loadout-catalog-handle-claude-code:default")
            .expect("the Claude Code provider header must expose a drag handle");
        let target = cx
            .debug_bounds("loadout-catalog-column-cursor:default")
            .expect("the Cursor provider column must render as a drop target");
        let source_position = source.center();
        let target_position = target.center();
        cx.simulate_mouse_down(
            source_position,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            source_position + gpui::point(px(12.0), px(0.0)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            target_position,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.refresh().unwrap();
        cx.run_until_parked();

        assert!(
            cx.debug_bounds("loadout-catalog-drop-indicator-2")
                .is_some(),
            "dragging a provider header over a later column must render insertion feedback"
        );
        page.read_with(cx, |page, _| {
            assert_eq!(
                page.catalog_drag,
                Some(ProviderColumnDragState {
                    key: "claude-code:default".into(),
                    over: 2,
                })
            );
        });

        cx.simulate_mouse_move(
            gpui::point(px(8.0), px(8.0)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("loadout-catalog-drop-indicator-2")
                .is_none(),
            "provider insertion feedback must clear when the drag leaves the catalog"
        );
        page.read_with(cx, |page, _| assert!(page.catalog_drag.is_none()));
        cx.simulate_mouse_up(
            gpui::point(px(8.0), px(8.0)),
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );

        let source = cx
            .debug_bounds("loadout-catalog-handle-claude-code:default")
            .expect("the provider header must remain draggable after leaving the catalog");
        let target = cx
            .debug_bounds("loadout-catalog-column-cursor:default")
            .expect("the Cursor provider column must remain a drop target");
        let source_position = source.center();
        let target_position = target.center();
        cx.simulate_mouse_down(
            source_position,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            source_position + gpui::point(px(12.0), px(0.0)),
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.simulate_mouse_move(
            target_position,
            Some(gpui::MouseButton::Left),
            gpui::Modifiers::default(),
        );
        cx.refresh().unwrap();
        cx.run_until_parked();

        cx.simulate_mouse_up(
            target_position,
            gpui::MouseButton::Left,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert_eq!(
                page.loadout.provider_order,
                vec![
                    "codex:default",
                    "cursor:default",
                    "claude-code:default",
                    "grok:default",
                    "opencode:default",
                    "opencode:openrouter",
                ]
            );
            assert!(page.catalog_drag.is_none());
        });
    }
}
