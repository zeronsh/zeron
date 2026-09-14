//! Settings → Model Loadout: a five-slot loadout fed by drag-and-drop from
//! provider lists, with per-entry activation shortcuts.

mod bindings;
mod cards;
mod catalog;
mod indicator;
mod options;

use std::collections::HashMap;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, KeyDownEvent, Render,
    SharedString, Subscription, Window, div, prelude::*, px,
};

use zeron_engine::registry::{HarnessDescriptor, descriptor_enabled};
use zeron_proto::{HarnessId, Model, ReasoningLevel};
use zeron_rpc::methods;

use crate::composer::{ComposerInput, ComposerInputEvent};
use crate::icons::{self, icon};
use crate::pickers::{
    default_model, default_reasoning, harness_brand_icon, normalize_model_rows, reasoning_label,
    visible_harnesses,
};
use crate::popover::{self, Loadable};
use crate::settings::loadout_model::{
    LOADOUT_SLOTS, LoadoutConfig, LoadoutSlot, harness_display_name, loadout_supports_harness,
    set_slot_speed, slot_speed_enabled, speed_option, speed_option_label,
};
use crate::settings::{KeymapConfig, widgets};
use crate::state::AppState;
use crate::theme::{self, Theme, ink};

#[derive(Debug, Clone)]
pub enum LoadoutEvent {
    Changed(LoadoutConfig),
    RecordingChanged(bool),
    OpenAgents,
}

#[derive(Clone)]
struct LoadoutModelDrag {
    harness: HarnessId,
    model_id: String,
    label: String,
}

struct LoadoutDragGhost {
    label: SharedString,
    cursor_offset: gpui::Point<gpui::Pixels>,
}

