//! Built-in Drafts section; its entries are not sessions and cannot be pinned
//! or assigned to user sections before their first send.
use super::*;
use zeron_proto::{DraftChange, PromptDraft};

#[derive(Clone)]
pub(super) struct DraftDrag {
    id: String,
    profile: String,
}

/// A draft lifted out of the list. Like a dragged session row it keeps its
/// column and follows the pointer as frosted glass while its siblings slide
/// between slots; released anywhere but the Drafts section it slides home.
pub(super) struct DraftLift {
    id: String,
    ids: Vec<String>,
    from: usize,
    over: usize,
    prev_over: usize,
    epoch: u64,
    origin: std::rc::Rc<std::cell::Cell<gpui::Bounds<Pixels>>>,
    cursor_offset: Point<Pixels>,
    pointer: Point<Pixels>,
    returning: bool,
}

pub(super) struct PendingDraftChanges {
    engine: crate::state::EngineHandle,
    queue: std::collections::VecDeque<DraftChange>,
}
impl Shell {
    fn prompt_draft_row_height(&self) -> f32 {
        sidebar_row_height(self.settings.sidebar_compact, true, false, false)
    }

    fn begin_draft_lift(
        &mut self,
        payload: &DraftDrag,
        cursor_offset: Point<Pixels>,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        let ids: Vec<String> = self
            .visible_prompt_drafts(cx)
            .into_iter()
            .map(|row| row.id)
            .collect();
        let Some(from) = ids.iter().position(|id| id == &payload.id) else {
            return;
        };
        self.pinned_session_drag_generation = self.pinned_session_drag_generation.wrapping_add(1);
        let pointer = window.mouse_position();
        let width = f32::from(self.sidebar_scroll.bounds().size.width) - 2.0 * Theme::SPACE_SM;
        self.draft_lift = Some(DraftLift {
            id: payload.id.clone(),
            ids,
            from,
            over: from,
            prev_over: from,
            epoch: self.pinned_session_drag_generation << 32,
            origin: std::rc::Rc::new(std::cell::Cell::new(gpui::Bounds::new(
                pointer - cursor_offset,
                gpui::size(px(width.max(0.0)), px(self.prompt_draft_row_height())),
            ))),
            cursor_offset,
            pointer,
            returning: false,
        });
        self.scroll_draft_drag(f32::from(pointer.y), cx);
        cx.notify();
    }

    fn move_draft_lift(&mut self, event: &gpui::DragMoveEvent<DraftDrag>, cx: &mut Context<Self>) {
        let id = event.drag(cx).id.clone();
        let row_height = self.prompt_draft_row_height();
        let Some(lift) = self
            .draft_lift
            .as_mut()
            .filter(|lift| lift.id == id && !lift.returning)
        else {
            return;
        };
        lift.pointer = event.event.position;
        let rel_y = f32::from(event.event.position.y - event.bounds.top());
        let heights = vec![row_height; lift.ids.len()];
        if let Some(over) = spaces::row_drop_index(rel_y, &heights, true)
            && over != lift.over
        {
            lift.prev_over = lift.over;
            lift.over = over;
            lift.epoch = lift.epoch.wrapping_add(1);
        }
        self.scroll_draft_drag(f32::from(event.event.position.y), cx);
        cx.notify();
    }

