use super::*;

impl LoadoutPage {
    pub(super) fn set_recording(
        &mut self,
        recording: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.recording == recording && (recording || self.recording_slot.is_none()) {
            return;
        }
        self.recording = recording;
        if !recording {
            self.recording_slot = None;
        } else {
            self.conflict_notice = None;
            window.focus(&self.focus, cx);
        }
        cx.emit(LoadoutEvent::RecordingChanged(recording));
        cx.notify();
    }

    pub(super) fn begin_shortcut_recording(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.loadout.slot(index).is_none() {
            return;
        }
        self.gear_open = false;
        self.menu_in_submenu = true;
        self.recording_slot = Some(index);
        self.recording = true;
        self.conflict_notice = None;
        window.focus(&self.focus, cx);
        cx.emit(LoadoutEvent::RecordingChanged(true));
        cx.notify();
    }

    pub(super) fn reset_shortcut(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(slot) = self.loadout.slots.get_mut(index).and_then(Option::as_mut) {
            slot.shortcut = None;
            let was_recording = self.recording || self.recording_slot.is_some();
            self.recording_slot = None;
            self.recording = false;
            self.conflict_notice = None;
            self.slot_menu = SlotMenu::Root(index);
            self.menu_in_submenu = false;
            if was_recording {
                cx.emit(LoadoutEvent::RecordingChanged(false));
            }
            self.commit(cx);
        }
    }

    pub(super) fn clear_shortcut(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(slot) = self.loadout.slots.get_mut(index).and_then(Option::as_mut) {
            slot.shortcut = Some(String::new());
            let was_recording = self.recording || self.recording_slot.is_some();
            self.recording_slot = None;
            self.recording = false;
            self.conflict_notice = None;
            self.slot_menu = SlotMenu::Root(index);
            self.menu_in_submenu = false;
            if was_recording {
                cx.emit(LoadoutEvent::RecordingChanged(false));
            }
            self.commit(cx);
        }
    }