impl Render for LoadoutDragGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = crate::motion::pulse_delta(&crate::motion::GRADIENT_SPIN, cx.entity_id(), cx);
        let wiggle = if cx.reduce_motion() {
            0.0
        } else {
            (phase * std::f32::consts::TAU).sin()
        };
        let theme = Theme::of(cx);
        // GPUI subtracts the original grab offset when positioning a drag view.
        // Cancel it so this compact preview stays beside the pointer.
        div().child(
            div()
                .debug_selector(|| "loadout-drag-ghost".into())
                .relative()
                .left(self.cursor_offset.x + px(12.0 + wiggle))
                .top(self.cursor_offset.y + px(12.0 + 2.0 * wiggle))
                .h(px(28.0))
                .max_w(px(200.0))
                .px(px(10.0))
                .flex()
                .items_center()
                .rounded(px(8.0))
                .bg(theme.surface_raised)
                .border_1()
                .border_color(theme.border_strong)
                .text_size(px(12.0))
                .text_color(theme.text)
                .opacity(0.95)
                .child(div().min_w_0().truncate().child(self.label.clone())),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotMenu {
    Closed,
    Root(usize),
    Agent(usize),
    Model(usize),
    Effort(usize),
    Option(usize, usize),
    Shortcut(usize),
    Speed(usize),
    Remove(usize),
}

impl SlotMenu {
    fn slot(self) -> Option<usize> {
        match self {
            SlotMenu::Closed => None,
            SlotMenu::Root(i)
            | SlotMenu::Agent(i)
            | SlotMenu::Model(i)
            | SlotMenu::Effort(i)
            | SlotMenu::Option(i, _)
            | SlotMenu::Shortcut(i)
            | SlotMenu::Speed(i)
            | SlotMenu::Remove(i) => Some(i),
        }
    }

    fn is_action(self) -> bool {
        matches!(self, Self::Speed(_) | Self::Remove(_))
    }
}

pub struct LoadoutPage {
    state: Entity<AppState>,
    loadout: LoadoutConfig,
    keymap: KeymapConfig,
    harnesses: Loadable<Vec<HarnessDescriptor>>,
    models: HashMap<HarnessId, Loadable<Vec<Model>>>,
    slot_menu: SlotMenu,
    menu_root_choice: usize,
    menu_choice: usize,
    menu_in_submenu: bool,
    menu_width: f32,
    menu_scroll: gpui::ScrollHandle,
    page_scroll: gpui::ScrollHandle,
    catalog_scroll: gpui::ScrollHandle,
    catalog_scrollbar: popover::HorizontalScrollbarState,
    catalog_drag: Option<catalog::ProviderColumnDragState>,
    catalog_focus: FocusHandle,
    recording_slot: Option<usize>,
    gear_open: bool,
    recording: bool,
    conflict_notice: Option<SharedString>,
    drag_over: Option<usize>,
    slot_drag_over: Option<(usize, bool)>,
    catalog_entrance: HashMap<String, std::time::Instant>,
    hover_slot: Option<usize>,
    slot_focus: Vec<FocusHandle>,
    error: Option<SharedString>,
    search: Entity<ComposerInput>,
    focus: FocusHandle,
    load_task: Option<gpui::Task<()>>,
    catalog_scope: (Option<String>, Option<String>),
    catalog_generation: u64,
    pending_agent: Option<(usize, HarnessId)>,
    _catalog_events: Subscription,
    _search_events: Subscription,
}

impl EventEmitter<LoadoutEvent> for LoadoutPage {}
impl Focusable for LoadoutPage {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LoadoutPage {
    pub fn new(
        state: Entity<AppState>,
        loadout: LoadoutConfig,
        keymap: KeymapConfig,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            ComposerInput::new("Search models…", cx)
                .with_accessibility_role(gpui::Role::SearchInput)
        });
        let search_events = cx.subscribe(&search, |_this: &mut Self, _, event, cx| {
            if matches!(event, ComposerInputEvent::Edited) {
                cx.notify();
            }
        });
        let catalog_scope = request_scope(state.read(cx));
        let catalog_events = cx.observe(&state, |page: &mut Self, _, cx| {
            let scope = request_scope(page.state.read(cx));
            if scope != page.catalog_scope {
                page.catalog_scope = scope;
                page.catalog_generation = page.catalog_generation.wrapping_add(1);
                page.models.clear();
                page.catalog_entrance.clear();
                page.error = None;
                page.pending_agent = None;
                page.close_menus(cx);
                page.load(cx);
            }
        });
        let mut page = Self {
            state,
            loadout: loadout.clamped(),
            keymap,
            harnesses: Loadable::Idle,
            models: HashMap::new(),
            slot_menu: SlotMenu::Closed,
            menu_root_choice: 0,
            menu_choice: 0,
            menu_in_submenu: false,
            menu_width: 260.0,
            menu_scroll: gpui::ScrollHandle::new(),
            page_scroll: gpui::ScrollHandle::new(),
            catalog_scroll: gpui::ScrollHandle::new(),
            catalog_scrollbar: popover::HorizontalScrollbarState::default(),
            catalog_drag: None,
            catalog_focus: cx.focus_handle(),
            recording_slot: None,
            gear_open: false,
            recording: false,
            conflict_notice: None,
            drag_over: None,
            slot_drag_over: None,
            catalog_entrance: HashMap::new(),
            hover_slot: None,
            slot_focus: (0..LOADOUT_SLOTS)
                .map(|_| cx.focus_handle().tab_stop(true))
                .collect(),
            error: None,
            search,
            focus: cx.focus_handle(),
            load_task: None,
            catalog_scope,
            catalog_generation: 0,
            pending_agent: None,
            _catalog_events: catalog_events,
            _search_events: search_events,
        };
        page.load(cx);
        page
    }

    pub fn is_recording(&self) -> bool {
        self.recording
    }

    pub fn set_keymap(&mut self, keymap: KeymapConfig, cx: &mut Context<Self>) {
        self.keymap = keymap;
        cx.notify();
    }

    fn commit(&mut self, cx: &mut Context<Self>) {
        cx.emit(LoadoutEvent::Changed(self.loadout.clone()));
        cx.notify();
    }

    fn engine(&self, cx: &App) -> Option<crate::state::EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.harnesses = Loadable::Loading;
        let generation = self.catalog_generation;
        let params = catalog_params(&self.catalog_scope, None);
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::LIST_HARNESSES, params).await;
            this.update(cx, |page, cx| {
                if page.catalog_generation != generation {
                    return;
                }
                page.harnesses = match result {
                    Ok(value) => match serde_json::from_value::<Vec<HarnessDescriptor>>(value) {
                        Ok(list) => Loadable::Ready(list),
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                    Err(err) => Loadable::Error(err.to_string()),
                };
                if let Loadable::Ready(list) = &page.harnesses {
                    let ids: Vec<HarnessId> =
                        catalog_columns(list).into_iter().map(|d| d.id).collect();
                    for id in ids {
                        page.ensure_models(id, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn ensure_models(&mut self, harness: HarnessId, cx: &mut Context<Self>) {
        if matches!(
            self.models.get(&harness),
            Some(Loadable::Loading | Loadable::Ready(_))
        ) {
            return;
        }
        let Some(engine) = self.engine(cx) else {
            return;
        };
        self.models.insert(harness, Loadable::Loading);
        let generation = self.catalog_generation;
        let params = catalog_params(&self.catalog_scope, Some(harness));
        cx.spawn(async move |this, cx| {
            let result = engine.client().call(methods::LIST_MODELS, params).await;
            this.update(cx, |page, cx| {
                if page.catalog_generation != generation {
                    return;
                }
                page.models.insert(
                    harness,
                    match result {
                        Ok(value) => match serde_json::from_value::<Vec<Model>>(value) {
                            Ok(models) => Loadable::Ready(normalize_model_rows(harness, models)),
                            Err(err) => Loadable::Error(err.to_string()),
                        },
                        Err(err) => Loadable::Error(err.to_string()),
                    },
                );
                if let Some((index, wanted)) = page.pending_agent {
                    if wanted == harness {
                        page.pending_agent = None;
                        page.change_slot_agent(index, harness, cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn model_for(&self, harness: HarnessId, model_id: &str) -> Option<&Model> {
        self.models
            .get(&harness)
            .and_then(Loadable::ready)
            .and_then(|models| models.iter().find(|model| model.id == model_id))
    }

    fn drop_model(&mut self, index: usize, drag: &LoadoutModelDrag, cx: &mut Context<Self>) {
        self.close_menus(cx);
        let model = self.model_for(drag.harness, &drag.model_id);
        let reasoning = model
            .map(|model| default_reasoning(&model.reasoning_levels))
            .unwrap_or(Some(ReasoningLevel::High));
        let slot = LoadoutSlot {
            harness: drag.harness,
            model: drag.model_id.clone(),
            label: drag.label.clone(),
            reasoning,
            model_options: serde_json::Map::new(),
            shortcut: None,
        };
        self.loadout.place(index, slot);
        self.drag_over = None;
        self.commit(cx);
    }

    fn remove_slot(&mut self, index: usize, cx: &mut Context<Self>) {
        self.close_menus(cx);
        self.drag_over = None;
        self.slot_drag_over = None;
        self.loadout.remove(index);
        self.commit(cx);
    }

    fn close_menus(&mut self, cx: &mut Context<Self>) {
        if self.recording || self.recording_slot.is_some() {
            self.recording = false;
            self.recording_slot = None;
            cx.emit(LoadoutEvent::RecordingChanged(false));
        }
        self.menu_in_submenu = false;
        self.slot_menu = SlotMenu::Closed;
        self.pending_agent = None;
        self.gear_open = false;
        cx.notify();
    }

    fn change_slot_agent(&mut self, index: usize, harness: HarnessId, cx: &mut Context<Self>) {
        let model = self
            .models
            .get(&harness)
            .and_then(Loadable::ready)
            .and_then(|models| default_model(models))
            .cloned();
        let Some(model) = model else {
            match self.models.get(&harness) {
                Some(Loadable::Ready(_)) => {
                    self.error = Some(
                        format!("No models available for {}.", harness_display_name(harness))
                            .into(),
                    )
                }
                Some(Loadable::Error(error)) => self.error = Some(error.clone().into()),
                _ => {
                    self.pending_agent = Some((index, harness));
                    self.ensure_models(harness, cx);
                }
            }
            cx.notify();
            return;
        };
        self.pending_agent = None;
        self.error = None;
        if let Some(slot) = self.loadout.slots.get_mut(index).and_then(Option::as_mut) {
            slot.harness = harness;
            slot.model = model.id;
            slot.label = model.label;
            slot.reasoning = default_reasoning(&model.reasoning_levels);
            slot.model_options.clear();
        }
        self.slot_menu = SlotMenu::Root(index);
        self.menu_in_submenu = false;
        self.commit(cx);
    }

    fn change_slot_model(
        &mut self,
        index: usize,
        model_id: String,
        label: String,
        cx: &mut Context<Self>,
    ) {
        self.pending_agent = None;
        let reasoning = self
            .loadout
            .slot(index)
            .and_then(|slot| self.model_for(slot.harness, &model_id))
            .map(|model| default_reasoning(&model.reasoning_levels))
            .unwrap_or(Some(ReasoningLevel::High));
        if let Some(slot) = self.loadout.slots.get_mut(index).and_then(|s| s.as_mut()) {
            slot.model = model_id;
            slot.label = label;
            slot.reasoning = reasoning;
            slot.model_options.clear();
        }
        self.slot_menu = SlotMenu::Root(index);
        self.commit(cx);
    }
}

fn request_scope(state: &AppState) -> (Option<String>, Option<String>) {
    let target = state
        .effective_device_id()
        .filter(|device| state.local_device_id.as_deref() != Some(device.as_str()));
    let cwd = state
        .selected_chat_row()
        .and_then(|chat| chat.cwd.clone())
        .or_else(|| state.selected_space_row().map(|space| space.path.clone()));
    (target, cwd)
}

fn catalog_params(
    scope: &(Option<String>, Option<String>),
    harness: Option<HarnessId>,
) -> serde_json::Value {
    let mut params = serde_json::Map::new();
    if let Some(target) = &scope.0 {
        params.insert("targetDeviceId".into(), target.clone().into());
    }
    if let Some(harness) = harness {
        params.insert(
            "harness".into(),
            serde_json::to_value(harness).expect("harness serializes"),
        );
        if let Some(cwd) = &scope.1 {
            params.insert("cwd".into(), cwd.clone().into());
        }
    }
    params.into()
}

fn catalog_columns(list: &[HarnessDescriptor]) -> Vec<HarnessDescriptor> {
    visible_harnesses(list)
        .into_iter()
        .filter(|descriptor| {
            loadout_supports_harness(descriptor.id) && descriptor_enabled(descriptor)
        })
        .collect()
}

const PAGE_GUTTER: f32 = 28.0;
const MODEL_COLUMN_LIMIT: usize = 80;

// These rows own hover navigation, so they must not install the generic
// popover row's separate hover-animation listener.
fn loadout_menu_row(theme: &Theme, highlighted: bool) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(8.0))
        .py(px(6.0))
        .rounded(px(8.0))
        .text_size(crate::typography::ui_rems(13.0))
        .text_color(theme.text)
        .cursor_pointer()
        .when(highlighted, |el| el.bg(theme::card_selected_bg()))
        .hover(|style| style.bg(theme::card_selected_bg()))
}

struct MenuEntry {
    menu: SlotMenu,
    label: String,
    value: String,
    enabled: bool,
}

impl LoadoutPage {
    fn open_slot_menu(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.recording {
            self.set_recording(false, window, cx);
        }
        let was_open = self.slot_menu.slot() == Some(index);
        self.close_menus(cx);
        if !was_open && self.loadout.slot(index).is_some() {
            self.slot_menu = SlotMenu::Root(index);
            self.menu_root_choice = 0;
            self.menu_choice = 0;
            self.menu_in_submenu = false;
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    fn root_menu_entries(&self, index: usize) -> Vec<MenuEntry> {
        let Some(slot) = self.loadout.slot(index) else {
            return Vec::new();
        };
        let model = self.model_for(slot.harness, &slot.model);
        let mut entries = vec![
            MenuEntry {
                menu: SlotMenu::Agent(index),
                label: "Agent".into(),
                value: if self.pending_agent.is_some_and(|(at, _)| at == index) {
                    "Loading…".into()
                } else {
                    harness_display_name(slot.harness).into()
                },
                enabled: true,
            },
            MenuEntry {
                menu: SlotMenu::Model(index),
                label: "Model".into(),
                value: slot.label.clone(),
                enabled: true,
            },
            MenuEntry {
                menu: SlotMenu::Effort(index),
                label: "Effort".into(),
                value: slot
                    .reasoning
                    .map(reasoning_label)
                    .unwrap_or("Default")
                    .into(),
                enabled: model.is_some_and(|m| !m.reasoning_levels.is_empty()),
            },
        ];
        for (option_index, option) in self.available_slot_options(index).into_iter().enumerate() {
            let selected = slot
                .model_options
                .get(&option.id)
                .and_then(|v| v.as_str())
                .unwrap_or(&option.default_choice);
            let value = option
                .choices
                .iter()
                .find(|c| c.id == selected)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| "Default".into());
            entries.push(MenuEntry {
                menu: SlotMenu::Option(index, option_index),
                label: option.label,
                value,
                enabled: !option.choices.is_empty(),
            });
        }
        if let Some(option) = model.and_then(speed_option) {
            entries.push(MenuEntry {
                menu: SlotMenu::Speed(index),
                label: speed_option_label(option).into(),
                value: String::new(),
                enabled: true,
            });
        }
        entries.push(MenuEntry {
            menu: SlotMenu::Shortcut(index),
            label: "Shortcut".into(),
            value: {
                let combo = self
                    .loadout
                    .resolved_combos()
                    .get(index)
                    .cloned()
                    .unwrap_or_default();
                if combo.is_empty() {
                    "None".into()
                } else {
                    crate::settings::badge_combo(&combo)
                }
            },
            enabled: true,
        });
        entries.push(MenuEntry {
            menu: SlotMenu::Remove(index),
            label: "Remove".into(),
            value: String::new(),
            enabled: true,
        });
        entries
    }

    fn open_submenu(&mut self, menu: SlotMenu, cx: &mut Context<Self>) {
        if self.recording || self.slot_menu == menu {
            return;
        }
        if let SlotMenu::Model(index) = menu {
            if let Some(slot) = self.loadout.slot(index) {
                self.ensure_models(slot.harness, cx);
            }
        }
        self.slot_menu = menu;
        self.menu_choice = 0;
        self.menu_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
        cx.notify();
    }

    fn toggle_slot_speed(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(slot) = self.loadout.slot(index) else {
            return;
        };
        let model = self.model_for(slot.harness, &slot.model).cloned();
        let on = slot_speed_enabled(slot, model.as_ref());
        if let Some(slot) = self.loadout.slots[index].as_mut() {
            set_slot_speed(slot, model.as_ref(), !on);
        }
        self.commit(cx);
    }

    fn menu_choices(&self, index: usize) -> Vec<(String, bool)> {
        let Some(slot) = self.loadout.slot(index) else {
            return Vec::new();
        };
        match self.slot_menu {
            SlotMenu::Agent(_) => {
                catalog_columns(self.harnesses.ready().map(Vec::as_slice).unwrap_or(&[]))
                    .into_iter()
                    .filter(|d| d.installed)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .map(|d| (harness_display_name(d.id).into(), d.id == slot.harness))
                    .collect()
            }
            SlotMenu::Model(_) => self
                .models
                .get(&slot.harness)
                .and_then(Loadable::ready)
                .into_iter()
                .flatten()
                .map(|m| (m.label.clone(), m.id == slot.model))
                .collect(),
            SlotMenu::Effort(_) => self
                .model_for(slot.harness, &slot.model)
                .into_iter()
                .flat_map(|m| &m.reasoning_levels)
                .map(|level| {
                    (
                        reasoning_label(*level).into(),
                        Some(*level) == slot.reasoning,
                    )
                })
                .collect(),
            SlotMenu::Option(_, option) => self
                .available_slot_options(index)
                .get(option)
                .map(|o| {
                    o.choices
                        .iter()
                        .map(|c| {
                            (
                                c.label.clone(),
                                slot.model_options
                                    .get(&o.id)
                                    .and_then(|v| v.as_str())
                                    .unwrap_or(&o.default_choice)
                                    == c.id,
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn activate_menu_choice(&mut self, index: usize, choice: usize, cx: &mut Context<Self>) {
        match self.slot_menu {
            SlotMenu::Agent(_) => {
                let entries =
                    catalog_columns(self.harnesses.ready().map(Vec::as_slice).unwrap_or(&[]))
                        .into_iter()
                        .filter(|d| d.installed)
                        .collect::<Vec<_>>();
                if let Some(entry) = entries.get(choice) {
                    self.change_slot_agent(index, entry.id, cx);
                }
            }
            SlotMenu::Model(_) => {
                let model = self
                    .loadout
                    .slot(index)
                    .and_then(|s| self.models.get(&s.harness))
                    .and_then(Loadable::ready)
                    .and_then(|models| models.get(choice))
                    .cloned();
                if let Some(model) = model {
                    self.change_slot_model(index, model.id, model.label, cx);
                }
            }
            SlotMenu::Effort(_) => {
                let level = self
                    .loadout
                    .slot(index)
                    .and_then(|s| self.model_for(s.harness, &s.model))
                    .and_then(|m| m.reasoning_levels.get(choice))
                    .copied();
                if let Some(level) = level {
                    if let Some(slot) = self.loadout.slots[index].as_mut() {
                        slot.reasoning = Some(level);
                    }
                    self.slot_menu = SlotMenu::Root(index);
                    self.commit(cx);
                }
            }
            SlotMenu::Option(_, option) => self.select_slot_option(index, option, choice, cx),
            _ => {}
        }
        self.menu_in_submenu = false;
    }

    fn on_menu_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.slot_menu.slot() else {
            return;
        };
        if self.menu_in_submenu && self.on_shortcut_menu_key_down(event, window, cx) {
            return;
        }
        let key = event.keystroke.key.as_str();
        match key {
            "escape" | "left" => {
                if self.slot_menu != SlotMenu::Root(index) {
                    self.slot_menu = SlotMenu::Root(index);
                    self.menu_in_submenu = false;
                } else {
                    self.close_menus(cx);
                }
            }
            "tab" => {
                self.close_menus(cx);
                return;
            }
            "up" | "down" => {
                let delta = if key == "up" { -1 } else { 1 };
                if self.menu_in_submenu {
                    self.menu_choice = popover::menu_step(
                        Some(self.menu_choice),
                        self.menu_choices(index).len(),
                        delta,
                    )
                    .unwrap_or(0);
                    self.menu_scroll.scroll_to_item(self.menu_choice);
                } else {
                    let entries = self.root_menu_entries(index);
                    for _ in 0..entries.len() {
                        self.menu_root_choice =
                            popover::menu_step(Some(self.menu_root_choice), entries.len(), delta)
                                .unwrap_or(0);
                        if entries[self.menu_root_choice].enabled {
                            break;
                        }
                    }
                    self.slot_menu = SlotMenu::Root(index);
                }
            }
            "right" | "enter" | "space" => {
                if self.menu_in_submenu {
                    if matches!(self.slot_menu, SlotMenu::Shortcut(_)) {
                        self.begin_shortcut_recording(index, window, cx);
                    } else if key != "right" {
                        self.activate_menu_choice(index, self.menu_choice, cx);
                    }
                } else if let Some(entry) = self.root_menu_entries(index).get(self.menu_root_choice)
                {
                    if !entry.enabled {
                        return;
                    }
                    if let SlotMenu::Speed(_) = entry.menu {
                        self.toggle_slot_speed(index, cx);
                    } else if let SlotMenu::Remove(_) = entry.menu {
                        self.remove_slot(index, cx);
                    } else {
                        self.open_submenu(entry.menu, cx);
                        self.menu_in_submenu = true;
                    }
                }
            }
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn render_slot_menu(
        &mut self,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(slot) = self.loadout.slot(index).cloned() else {
            return div().into_any_element();
        };
        let mut root = popover::popover_card(theme)
            .id("loadout-menu-root")
            .debug_selector(|| "loadout-menu-root".into())
            .w(px(self.menu_width))
            .flex_none();
        for (row_index, entry) in self.root_menu_entries(index).into_iter().enumerate() {
            let menu = entry.menu;
            let active = self.slot_menu == menu
                || (!self.menu_in_submenu && self.menu_root_choice == row_index);
            let mut row = div()
                .id(("loadout-nav", row_index))
                .debug_selector(move || format!("loadout-nav-{row_index}").into())
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(8.0))
                .flex()
                .items_center()
                .gap(px(10.0))
                .when(active, |el| el.bg(theme::wash(0.06)))
                .when(!entry.enabled, |el| el.opacity(0.45))
                .when(entry.enabled, |el| {
                    el.cursor_pointer()
                        .hover(|s| s.bg(theme::wash(0.06)))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && !this.recording {
                                this.menu_root_choice = row_index;
                                this.menu_in_submenu = false;
                                if entry.menu.is_action() {
                                    this.slot_menu = SlotMenu::Root(index);
                                    cx.notify();
                                } else {
                                    this.open_submenu(menu, cx);
                                }
                            }
                        }))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            window.focus(&this.focus, cx);
                            this.menu_root_choice = row_index;
                            if matches!(menu, SlotMenu::Speed(_)) {
                                this.toggle_slot_speed(index, cx);
                            } else if matches!(menu, SlotMenu::Remove(_)) {
                                this.remove_slot(index, cx);
                            } else {
                                this.open_submenu(menu, cx);
                                this.menu_in_submenu = true;
                            }
                        }))
                })
                .child(div().flex_none().child(SharedString::from(entry.label)));
            if matches!(menu, SlotMenu::Remove(_)) {
                row = row
                    .debug_selector(|| "loadout-remove".into())
                    .text_color(theme.danger_muted);
            }
            if matches!(menu, SlotMenu::Speed(_)) {
                row = row.child(div().flex_1()).child(widgets::toggle_switch(
                    theme,
                    slot_speed_enabled(&slot, self.model_for(slot.harness, &slot.model)),
                ));
            } else if !matches!(menu, SlotMenu::Remove(_)) {
                row = row
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_right()
                            .truncate()
                            .text_color(theme.text_muted)
                            .child(SharedString::from(entry.value)),
                    )
                    .child(
                        icon(icons::ALT_ARROW_RIGHT)
                            .size(px(12.0))
                            .text_color(theme.text_muted),
                    );
            }
            if matches!(menu, SlotMenu::Remove(_)) {
                root = root.child(popover::menu_separator());
            }
            root = root.child(row);
        }
        let mut menus = div()
            .id("loadout-menu-group")
            .flex()
            .items_start()
            .gap(px(4.0))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.recording {
                    this.set_recording(false, window, cx);
                }
                this.close_menus(cx);
            }))
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(popover::frosted_card(root));
        match self.slot_menu {
            SlotMenu::Root(_) | SlotMenu::Closed | SlotMenu::Speed(_) | SlotMenu::Remove(_) => {}
            SlotMenu::Shortcut(_) => {
                menus = menus.child(popover::frosted_card(
                    self.render_shortcut_menu(index, theme, cx),
                ));
            }
            SlotMenu::Option(_, option) => {
                menus = menus.child(popover::frosted_card(
                    self.render_option_menu(index, option, theme, cx),
                ));
            }
            _ => {
                let choices = self.menu_choices(index);
                let mut submenu = popover::popover_card(theme)
                    .id("loadout-submenu")
                    .debug_selector(|| "loadout-submenu".into())
                    .w(px(self.menu_width))
                    .max_h(px(320.0))
                    .flex_none()
                    .overflow_y_scroll()
                    .track_scroll(&self.menu_scroll);
                for (choice, (label, selected)) in choices.into_iter().enumerate() {
                    submenu = submenu.child(
                        loadout_menu_row(theme, self.menu_in_submenu && choice == self.menu_choice)
                            .id(("loadout-choice", choice))
                            .debug_selector(move || format!("loadout-choice-{choice}").into())
                            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                                if *hovered {
                                    this.menu_in_submenu = true;
                                    this.menu_choice = choice;
                                    cx.notify();
                                }
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.activate_menu_choice(index, choice, cx);
                            }))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(SharedString::from(label)),
                            )
                            .when(selected, |el| {
                                el.child(
                                    icon(icons::CHECK)
                                        .size(px(12.0))
                                        .text_color(theme.text_muted),
                                )
                            }),
                    );
                }
                if self.menu_choices(index).is_empty() {
                    let message = match self.models.get(&slot.harness) {
                        Some(Loadable::Error(error)) => error.clone(),
                        Some(Loadable::Ready(_)) => "No choices available".into(),
                        _ => "Loading…".into(),
                    };
                    submenu = submenu.child(
                        div()
                            .p(px(8.0))
                            .text_color(theme.text_muted)
                            .child(SharedString::from(message)),
                    );
                }
                menus = menus.child(popover::frosted_card(submenu));
            }
        }
        menus.into_any_element()
    }
}

impl Render for LoadoutPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.menu_width =
            ((f32::from(window.viewport_size().width) - 32.0) / 2.0).clamp(140.0, 260.0);
        if !cx.has_active_drag() {
            self.drag_over = None;
            self.slot_drag_over = None;
            self.catalog_drag = None;
        }
        let theme = Theme::of(cx).clone();
        let body = self.render_catalog(&theme, cx);

        let mut gear = div()
            .id("loadout-gear")
            .relative()
            .size(px(24.0))
            .rounded(px(6.0))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .hover(|s| s.bg(ink(0.06)))
            .on_click(cx.listener(|this, _, window, cx| {
                let was_open = this.gear_open;
                this.close_menus(cx);
                this.gear_open = !was_open;
                if this.gear_open {
                    window.focus(&this.focus, cx);
                } else if this.recording {
                    this.set_recording(false, window, cx);
                }
                cx.notify();
            }))
            .child(
                icon(icons::SETTINGS_MINIMALISTIC)
                    .size(px(14.0))
                    .text_color(theme.text_muted),
            );
        if self.gear_open {
            let menu = self.render_gear_menu(&theme, cx);
            gear = gear.child(popover::anchored_menu_below(
                "loadout-gear-menu",
                menu,
                None,
            ));
        }

        let visible_slots = self
            .loadout
            .first_empty()
            .map_or(LOADOUT_SLOTS, |index| index + 1);
        let slots: Vec<_> = (0..visible_slots)
            .map(|index| self.render_slot(index, &theme, cx).into_any_element())
            .collect();

        div()
            .id("loadout-page")
            .debug_selector(|| "loadout-page".into())
            .track_focus(&self.focus)
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.page_scroll)
            .on_key_down(cx.listener(Self::on_key_down))
            .on_drop::<cards::LoadoutSlotDrag>(cx.listener(
                |this, payload: &cards::LoadoutSlotDrag, _, cx| {
                    this.remove_slot(payload.from, cx);
                },
            ))
            .child(
                div()
                    .w_full()
                    .px(px(PAGE_GUTTER))
                    .pt(px(32.0))
                    .pb(px(64.0))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .text_size(crate::typography::ui_rems(14.0))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(theme.text)
                                    .child(SharedString::from("Loadout")),
                            )
                            .child(gear),
                    )
                    .child(widgets::page_subtitle(
                        &theme,
                        "Choose which models you want to use",
                    ))
                    .when_some(self.error.clone(), |el, message| {
                        el.child(widgets::error_strip(&theme, message))
                    })
                    .child(
                        div()
                            .mt(px(16.0))
                            .flex()
                            .flex_row()
                            .items_stretch()
                            .flex_wrap()
                            .gap(px(10.0))
                            .children(slots),
                    )
                    .child(
                        div()
                            .mt(px(24.0))
                            .h(px(36.0))
                            .max_w(px(420.0))
                            .px(px(10.0))
                            .rounded(px(8.0))
                            .border_1()
                            .border_color(theme.border)
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                icon(icons::MAGNIFER)
                                    .size(px(14.0))
                                    .text_color(theme.text_muted),
                            )
                            .child(div().flex_1().min_w_0().child(self.search.clone())),
                    )
                    .child(body),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_page(cx: &mut Context<LoadoutPage>) -> LoadoutPage {
        let state = cx.new(|_| AppState::new());
        let mut config = LoadoutConfig::default();
        config.slots[0] = Some(LoadoutSlot {
            harness: HarnessId::Codex,
            model: "one".into(),
            label: "One".into(),
            reasoning: Some(ReasoningLevel::Low),
            model_options: Default::default(),
            shortcut: None,
        });
        let mut page = LoadoutPage::new(state, config, KeymapConfig::default(), cx);
        page.harnesses = Loadable::Ready(vec![HarnessDescriptor {
            id: HarnessId::Codex,
            name: "Codex".into(),
            installed: true,
            enabled: Some(true),
            reasoning_levels: vec![],
            steering_mode: zeron_proto::SteeringMode::StepBoundary,
            supports_steering: false,
        }]);
        page.models.insert(
            HarnessId::Codex,
            Loadable::Ready(
                ["one", "two"]
                    .into_iter()
                    .map(|id| Model {
                        id: id.into(),
                        label: id.into(),
                        description: None,
                        reasoning_levels: vec![ReasoningLevel::Low, ReasoningLevel::High],
                        options: vec![],
                    })
                    .collect(),
            ),
        );
        page
    }

    #[gpui::test]
    fn visible_slots_follow_add_remove_and_capacity(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|_, cx| fixture_page(cx));
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-slot-0").is_some());
        assert!(cx.debug_bounds("loadout-slot-1").is_some());
        assert!(cx.debug_bounds("loadout-slot-2").is_none());
        page.update(cx, |page, cx| {
            let slot = page.loadout.slot(0).unwrap().clone();
            for index in 1..LOADOUT_SLOTS {
                page.loadout.place(index, slot.clone());
            }
            cx.notify();
        });
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-slot-4").is_some());
        assert!(cx.debug_bounds("loadout-first-vacant").is_none());
        page.update(cx, |page, cx| {
            for _ in 0..LOADOUT_SLOTS {
                page.loadout.remove(0);
            }
            cx.notify();
        });
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-slot-0").is_some());
        assert!(cx.debug_bounds("loadout-slot-1").is_none());
    }

    #[gpui::test]
    fn fast_badge_tracks_the_selected_speed_option(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|_, cx| {
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
        assert!(cx.debug_bounds("loadout-fast-0").is_some());
        page.update(cx, |page, cx| {
            page.loadout.slots[0]
                .as_mut()
                .unwrap()
                .model_options
                .clear();
            cx.notify();
        });
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-fast-0").is_none());
    }

    #[gpui::test]
    fn drop_model_fills_the_slot(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|_, cx| fixture_page(cx));
        let drag = LoadoutModelDrag {
            harness: HarnessId::Codex,
            model_id: "two".into(),
            label: "Two".into(),
        };
        page.update(cx, |page, cx| page.drop_model(0, &drag, cx));
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-drop-splash-0").is_none());
        page.read_with(cx, |page, _| {
            assert_eq!(page.loadout.slot(0).unwrap().model, "two");
        });
        page.update(cx, |page, cx| page.drop_model(1, &drag, cx));
        page.read_with(cx, |page, _| {
            assert_eq!(page.loadout.slot(1).unwrap().model, "two");
        });
    }

    #[gpui::test]
    fn menu_remove_clears_the_slot(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let mut page = fixture_page(cx);
            page.open_slot_menu(0, window, cx);
            page
        });
        cx.refresh().unwrap();
        let remove = cx.debug_bounds("loadout-remove").unwrap();
        cx.simulate_click(remove.center(), gpui::Modifiers::default());
        cx.refresh().unwrap();
        page.read_with(cx, |page, _| {
            assert!(page.loadout.slot(0).is_none());
            assert_eq!(page.slot_menu, SlotMenu::Closed);
        });
        assert!(cx.debug_bounds("loadout-menu-root").is_none());
    }

    #[gpui::test]
    fn submenu_keeps_parent_visible_and_keyboard_selection_updates_slot(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let mut page = fixture_page(cx);
            page.open_slot_menu(0, window, cx);
            page
        });
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-menu-root").is_some());
        assert!(cx.debug_bounds("loadout-submenu").is_none());
        cx.simulate_keystrokes("down right");
        cx.refresh().unwrap();
        let parent = cx.debug_bounds("loadout-menu-root").unwrap();
        let child = cx.debug_bounds("loadout-submenu").unwrap();
        assert!(child.origin.x >= parent.origin.x + parent.size.width);
        cx.simulate_keystrokes("down enter");
        page.read_with(cx, |page, _| {
            assert_eq!(page.loadout.slot(0).unwrap().model, "two")
        });
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-menu-root").is_some());
        assert!(cx.debug_bounds("loadout-submenu").is_none());
        cx.simulate_keystrokes("escape");
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-menu-root").is_none());
    }

    #[gpui::test]
    fn submenu_click_keeps_parent_and_applies_effort(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let mut page = fixture_page(cx);
            page.open_slot_menu(0, window, cx);
            page
        });
        cx.refresh().unwrap();
        let effort = cx.debug_bounds("loadout-nav-2").unwrap();
        cx.simulate_click(effort.center(), gpui::Modifiers::default());
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-menu-root").is_some());
        let high = cx.debug_bounds("loadout-choice-1").unwrap();
        cx.simulate_click(high.center(), gpui::Modifiers::default());
        page.read_with(cx, |page, _| {
            assert_eq!(
                page.loadout.slot(0).unwrap().reasoning,
                Some(ReasoningLevel::High)
            )
        });
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-menu-root").is_some());
    }

    #[gpui::test]
    fn context_choice_persists_and_model_change_clears_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let mut page = fixture_page(cx);
            if let Some(Loadable::Ready(models)) = page.models.get_mut(&HarnessId::Codex) {
                models[0].options = vec![zeron_proto::ModelOption {
                    id: "contextWindow".into(),
                    label: "Context window".into(),
                    default_choice: "standard".into(),
                    choices: vec![
                        zeron_proto::ModelOptionChoice {
                            id: "standard".into(),
                            label: "Standard".into(),
                        },
                        zeron_proto::ModelOptionChoice {
                            id: "extended".into(),
                            label: "Extended".into(),
                        },
                    ],
                }];
            }
            page.open_slot_menu(0, window, cx);
            page
        });
        cx.refresh().unwrap();
        cx.simulate_keystrokes("down down down right down enter");
        page.read_with(cx, |page, _| {
            assert_eq!(
                page.loadout.slot(0).unwrap().model_options["contextWindow"],
                "extended"
            );
            let restored: LoadoutConfig =
                serde_json::from_str(&serde_json::to_string(&page.loadout).unwrap()).unwrap();
            assert_eq!(
                restored.slot(0).unwrap().model_options["contextWindow"],
                "extended"
            );
        });
        cx.simulate_keystrokes("up up right down enter");
        page.read_with(cx, |page, _| {
            assert_eq!(page.loadout.slot(0).unwrap().model, "two");
            assert!(page.loadout.slot(0).unwrap().model_options.is_empty());
            assert!(page.available_slot_options(0).is_empty());
        });
    }

    #[gpui::test]
    fn shortcut_menu_records_full_chord_and_clears_it(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let mut page = fixture_page(cx);
            page.open_slot_menu(0, window, cx);
            page
        });
        cx.refresh().unwrap();
        cx.simulate_keystrokes("down down down right enter");
        page.read_with(cx, |page, _| assert!(page.is_recording()));
        cx.simulate_keystrokes(&crate::settings::platform_combo("mod-shift-h"));
        page.read_with(cx, |page, _| {
            assert!(!page.is_recording());
            assert_eq!(
                page.loadout.slot(0).unwrap().shortcut.as_deref(),
                Some("mod-shift-h")
            );
        });
        cx.simulate_keystrokes("down down enter");
        page.read_with(cx, |page, _| {
            assert_eq!(page.loadout.slot(0).unwrap().shortcut.as_deref(), Some(""));
            assert!(page.loadout.combo(0).is_empty());
            assert!(!page.menu_in_submenu);
        });
    }

    #[gpui::test]
    fn first_vacant_prompt_tracks_filled_slots_and_disappears_when_full(
        cx: &mut gpui::TestAppContext,
    ) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|_, cx| {
            let mut page = fixture_page(cx);
            let slot = page.loadout.slot(0).unwrap().clone();
            page.loadout.slots[1] = Some(slot.clone());
            page.loadout.slots[2] = Some(slot);
            page
        });
        cx.refresh().unwrap();
        let prompt = cx.debug_bounds("loadout-first-vacant").unwrap();
        assert!(
            cx.debug_bounds("loadout-slot-3")
                .unwrap()
                .contains(&prompt.center())
        );
        page.update(cx, |page, cx| {
            page.loadout.remove(1);
            cx.notify();
        });
        cx.refresh().unwrap();
        let prompt = cx.debug_bounds("loadout-first-vacant").unwrap();
        assert!(
            cx.debug_bounds("loadout-slot-2")
                .unwrap()
                .contains(&prompt.center())
        );
        page.update(cx, |page, cx| {
            let slot = page.loadout.slot(0).unwrap().clone();
            for at in 0..LOADOUT_SLOTS {
                page.loadout.slots[at] = Some(slot.clone());
            }
            cx.notify();
        });
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-first-vacant").is_none());
    }

    #[gpui::test]
    fn scope_change_cancels_shortcut_recording(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let mut page = fixture_page(cx);
            page.open_slot_menu(0, window, cx);
            page.begin_shortcut_recording(0, window, cx);
            page
        });
        page.update(cx, |page, cx| {
            page.state.update(cx, |state, cx| {
                state.selected_device = Some("another-device".into());
                cx.notify();
            });
        });
        cx.run_until_parked();
        page.read_with(cx, |page, _| {
            assert!(!page.recording);
            assert!(page.recording_slot.is_none());
            assert_eq!(page.slot_menu, SlotMenu::Closed);
        });
    }

    #[gpui::test]
    fn submenu_hover_crosses_gap_and_outside_click_dismisses(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            cx.set_global(Theme::dark());
            cx.set_reduce_motion(true);
        });
        let (page, cx) = cx.add_window_view(|window, cx| {
            let mut page = fixture_page(cx);
            page.open_slot_menu(0, window, cx);
            page
        });
        cx.refresh().unwrap();
        let row = cx.debug_bounds("loadout-nav-1").unwrap();
        cx.simulate_mouse_move(row.center(), None, gpui::Modifiers::default());
        cx.refresh().unwrap();
        let parent = cx.debug_bounds("loadout-menu-root").unwrap();
        assert!(cx.debug_bounds("loadout-submenu").is_some());
        cx.simulate_mouse_move(
            gpui::point(parent.right() + px(2.0), row.center().y),
            None,
            gpui::Modifiers::default(),
        );
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-submenu").is_some());
        let choice = cx.debug_bounds("loadout-choice-1").unwrap();
        cx.simulate_mouse_move(choice.center(), None, gpui::Modifiers::default());
        cx.simulate_click(choice.center(), gpui::Modifiers::default());
        page.read_with(cx, |page, _| {
            assert_eq!(page.loadout.slot(0).unwrap().model, "two")
        });
        cx.simulate_click(gpui::point(px(10.0), px(10.0)), gpui::Modifiers::default());
        cx.refresh().unwrap();
        assert!(cx.debug_bounds("loadout-menu-root").is_none());
    }

    #[test]
    fn catalog_requests_use_the_launch_device_and_workspace() {
        let scope = (
            Some("remote-device".into()),
            Some("/workspace/project".into()),
        );
        let models = catalog_params(&scope, Some(HarnessId::Codex));
        assert_eq!(models["targetDeviceId"], "remote-device");
        assert_eq!(models["cwd"], "/workspace/project");
        assert_eq!(
            models["harness"],
            serde_json::to_value(HarnessId::Codex).unwrap()
        );
        assert!(
            catalog_params(&(None, None), None)
                .as_object()
                .unwrap()
                .is_empty()
        );
    }

    #[gpui::test]
    fn pending_agent_selection_never_persists_an_empty_model(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| cx.set_global(Theme::dark()));
        let handle = cx.add_window(|_, cx| fixture_page(cx));
        handle
            .update(cx, |page, _, cx| {
                let before = page.loadout.slot(0).unwrap().clone();
                page.models.insert(HarnessId::ClaudeCode, Loadable::Loading);
                page.change_slot_agent(0, HarnessId::ClaudeCode, cx);
                assert_eq!(page.loadout.slot(0), Some(&before));
                assert_eq!(page.pending_agent, Some((0, HarnessId::ClaudeCode)));
                page.close_menus(cx);
                assert!(page.pending_agent.is_none());
                page.models.insert(
                    HarnessId::ClaudeCode,
                    Loadable::Ready(vec![Model {
                        id: "valid".into(),
                        label: "Valid".into(),
                        description: None,
                        reasoning_levels: vec![ReasoningLevel::High],
                        options: vec![],
                    }]),
                );
                page.change_slot_agent(0, HarnessId::ClaudeCode, cx);
                assert_eq!(page.loadout.slot(0).unwrap().model, "valid");
            })
            .unwrap();
    }
}