    /// Ends a lift without a move: the row and its siblings ease back.
    fn return_draft_lift(&mut self, cx: &mut Context<Self>) {
        let Some(lift) = self.draft_lift.as_mut().filter(|lift| !lift.returning) else {
            return;
        };
        if self.reduced_motion {
            self.draft_lift = None;
            cx.notify();
            return;
        }
        lift.returning = true;
        lift.prev_over = lift.over;
        lift.over = lift.from;
        lift.epoch = lift.epoch.wrapping_add(1);
        let epoch = lift.epoch;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(TAB_SLIDE.total()).await;
            this.update(cx, |shell, cx| {
                if shell
                    .draft_lift
                    .as_ref()
                    .is_some_and(|lift| lift.epoch == epoch)
                {
                    shell.draft_lift = None;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    fn finish_draft_drop(&mut self, payload: &DraftDrag, cx: &mut Context<Self>) {
        let ids: Vec<String> = self
            .visible_prompt_drafts(cx)
            .into_iter()
            .map(|row| row.id)
            .collect();
        let valid = self.active_sidebar_pin_profile_key(cx).as_ref() == Some(&payload.profile)
            && self
                .draft_lift
                .as_ref()
                .is_some_and(|lift| lift.id == payload.id && !lift.returning && lift.ids == ids);
        let Some((from, over)) = self
            .draft_lift
            .as_ref()
            .filter(|lift| valid && lift.over != lift.from)
            .map(|lift| (lift.from, lift.over))
        else {
            self.return_draft_lift(cx);
            return;
        };
        let mut rest = ids;
        rest.remove(from);
        self.draft_lift = None;
        self.change_prompt_draft(
            DraftChange::Move {
                id: payload.id.clone(),
                before: rest.get(over).cloned(),
                after: over.checked_sub(1).map(|i| rest[i].clone()),
            },
            cx,
        );
    }

    fn render_lifted_draft(&self, row: AnyElement, theme: &Theme) -> Option<AnyElement> {
        let lift = self.draft_lift.as_ref()?;
        let origin = lift.origin.get();
        let height = self.prompt_draft_row_height();
        let lifted = lift.pointer - lift.cursor_offset;
        let frame = div()
            .relative()
            .w(origin.size.width)
            .h(px(height))
            .rounded(px(8.0));
        // Window coordinates, drawn above the sidebar's clip: the same lifted
        // glass as a dragged session row, settling back unlifted.
        let (position, frame) = if lift.returning {
            let dy = f32::from(lifted.y - origin.origin.y);
            (
                origin.origin,
                frame
                    .child(row)
                    .with_animation(
                        ("sidebar-draft-return", lift.epoch),
                        TAB_SLIDE.animation(),
                        move |el, t| el.top(px(motion::lerp(dy, 0.0, t))),
                    )
                    .into_any_element(),
            )
        } else {
            (
                gpui::point(origin.origin.x, lifted.y),
                frame
                    .child(crate::frost::frosted(
                        8.0,
                        crate::frost::MENU_BLUR,
                        div()
                            .size_full()
                            .rounded(px(8.0))
                            .bg(crate::popover::surface_bg(theme))
                            .shadow_md()
                            .child(
                                div()
                                    .size_full()
                                    .rounded(px(8.0))
                                    .bg(crate::theme::wash(0.06))
                                    .child(row),
                            ),
                    ))
                    .into_any_element(),
            )
        };
        Some(
            gpui::deferred(gpui::anchored().position(position).child(frame))
                .with_priority(1)
                .into_any_element(),
        )
    }

    fn scroll_draft_drag(&mut self, y: f32, cx: &mut Context<Self>) {
        self.draft_drag_y = y;
        if self.draft_drag_scroll.is_some() {
            return;
        }
        self.draft_drag_scroll = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(16))
                    .await;
                if !this
                    .update(cx, |shell, cx| {
                        if !cx.has_active_drag() {
                            shell.draft_drag_scroll = None;
                            shell.return_draft_lift(cx);
                            cx.notify();
                            return false;
                        }
                        let bounds = shell.sidebar_scroll.bounds();
                        let delta = spaces::pinned_drag_scroll_delta(
                            shell.draft_drag_y,
                            f32::from(bounds.top()),
                            f32::from(bounds.bottom()),
                        );
                        let offset = shell.sidebar_scroll.offset();
                        let next = (-f32::from(offset.y) + delta)
                            .clamp(0.0, f32::from(shell.sidebar_scroll.max_offset().y));
                        shell
                            .sidebar_scroll
                            .set_offset(gpui::point(offset.x, px(-next)));
                        cx.notify();
                        true
                    })
                    .unwrap_or(false)
                {
                    break;
                }
            }
        }));
    }

    pub(super) fn visible_prompt_drafts(&self, cx: &App) -> Vec<PromptDraft> {
        let state = self.state.read(cx);
        let mut rows = state.prompt_drafts.drafts.clone();
        if let Some(pending) = &self.draft_changes {
            if state
                .engine()
                .is_some_and(|e| e.same_connection(&pending.engine))
            {
                for change in &pending.queue {
                    match change {
                        DraftChange::Discard { id } | DraftChange::Consume { id, .. } => {
                            rows.retain(|r| &r.id != id)
                        }
                        DraftChange::Move { id, before, after } => {
                            if let Some(i) = rows.iter().position(|r| &r.id == id) {
                                let row = rows.remove(i);
                                let at = before
                                    .as_ref()
                                    .and_then(|b| rows.iter().position(|r| &r.id == b))
                                    .or_else(|| {
                                        after.as_ref().and_then(|a| {
                                            rows.iter().position(|r| &r.id == a).map(|i| i + 1)
                                        })
                                    })
                                    .unwrap_or(rows.len());
                                rows.insert(at, row);
                            }
                        }
                    }
                }
            }
        }
        rows
    }
    pub(super) fn change_prompt_draft(&mut self, change: DraftChange, cx: &mut Context<Self>) {
        let Some(engine) = self.state.read(cx).engine().cloned() else {
            return;
        };
        if self
            .draft_changes
            .as_ref()
            .is_some_and(|p| !p.engine.same_connection(&engine))
        {
            self.draft_changes = None;
        }
        if let Some(pending) = &mut self.draft_changes {
            pending.queue.push_back(change);
            cx.notify();
            return;
        }
        self.draft_changes = Some(PendingDraftChanges {
            engine: engine.clone(),
            queue: std::collections::VecDeque::from([change.clone()]),
        });
        cx.spawn(async move |this, cx| {
            let mut next = change;
            loop {
                let result = engine
                    .client()
                    .call(methods::CHANGE_DRAFT, serde_json::to_value(&next).unwrap())
                    .await;
                let following = this
                    .update(cx, |shell, cx| {
                        if !shell
                            .state
                            .read(cx)
                            .engine()
                            .is_some_and(|e| e.same_connection(&engine))
                        {
                            return None;
                        }
                        if let Err(error) = &result {
                            shell.sidebar_notice =
                                Some(format!("Couldn't save draft change: {error}").into());
                            shell.draft_changes = None;
                            cx.notify();
                            return None;
                        }
                        if let Ok(value) = result {
                            if let Ok(value) =
                                serde_json::from_value::<zeron_proto::DraftsState>(value)
                            {
                                shell.state.update(cx, |state, cx| {
                                    if value.revision >= state.prompt_drafts.revision {
                                        state.prompt_drafts = value;
                                        cx.notify();
                                    }
                                });
                            }
                        }
                        let pending = shell.draft_changes.as_mut()?;
                        pending.queue.pop_front();
                        let next = pending.queue.front().cloned();
                        if next.is_none() {
                            shell.draft_changes = None;
                        }
                        cx.notify();
                        next
                    })
                    .ok()
                    .flatten();
                let Some(following) = following else {
                    break;
                };
                next = following;
            }
        })
        .detach();
        cx.notify();
    }
    pub(super) fn render_drafts_section(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let rows = self.visible_prompt_drafts(cx);
        if rows.is_empty() {
            return None;
        }
        if self.draft_lift.as_ref().is_some_and(|lift| {
            lift.ids.len() != rows.len()
                || lift.ids.iter().zip(&rows).any(|(id, row)| id != &row.id)
        }) {
            self.draft_lift = None;
        }
        let open = self.drafts_open;
        let count = (!open).then_some(rows.len());
        let row_height = self.prompt_draft_row_height();
        let height = rows.len() as f32 * row_height
            + rows.len().saturating_sub(1) as f32 * SIDEBAR_LIST_GAP
            + spaces::SIDEBAR_DISCLOSURE_BODY_INSET;
        let chevron = self.sidebar_disclosure_chevron("drafts", open, theme);
        let header =
            spaces::sidebar_disclosure_header(theme, None, "Drafts".into(), count, None, chevron)
                .id("drafts-toggle")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.begin_sidebar_disclosure_motion(
                        "drafts",
                        if open { height } else { 0.0 },
                        if open { 0.0 } else { height },
                    );
                    this.drafts_open = !open;
                    cx.notify();
                }));
        let slide = self.draft_lift.as_ref().map(|lift| {
            (
                lift.id.clone(),
                lift.from,
                lift.over,
                lift.prev_over,
                lift.epoch,
            )
        });
        let step = row_height + SIDEBAR_LIST_GAP;
        let mut lifted = None;
        let mut items = Vec::with_capacity(rows.len());
        for (ix, row) in rows.into_iter().enumerate() {
            let is_lifted = slide.as_ref().is_some_and(|(id, ..)| id == &row.id);
            let element = self.render_prompt_draft_row(row, is_lifted, theme, cx);
            let Some((_, from, over, prev_over, epoch)) = &slide else {
                items.push(element);
                continue;
            };
            if is_lifted {
                // The row itself rides the movement layer; its slot stays put
                // and reports where the row settles back to.
                lifted = Some(element);
                let origin = self.draft_lift.as_ref().unwrap().origin.clone();
                items.push(
                    div()
                        .child(div().h(px(row_height)))
                        .on_children_prepainted(move |bounds, _, _| {
                            if let Some(bounds) = bounds.first() {
                                origin.set(*bounds);
                            }
                        })
                        .into_any_element(),
                );
                continue;
            }
            let start = crate::terminal::panel::slide_offset(ix, *from, *prev_over) * step;
            let target = crate::terminal::panel::slide_offset(ix, *from, *over) * step;
            items.push(if self.reduced_motion {
                div()
                    .relative()
                    .top(px(target))
                    .child(element)
                    .into_any_element()
            } else {
                div()
                    .child(element)
                    .with_animation(
                        ("sidebar-draft-slide", (ix as u64) | (epoch << 32)),
                        TAB_SLIDE.animation(),
                        move |el, t| el.relative().top(px(motion::lerp(start, target, t))),
                    )
                    .into_any_element()
            });
        }
        let lifted = lifted.and_then(|row| self.render_lifted_draft(row, theme));
        let body = div()
            .pt(px(spaces::SIDEBAR_DISCLOSURE_BODY_INSET))
            .child(
                div()
                    .id("sidebar-drafts-list")
                    .flex()
                    .flex_col()
                    .gap(px(SIDEBAR_LIST_GAP))
                    .on_drag_move::<DraftDrag>(cx.listener(
                        |this, event: &gpui::DragMoveEvent<DraftDrag>, _, cx| {
                            this.move_draft_lift(event, cx)
                        },
                    ))
                    .children(items),
            )
            .into_any_element();
        Some(
            div()
                .id("sidebar-drafts-section")
                .flex()
                .flex_col()
                .pb(px(8.0))
                .on_drop::<DraftDrag>(
                    cx.listener(|this, payload, _, cx| this.finish_draft_drop(payload, cx)),
                )
                .child(header)
                .child(self.render_sidebar_disclosure_body("drafts", open, height, body))
                .children(lifted)
                .into_any_element(),
        )
    }
    fn render_prompt_draft_row(
        &mut self,
        row: PromptDraft,
        lifted: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let compact = self.settings.sidebar_compact;
        let state = self.state.read(cx);
        let space = state
            .spaces
            .iter()
            .find(|space| {
                Some(&space.id) == row.target.space_id.as_ref()
                    && space.device_id == row.target.device_id
            })
            .cloned();
        let label = space
            .as_ref()
            .map(|space| space.display_name().to_string())
            .or_else(|| row.target.project_name.clone())
            .unwrap_or_else(|| "No project".into());
        let host = state
            .devices
            .iter()
            .find(|d| d.id == row.target.device_id)
            .map(|d| d.name.clone())
            .unwrap_or_else(|| row.target.device_id.clone());
        let target = if host.is_empty() {
            label
        } else {
            format!("{label} @ {host}")
        };
        let selected = self.composer.read(cx).active_prompt_draft() == Some(row.id.as_str());
        let subline = theme.text_muted.opacity(0.5);
        // Drafts read as sessions-in-waiting: same geometry and type scale as
        // session rows, with a pen standing in for the status glyph.
        let project_icon = self.settings.sidebar_show_project_icon.then(|| {
            self.render_space_project_icon(
                &format!("draft-{}", row.id),
                space.as_ref(),
                Some(&row.target.device_id),
                SIDEBAR_ACTIVE_HARNESS_ICON_SIZE,
                false,
                cx,
            )
        });
        let harness = self
            .settings
            .sidebar_show_harness
            .then(|| row.target.config.as_ref().map(|config| config.harness))
            .flatten();
        let id = row.id.clone();
        let discard = row.id.clone();
        let activate = row.clone();
        let drag = self
            .active_sidebar_pin_profile_key(cx)
            .filter(|_| !lifted)
            .map(|profile| DraftDrag {
                id: id.clone(),
                profile,
            });

        let (status_label, status_color) = if row.conflict {
            ("Recovered", theme.warning)
        } else {
            ("Draft", theme.text_muted)
        };
        // The corner shows the draft badge at rest and swaps to Discard while
        // the row is hovered, mirroring the session rows' Archive swap.
        let rest = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .group_hover("sidebar-session-row", |s| s.opacity(0.0))
            .when(compact, |el| el.justify_center())
            .child(
                icon(icons::PEN)
                    .size(px(if compact { 11.0 } else { 10.0 }))
                    .flex_none()
                    .text_color(status_color),
            )
            .when(!compact, |el| {
                el.child(
                    div()
                        .text_size(crate::typography::ui_rems(10.0))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(status_color)
                        .child(status_label),
                )
            });
        let discard_button = div()
            .id(SharedString::from(format!("discard-draft-{id}")))
            .absolute()
            .top_0()
            .bottom_0()
            .right_0()
            .flex()
            .items_center()
            .gap(px(4.0))
            .opacity(0.0)
            .group_hover("sidebar-session-row", |s| s.opacity(1.0))
            .when(!compact, |el| {
                el.h(px(18.0))
                    .top(px(-2.0))
                    .px(px(4.0))
                    .mr(px(-4.0))
                    .rounded(px(5.0))
                    .bg(crate::theme::wash(0.10))
                    .hover(|s| s.bg(crate::theme::wash(0.18)))
            })
            .when(compact, |el| el.left_0().justify_center())
            .cursor_pointer()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.change_prompt_draft(
                    DraftChange::Discard {
                        id: discard.clone(),
                    },
                    cx,
                );
                if this.composer.read(cx).active_prompt_draft() == Some(discard.as_str()) {
                    this.composer
                        .update(cx, |composer, cx| composer.abandon_prompt_draft(cx));
                }
            }))
            .tooltip(crate::settings::widgets::text_tooltip_above(
                "Discard draft",
            ))
            .child(
                icon(icons::CLOSE)
                    .size(px(if compact {
                        SIDEBAR_ACTIVE_HARNESS_ICON_SIZE
                    } else {
                        11.0
                    }))
                    .flex_none()
                    .text_color(theme.text_muted),
            )
            .when(!compact, |el| {
                el.child(
                    div()
                        .text_size(crate::typography::ui_rems(10.0))
                        .text_color(theme.text_muted)
                        .child("Discard"),
                )
            });
        let corner = div()
            .relative()
            .flex_none()
            .h(px(14.0))
            .flex()
            .items_center()
            .when(compact, |el| el.w(px(18.0)).justify_center())
            .child(rest)
            .child(discard_button);
        let mut corner = Some(corner);

        let preview = sidebar_faded_label(
            format!("draft-title-{id}").into(),
            true,
            div()
                .text_size(crate::typography::ui_rems(13.0))
                .line_height(px(17.0))
                .child(row.preview.clone()),
        );
        let fade_key = format!("draft-row-{id}-hover");
        let rest_bg = if selected {
            crate::theme::glass_selected_bg()
        } else {
            crate::theme::wash(0.0)
        };
        let hover_bg = if selected {
            crate::theme::glass_selected_bg()
        } else {
            theme.glass_hover()
        };
        let rest_text = if selected {
            theme.text
        } else {
            theme.text.opacity(0.8)
        };
        div()
            .id(SharedString::from(format!("draft-row-{id}")))
            .group("sidebar-session-row")
            .h(px(self.prompt_draft_row_height()))
            .w_full()
            .flex()
            .flex_col()
            .when(compact, |el| el.justify_center())
            .gap(px(2.0))
            .rounded(px(8.0))
            .px(px(Theme::SPACE_SM))
            .py(px(6.0))
            .text_color(motion::hover_blend(&fade_key, rest_text, theme.text))
            .bg(motion::hover_blend(&fade_key, rest_bg, hover_bg))
            .on_hover(motion::hover_listener(fade_key.clone()))
            .cursor_pointer()
            .when_some(drag, |el, drag| {
                let shell = cx.entity();
                el.on_drag(drag, move |payload, offset, window, cx| {
                    shell.update(cx, |shell, cx| {
                        shell.begin_draft_lift(payload, offset, window, cx)
                    });
                    cx.stop_propagation();
                    cx.new(|_| DragGhost)
                })
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                this.route = Route::Chat;
                this.composer.update(cx, |composer, cx| {
                    composer.open_prompt_draft(activate.clone(), cx)
                });
                cx.notify();
            }))
            // Line 1: "project @ device", draft badge / Discard right.
            .when(!compact, |el| {
                el.child(
                    div()
                        .w_full()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(Theme::SPACE_SM))
                        .child(sidebar_faded_label(
                            format!("draft-target-{id}").into(),
                            true,
                            div()
                                .text_size(crate::typography::ui_rems(11.0))
                                .line_height(px(14.0))
                                .text_color(subline)
                                .child(target),
                        ))
                        .children(corner.take()),
                )
            })
            // Line 2: harness and project identity lead the prompt preview.
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(if compact {
                        4.0
                    } else {
                        SIDEBAR_ACTIVE_HARNESS_TITLE_GAP
                    }))
                    .when_some(
                        harness.map(crate::pickers::harness_brand_icon),
                        |el, (path, tint)| {
                            el.child(
                                icon(path)
                                    .size(px(SIDEBAR_ACTIVE_HARNESS_ICON_SIZE))
                                    .flex_none()
                                    .text_color(tint.unwrap_or(subline).opacity(0.8)),
                            )
                        },
                    )
                    .children(project_icon)
                    .child(preview)
                    .children(corner),
            )
            .into_any_element()
    }
}
