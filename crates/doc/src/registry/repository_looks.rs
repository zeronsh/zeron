//! A project's name and icon belong to its repository, wherever it's cloned.
//!
//! A `repositoryLooks` row holds them, per-field LWW like every row, keyed by
//! a hash of the repository identity: that identity may name a subfolder, in
//! any spelling a path allows, and the room refuses a batch with an id outside
//! its alphabet.
//!
//! What a Space reads is decided here, on read, so every reader of the
//! registry (engine, client, desktop, mobile) sees one answer and nothing needs
//! migrating: the most recently set value, by clock, among the repository's
//! look row and the fields of its copies. Those fields still count because
//! older engines and Cloud runners (which may not write this kind) only ever
//! write there, and because they hold every choice made before this row
//! existed. A folder with no shareable identity reads its own fields, as ever.
//!
//! A write goes to the look row and to the one Space it was made on, in one
//! batch, so an older engine still sees it on that row.

use super::*;
use sha2::{Digest, Sha256};
use zeron_proto::SpaceIcon;

const NAME: &str = "name";
const ICON: &str = "icon";
/// Written once, by the upsert that creates the row: what else carries its
/// clock was written at creation, not chosen later.
const CREATED_AT: &str = "createdAt";

/// The identity a look can be shared under. A `local:` identity is scoped to
/// one device's git dir, so nothing else can safely match it.
fn shared_identity(repository_id: Option<&str>) -> Option<&str> {
    repository_id.filter(|id| !id.is_empty() && !id.starts_with("local:"))
}

/// The look row of a repository. Hex, so always inside the room's id alphabet.
fn look_row_id(repository_id: &str) -> String {
    format!("{:x}", Sha256::digest(repository_id.as_bytes()))
}

/// One field's newest choice; a `None` value is a choice too (cleared).
struct Latest<T> {
    clock: String,
    value: Option<T>,
}

fn offer<T>(latest: &mut Option<Latest<T>>, clock: &str, value: Option<T>) {
    if latest.as_ref().is_none_or(|l| clock > l.clock.as_str()) {
        *latest = Some(Latest {
            clock: clock.to_owned(),
            value,
        });
    }
}

#[derive(Default)]
struct RepositoryLook {
    name: Option<Latest<String>>,
    icon: Option<Latest<SpaceIcon>>,
}

