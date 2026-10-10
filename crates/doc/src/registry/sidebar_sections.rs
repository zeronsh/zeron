//! Per-section metadata and one placement register per session. Concurrent moves
//! can never place a session in two sections, or in both a section and Pinned.
use super::*;
use zeron_proto::{
    SidebarSection, SidebarSectionChange, pin_order_key_between, section_insert_index,
    valid_pin_order_key,
};

const SECTIONS: &str = "sidebarSections";
const LOCATIONS: &str = "sidebarLocations";

impl RegistryDoc {
    pub(super) fn observe_sidebar_row(&mut self, kind: &str, id: &str) {
        if let Some(clock) = self
            .overlay_row(kind, id)
            .and_then(|r| r.max_clock().map(str::to_owned))
        {
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

    fn sidebar_location(&self, id: &str, pin: Option<&RegistryRow>) -> String {
        let location = self.overlay_row(LOCATIONS, id);
        // Older apps write only pinned. Honor a later legacy toggle while
        // keeping all new section/pin moves in one shared placement register.
        let location_clock = location.as_ref().and_then(|r| r.clocks.get("location"));
        let pin_clock = pin.and_then(|r| r.clocks.get("pinned"));
        if location_clock.is_some() && location_clock >= pin_clock {
            return location
                .and_then(|r| {
                    r.fields
                        .get("location")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or_default();
        }
        if pin
            .and_then(|r| r.fields.get("pinned"))
            .and_then(Value::as_bool)
            == Some(true)
        {
            "pinned".into()
        } else {
            String::new()
        }
    }

    pub(super) fn sidebar_location_is_pinned(&self, id: &str, pin: &RegistryRow) -> bool {
        self.sidebar_location(id, Some(pin)) == "pinned"
    }

    pub(super) fn write_sidebar_location(&mut self, id: &str, location: &str) {
        self.observe_sidebar_row(LOCATIONS, id);
        self.observe_sidebar_row(KIND_SIDEBAR_PINS, id);
        self.write(
            LOCATIONS,
            id,
            OpKind::Upsert,
            fields([("location", json!(location))]),
        );
    }

    pub(super) fn sidebar_sections(&self) -> Vec<SidebarSection> {
        let mut rows = self.overlay_rows(SECTIONS);
        rows.retain(|row| row.fields.get("deleted").and_then(Value::as_bool) != Some(true));
        rows.sort_by(|a, b| {
            a.fields
                .get("createdAt")
                .and_then(Value::as_str)
                .cmp(&b.fields.get("createdAt").and_then(Value::as_str))
                .then(a.id.cmp(&b.id))
        });
        rows.into_iter()
            .filter_map(|row| {
                let session_ids = self
                    .sidebar_section_members(&row.id)
                    .into_iter()
                    .map(|(id, _)| id)
                    .collect();
                Some(SidebarSection {
                    name: row.fields.get("name")?.as_str()?.into(),
                    collapsed: row
                        .fields
                        .get("collapsed")
                        .and_then(Value::as_bool)
                        .unwrap_or(false),
                    id: row.id,
                    session_ids,
                })
            })
            .collect()
    }

    /// A section's members in their placed order, each with its order key.
    /// Members placed by an engine that predates ordering carry no key and
    /// follow the ordered ones by id, where an append would have put them.
    fn sidebar_section_members(&self, section: &str) -> Vec<(String, Option<String>)> {
        let mut members: Vec<_> = self
            .overlay_rows(LOCATIONS)
            .into_iter()
            .filter(|r| {
                self.sidebar_location(&r.id, self.overlay_row(KIND_SIDEBAR_PINS, &r.id).as_ref())
                    == section
            })
            .map(|r| {
                // A key only orders the placement it was written with: a later
                // keyless move into another section must not inherit it.
                let key = r
                    .fields
                    .get("orderKey")
                    .and_then(Value::as_str)
                    .filter(|key| valid_pin_order_key(key))
                    .filter(|_| r.clocks.get("orderKey") >= r.clocks.get("location"))
                    .map(str::to_owned);
                (r.id, key)
            })
            .collect();
        members.sort_by(|a, b| match (&a.1, &b.1) {
            (Some(x), Some(y)) => x.cmp(y).then(a.0.cmp(&b.0)),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.0.cmp(&b.0),
        });
        members
    }

    /// Place `session` into `section` between its anchors: the order key and
    /// the clock to write it with. Keyless members are keyed first, in their
    /// current order, so every placement has real neighbours to sit between.
    fn place_in_sidebar_section(
        &mut self,
        section: &str,
        session: &str,
        after: &Option<String>,
        before: &Option<String>,
    ) -> Result<(String, String), DocError> {
        let mut members = self.sidebar_section_members(section);
        members.retain(|(id, _)| id != session);
        if members.iter().any(|(_, key)| key.is_none()) {
            let mut lower: Option<String> = None;
            for (id, key) in &mut members {
                let hlc = self.next_hlc();
                let next = pin_order_key_between(lower.as_deref(), None, &hlc)
                    .map_err(|e| DocError::Schema(e.into()))?;
                self.enqueue_ops(vec![RowOp {
                    kind: LOCATIONS.into(),
                    id: id.clone(),
                    op: OpKind::Upsert,
                    set: Some(fields([("orderKey", json!(next))])),
                    hlc,
                    clocks: None,
                }]);
                *key = Some(next.clone());
                lower = Some(next);
            }
        }
        let ids: Vec<String> = members.iter().map(|(id, _)| id.clone()).collect();
        let index = section_insert_index(&ids, after, before);
        let lower = index
            .checked_sub(1)
            .and_then(|i| members.get(i))
            .and_then(|(_, key)| key.as_deref());
        let upper = members.get(index).and_then(|(_, key)| key.as_deref());
        let hlc = self.next_hlc();
        let key =
            pin_order_key_between(lower, upper, &hlc).map_err(|e| DocError::Schema(e.into()))?;
        Ok((key, hlc))
    }

    /// [`Self::write_sidebar_location`] into a section, at a placed position.
    /// One op, one clock: the key orders exactly this placement.
    fn write_sidebar_placement(
        &mut self,
        session: &str,
        section: &str,
        after: &Option<String>,
        before: &Option<String>,
    ) -> Result<(), DocError> {
        self.observe_sidebar_row(LOCATIONS, session);
        self.observe_sidebar_row(KIND_SIDEBAR_PINS, session);
        let (key, hlc) = self.place_in_sidebar_section(section, session, after, before)?;
        self.enqueue_ops(vec![RowOp {
            kind: LOCATIONS.into(),
            id: session.into(),
            op: OpKind::Upsert,
            set: Some(fields([
                ("location", json!(section)),
                ("orderKey", json!(key)),
            ])),
            hlc,
            clocks: None,
        }]);
        Ok(())
    }

    fn live_sidebar_section(&self, id: &str) -> bool {
        self.overlay_row(SECTIONS, id)
            .is_some_and(|r| r.fields.get("deleted").and_then(Value::as_bool) != Some(true))
    }

    pub(super) fn change_sidebar_section(
        &mut self,
        change: &SidebarSectionChange,
    ) -> Result<(), DocError> {
        use SidebarSectionChange::*;
        let valid_id = |id: &str| {
            !id.is_empty()
                && id.len() <= 256
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.:@/-".contains(&b))
        };
        let valid_section_id = |id: &str| valid_id(id) && id != "pinned";
        let valid_name = |name: &str| !name.trim().is_empty() && name.chars().count() <= 120;
        match change {
            Create { id, name } | Rename { id, name }
                if !valid_section_id(id) || !valid_name(name) =>
            {
                return Err(DocError::Schema("Invalid section name or ID".into()));
            }
            Collapse { id, .. } | Delete { id } if !valid_section_id(id) => {
                return Err(DocError::Schema("Invalid section ID".into()));
            }
            Assign {
                session_id,
                section_id,
                after,
                before,
            } if !valid_id(session_id)
                || section_id.as_ref().is_some_and(|id| !valid_section_id(id))
                || [after, before]
                    .into_iter()
                    .flatten()
                    .any(|anchor| !valid_id(anchor)) =>
            {
                return Err(DocError::Schema("Invalid section membership".into()));
            }
            Import { sections }
                if sections.iter().any(|s| {
                    !valid_section_id(&s.id)
                        || !valid_name(&s.name)
                        || s.session_ids.iter().any(|id| !valid_id(id))
                }) =>
            {
                return Err(DocError::Schema("Invalid section import".into()));
            }
            _ => {}
        }
        self.initialize_sidebar_pins();
        match change {
            Create { id, name } => {
                // Existing IDs, including deleted sections, are never resurrected.
                if self.overlay_row(SECTIONS, id).is_none() {
                    let created = self.next_hlc();
                    self.write(
                        SECTIONS,
                        id,
                        OpKind::Upsert,
                        fields([
                            ("name", json!(name)),
                            ("collapsed", json!(false)),
                            ("deleted", json!(false)),
                            ("createdAt", json!(created)),
                        ]),
                    );
                }
            }
            Rename { id, name } => {
                if self.live_sidebar_section(id) {
                    self.observe_sidebar_row(SECTIONS, id);
                    self.write(
                        SECTIONS,
                        id,
                        OpKind::Upsert,
                        fields([("name", json!(name))]),
                    );
                }
            }
            Collapse { id, collapsed } => {
                if self.live_sidebar_section(id) {
                    self.observe_sidebar_row(SECTIONS, id);
                    self.write(
                        SECTIONS,
                        id,
                        OpKind::Upsert,
                        fields([("collapsed", json!(collapsed))]),
                    );
                }
            }
            Delete { id } => {
                self.observe_sidebar_row(SECTIONS, id);
                self.write(
                    SECTIONS,
                    id,
                    OpKind::Upsert,
                    fields([("deleted", json!(true))]),
                );
            }
            Assign {
                session_id,
                section_id,
                after,
                before,
            } => {
                if section_id
                    .as_ref()
                    .is_some_and(|id| !self.live_sidebar_section(id))
                {
                    return Err(DocError::Schema("Section no longer exists".into()));
                }
                if self.overlay_row(KIND_CHATS, session_id).is_none() {
                    return Err(DocError::Schema("Session no longer exists".into()));
                }
                // Keep legacy clients from displaying section members as pins.
                self.observe_sidebar_row(KIND_SIDEBAR_PINS, session_id);
                self.write(
                    KIND_SIDEBAR_PINS,
                    session_id,
                    OpKind::Upsert,
                    fields([("pinned", json!(false))]),
                );
                match section_id {
                    Some(id) => self.write_sidebar_placement(session_id, id, after, before)?,
                    None => self.write_sidebar_location(session_id, ""),
                }
                if let Some(id) = section_id {
                    self.change_sidebar_section(&Collapse {
                        id: id.clone(),
                        collapsed: false,
                    })?;
                }
            }
            Import { sections } => {
                for section in sections {
                    // Replay and migration from another device must not undo a
                    // rename, deletion, collapse, or explicit session move.
                    if self.overlay_row(SECTIONS, &section.id).is_some() {
                        continue;
                    }
                    self.change_sidebar_section(&Create {
                        id: section.id.clone(),
                        name: section.name.clone(),
                    })?;
                    for id in &section.session_ids {
                        if self.overlay_row(LOCATIONS, id).is_none()
                            && !self.ordered_sidebar_pins().iter().any(|(pin, _)| pin == id)
                            && self.overlay_row(KIND_CHATS, id).is_some()
                        {
                            // Appending one by one keeps the imported order.
                            self.write_sidebar_placement(id, &section.id, &None, &None)?;
                        }
                    }
                    self.change_sidebar_section(&Collapse {
                        id: section.id.clone(),
                        collapsed: section.collapsed,
                    })?;
                }
            }
        }
        Ok(())
    }
}
