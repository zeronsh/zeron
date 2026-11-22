//! The credential environment an agent's model discovery runs with, per
//! agent. Empty everywhere but a Cloud machine: there an agent's login is
//! often a key in its RUN's environment (`RunRequest::env`, set by the
//! machine's credential broker), never in the engine's own, so discovery
//! launched from the engine would list the models of an agent signed in to
//! nothing. The engine hands each agent's credential variables here after a
//! run prepares them; discovery spawns apply them like a run's environment,
//! and the catalog cache key covers them (a new key is a new catalog).
//!
//! Values are secrets: never logged, only hashed.

use std::collections::{BTreeMap, HashMap};
use std::sync::{OnceLock, PoisonError, RwLock};

use sha2::{Digest, Sha256};
use zeron_proto::HarnessId;

fn table() -> &'static RwLock<HashMap<HarnessId, BTreeMap<String, String>>> {
    static TABLE: OnceLock<RwLock<HashMap<HarnessId, BTreeMap<String, String>>>> = OnceLock::new();
    TABLE.get_or_init(Default::default)
}

/// What `harness`'s discovery runs with from now on (empty clears it).
/// Returns whether it changed.
pub fn set(harness: HarnessId, env: BTreeMap<String, String>) -> bool {
    let mut table = table().write().unwrap_or_else(PoisonError::into_inner);
    if env.is_empty() {
        return table.remove(&harness).is_some();
    }
    table.insert(harness, env.clone()) != Some(env)
}

/// `harness`'s discovery environment (empty when none was set).
pub fn get(harness: HarnessId) -> BTreeMap<String, String> {
    table()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&harness)
        .cloned()
        .unwrap_or_default()
}

/// Fold `harness`'s discovery environment into a catalog cache key.
pub(crate) fn hash_into(hash: &mut Sha256, harness: HarnessId) {
    for (key, value) in get(harness) {
        for part in [key.as_bytes(), value.as_bytes()] {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_replaces_and_reports_a_change_and_empty_clears() {
        // (A harness no other test sets.)
        let id = HarnessId::Fx;
        let env = |v: &str| BTreeMap::from([("AI_GATEWAY_API_KEY".to_string(), v.to_string())]);
        assert!(set(id, env("k1")));
        assert!(!set(id, env("k1")));
        assert_eq!(get(id), env("k1"));
        let hash = |id| {
            let mut h = Sha256::new();
            hash_into(&mut h, id);
            h.finalize()
        };
        let first = hash(id);
        assert!(set(id, env("k2")));
        assert_ne!(hash(id), first, "a new key is a new catalog key");
        assert!(set(id, BTreeMap::new()));
        assert!(get(id).is_empty());
        assert!(!set(id, BTreeMap::new()));
    }
}
