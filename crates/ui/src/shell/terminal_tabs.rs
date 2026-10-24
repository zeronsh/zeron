//! Move a terminal's stable session between views only when the drop commits.

use super::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TerminalTabInsertion {
    pub chat: String,
    pub key: u64,
    pub index: usize,
}

/// Insertions have count + 1 slots; reorders use a different count-slot rule.
fn insertion_index(content_x: f32, slot_width: f32, count: usize) -> usize {
    ((content_x / slot_width + 0.5).floor().max(0.0) as usize).min(count)
}

impl Shell {
    fn accepts_terminal_transfer(&self, payload: &TerminalTabDrag, cx: &App) -> bool {
        matches!(self.route, Route::Chat)
            && self.right_pane_open(cx)
            && self.terminal_open(cx)
            && self.panel_key(cx) == payload.chat
            && self.terminal.as_ref().is_some_and(|origin| {
                origin.downgrade() == payload.origin && origin.read(cx).accepts_drag(payload, cx)
            })
    }

    pub(super) fn update_terminal_tab_insertion(
        &mut self,
        event: &gpui::DragMoveEvent<TerminalTabDrag>,
        scroll: &gpui::ScrollHandle,
        slot_width: f32,
        count: usize,
        cx: &mut Context<Self>,
    ) {
        let payload = event.drag(cx);
        let next = if event.bounds.contains(&event.event.position)
            && self.accepts_terminal_transfer(payload, cx)
        {
            let x = f32::from(event.event.position.x - event.bounds.left() - scroll.offset().x);
            Some(TerminalTabInsertion {
                chat: payload.chat.clone(),
                key: payload.key,
                index: insertion_index(x, slot_width, count),
            })
        } else {
            None
        };
        if self.terminal_tab_insertion != next {
            self.terminal_tab_insertion = next;
            cx.notify();
        }
    }

    pub(super) fn transfer_terminal_to_right(
        &mut self,
        payload: &TerminalTabDrag,
        fallback_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let insertion = self.terminal_tab_insertion.take();
        if !self.accepts_terminal_transfer(payload, cx) {
            cx.notify();
            return;
        }
        let rows = self.right_surface_rows(cx);
        let index = insertion
            .filter(|drop| drop.chat == payload.chat && drop.key == payload.key)
            .map(|drop| drop.index)
            .unwrap_or(fallback_index)
            .min(rows.len());
        let origin = self.terminal.clone().expect("validated terminal origin");
        let Some(session) = origin.update(cx, |panel, cx| panel.take_session(payload, cx)) else {
            return;
        };
        let key = session.read(cx).key;
        let destination = self.right_terminal_panel(cx);
        destination.update(cx, |panel, cx| {
            panel.insert_session(payload.chat.clone(), session, cx);
        });
        let surface = RightSurface::Terminal(key);
        let tabs = self.right_tabs.entry(payload.chat.clone()).or_default();
        // Stored lists may include stale surfaces; the drop position indexes only
        // visible rows. Resolve its neighboring surface before inserting.
        let stored_index = rows
            .get(index)
            .and_then(|(next, _, _, _)| tabs.iter().position(|tab| tab == next))
            .unwrap_or(tabs.len());
        tabs.insert(stored_index, surface);
        if !origin.read(cx).is_open() {
            self.toggle_terminal(window, cx);
        }
        self.right_tab_drag = None;
        self.set_right_active(surface, cx);
        window.focus(&destination.read(cx).focus_handle(), cx);
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::insertion_index;

    #[test]
    fn insertions_include_both_edges_and_use_the_tab_midpoint() {
        assert_eq!(insertion_index(100., 116., 0), 0);
        assert_eq!(insertion_index(-20., 116., 3), 0);
        assert_eq!(insertion_index(57., 116., 3), 0);
        assert_eq!(insertion_index(59., 116., 3), 1);
        assert_eq!(insertion_index(232., 116., 3), 2);
        assert_eq!(insertion_index(900., 116., 3), 3);
    }
}
