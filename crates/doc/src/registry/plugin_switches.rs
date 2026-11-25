//! Account-wide plugin switches: whether a plugin whose switch is the
//! account's (its manifest's `"switch": { "scope": "account" }`) is on, for
//! every engine of the account, headless and Cloud ones included.
//!
//! One `pluginSwitches` row per plugin, keyed by its id, with one field,
//! `on`. Per-field LWW like every row, so the last toggle on any device wins.
//! No row: the plugin's own default (its manifest's `"default"`), which isn't
//! written, so a default change in a later build still reaches an account
//! that never chose. Nothing here deletes a row.
//!
//! Reads are lenient: a row whose `on` this build can't read (a newer peer's
//! shape) is passed over, not an error. Builds that don't know the kind keep
//! its rows as they keep every row, by kind and id, and pass them on.

use super::*;

/// The account's plugin switches, one row per plugin id.
pub const KIND_PLUGIN_SWITCHES: &str = "pluginSwitches";
/// The switch's field.
const ON: &str = "on";

impl RegistryDoc {
    /// Every switch the account has set, by plugin id.
    pub fn plugin_switches(&self) -> BTreeMap<String, bool> {
        self.overlay_rows(KIND_PLUGIN_SWITCHES)
            .into_iter()
            .filter_map(|row| Some((row.id.clone(), row.fields.get(ON)?.as_bool()?)))
            .collect()
    }

    /// The account's switch for plugin `id`, when it has set one.
    pub fn plugin_switch(&self, id: &str) -> Option<bool> {
        self.overlay_row(KIND_PLUGIN_SWITCHES, id)?
            .fields
            .get(ON)?
            .as_bool()
    }

    /// Changes whenever a switch may have changed.
    pub fn plugin_switches_version(&self) -> u64 {
        self.kind_version(KIND_PLUGIN_SWITCHES)
    }

    /// Set plugin `id`'s switch for the whole account. Whether anything was
    /// written (nothing when it already reads `on`).
    pub fn set_plugin_switch(&mut self, id: &str, on: bool) -> Result<bool, DocError> {
        // (A plugin id is `[a-z0-9-]`, well inside the room's id alphabet.)
        if id.is_empty()
            || id.len() > 64
            || !id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return Err(DocError::Schema(format!("\"{id}\" isn't a plugin id")));
        }
        let row = self.overlay_row(KIND_PLUGIN_SWITCHES, id);
        if row.as_ref().and_then(|r| r.fields.get(ON)?.as_bool()) == Some(on) {
            return Ok(false);
        }
        // A toggle made after seeing the switch beats it, whatever this
        // device's wall clock says: observe its clock first.
        if let Some(clock) = row.as_ref().and_then(|r| r.clocks.get(ON)) {
            let mut parts = clock.splitn(3, '-');
            if let (Some(ms), Some(counter)) = (
                parts.next().and_then(|v| v.parse::<i64>().ok()),
                parts.next().and_then(|v| v.parse::<u32>().ok()),
            ) && (ms, counter) > (self.clock.last_ms, self.clock.counter)
            {
                self.clock.last_ms = ms;
                self.clock.counter = counter;
            }
        }
        self.write(
            KIND_PLUGIN_SWITCHES,
            id,
            OpKind::Upsert,
            fields([(ON, json!(on))]),
        );
        Ok(true)
    }
}
