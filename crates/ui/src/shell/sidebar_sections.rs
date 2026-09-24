//! Account-synced sections; local workspaces keep their settings on this device.
use super::*;
use crate::settings::SidebarSection;
use zeron_proto::{SidebarPinChange, SidebarSectionChange};

pub(super) struct SectionDialog {
    profile: String,
    id: Option<String>,
    input: Entity<ComposerInput>,
    focus_pending: bool,
    _events: Subscription,
}

impl Shell {
    pub(super) fn active_sidebar_sections(&self, cx: &App) -> Vec<SidebarSection> {
        let local = self.state.read(cx).workspace_scope == Some(WorkspaceScope::Local);
        let mut sections = if local {
            self.active_sidebar_pin_profile_key(cx)
                .and_then(|key| self.settings.sidebar_sections_by_profile.get(&key).cloned())
                .unwrap_or_default()
        } else {
            self.state.read(cx).sidebar_preferences.sections.clone()
        };
        if !local && self.optimistic_sidebar_pins(cx).is_some() {
            if let Some(pending) = &self.sidebar_pin_write {
                for change in &pending.queue {
                    change.project_sections(&mut sections);
                }
            }
        }
        sections
    }

    pub(super) fn assign_sidebar_section(
        &mut self,
        chat: &str,
        target: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.change_sidebar_section(
            SidebarSectionChange::Assign {
                session_id: chat.to_owned(),
                section_id: target.map(str::to_owned),
            },
            cx,
        );
    }

    pub(super) fn change_sidebar_section(
        &mut self,
        change: SidebarSectionChange,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(key) = self.active_sidebar_pin_profile_key(cx) else {
            return false;
        };
        if self.state.read(cx).workspace_scope == Some(WorkspaceScope::Local) {
            if let SidebarSectionChange::Assign {
                session_id,
                section_id,
            } = &change
            {
                if section_id
                    .as_ref()
                    .is_some_and(|id| !self.active_sidebar_sections(cx).iter().any(|s| &s.id == id))
                {
                    return false;
                }
                if let Some(pins) = self
                    .settings
                    .sidebar_pinned_session_ids_by_profile
                    .get_mut(&key)
                {
                    // Clearing membership after a local pin must preserve the pin.
                    if section_id.is_some() {
                        pins.retain(|id| id != session_id);
                    }
                }
            }
            change.project(
                self.settings
                    .sidebar_sections_by_profile
                    .entry(key)
                    .or_default(),
            );
            self.schedule_save(cx);
            cx.notify();
            true
        } else {
            if !self.state.read(cx).sidebar_preferences.can_edit() {
                self.sidebar_notice = Some(
                    i18n::translate(MessageId::SidebarSectionsStillSyncing, i18n::locale(cx))
                        .into(),
                );
                cx.notify();
                return false;
            }
            self.queue_sidebar_pin_write(key, SidebarPinChange::Section { change }, cx)
        }
    }

    pub(super) fn migrate_sidebar_sections(&mut self, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        if state.workspace_scope == Some(WorkspaceScope::Local) || !state.sidebar_preferences.synced
        {
            return;
        }
        let Some(engine) = state.engine().cloned() else {
            return;
        };
        let Some(key) = self.active_sidebar_pin_profile_key(cx) else {
            return;
        };
        if self
            .sidebar_section_migration
            .as_ref()
            .is_some_and(|(profile, previous)| profile == &key && previous.same_connection(&engine))
        {
            return;
        }
        let Some(sections) = self
            .settings
            .sidebar_sections_by_profile
            .get(&key)
            .filter(|s| !s.is_empty())
            .cloned()
        else {
            return;
        };
        if self.queue_sidebar_pin_write(
            key.clone(),
            SidebarPinChange::Section {
                change: SidebarSectionChange::Import { sections },
            },
            cx,
        ) {
            self.sidebar_section_migration = Some((key, engine));
        }
    }

