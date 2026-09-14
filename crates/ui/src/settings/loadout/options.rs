use super::*;

use zeron_proto::{ModelOption, ModelOptionChoice};

fn non_speed_options(model: &Model) -> Vec<ModelOption> {
    let speed_id = speed_option(model).map(|option| option.id.as_str());
    model
        .options
        .iter()
        .filter(|option| Some(option.id.as_str()) != speed_id)
        .cloned()
        .collect()
}

fn selected_choice<'a>(
    option: &'a ModelOption,
    model_options: &'a serde_json::Map<String, serde_json::Value>,
) -> Option<&'a ModelOptionChoice> {
    model_options
        .get(&option.id)
        .and_then(serde_json::Value::as_str)
        .and_then(|choice_id| option.choices.iter().find(|choice| choice.id == choice_id))
        .or_else(|| {
            option
                .choices
                .iter()
                .find(|choice| choice.id == option.default_choice)
        })
        .or_else(|| option.choices.first())
}

impl LoadoutPage {
    /// Return the model's selectable options in wire order, omitting the
    /// provider speed toggle that the root menu renders separately.
    pub(super) fn available_slot_options(&self, index: usize) -> Vec<ModelOption> {
        self.loadout
            .slot(index)
            .and_then(|slot| self.model_for(slot.harness, &slot.model))
            .map(non_speed_options)
            .unwrap_or_default()
    }

    pub(super) fn select_slot_option(
        &mut self,
        index: usize,
        option_index: usize,
        choice_index: usize,
        cx: &mut Context<Self>,
    ) {
        let options = self.available_slot_options(index);
        let Some(option) = options.get(option_index) else {
            return;
        };
        let Some(choice) = option.choices.get(choice_index) else {
            return;
        };
        let option_id = option.id.clone();
        let choice_id = choice.id.clone();
        let is_default = choice_id == option.default_choice;
        if let Some(slot) = self.loadout.slots.get_mut(index).and_then(Option::as_mut) {
            if is_default {
                slot.model_options.remove(&option_id);
            } else {
                slot.model_options
                    .insert(option_id, serde_json::Value::String(choice_id));
            }
            self.slot_menu = SlotMenu::Root(index);
            self.menu_in_submenu = false;
            self.menu_choice = 0;
            self.commit(cx);
        }
    }

    /// Render one option's choices as a nested loadout menu.
    pub(super) fn render_option_menu(
        &mut self,
        index: usize,
        option_index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let Some(slot) = self.loadout.slot(index).cloned() else {
            return div().into_any_element();
        };
        let options = self.available_slot_options(index);
        let Some(option) = options.get(option_index).cloned() else {
            return div().into_any_element();
        };
        let selected =
            selected_choice(&option, &slot.model_options).map(|choice| choice.id.as_str());
        let mut choices = div()
            .id("loadout-option-submenu")
            .max_h(px(320.0))
            .overflow_y_scroll()
            .track_scroll(&self.menu_scroll);

        for (choice_index, choice) in option.choices.iter().enumerate() {
            let is_selected = selected == Some(choice.id.as_str());
            let is_highlighted = self.menu_choice == choice_index;
            let row_id = format!("loadout-option-{index}-{option_index}-{choice_index}");
            choices = choices.child(
                loadout_menu_row(theme, is_highlighted)
                    .when(is_selected, |el| el.bg(theme::card_selected_bg()))
                    .id(SharedString::from(row_id))
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            let changed = this.menu_choice != choice_index || !this.menu_in_submenu;
                            this.menu_choice = choice_index;
                            this.menu_in_submenu = true;
                            if changed {
                                cx.notify();
                            }
                        }
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_slot_option(index, option_index, choice_index, cx);
                    }))
                    .child(SharedString::from(choice.label.clone())),
            );
        }
        popover::popover_card(theme)
            .w(px(self.menu_width))
            .debug_selector(|| "loadout-option-submenu-card".into())
            .child(popover::menu_heading(theme, &option.label))
            .child(choices)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn option(id: &str, default_choice: &str) -> ModelOption {
        ModelOption {
            id: id.into(),
            label: id.into(),
            choices: vec![ModelOptionChoice {
                id: default_choice.into(),
                label: default_choice.into(),
            }],
            default_choice: default_choice.into(),
        }
    }

    #[test]
    fn non_speed_options_preserve_order_and_remove_speed_toggle() {
        let model = Model {
            id: "claude-opus-5".into(),
            label: "Opus 5".into(),
            description: None,
            reasoning_levels: Vec::new(),
            options: vec![
                option("contextWindow", "200k"),
                ModelOption {
                    id: "fastMode".into(),
                    label: "Fast Mode".into(),
                    choices: vec![
                        ModelOptionChoice {
                            id: "off".into(),
                            label: "Off".into(),
                        },
                        ModelOptionChoice {
                            id: "on".into(),
                            label: "On".into(),
                        },
                    ],
                    default_choice: "off".into(),
                },
                option("mode", "default"),
            ],
        };

        let ids: Vec<_> = non_speed_options(&model)
            .into_iter()
            .map(|option| option.id)
            .collect();
        assert_eq!(ids, ["contextWindow", "mode"]);
    }

    #[test]
    fn selected_choice_falls_back_to_default_when_saved_value_is_stale() {
        let option = ModelOption {
            id: "contextWindow".into(),
            label: "Context Window".into(),
            choices: vec![
                ModelOptionChoice {
                    id: "200k".into(),
                    label: "200K".into(),
                },
                ModelOptionChoice {
                    id: "1m".into(),
                    label: "1M".into(),
                },
            ],
            default_choice: "200k".into(),
        };
        let mut saved = serde_json::Map::new();
        saved.insert(
            "contextWindow".into(),
            serde_json::Value::String("removed".into()),
        );
        assert_eq!(
            selected_choice(&option, &saved).map(|choice| choice.id.as_str()),
            Some("200k")
        );
    }
}
