use super::*;
use zeron_proto::{SidebarPinChange, pin_order_key_between, valid_pin_order_key};

impl RegistryDoc {
    pub fn sidebar_pins_initialized(&self) -> bool {
        self.overlay_row(KIND_PREFERENCES, SIDEBAR_PINS_STATE_ID)
            .is_some()
    }

    pub(super) fn ordered_sidebar_pins(&self) -> Vec<(String, String)> {
        let mut pins: Vec<_> = self
            .overlay_rows(KIND_SIDEBAR_PINS)
            .into_iter()
            .filter_map(|row| {
                if !self.sidebar_location_is_pinned(&row.id, &row) {
                    return None;
                }
                let key = row.fields.get("orderKey")?.as_str()?;
                valid_pin_order_key(key).then(|| (row.id, key.to_owned()))
            })
            .collect();
        pins.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        pins
    }

    /// Readiness survives restarts even when the list is empty. No data import.
    pub(super) fn initialize_sidebar_pins(&mut self) {
        if !self.sidebar_pins_initialized() {
            self.write(
                KIND_PREFERENCES,
                SIDEBAR_PINS_STATE_ID,
                OpKind::Upsert,
                fields([("initialized", json!(true))]),
            );
        }
    }

    pub fn change_sidebar_pin(&mut self, change: &SidebarPinChange) -> Result<(), DocError> {
        if let SidebarPinChange::Section { change } = change {
            return self.change_sidebar_section(change);
        }
        let id = change.session_id();
        if id.is_empty()
            || id.len() > 256
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:@/-".contains(&b))
        {
            return Err(DocError::Schema("Invalid pinned session ID".into()));
        }
        let current = self
            .sidebar_preferences()
            .unwrap_or_default()
            .pinned_session_ids;
        if matches!(change, SidebarPinChange::Pin { .. })
            && !current.iter().any(|v| v == id)
            && current.len() >= MAX_SIDEBAR_PINS
        {
            return Err(DocError::Schema("You can pin up to 200 sessions".into()));
        }
        if matches!(change, SidebarPinChange::Pin { .. })
            && self.overlay_row(KIND_CHATS, id).is_none()
        {
            return Err(DocError::Schema("Session no longer exists".into()));
        }
        self.initialize_sidebar_pins();
        self.observe_row("sidebarLocations", id);
        // Observe this pin's field clocks before a causally subsequent edit,
        // including clocks from a device whose wall clock runs ahead of ours.
        if let Some(row) = self.overlay_row(KIND_SIDEBAR_PINS, id) {
            if let Some(clock) = row.max_clock() {
                let mut parts = clock.splitn(3, '-');
                if let (Some(ms), Some(counter)) = (
                    parts.next().and_then(|v| v.parse::<i64>().ok()),
                    parts.next().and_then(|v| v.parse::<u32>().ok()),
                ) {
                    if (ms, counter) > (self.clock.last_ms, self.clock.counter) {
                        self.clock.last_ms = ms;
                        self.clock.counter = counter;
                    }
                }
            }
        }
        if matches!(change, SidebarPinChange::Unpin { .. }) {
            if current.iter().any(|v| v == id) {
                self.write_sidebar_location(id, "");
            }
            self.write(
                KIND_SIDEBAR_PINS,
                id,
                OpKind::Upsert,
                fields([("pinned", json!(false))]),
            );
            return Ok(());
        }
        if matches!(change, SidebarPinChange::Move { .. }) && !current.iter().any(|v| v == id) {
            return Ok(());
        }
        let pins = self.ordered_sidebar_pins();
        let mut next = current.clone();
        change.project(&mut next);
        let index = next.iter().position(|v| v == id).unwrap();
        let key_for = |id: &String| {
            pins.iter()
                .find(|(pin, _)| pin == id)
                .map(|(_, key)| key.as_str())
        };
        let lower = index
            .checked_sub(1)
            .and_then(|i| next.get(i))
            .and_then(key_for);
        let upper = next.get(index + 1).and_then(key_for);
        let hlc = self.next_hlc();
        let key =
            pin_order_key_between(lower, upper, &hlc).map_err(|e| DocError::Schema(e.into()))?;
        let mut set = fields([("orderKey", json!(key))]);
        if matches!(change, SidebarPinChange::Pin { .. }) {
            set.insert("pinned".into(), json!(true));
            self.write_sidebar_location(id, "pinned");
        }
        self.enqueue_ops(vec![RowOp {
            kind: KIND_SIDEBAR_PINS.into(),
            id: id.into(),
            op: OpKind::Upsert,
            set: Some(set),
            hlc,
            clocks: None,
        }]);
        Ok(())
    }
}
