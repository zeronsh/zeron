//! Per-pin intents and dense, collision-free hexadecimal ordering keys.
use serde::{Deserialize, Serialize};

// Compatibility exports: pin keys retain their exact wire format.
pub use crate::ordering::{
    MAX_ORDER_KEY_BYTES as MAX_PIN_ORDER_KEY_BYTES, order_key_between as pin_order_key_between,
    valid_order_key as valid_pin_order_key,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum SidebarPinChange {
    #[serde(rename_all = "camelCase")]
    Pin {
        session_id: String,
        after: Option<String>,
        before: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Move {
        session_id: String,
        after: Option<String>,
        before: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    Unpin {
        session_id: String,
    },
    Section {
        change: SidebarSectionChange,
    },
}

impl SidebarPinChange {
    pub fn session_id(&self) -> &str {
        match self {
            Self::Section { .. } => "",
            Self::Pin { session_id, .. }
            | Self::Move { session_id, .. }
            | Self::Unpin { session_id } => session_id,
        }
    }
    /// Rebase a pending intent on the latest confirmed projection. A stale
    /// move never resurrects an unpinned item. A surviving right anchor wins;
    /// otherwise use the left anchor, or append when both disappeared.
    pub fn project(&self, ids: &mut Vec<String>) {
        if let Self::Section { change } = self {
            if let SidebarSectionChange::Assign { session_id, .. } = change {
                ids.retain(|id| id != session_id);
            }
            return;
        }
        let id = self.session_id();
        if matches!(self, Self::Move { .. }) && !ids.iter().any(|v| v == id) {
            return;
        }
        ids.retain(|v| v != id);
        let (after, before) = match self {
            Self::Unpin { .. } | Self::Section { .. } => return,
            Self::Pin { after, before, .. } | Self::Move { after, before, .. } => (after, before),
        };
        let index = before
            .as_ref()
            .and_then(|v| ids.iter().position(|i| i == v))
            .or_else(|| {
                after
                    .as_ref()
                    .and_then(|v| ids.iter().position(|i| i == v).map(|i| i + 1))
            })
            .unwrap_or(ids.len());
        ids.insert(index, id.to_owned());
    }
}

/// Section intents share the pin queue so moves cannot overtake one another.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum SidebarSectionChange {
    Create {
        id: String,
        name: String,
    },
    Rename {
        id: String,
        name: String,
    },
    Collapse {
        id: String,
        collapsed: bool,
    },
    Delete {
        id: String,
    },
    #[serde(rename_all = "camelCase")]
    Assign {
        session_id: String,
        section_id: Option<String>,
    },
    Import {
        sections: Vec<crate::SidebarSection>,
    },
}

impl SidebarPinChange {
    pub fn project_sections(&self, sections: &mut Vec<crate::SidebarSection>) {
        match self {
            Self::Pin { session_id, .. } => {
                for section in sections {
                    section.session_ids.retain(|id| id != session_id);
                }
            }
            Self::Section { change } => change.project(sections),
            _ => {}
        }
    }
}

impl SidebarSectionChange {
    pub fn project(&self, sections: &mut Vec<crate::SidebarSection>) {
        match self {
            Self::Create { id, name } => {
                if !sections.iter().any(|s| &s.id == id) {
                    sections.push(crate::SidebarSection {
                        id: id.clone(),
                        name: name.clone(),
                        session_ids: vec![],
                        collapsed: false,
                    });
                }
            }
            Self::Rename { id, name } => {
                if let Some(s) = sections.iter_mut().find(|s| &s.id == id) {
                    s.name = name.clone();
                }
            }
            Self::Collapse { id, collapsed } => {
                if let Some(s) = sections.iter_mut().find(|s| &s.id == id) {
                    s.collapsed = *collapsed;
                }
            }
            Self::Delete { id } => sections.retain(|s| &s.id != id),
            Self::Assign {
                session_id,
                section_id,
            } => {
                for s in sections {
                    s.session_ids.retain(|id| id != session_id);
                    if section_id.as_ref() == Some(&s.id) {
                        s.session_ids.push(session_id.clone());
                        s.collapsed = false;
                    }
                }
            }
            // Import is resolved by the registry, which also knows tombstones.
            Self::Import { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_keys_are_distinct_and_dense() {
        let a = pin_order_key_between(None, None, "0000000000001-000000-device-a").unwrap();
        let nonce = "0000000000001-000000-device-a";
        let encoded: String = nonce.bytes().map(|b| format!("{b:02x}")).collect();
        assert_eq!(
            a,
            format!("8{encoded}{}8", "0".repeat((149 - nonce.len()) * 2))
        );
        let b = pin_order_key_between(None, None, "0000000000001-000000-device-b").unwrap();
        assert!(a < b);
        let mid =
            pin_order_key_between(Some(&a), Some(&b), "0000000000002-000000-device-c").unwrap();
        assert!(a < mid && mid < b);
        let mut upper = a.clone();
        for i in 0..1000 {
            let key = pin_order_key_between(None, Some(&upper), &format!("{i:013}-000000-device"))
                .unwrap();
            assert!(key < upper && valid_pin_order_key(&key));
            upper = key;
        }
    }
    #[test]
    fn rejects_invalid_keys_and_bounds() {
        assert!(pin_order_key_between(Some("80"), None, "x").is_err());
        assert!(pin_order_key_between(Some("8"), Some("8"), "x").is_err());
    }
    #[test]
    fn pending_move_does_not_revive_unpinned_item() {
        let mut ids = vec!["remote".into()];
        SidebarPinChange::Move {
            session_id: "gone".into(),
            after: None,
            before: None,
        }
        .project(&mut ids);
        assert_eq!(ids, vec!["remote"]);
    }
}