fn parse_name(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

/// (Leniently, as a Space reads it: a malformed icon is no choice at all.)
fn parse_icon(value: &Value) -> Option<SpaceIcon> {
    serde_json::from_value::<SpaceIcon>(value.clone())
        .ok()
        .and_then(|icon| icon.normalized().ok().flatten())
}

impl RepositoryLook {
    fn of_row(row: Option<&RegistryRow>) -> Self {
        let mut look = Self::default();
        let Some(row) = row else {
            return look;
        };
        // (A value this build can't read is passed over, never a lost row.)
        if let Some(clock) = row.clocks.get(NAME) {
            match row.fields.get(NAME) {
                None => offer(&mut look.name, clock, None),
                Some(value) => {
                    if let Some(name) = parse_name(value) {
                        offer(&mut look.name, clock, Some(name));
                    }
                }
            }
        }
        if let Some(clock) = row.clocks.get(ICON) {
            match row.fields.get(ICON) {
                None => offer(&mut look.icon, clock, None),
                Some(value) => {
                    if let Some(icon) = parse_icon(value) {
                        offer(&mut look.icon, clock, Some(icon));
                    }
                }
            }
        }
        look
    }

    /// What this copy's own row has to say. A field written by the row's
    /// creating upsert is no choice unless it says something: every create
    /// writes `name: null` on a fresh clock, which must not read as a clear
    /// made just now by the device that added the project.
    fn consider(&mut self, space: &Space, row: &RegistryRow) {
        let created = row.clocks.get(CREATED_AT);
        if let Some(clock) = row.clocks.get(NAME) {
            let chosen = created != Some(clock)
                || space.name.as_deref().is_some_and(|name| {
                    name != Space {
                        name: None,
                        ..space.clone()
                    }
                    .display_name()
                });
            if chosen {
                offer(&mut self.name, clock, space.name.clone());
            }
        }
        if let Some(clock) = row.clocks.get(ICON) {
            let malformed = row.fields.contains_key(ICON) && space.icon.is_none();
            if !malformed && (created != Some(clock) || space.icon.is_some()) {
                offer(&mut self.icon, clock, space.icon.clone());
            }
        }
    }

    /// With no choice made, a copy keeps its own fields (its folder's name).
    fn apply(&self, space: &mut Space) {
        if let Some(latest) = &self.name {
            space.name.clone_from(&latest.value);
        }
        if let Some(latest) = &self.icon {
            space.icon.clone_from(&latest.value);
        }
    }
}

impl RegistryDoc {
    /// Every Space as it reads: its repository's look over its own fields.
    pub(super) fn spaces_with_looks(&self) -> Vec<Space> {
        let rows = self.overlay_rows(KIND_SPACES);
        let mut spaces: Vec<(Space, &RegistryRow)> = rows
            .iter()
            .filter_map(|row| {
                let raw = row_to::<crate::workspace::RawSpace>(row)?;
                Some((Space::from(raw), row))
            })
            .collect();
        // (By id first: two copies on one clock, which only migration seeds
        // can make, resolve alike on every device.)
        spaces.sort_by(|a, b| a.0.id.cmp(&b.0.id));

        let mut looks: HashMap<String, RepositoryLook> = HashMap::new();
        for (space, row) in &spaces {
            if let Some(identity) = shared_identity(space.repository_id.as_deref()) {
                looks
                    .entry(identity.to_owned())
                    .or_insert_with(|| {
                        let row = self.overlay_row(KIND_REPOSITORY_LOOKS, &look_row_id(identity));
                        RepositoryLook::of_row(row.as_ref())
                    })
                    .consider(space, row);
            }
        }
        spaces
            .into_iter()
            .map(|(mut space, _)| {
                if let Some(look) = shared_identity(space.repository_id.as_deref())
                    .and_then(|identity| looks.get(identity))
                {
                    look.apply(&mut space);
                }
                space
            })
            .collect()
    }

    /// One Space as it reads. (Its own row alone when it shares nothing.)
    pub(super) fn space_with_look(&self, space_id: &str) -> Option<Space> {
        let row = self.overlay_row(KIND_SPACES, space_id)?;
        let space = Space::from(row_to::<crate::workspace::RawSpace>(&row)?);
        if shared_identity(space.repository_id.as_deref()).is_none() {
            return Some(space);
        }
        self.spaces_with_looks()
            .into_iter()
            .find(|s| s.id == space_id)
    }

    /// LWW set of `name` or `icon` from any device, on the Space it was made
    /// on and on its repository's look. `false` when no such Space.
    pub(super) fn write_look_field(&mut self, space_id: &str, field: &str, value: Value) -> bool {
        let Some(row) = self.overlay_row(KIND_SPACES, space_id) else {
            return false;
        };
        let hlc = self.next_hlc();
        let op = |kind: &str, id: &str, op: OpKind| RowOp {
            kind: kind.to_owned(),
            id: id.to_owned(),
            op,
            set: Some(fields([(field, value.clone())])),
            hlc: hlc.clone(),
            clocks: None,
        };
        let mut ops = vec![op(KIND_SPACES, space_id, OpKind::Update)];
        let identity = row.fields.get("repositoryId").and_then(Value::as_str);
        if let Some(identity) = shared_identity(identity) {
            // (An upsert: the first choice for a repository creates its row.)
            ops.push(op(
                KIND_REPOSITORY_LOOKS,
                &look_row_id(identity),
                OpKind::Upsert,
            ));
        }
        self.enqueue_ops(ops);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The room's `ID_RE` (edge/src/registry-core.ts).
    #[test]
    fn a_look_row_id_is_always_one_the_room_accepts() {
        let id = look_row_id("commit:abc:apps/my web/é");
        assert!(id.len() <= 256);
        assert!(
            id.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_.:@/-".contains(&b))
        );
        assert_eq!(id, look_row_id("commit:abc:apps/my web/é"));
        assert_ne!(id, look_row_id("commit:abc"));
    }

    #[test]
    fn only_a_device_scoped_or_missing_identity_keeps_to_itself() {
        assert_eq!(shared_identity(Some("commit:abc")), Some("commit:abc"));
        assert_eq!(
            shared_identity(Some("commit:abc:apps/web")),
            Some("commit:abc:apps/web")
        );
        assert_eq!(
            shared_identity(Some("github.com/o/r")),
            Some("github.com/o/r")
        );
        assert_eq!(shared_identity(Some("local:abc")), None);
        assert_eq!(shared_identity(Some("")), None);
        assert_eq!(shared_identity(None), None);
    }
}
