//! Files an agent ships with a visual (images, glTF models, environment
//! maps), by name. The agent passes them to `show_visual`; the engine stores
//! them beside the script, and every runtime that loads the script gets them.
//! Scripts refer to them by name (`texture = "wood.jpg"`,
//! `s:model { asset = "prism.glb" }`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// At most this many assets per visual.
pub const MAX_ASSETS: usize = 16;
/// Largest single asset.
pub const MAX_ASSET_BYTES: usize = 8 * 1024 * 1024;
/// All of a visual's assets together.
pub const MAX_TOTAL_ASSET_BYTES: usize = 10 * 1024 * 1024;

/// A visual's named files, cheap to clone.
#[derive(Clone, Default)]
pub struct Assets {
    files: Arc<HashMap<String, Arc<[u8]>>>,
    /// Parsed models, so a scene redrawing every frame parses each once.
    pub(crate) models: Arc<Mutex<HashMap<String, Arc<crate::model::Model>>>>,
}

impl std::fmt::Debug for Assets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.files.keys()).finish()
    }
}

impl PartialEq for Assets {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.files, &other.files)
    }
}

impl Assets {
    pub fn new(files: HashMap<String, Arc<[u8]>>) -> Self {
        Self {
            files: Arc::new(files),
            models: Arc::default(),
        }
    }

    pub fn get(&self, name: &str) -> Option<Arc<[u8]>> {
        self.files.get(name).cloned()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// The file `name`, or an error naming what the visual does have.
    pub(crate) fn require(&self, name: &str) -> Result<Arc<[u8]>, String> {
        self.get(name).ok_or_else(|| {
            let mut known: Vec<&str> = self.names().collect();
            known.sort_unstable();
            if known.is_empty() {
                format!("no asset named '{name}' (pass files in show_visual's assets)")
            } else {
                format!(
                    "no asset named '{name}' (this visual has {})",
                    known.join(", ")
                )
            }
        })
    }
}

/// Whether `name` can name an asset: a plain file name.
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}