    pub(super) fn on_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(index) = self.recording_slot else {
            self.on_menu_key_down(event, window, cx);
            return;
        };
        let mods = &event.keystroke.modifiers;
        match crate::settings::shortcuts::record_key(
            &event.keystroke.key,
            mods.control,
            mods.alt,
            mods.shift,
            mods.platform,
        ) {
            crate::settings::shortcuts::RecordOutcome::Cancelled => {
                self.set_recording(false, window, cx);
            }
            crate::settings::shortcuts::RecordOutcome::Ignored => {}
            crate::settings::shortcuts::RecordOutcome::Set(combo) => {
                let conflict = crate::settings::loadout_model::loadout_shortcut_conflict(
                    cfg!(target_os = "macos"),
                    &self.keymap,
                    &self.loadout,
                    index,
                    &combo,
                );
                if let Some(conflict) = conflict {
                    self.conflict_notice = Some(shortcut_conflict_message(&combo, conflict).into());
                    self.set_recording(false, window, cx);
                } else {
                    if let Some(slot) = self.loadout.slots.get_mut(index).and_then(Option::as_mut) {
                        slot.shortcut = crate::settings::loadout_model::normalize_loadout_shortcut(
                            Some(&combo),
                        );
                    }
                    self.conflict_notice = None;
                    self.set_recording(false, window, cx);
                    self.commit(cx);
                }
            }
        }
        cx.stop_propagation();
    }

    /// Handle keys belonging to the shortcut submenu. The parent menu owns
    /// dismissal and calls this while `SlotMenu::Shortcut` is active.
    pub(super) fn on_shortcut_menu_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(index) = self.slot_menu.slot() else {
            return false;
        };
        if !matches!(self.slot_menu, SlotMenu::Shortcut(_)) || self.recording_slot.is_some() {
            return false;
        }
        let handled = match event.keystroke.key.as_str() {
            "up" | "down" => {
                let delta = if event.keystroke.key == "up" { -1 } else { 1 };
                self.menu_choice =
                    popover::menu_step(Some(self.menu_choice.min(2)), 3, delta).unwrap_or(0);
                true
            }
            "right" | "enter" | "space" => {
                match self.menu_choice.min(2) {
                    0 => self.begin_shortcut_recording(index, window, cx),
                    1 => self.reset_shortcut(index, cx),
                    _ => self.clear_shortcut(index, cx),
                }
                true
            }
            "left" => {
                self.slot_menu = SlotMenu::Root(index);
                self.menu_in_submenu = false;
                true
            }
            _ => false,
        };
        if handled {
            cx.stop_propagation();
            cx.notify();
        }
        handled
    }

    pub(super) fn render_shortcut_menu(
        &mut self,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let combo = self.loadout.combo(index);
        let recording = self.recording_slot == Some(index);
        let chip = if recording {
            "Press keys…".to_string()
        } else if combo.is_empty() {
            "Disabled".to_string()
        } else {
            crate::settings::display_combo(&combo)
        };
        let mut menu = popover::popover_card(theme)
            .id("loadout-shortcut-menu")
            .w(px(self.menu_width))
            .p(px(8.0))
            .flex()
            .flex_col()
            .gap(px(4.0));
        menu = menu.child(
            div()
                .px(px(8.0))
                .py(px(4.0))
                .text_size(crate::typography::ui_rems(11.0))
                .text_color(theme.text_muted)
                .child(SharedString::from("Slot shortcut")),
        );
        menu = menu.child(
            div()
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(7.0))
                .border_1()
                .border_color(if recording {
                    theme.text.opacity(0.3)
                } else {
                    theme.border
                })
                .bg(if recording { theme.text } else { theme.bg })
                .text_color(if recording {
                    theme.on_solid
                } else {
                    theme.text
                })
                .font_family(theme.font_mono.clone())
                .text_size(crate::typography::ui_rems(12.0))
                .child(SharedString::from(chip)),
        );

        for (row, label) in ["Record shortcut", "Reset to default", "Clear shortcut"]
            .into_iter()
            .enumerate()
        {
            let active = self.menu_choice == row;
            menu = menu.child(
                div()
                    .id(("loadout-shortcut-action", row))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(7.0))
                    .flex()
                    .items_center()
                    .when(active, |el| el.bg(crate::theme::wash(0.06)))
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered {
                            this.menu_in_submenu = true;
                            this.menu_choice = row;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        match row {
                            0 => this.begin_shortcut_recording(index, window, cx),
                            1 => this.reset_shortcut(index, cx),
                            _ => this.clear_shortcut(index, cx),
                        }
                    }))
                    .child(SharedString::from(label)),
            );
        }
        if let Some(notice) = self.conflict_notice.clone() {
            menu = menu.child(
                div()
                    .px(px(8.0))
                    .pt(px(4.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.danger_muted)
                    .child(notice),
            );
        }
        menu.into_any_element()
    }

    pub(super) fn render_gear_menu(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        popover::popover_card(theme)
            .w(px(280.0))
            .p(px(10.0))
            .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                if this.recording {
                    this.set_recording(false, window, cx);
                }
                this.gear_open = false;
                cx.notify();
            }))
            .child(
                div()
                    .px(px(6.0))
                    .pb(px(8.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted)
                    .child(SharedString::from("Activation shortcuts")),
            )
            .child(
                div()
                    .px(px(6.0))
                    .text_size(crate::typography::ui_rems(11.0))
                    .text_color(theme.text_muted.opacity(0.7))
                    .child(SharedString::from(
                        "Each slot can use its own shortcut. Empty slots use the default Cmd/Ctrl+Shift number keys.",
                    )),
            )
            .into_any_element()
    }
}

fn shortcut_conflict_message(
    combo: &str,
    conflict: crate::settings::loadout_model::LoadoutShortcutConflict,
) -> String {
    let combo = crate::settings::display_combo(combo);
    match conflict {
        crate::settings::loadout_model::LoadoutShortcutConflict::OtherLoadout(index) => {
            format!("{combo} is already assigned to loadout slot {}.", index + 1)
        }
        crate::settings::loadout_model::LoadoutShortcutConflict::AppShortcut(owner) => {
            format!("{combo} is already assigned to {}.", owner.label())
        }
        crate::settings::loadout_model::LoadoutShortcutConflict::Reserved(owner) => {
            format!("{combo} is reserved for {owner}.")
        }
    }
}