    pub(super) fn open_section_dialog(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        let Some(profile) = self.active_sidebar_pin_profile_key(cx) else {
            return;
        };
        self.close_sidebar_view_menu(cx);
        self.section_menu = None;
        let name = self
            .active_sidebar_sections(cx)
            .into_iter()
            .find(|section| Some(&section.id) == id.as_ref())
            .map(|s| s.name)
            .unwrap_or_default();
        let locale = i18n::locale(cx);
        let input = cx.new(|cx| {
            ComposerInput::new(
                i18n::translate(MessageId::SidebarSectionNamePlaceholder, locale),
                cx,
            )
        });
        input.update(cx, |input, cx| input.set_text(name, cx));
        let events = cx.subscribe(&input, |this: &mut Shell, _, event, cx| {
            if matches!(event, ComposerInputEvent::Submitted) {
                this.submit_section_dialog(cx);
            } else {
                cx.notify();
            }
        });
        self.section_dialog = Some(SectionDialog {
            profile,
            id,
            input,
            focus_pending: true,
            _events: events,
        });
        cx.notify();
    }

    fn submit_section_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.section_dialog.as_ref() else {
            return;
        };
        if self.active_sidebar_pin_profile_key(cx).as_ref() != Some(&dialog.profile) {
            self.section_dialog = None;
            cx.notify();
            return;
        }
        let name = dialog.input.read(cx).text().trim().to_string();
        if name.is_empty() || name.chars().count() > 120 {
            return;
        }
        let dialog = self.section_dialog.take().unwrap();
        let change = match dialog.id.clone() {
            Some(id) => SidebarSectionChange::Rename { id, name },
            None => SidebarSectionChange::Create {
                id: uuid::Uuid::new_v4().to_string(),
                name,
            },
        };
        if !self.change_sidebar_section(change, cx) {
            self.section_dialog = Some(dialog);
        }
        cx.notify();
    }

    fn delete_sidebar_section(&mut self, id: &str, cx: &mut Context<Self>) {
        self.section_menu = None;
        self.cancel_sidebar_session_transfer(cx);
        self.change_sidebar_section(SidebarSectionChange::Delete { id: id.to_owned() }, cx);
        cx.notify();
    }

    fn archive_sidebar_section(&mut self, id: &str, cx: &mut Context<Self>) {
        self.section_menu = None;
        let Some(section) = self
            .active_sidebar_sections(cx)
            .into_iter()
            .find(|s| s.id == id)
        else {
            return;
        };
        let state = self.state.read(cx);
        let ids: Vec<_> = state
            .chats
            .iter()
            .filter(|chat| !chat.archived && section.session_ids.contains(&chat.id))
            .map(|chat| chat.id.clone())
            .collect();
        if ids.is_empty() {
            cx.notify();
            return;
        }
        let Some(engine) = state.engine().cloned() else {
            self.sidebar_notice =
                Some(i18n::translate(MessageId::ErrorEngineNotConnected, i18n::locale(cx)).into());
            cx.notify();
            return;
        };
        // A separate task preserves every archive request; the single-row mutation
        // slot intentionally replaces previous work and cannot be used in a loop.
        cx.spawn(async move |this, cx| {
            let mut failed = 0;
            for chat_id in ids {
                if engine.client().call(methods::MUTATE, serde_json::json!({"op":"setChatArchived", "chatId":chat_id, "archived":true})).await.is_err() { failed += 1; }
            }
            if failed > 0 {
                let _ = this.update(cx, |this, cx| {
                    if this.state.read(cx).engine().is_some_and(|current| current.same_connection(&engine)) {
                        this.sidebar_notice = Some(
                            i18n::fill(
                                MessageId::SidebarSectionsArchiveFailed,
                                "{failed}",
                                &failed.to_string(),
                                i18n::locale(cx),
                            )
                            .into(),
                        );
                        cx.notify();
                    }
                });
            }
        }).detach();
        cx.notify();
    }

    pub(super) fn render_custom_sidebar_section(
        &mut self,
        section: SidebarSection,
        rows: Vec<SidebarKeyedRow>,
        drag_group: String,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> SidebarKeyedRow {
        let locale = i18n::locale(cx);
        let id = section.id.clone();
        let open = !section.collapsed;
        let empty = rows.is_empty();
        let extra = self.sidebar_transfer_extra_gap(&drag_group);
        let height = spaces::SIDEBAR_DISCLOSURE_BODY_INSET
            + extra
            + if empty {
                40.0
            } else {
                rows.iter().map(|(_, h, _)| h).sum::<f32>()
                    + SIDEBAR_LIST_GAP * rows.len().saturating_sub(1) as f32
            };
        let motion_key = format!("custom:{id}");
        let hover_id = id.clone();
        let toggle_id = id.clone();
        let toggle_motion = motion_key.clone();
        let menu_id = id.clone();
        let show_menu = self.section_header_hover.as_ref() == Some(&id)
            || self.section_menu.as_ref().is_some_and(|(s, _)| s == &id);
        let chevron = self.sidebar_disclosure_chevron(&motion_key, open, theme);
        let header = div()
            .id(SharedString::from(format!("section-header-{id}")))
            .h(px(spaces::SIDEBAR_DISCLOSURE_HEADER_HEIGHT))
            .px(px(Theme::SPACE_SM))
            .flex()
            .items_center()
            .gap(px(8.0))
            .cursor_pointer()
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.section_header_hover = hovered.then(|| hover_id.clone());
                cx.notify();
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.begin_sidebar_disclosure_motion(
                    &toggle_motion,
                    if open { height } else { 0.0 },
                    if open { 0.0 } else { height },
                );
                this.change_sidebar_section(
                    SidebarSectionChange::Collapse {
                        id: toggle_id.clone(),
                        collapsed: open,
                    },
                    cx,
                );
                cx.notify();
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(crate::typography::ui_rems(12.0))
                    .text_color(theme.text_muted.opacity(0.5))
                    .child(section.name),
            )
            .when(show_menu, |el| {
                el.child(
                    div()
                        .id(SharedString::from(format!("section-menu-{menu_id}")))
                        .size(px(20.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.0))
                        .hover(|el| el.bg(theme.glass_hover()))
                        .on_click(
                            cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                this.section_menu = Some((menu_id.clone(), event.position()));
                                this.section_menu_active = None;
                                window.focus(&this.section_menu_focus, cx);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .child(
                            icon(icons::MORE_HORIZONTAL)
                                .size(px(14.0))
                                .text_color(theme.text_muted),
                        ),
                )
            })
            .child(chevron);
        let content = div()
            .w_full()
            .flex()
            .flex_col()
            .pt(px(spaces::SIDEBAR_DISCLOSURE_BODY_INSET))
            .gap(px(SIDEBAR_LIST_GAP))
            .when(empty, |el| {
                el.child(
                    div()
                        .h(px(40.0))
                        .px(px(Theme::SPACE_SM))
                        .flex()
                        .items_center()
                        .text_size(crate::typography::ui_rems(12.0))
                        .text_color(theme.text_muted.opacity(0.5))
                        .child(i18n::translate(MessageId::SidebarSectionEmpty, locale)),
                )
            })
            .children(rows.into_iter().map(|(_, _, row)| row))
            .when(extra > 0.0, |el| el.child(div().h(px(extra)).flex_none()));
        let body = self.render_sidebar_disclosure_body(
            &motion_key,
            open,
            height,
            content.into_any_element(),
        );
        let drop_id = id.clone();
        let element = div()
            .id(SharedString::from(format!("custom-section-{id}")))
            .w_full()
            .flex()
            .flex_col()
            .pt(px(12.0))
            .on_drag_move::<SidebarSessionDrag>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, cx| {
                    if (empty || !open) && event.bounds.contains(&event.event.position) {
                        let top = f32::from(
                            event.bounds.top()
                                - this.sidebar_scroll.bounds().top()
                                - this.sidebar_scroll.offset().y,
                        ) + 12.0
                            + spaces::SIDEBAR_DISCLOSURE_HEADER_HEIGHT;
                        if let Some(drag) = this.sidebar_session_transfer.as_mut() {
                            drag.preview = Some(SidebarSessionGap {
                                group: drag_group.clone(),
                                index: 0,
                                pinned: false,
                                top,
                            });
                        }
                        cx.notify();
                    }
                },
            ))
            .on_drop::<SidebarSessionDrag>(cx.listener(move |this, payload, _, cx| {
                this.finish_sidebar_session_transfer(
                    payload,
                    SidebarSessionDrop::Section(drop_id.clone()),
                    cx,
                );
                cx.stop_propagation();
            }))
            .child(header)
            .child(body)
            .into_any_element();
        (
            format!("custom:{id}"),
            12.0 + spaces::SIDEBAR_DISCLOSURE_HEADER_HEIGHT + if open { height } else { 0.0 },
            element,
        )
    }

    fn activate_section_menu(&mut self, action: usize, id: &str, cx: &mut Context<Self>) {
        match action {
            0 => self.open_section_dialog(Some(id.to_string()), cx),
            1 => self.archive_sidebar_section(id, cx),
            _ => self.delete_sidebar_section(id, cx),
        }
    }

    pub(super) fn render_section_overlays(
        &mut self,
        viewport: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let theme = Theme::of(cx).for_popup();
        let locale = i18n::locale(cx);
        let mut overlays = Vec::new();
        if let Some((id, position)) = self.section_menu.clone() {
            if !self.active_sidebar_sections(cx).iter().any(|s| s.id == id) {
                self.section_menu = None;
                return overlays;
            }
            let mut card = popover::popover_card(&theme)
                .w(px(180.0))
                .flex()
                .flex_col()
                .track_focus(&self.section_menu_focus)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    match event.keystroke.key.as_str() {
                        "escape" => this.section_menu = None,
                        "up" | "down" => {
                            this.section_menu_active = popover::menu_step(
                                this.section_menu_active,
                                3,
                                if event.keystroke.key == "up" { -1 } else { 1 },
                            )
                        }
                        "enter" => {
                            if let (Some(action), Some((id, _))) =
                                (this.section_menu_active, this.section_menu.clone())
                            {
                                this.activate_section_menu(action, &id, cx);
                            }
                        }
                        _ => return,
                    }
                    cx.stop_propagation();
                    cx.notify();
                }))
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.section_menu = None;
                    cx.notify();
                }));
            for (action, label) in [
                MessageId::SidebarSectionEdit,
                MessageId::SidebarSectionArchiveAll,
                MessageId::CommonDelete,
            ]
            .into_iter()
            .enumerate()
            {
                let target = id.clone();
                card = card.child(
                    popover::menu_row_nav(
                        &theme,
                        false,
                        self.section_menu_active == Some(action),
                        format!("section-action-{action}"),
                    )
                    .id(("section-action", action))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.activate_section_menu(action, &target, cx);
                        cx.stop_propagation();
                    }))
                    .child(i18n::translate(label, locale)),
                );
            }
            overlays.push(popover::menu_at(
                "section-context-menu",
                position,
                card.into_any_element(),
                None,
            ));
        }
        if let Some(dialog) = &mut self.section_dialog {
            if std::mem::take(&mut dialog.focus_pending) {
                window.focus(&dialog.input.focus_handle(cx), cx);
            }
            let edit = dialog.id.is_some();
            let input = dialog.input.clone();
            let name = input.read(cx).text().trim().to_string();
            let valid = !name.is_empty() && name.chars().count() <= 120;
            let card = popover::dialog_card(&theme)
                .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                    if event.keystroke.key == "escape" {
                        this.section_dialog = None;
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(popover::dialog_title(
                            &theme,
                            i18n::translate(
                                if edit {
                                    MessageId::SidebarSectionEdit
                                } else {
                                    MessageId::SidebarSectionNew
                                },
                                locale,
                            ),
                        ))
                        .child(
                            div()
                                .id("section-dialog-close")
                                .size(px(24.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.section_dialog = None;
                                    cx.notify();
                                }))
                                .child(
                                    icon(icons::CLOSE)
                                        .size(px(16.0))
                                        .text_color(theme.text_muted),
                                ),
                        ),
                )
                .child(
                    div()
                        .mt(px(8.0))
                        .text_color(theme.text_muted)
                        .child(i18n::translate(MessageId::SidebarSectionDialogHint, locale)),
                )
                .child(
                    div()
                        .mt(px(16.0))
                        .child(popover::dialog_field(input.into_any_element())),
                )
                .child(
                    div()
                        .mt(px(16.0))
                        .flex()
                        .justify_end()
                        .gap(px(8.0))
                        .child(
                            popover::btn_ghost(
                                &theme,
                                i18n::translate(MessageId::CommonCancel, locale),
                                "section-cancel",
                            )
                            .id("section-cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.section_dialog = None;
                                cx.notify();
                            })),
                        )
                        .child(
                            popover::btn_primary(
                                &theme,
                                i18n::translate(
                                    if edit {
                                        MessageId::CommonSave
                                    } else {
                                        MessageId::SidebarSectionCreate
                                    },
                                    locale,
                                ),
                            )
                            .id("section-save")
                            .opacity(if valid { 1.0 } else { 0.5 })
                            .on_click(cx.listener(|this, _, _, cx| this.submit_section_dialog(cx))),
                        ),
                );
            overlays.push(popover::modal(
                "section-dialog",
                viewport,
                card.into_any_element(),
            ));
        }
        overlays
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_shell(
        cx: &mut gpui::TestAppContext,
        path: &std::path::Path,
    ) -> gpui::WindowHandle<super::Shell> {
        use super::*;
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
            crate::app_menus::init(cx);
            crate::history::init(
                Default::default(),
                Default::default(),
                Default::default(),
                Default::default(),
                cx,
            );
        });
        cx.add_window(|_, cx| {
            let state = cx.new(|_| AppState::new());
            Shell::new(
                state,
                EngineBootConfig {
                    data_dir: path.into(),
                    ipc_port: 0,
                    edge_url: "http://127.0.0.1:1".into(),
                    edge_token: None,
                    org_id: None,
                    workos_client_id: None,
                    default_harness: zeron_proto::HarnessId::Mock,
                },
                cx,
            )
        })
    }

    fn chat(id: &str) -> zeron_proto::Chat {
        serde_json::from_value(serde_json::json!({"id":id,"title":id,"deviceId":"local","archived":false,"createdAt":"2026-09-20T00:00:00Z"})).unwrap()
    }
    fn prepare(shell: &mut Shell, cx: &mut Context<Shell>) {
        shell.state.update(cx, |state, _| {
            state.workspace_scope = Some(WorkspaceScope::Local);
            state.local_device_id = Some("local".into());
            state.chats_synced = true;
            state.chats = vec![chat("pin"), chat("regular"), chat("other")];
        });
        shell.settings.space_filter = None;
        shell.settings.sidebar_organization = SidebarOrganization::InOneList;
        shell
            .settings
            .sidebar_pins_mut("local".into())
            .push("pin".into());
        shell.settings.sidebar_sections_by_profile.insert(
            "local".into(),
            vec![
                SidebarSection {
                    id: "a".into(),
                    name: "Focus".into(),
                    session_ids: vec![],
                    collapsed: false,
                },
                SidebarSection {
                    id: "b".into(),
                    name: "Later".into(),
                    session_ids: vec![],
                    collapsed: false,
                },
            ],
        );
    }

    #[gpui::test]
    fn sections_transfer_between_pins_sections_and_regular_without_duplicates(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let window = test_shell(cx, dir.path());
        window
            .update(cx, |shell, window, cx| {
                prepare(shell, cx);
                for (id, target) in [
                    ("pin", SidebarSessionDrop::Section("a".into())),
                    ("regular", SidebarSessionDrop::Section("a".into())),
                    ("regular", SidebarSessionDrop::Section("b".into())),
                    ("regular", SidebarSessionDrop::Regular),
                    ("pin", SidebarSessionDrop::Pinned(0)),
                ] {
                    let payload = SidebarSessionDrag {
                        chat_id: id.into(),
                        visible_ids: std::sync::Arc::new(shell.active_sidebar_pins(cx)),
                        filter: None,
                        profile_key: "local".into(),
                    };
                    shell.begin_sidebar_session_transfer(
                        &payload,
                        gpui::point(px(10.0), px(10.0)),
                        window,
                        cx,
                    );
                    shell.finish_sidebar_session_transfer(&payload, target.clone(), cx);
                    let sections = shell.active_sidebar_sections(cx);
                    let memberships: Vec<_> = sections
                        .iter()
                        .filter(|section| section.session_ids.contains(&id.to_string()))
                        .collect();
                    match target {
                        SidebarSessionDrop::Section(section) => {
                            assert_eq!(memberships.len(), 1);
                            assert_eq!(memberships[0].id, section);
                            assert!(!shell.active_sidebar_pins(cx).contains(&id.to_string()));
                        }
                        SidebarSessionDrop::Pinned(_) => {
                            assert!(memberships.is_empty());
                            assert!(shell.active_sidebar_pins(cx).contains(&id.to_string()));
                        }
                        SidebarSessionDrop::Regular => {
                            assert!(memberships.is_empty());
                            assert!(!shell.active_sidebar_pins(cx).contains(&id.to_string()));
                        }
                    }
                    let order = shell.sidebar_visible_order(cx);
                    assert_eq!(order.len(), 3);
                    assert_eq!(
                        order.iter().collect::<std::collections::HashSet<_>>().len(),
                        3
                    );
                }
                shell.assign_sidebar_section("regular", Some("a"), cx);
                shell.set_chat_pinned("regular".into(), true, cx);
                assert!(shell.active_sidebar_pins(cx).contains(&"regular".into()));
                assert!(
                    shell
                        .active_sidebar_sections(cx)
                        .iter()
                        .all(|s| !s.session_ids.contains(&"regular".into()))
                );
            })
            .unwrap();
    }

    #[gpui::test]
    fn sections_dialog_persistence_deletion_and_profile_isolation(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                prepare(shell, cx);
                shell.open_section_dialog(None, cx);
                shell.submit_section_dialog(cx);
                assert!(
                    shell.section_dialog.is_some(),
                    "Empty name must not create a section"
                );
                let input = shell.section_dialog.as_ref().unwrap().input.clone();
                input.update(cx, |input, cx| input.set_text("  Review  ", cx));
                shell.submit_section_dialog(cx);
                let created = shell.active_sidebar_sections(cx).last().unwrap().clone();
                assert_eq!(created.name, "Review");
                shell.assign_sidebar_section("regular", Some(&created.id), cx);
                shell.open_section_dialog(Some(created.id.clone()), cx);
                let input = shell.section_dialog.as_ref().unwrap().input.clone();
                input.update(cx, |input, cx| input.set_text("Today", cx));
                shell.submit_section_dialog(cx);
                assert_eq!(
                    shell.active_sidebar_sections(cx).last().unwrap().name,
                    "Today"
                );
                shell.settings.save(dir.path()).unwrap();
                let loaded = crate::settings::UiSettings::load(dir.path());
                assert_eq!(
                    loaded.sidebar_sections_by_profile,
                    shell.settings.sidebar_sections_by_profile
                );
                shell
                    .settings
                    .sidebar_sections_by_profile
                    .get_mut("local")
                    .unwrap()
                    .last_mut()
                    .unwrap()
                    .collapsed = true;
                assert!(!shell.sidebar_visible_order(cx).contains(&"regular".into()));
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Synced)
                });
                assert!(shell.active_sidebar_sections(cx).is_empty());
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Local)
                });
                assert_eq!(shell.active_sidebar_sections(cx).len(), 3);
                shell.delete_sidebar_section(&created.id, cx);
                assert_eq!(shell.state.read(cx).chats.len(), 3);
                assert!(shell.sidebar_visible_order(cx).contains(&"regular".into()));
                assert_eq!(
                    shell.active_sidebar_sections(cx).len(),
                    2,
                    "Empty sections are retained"
                );
            })
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn sections_archive_all_sends_every_request_and_reports_failures(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, mut requests) = tokio::sync::mpsc::channel(16);
        let (replies, inbound) = tokio::sync::mpsc::channel(16);
        let engine =
            crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound));
        let dir = tempfile::tempdir().unwrap();
        let window = test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                prepare(shell, cx);
                shell.assign_sidebar_section("regular", Some("a"), cx);
                shell.assign_sidebar_section("other", Some("a"), cx);
                shell
                    .state
                    .update(cx, |state, _| state.set_test_engine(engine));
                shell.archive_sidebar_section("a", cx);
            })
            .unwrap();
        for (index, expected) in ["regular", "other"].into_iter().enumerate() {
            cx.run_until_parked();
            let request: serde_json::Value = serde_json::from_str(
                &requests
                    .try_recv()
                    .expect("Each section session must be archived"),
            )
            .unwrap();
            assert_eq!(request["params"]["op"], "setChatArchived");
            assert_eq!(request["params"]["chatId"], expected);
            assert_eq!(request["params"]["archived"], true);
            let reply = if index == 0 {
                serde_json::json!({"id":request["id"],"err":"rejected"})
            } else {
                serde_json::json!({"id":request["id"],"ok":{"ok":true}})
            };
            runtime.block_on(async {
                replies.send(reply.to_string()).await.unwrap();
                while replies.capacity() < replies.max_capacity() {
                    tokio::task::yield_now().await;
                }
            });
        }
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                assert!(
                    shell
                        .sidebar_notice
                        .as_ref()
                        .is_some_and(|text| text.contains("1 sessions"))
                );
                assert_eq!(
                    shell.active_sidebar_sections(cx)[0].session_ids.len(),
                    2,
                    "Archiving preserves section membership"
                );
                assert_eq!(shell.state.read(cx).chats.len(), 3);
            })
            .unwrap();
    }
    #[gpui::test]
    fn sections_remote_pin_rejection_restores_membership_and_old_ack_keeps_new_move(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, _requests) = tokio::sync::mpsc::channel(16);
        let (_replies, inbound) = tokio::sync::mpsc::channel(16);
        let engine =
            crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound));
        let dir = tempfile::tempdir().unwrap();
        let window = test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                prepare(shell, cx);
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Synced);
                    state.auth = Some(zeron_proto::AuthState::SignedIn {
                        user: zeron_proto::UserProfile {
                            id: "user".into(),
                            email: "test@example.test".into(),
                            name: None,
                        },
                        org_id: Some("org".into()),
                    });
                    state.sidebar_preferences.initialized = true;
                    state.set_test_engine(engine.clone());
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                shell.settings.sidebar_sections_by_profile.insert(
                    key.clone(),
                    vec![SidebarSection {
                        id: "a".into(),
                        name: "Focus".into(),
                        session_ids: vec!["regular".into()],
                        collapsed: false,
                    }],
                );
                let section = shell.settings.sidebar_sections_by_profile[&key][0].clone();
                shell.state.update(cx, |state, _| {
                    state.sidebar_preferences.sections = vec![section.clone()];
                });
                let empty_section = SidebarSection {
                    session_ids: vec![],
                    ..section.clone()
                };
                let pin = zeron_proto::SidebarPinChange::Pin {
                    session_id: "regular".into(),
                    after: None,
                    before: None,
                };
                shell.sidebar_pin_write = Some(super::super::sidebar_pins::PendingSidebarPins {
                    id: 1,
                    profile_key: key.clone(),
                    engine: engine.clone(),
                    queue: std::collections::VecDeque::from([pin.clone()]),
                    unconfirmed: false,
                });
                assert!(shell.active_sidebar_sections(cx)[0].session_ids.is_empty());
                assert_eq!(shell.active_sidebar_pins(cx), vec!["regular".to_string()]);
                shell.finish_sidebar_pin_write(
                    1,
                    Err(super::super::sidebar_pins::PinWriteFailure::Detail(
                        "rejected".into(),
                    )),
                    cx,
                );
                assert_eq!(
                    shell.active_sidebar_sections(cx)[0].session_ids,
                    vec!["regular".to_string()]
                );
                assert!(shell.active_sidebar_pins(cx).is_empty());
                shell.sidebar_pin_write = Some(super::super::sidebar_pins::PendingSidebarPins {
                    id: 2,
                    profile_key: key.clone(),
                    engine: engine.clone(),
                    queue: std::collections::VecDeque::from([
                        pin.clone(),
                        zeron_proto::SidebarPinChange::Section {
                            change: SidebarSectionChange::Assign {
                                session_id: "regular".into(),
                                section_id: Some("a".into()),
                            },
                        },
                    ]),
                    unconfirmed: false,
                });
                shell.finish_sidebar_pin_write(
                    2,
                    Ok(zeron_proto::SidebarPreferencesState {
                        sections: vec![empty_section.clone()],
                        revision: 1,
                        synced: true,
                        initialized: true,
                        pinned_session_ids: vec!["regular".into()],
                    }),
                    cx,
                );
                assert_eq!(
                    shell.active_sidebar_sections(cx)[0].session_ids,
                    vec!["regular".to_string()]
                );
                assert!(shell.active_sidebar_pins(cx).is_empty());
                shell.finish_sidebar_pin_write(
                    2,
                    Ok(zeron_proto::SidebarPreferencesState {
                        sections: vec![section.clone()],
                        revision: 2,
                        synced: true,
                        initialized: true,
                        pinned_session_ids: vec![],
                    }),
                    cx,
                );
                shell.sidebar_pin_write = Some(super::super::sidebar_pins::PendingSidebarPins {
                    id: 3,
                    profile_key: key.clone(),
                    engine: engine.clone(),
                    queue: std::collections::VecDeque::from([pin]),
                    unconfirmed: false,
                });
                shell.finish_sidebar_pin_write(
                    3,
                    Ok(zeron_proto::SidebarPreferencesState {
                        sections: vec![empty_section],
                        revision: 3,
                        synced: true,
                        initialized: true,
                        pinned_session_ids: vec!["regular".into()],
                    }),
                    cx,
                );
                assert!(shell.active_sidebar_sections(cx)[0].session_ids.is_empty());
                assert_eq!(shell.active_sidebar_pins(cx), vec!["regular".to_string()]);
            })
            .unwrap();
    }
    #[gpui::test]
    fn sections_migration_waits_for_sync_and_keeps_local_copy_until_ack(
        cx: &mut gpui::TestAppContext,
    ) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let (out, _requests) = tokio::sync::mpsc::channel(16);
        let (_replies, inbound) = tokio::sync::mpsc::channel(16);
        let engine =
            crate::state::EngineHandle::from_test_client(zeron_rpc::RpcClient::new(out, inbound));
        let dir = tempfile::tempdir().unwrap();
        let window = test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                prepare(shell, cx);
                shell.state.update(cx, |state, _| {
                    state.workspace_scope = Some(WorkspaceScope::Synced);
                    state.auth = Some(zeron_proto::AuthState::SignedIn {
                        user: zeron_proto::UserProfile {
                            id: "user".into(),
                            email: "test@example.test".into(),
                            name: None,
                        },
                        org_id: Some("org".into()),
                    });
                    state.set_test_engine(engine.clone());
                });
                let key = shell.active_sidebar_pin_profile_key(cx).unwrap();
                let sections = shell.settings.sidebar_sections_by_profile["local"].clone();
                shell
                    .settings
                    .sidebar_sections_by_profile
                    .insert(key.clone(), sections.clone());
                shell.migrate_sidebar_sections(cx);
                assert!(
                    shell.sidebar_pin_write.is_none(),
                    "Wait for the authoritative snapshot"
                );
                shell
                    .state
                    .update(cx, |state, _| state.sidebar_preferences.synced = true);
                shell.migrate_sidebar_sections(cx);
                let id = shell.sidebar_pin_write.as_ref().unwrap().id;
                shell.migrate_sidebar_sections(cx);
                assert_eq!(shell.sidebar_pin_write.as_ref().unwrap().queue.len(), 1);
                shell.finish_sidebar_pin_write(
                    id,
                    Err(super::super::sidebar_pins::PinWriteFailure::Detail(
                        "disk full".into(),
                    )),
                    cx,
                );
                assert_eq!(shell.settings.sidebar_sections_by_profile[&key], sections);
                // A reconnect/restart permits another idempotent attempt.
                shell.sidebar_section_migration = None;
                shell.migrate_sidebar_sections(cx);
                let id = shell.sidebar_pin_write.as_ref().unwrap().id;
                shell.finish_sidebar_pin_write(
                    id,
                    Ok(zeron_proto::SidebarPreferencesState {
                        revision: 1,
                        synced: true,
                        initialized: true,
                        pinned_session_ids: vec![],
                        sections: sections.clone(),
                    }),
                    cx,
                );
                assert!(
                    !shell
                        .settings
                        .sidebar_sections_by_profile
                        .contains_key(&key)
                );
                assert_eq!(shell.active_sidebar_sections(cx), sections);
                assert!(
                    shell
                        .settings
                        .sidebar_sections_by_profile
                        .contains_key("local")
                );
            })
            .unwrap();
    }
}
