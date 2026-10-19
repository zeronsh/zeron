//! Account-synced sections; local workspaces keep their settings on this device.
use super::*;
use crate::settings::SidebarSection;
use zeron_proto::{SidebarPinChange, SidebarSectionChange};

/// The air above each custom section's header.
const SIDEBAR_SECTION_TOP_PAD: f32 = 12.0;

/// Where a lifted section lands among its siblings: past each one whose middle
/// its leading edge has crossed. `top` is the lifted section's top measured
/// from the first slot; natural slots keep the thresholds still while the
/// siblings slide.
fn section_drop_index(top: f32, heights: &[f32], from: usize) -> usize {
    let bottom = top + heights[from];
    let mut slot_top = 0.0;
    let mut over = 0;
    for (ix, height) in heights.iter().enumerate() {
        let middle = slot_top + height / 2.0;
        if (ix < from && top > middle) || (ix > from && bottom > middle) {
            over += 1;
        }
        slot_top += height + SIDEBAR_LIST_GAP;
    }
    over
}

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
                after: None,
                before: None,
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
                ..
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
                self.show_notice(crate::toast::ToastKind::Error, "Sidebar is still syncing. Try again shortly.", cx);
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
        let input = cx.new(|cx| ComposerInput::new("Section name", cx));
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
            self.show_notice(crate::toast::ToastKind::Error, "Engine not connected", cx);
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
                        this.show_notice(crate::toast::ToastKind::Error, format!("Could not archive {failed} sessions. Try again."), cx);
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
        drag: Option<SidebarSectionDrag>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> SidebarKeyedRow {
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
        self.begin_queued_sidebar_reveal(&motion_key, open, height);
        let hover_id = id.clone();
        let toggle_id = id.clone();
        let toggle_motion = motion_key.clone();
        let menu_id = id.clone();
        let show_menu = self.section_header_hover.as_ref() == Some(&id)
            || self.section_menu.as_ref().is_some_and(|(s, _)| s == &id);
        let chevron = self.sidebar_disclosure_chevron(&motion_key, open, theme);
        let session_count = rows.len();
        let menu_button = show_menu.then(|| {
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
                .tooltip(crate::settings::widgets::text_tooltip("Section options"))
                .child(
                    icon(icons::MORE_HORIZONTAL)
                        .size(px(14.0))
                        .text_color(spaces::sidebar_header_glyph(theme)),
                )
                .into_any_element()
        });
        let header = spaces::sidebar_disclosure_header(
            theme,
            None,
            section.name.clone().into(),
            (!open).then_some(session_count),
            menu_button,
            chevron,
        )
        .id(SharedString::from(format!("section-header-{id}")))
        .debug_selector({
            let id = id.clone();
            move || format!("section-header-{id}")
        })
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
        // The header lifts the whole section to reorder it among the others.
        .when_some(drag, |el, payload| {
            let shell = cx.entity();
            el.on_drag(payload, move |payload, grab, window, cx| {
                shell.update(cx, |shell, cx| {
                    shell.begin_sidebar_section_drag(payload, grab, window, cx);
                });
                cx.stop_propagation();
                cx.new(|_| DragGhost)
            })
        });
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
                        .child("Drop sessions here"),
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
            .pt(px(SIDEBAR_SECTION_TOP_PAD))
            .on_drag_move::<SidebarSessionDrag>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<SidebarSessionDrag>, _, cx| {
                    if (empty || !open) && event.bounds.contains(&event.event.position) {
                        let top = f32::from(
                            event.bounds.top()
                                - this.sidebar_scroll.bounds().top()
                                - this.sidebar_scroll.offset().y,
                        ) + SIDEBAR_SECTION_TOP_PAD
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
            SIDEBAR_SECTION_TOP_PAD
                + spaces::SIDEBAR_DISCLOSURE_HEADER_HEIGHT
                + if open { height } else { 0.0 },
            element,
        )
    }

    /// Sections reorder in this device's settings, or through an engine that
    /// orders them: an older engine rejects the move outright.
    pub(super) fn sidebar_sections_reorderable(&self, cx: &App) -> bool {
        let state = self.state.read(cx);
        state.workspace_scope == Some(WorkspaceScope::Local)
            || (state.sidebar_preferences.can_edit()
                && state.engine().is_some_and(|engine| {
                    engine
                        .engine_info()
                        .supports(zeron_proto::capabilities::SIDEBAR_SECTION_ORDER_V1)
                }))
    }

    /// `grab` is where the header was pressed, from its origin: the drag only
    /// starts once the pointer has already moved on.
    pub(super) fn begin_sidebar_section_drag(
        &mut self,
        payload: &SidebarSectionDrag,
        grab: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let bounds = self.sidebar_section_bounds.borrow().clone();
        let Some(from) = payload.ids.iter().position(|id| id == &payload.section_id) else {
            return;
        };
        if bounds.len() != payload.ids.len() {
            return;
        }
        self.cancel_sidebar_session_transfer(cx);
        self.section_menu = None;
        let viewport = self.sidebar_scroll.bounds();
        let scroll_top = -f32::from(self.sidebar_scroll.offset().y);
        let pointer = window.mouse_position();
        self.pinned_session_drag_generation = self.pinned_session_drag_generation.wrapping_add(1);
        let generation = self.pinned_session_drag_generation;
        self.sidebar_section_drag = Some(SidebarSectionDragState {
            payload: payload.clone(),
            from,
            over: from,
            heights: bounds.iter().map(|b| f32::from(b.size.height)).collect(),
            top: f32::from(bounds[0].top() - viewport.top()) + scroll_top,
            left: f32::from(bounds[0].left() - viewport.left()),
            width: f32::from(bounds[0].size.width),
            grab_y: f32::from(grab.y) + SIDEBAR_SECTION_TOP_PAD,
            pointer,
            siblings: Default::default(),
            generation,
        });
        // Siblings ease and the list edge-scrolls while the pointer rests.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(
                        SIDEBAR_DRAG_SCROLL_FRAME_MS,
                    ))
                    .await;
                let keep_running = this
                    .update(cx, |shell, cx| {
                        shell.sidebar_section_drag_tick(generation, cx)
                    })
                    .unwrap_or(false);
                if !keep_running {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn sidebar_section_drag_tick(&mut self, generation: u64, cx: &mut Context<Self>) -> bool {
        let Some(drag) = self.sidebar_section_drag.as_ref() else {
            return false;
        };
        if drag.generation != generation {
            return false;
        }
        cx.notify();
        if !cx.has_active_drag() {
            // A release outside the sidebar: render settles it home.
            return false;
        }
        let viewport = self.sidebar_scroll.bounds();
        let pointer = drag.pointer;
        if pointer.x >= viewport.left() && pointer.x <= viewport.right() {
            let delta = spaces::pinned_drag_scroll_delta(
                f32::from(pointer.y),
                f32::from(viewport.top()),
                f32::from(viewport.bottom()),
            );
            let offset = self.sidebar_scroll.offset();
            let scroll_top = -f32::from(offset.y);
            let max_scroll = f32::from(self.sidebar_scroll.max_offset().y);
            let next = (scroll_top + delta).clamp(0.0, max_scroll);
            if next != scroll_top {
                self.sidebar_scroll
                    .set_offset(gpui::point(offset.x, px(-next)));
                self.update_sidebar_section_drop();
            }
        }
        true
    }

    pub(super) fn track_sidebar_section_drag(
        &mut self,
        pointer: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if let Some(drag) = self.sidebar_section_drag.as_mut() {
            drag.pointer = pointer;
            self.update_sidebar_section_drop();
            cx.notify();
        }
    }

    /// The lifted section's top in the list's content coordinates, unclamped.
    fn lifted_sidebar_section_y(&self, drag: &SidebarSectionDragState) -> f32 {
        let viewport_top = f32::from(self.sidebar_scroll.bounds().top());
        let scroll_top = -f32::from(self.sidebar_scroll.offset().y);
        f32::from(drag.pointer.y) - viewport_top + scroll_top - drag.grab_y
    }

    /// Where the lifted section paints: under the pointer, held within the
    /// span of the sections it reorders among.
    fn lifted_sidebar_section_top(&self, drag: &SidebarSectionDragState) -> f32 {
        let total = drag.heights.iter().sum::<f32>()
            + SIDEBAR_LIST_GAP * drag.heights.len().saturating_sub(1) as f32;
        self.lifted_sidebar_section_y(drag).clamp(
            drag.top,
            drag.top + (total - drag.heights[drag.from]).max(0.0),
        )
    }

    fn update_sidebar_section_drop(&mut self) {
        let Some(drag) = self.sidebar_section_drag.as_ref() else {
            return;
        };
        let top = self.lifted_sidebar_section_y(drag) - drag.top;
        let over = section_drop_index(top, &drag.heights, drag.from);
        if let Some(drag) = self.sidebar_section_drag.as_mut() {
            drag.over = over;
        }
    }

    pub(super) fn sidebar_section_drag_is_valid(&self, cx: &App) -> bool {
        let Some(drag) = self.sidebar_section_drag.as_ref() else {
            return true;
        };
        self.active_sidebar_pin_profile_key(cx).as_deref() == Some(&drag.payload.profile_key)
            && self
                .active_sidebar_sections(cx)
                .iter()
                .map(|section| &section.id)
                .eq(drag.payload.ids.iter())
    }

    pub(super) fn finish_sidebar_section_drag(
        &mut self,
        payload: &SidebarSectionDrag,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.sidebar_section_drag.as_ref() else {
            return;
        };
        if drag.payload.section_id != payload.section_id
            || drag.payload.ids != payload.ids
            || !self.sidebar_section_drag_is_valid(cx)
        {
            self.end_sidebar_section_drag(false, cx);
            return;
        }
        let (from, over) = (drag.from, drag.over);
        if from == over {
            self.end_sidebar_section_drag(false, cx);
            return;
        }
        let mut ids = (*drag.payload.ids).clone();
        let id = ids.remove(from);
        let change = SidebarSectionChange::Move {
            id,
            after: over.checked_sub(1).and_then(|i| ids.get(i)).cloned(),
            before: ids.get(over).cloned(),
        };
        let placed = self.change_sidebar_section(change, cx);
        self.end_sidebar_section_drag(placed, cx);
    }

    pub(super) fn cancel_sidebar_section_drag(&mut self, cx: &mut Context<Self>) {
        self.end_sidebar_section_drag(false, cx);
    }

    /// Land the lifted section in its new slot, or home when `placed` is
    /// false, gliding every section from where it last painted.
    fn end_sidebar_section_drag(&mut self, placed: bool, cx: &mut Context<Self>) {
        let Some(drag) = self.sidebar_section_drag.as_ref() else {
            return;
        };
        let mut settle = std::collections::HashMap::new();
        if !self.reduced_motion {
            let over = if placed { drag.over } else { drag.from };
            let landed = drag.top
                + (0..drag.heights.len())
                    .filter(|ix| *ix != drag.from)
                    .take(over)
                    .map(|ix| drag.heights[ix] + SIDEBAR_LIST_GAP)
                    .sum::<f32>();
            settle.insert(
                format!("custom:{}", drag.payload.section_id),
                self.lifted_sidebar_section_top(drag) - landed,
            );
            let step = drag.heights[drag.from] + SIDEBAR_LIST_GAP;
            for (ix, id) in drag.payload.ids.iter().enumerate() {
                let current = drag.siblings.get(id).map_or(0.0, |slide| slide.current());
                let target = crate::terminal::panel::slide_offset(ix, drag.from, over) * step;
                if ix != drag.from && (current - target).abs() > 0.5 {
                    settle.insert(format!("custom:{id}"), current - target);
                }
            }
        }
        self.sidebar_section_drag = None;
        // The settle glide rides the resort animation; a fresh baseline keeps
        // the reorder itself from replaying as a second glide.
        self.sidebar_prev_order.clear();
        self.sidebar_new_keys.clear();
        self.sidebar_resort = settle;
        self.resort_epoch += 1;
        cx.notify();
    }

    /// Each custom section's displacement this frame while one is lifted.
    pub(super) fn sidebar_section_drag_offsets(&mut self) -> Option<(usize, Vec<f32>)> {
        let reduced_motion = self.reduced_motion;
        let drag = self.sidebar_section_drag.as_mut()?;
        let (from, over) = (drag.from, drag.over);
        let step = drag.heights[from] + SIDEBAR_LIST_GAP;
        let offsets = drag
            .payload
            .ids
            .iter()
            .enumerate()
            .map(|(ix, id)| {
                if ix == from {
                    return 0.0;
                }
                let slide =
                    drag.siblings
                        .entry(id.clone())
                        .or_insert_with(|| SidebarSessionSlide {
                            from: 0.0,
                            to: 0.0,
                            epoch: 0,
                            started: std::time::Instant::now(),
                        });
                slide.retarget(crate::terminal::panel::slide_offset(ix, from, over) * step);
                if reduced_motion {
                    slide.to
                } else {
                    slide.current()
                }
            })
            .collect();
        Some((from, offsets))
    }

    /// The lifted section, frosted a level above the list as a lifted session
    /// is. Its slot's top padding stays outside the card, which frames the
    /// header.
    pub(super) fn render_lifted_sidebar_section(
        &self,
        section: AnyElement,
        theme: &Theme,
    ) -> Option<AnyElement> {
        const LIFT_PAD: f32 = 4.0;
        let drag = self.sidebar_section_drag.as_ref()?;
        let trim = (SIDEBAR_SECTION_TOP_PAD - LIFT_PAD).max(0.0);
        let height = drag.heights[drag.from];
        let card = div()
            .size_full()
            .rounded(px(8.0))
            .bg(crate::popover::surface_bg(theme))
            .shadow_md()
            .child(
                div()
                    .size_full()
                    .rounded(px(8.0))
                    .bg(crate::theme::wash(0.06))
                    .child(div().relative().top(px(-trim)).w_full().child(section)),
            );
        Some(
            gpui::deferred(
                div()
                    .debug_selector(|| "sidebar-lifted-section".into())
                    .absolute()
                    .left(px(drag.left))
                    .top(px(self.lifted_sidebar_section_top(drag) + trim))
                    .w(px(drag.width))
                    .h(px(height - trim + LIFT_PAD))
                    .child(crate::frost::frosted(8.0, crate::frost::MENU_BLUR, card)),
            )
            .with_priority(1)
            .into_any_element(),
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
            for (action, label) in ["Edit section", "Archive all", "Delete"]
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
                    .child(label),
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
                            if edit { "Edit section" } else { "New section" },
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
                        .child("Group sessions however you like"),
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
                            popover::btn_ghost(&theme, "Cancel", "section-cancel")
                                .id("section-cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.section_dialog = None;
                                    cx.notify();
                                })),
                        )
                        .child(
                            popover::btn_primary(
                                &theme,
                                if edit { "Save" } else { "Create section" },
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
    fn the_session_card_shows_beside_a_wide_transcript_and_opens_over_a_narrow_one(
        cx: &mut gpui::TestAppContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let window = test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                prepare(shell, cx);
                shell.debug_gate = Some(super::GatePhase::Ready);
                shell.settings.sidebar_collapsed = true;
                shell
                    .state
                    .update(cx, |state, _| state.selected_chat = Some("regular".into()));
                // The card eases in and out; settle each change at once.
                crate::motion::set_reduced_motion(cx, true);
            })
            .unwrap();
        let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);

        // With room, it shows on its own, clear of the centered transcript.
        cx.simulate_resize(gpui::size(px(1600.0), px(900.0)));
        cx.run_until_parked();
        let card = cx
            .debug_bounds("session-info")
            .expect("a wide column shows the card unasked");
        assert!(card.right() <= px(1600.0));
        assert!(card.left() > px(800.0 + 736.0 / 2.0));
        let actions = cx.debug_bounds("session-info-side-chat").unwrap();
        assert!(card.contains(&actions.center()));

        // Without room it hides, and opening it floats it over the transcript.
        cx.simulate_resize(gpui::size(px(900.0), px(900.0)));
        cx.run_until_parked();
        assert!(cx.debug_bounds("session-info").is_none());
        window
            .update(&mut cx, |shell, _, cx| shell.toggle_session_info(cx))
            .unwrap();
        cx.run_until_parked();
        let card = cx
            .debug_bounds("session-info")
            .expect("opening shows the card over a narrow column");
        assert!(card.right() <= px(900.0));

        // The toggle closes it again.
        window
            .update(&mut cx, |shell, _, cx| shell.toggle_session_info(cx))
            .unwrap();
        cx.run_until_parked();
        assert!(cx.debug_bounds("session-info").is_none());

        // Choosing one of its rows dismisses it too.
        window
            .update(&mut cx, |shell, _, cx| shell.toggle_session_info(cx))
            .unwrap();
        cx.run_until_parked();
        let fork = cx.debug_bounds("session-info-fork").unwrap();
        cx.simulate_click(fork.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        assert!(cx.debug_bounds("session-info").is_none());
    }

    #[gpui::test]
    fn copying_from_the_copy_submenu_shows_a_toast(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                prepare(shell, cx);
                shell.debug_gate = Some(super::GatePhase::Ready);
                shell.chat_menu.open(super::ChatMenuState {
                    chat_id: "regular".into(),
                    tab: None,
                    position: gpui::point(px(40.0), px(120.0)),
                    page: super::ChatMenuPage::Copy,
                });
                cx.notify();
            })
            .unwrap();
        let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
        cx.run_until_parked();
        // The item sits in the submenu, outside the menu card: pressing it
        // must copy rather than dismiss the menu first.
        let item = cx
            .debug_bounds("chat-copy-zeron")
            .expect("the Copy submenu is open");
        cx.simulate_click(item.center(), gpui::Modifiers::none());
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert_eq!(crate::toast::messages(cx).len(), 1, "one toast");
        });
        assert!(cx.debug_bounds("toaster").is_some(), "the shell paints it");
    }

    #[gpui::test]
    fn sections_keep_placed_order_and_drags_reorder_them(cx: &mut gpui::TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let window = test_shell(cx, dir.path());
        window
            .update(cx, |shell, window, cx| {
                prepare(shell, cx);
                shell.assign_sidebar_section("regular", Some("a"), cx);
                shell.assign_sidebar_section("other", Some("a"), cx);
                let members = |shell: &Shell, cx: &mut Context<Shell>| {
                    shell.active_sidebar_sections(cx)[0].session_ids.clone()
                };
                // Appends keep arrival order, and the sidebar lists the section
                // in that order rather than by activity.
                assert_eq!(members(shell, cx), ["regular", "other"]);
                assert_eq!(shell.sidebar_visible_order(cx), ["pin", "regular", "other"]);
                let drop = |shell: &mut Shell,
                            index: usize,
                            window: &mut Window,
                            cx: &mut Context<Shell>| {
                    let payload = SidebarSessionDrag {
                        chat_id: "other".into(),
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
                    shell.sidebar_session_transfer.as_mut().unwrap().preview =
                        Some(SidebarSessionGap {
                            group: "regular:section:a".into(),
                            index,
                            pinned: false,
                            top: 0.0,
                        });
                    shell.finish_sidebar_session_transfer(
                        &payload,
                        SidebarSessionDrop::Section("a".into()),
                        cx,
                    );
                };
                // Dropping a member on its own slot changes nothing.
                drop(shell, 1, window, cx);
                assert_eq!(members(shell, cx), ["regular", "other"]);
                drop(shell, 0, window, cx);
                assert_eq!(members(shell, cx), ["other", "regular"]);
                assert_eq!(shell.sidebar_visible_order(cx), ["pin", "other", "regular"]);
            })
            .unwrap();
    }

    #[gpui::test]
    fn sections_reorder_by_dragging_their_header(cx: &mut gpui::TestAppContext) {
        use gpui::MouseButton;
        let dir = tempfile::tempdir().unwrap();
        let window = test_shell(cx, dir.path());
        window
            .update(cx, |shell, _, cx| {
                prepare(shell, cx);
                shell
                    .settings
                    .sidebar_sections_by_profile
                    .get_mut("local")
                    .unwrap()
                    .push(SidebarSection {
                        id: "c".into(),
                        name: "Someday".into(),
                        session_ids: vec![],
                        collapsed: false,
                    });
                shell.debug_gate = Some(super::GatePhase::Ready);
                crate::motion::set_reduced_motion(cx, true);
            })
            .unwrap();
        let mut cx = gpui::VisualTestContext::from_window(window.into(), cx);
        cx.simulate_resize(gpui::size(px(1200.0), px(900.0)));
        cx.run_until_parked();
        let order = |cx: &mut gpui::VisualTestContext| {
            window
                .update(cx, |shell, _, cx| {
                    shell
                        .active_sidebar_sections(cx)
                        .into_iter()
                        .map(|s| s.id)
                        .collect::<Vec<_>>()
                })
                .unwrap()
        };
        let none = gpui::Modifiers::default();
        let from = cx.debug_bounds("section-header-a").unwrap().center();
        let last = cx.debug_bounds("section-header-c").unwrap();
        let to = gpui::point(from.x, last.bottom() + px(30.0));
        let nudge = gpui::point(px(0.0), px(6.0));
        cx.simulate_mouse_down(from, MouseButton::Left, none);
        // The first move lifts the section where it sits; it then follows.
        cx.simulate_mouse_move(from + nudge, Some(MouseButton::Left), none);
        cx.update(|_, cx| assert!(cx.has_active_drag(), "section drag never started"));
        cx.simulate_mouse_move(to, Some(MouseButton::Left), none);
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("sidebar-lifted-section").is_some(),
            "the section lifts while it drags"
        );
        cx.simulate_mouse_up(to, MouseButton::Left, none);
        cx.run_until_parked();
        assert_eq!(order(&mut cx), ["b", "c", "a"]);
        assert!(cx.debug_bounds("sidebar-lifted-section").is_none());

        // Released outside the sidebar, the section goes home.
        let from = cx.debug_bounds("section-header-b").unwrap().center();
        let outside = gpui::point(px(900.0), from.y + px(200.0));
        cx.simulate_mouse_down(from, MouseButton::Left, none);
        cx.simulate_mouse_move(from + nudge, Some(MouseButton::Left), none);
        cx.simulate_mouse_move(outside, Some(MouseButton::Left), none);
        cx.simulate_mouse_up(outside, MouseButton::Left, none);
        cx.run_until_parked();
        assert_eq!(order(&mut cx), ["b", "c", "a"]);
        window
            .update(&mut cx, |shell, _, _| {
                assert!(shell.sidebar_section_drag.is_none());
            })
            .unwrap();
    }

    #[test]
    fn section_drop_index_follows_the_leading_edge() {
        let heights = [50.0, 300.0, 50.0];
        let gap = SIDEBAR_LIST_GAP;
        // At rest every section keeps its slot.
        assert_eq!(section_drop_index(0.0, &heights, 0), 0);
        assert_eq!(section_drop_index(50.0 + gap, &heights, 1), 1);
        // Dragged down, the bottom edge must pass the next middle.
        assert_eq!(section_drop_index(99.0, &heights, 0), 0);
        assert_eq!(section_drop_index(140.0 + gap, &heights, 0), 0);
        assert_eq!(section_drop_index(160.0 + gap, &heights, 0), 1);
        assert_eq!(section_drop_index(400.0, &heights, 0), 2);
        // Dragged up, the top edge must pass the previous middle.
        let last = 350.0 + 2.0 * gap;
        assert_eq!(section_drop_index(last - 100.0, &heights, 2), 2);
        assert_eq!(section_drop_index(150.0, &heights, 2), 1);
        assert_eq!(section_drop_index(-10.0, &heights, 2), 0);
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
                shell.finish_sidebar_pin_write(1, Err("rejected".into()), cx);
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
                                after: None,
                                before: None,
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
                shell.finish_sidebar_pin_write(id, Err("disk full".into()), cx);
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
