//! Stable device-local browser storage, independent of chats and repositories.
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use zeron_proto::{AuthState, WorkspaceScope};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BrowserProfile {
    pub root: PathBuf,
}

impl BrowserProfile {
    pub fn for_workspace(
        data_dir: &Path,
        scope: Option<WorkspaceScope>,
        auth: Option<&AuthState>,
        local_device_id: Option<&str>,
    ) -> Option<Self> {
        let scope = scope?;
        let scope_key = match scope {
            WorkspaceScope::Local => "local",
            WorkspaceScope::Synced => "synced",
            WorkspaceScope::Development => "development",
        };
        match scope {
            WorkspaceScope::Local => Some(Self::new(data_dir, &[scope_key, local_device_id?])),
            WorkspaceScope::Synced | WorkspaceScope::Development => {
                let AuthState::SignedIn { user, org_id } = auth? else {
                    return None;
                };
                Some(Self::new(
                    data_dir,
                    &[scope_key, &user.id, org_id.as_deref().unwrap_or("")],
                ))
            }
        }
    }

    fn new(data_dir: &Path, identity: &[&str]) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"zeron-browser-profile-v1");
        for part in identity {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part.as_bytes());
        }
        // Resolve aliases so windows using the same installation share a store.
        let data_dir = data_dir.canonicalize().unwrap_or_else(|_| {
            std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf())
        });
        Self {
            root: data_dir
                .join("browser")
                .join(format!("{:x}", hash.finalize())),
        }
    }

    #[cfg(any(target_os = "macos", test))]
    pub fn data_store_identifier(&self) -> [u8; 16] {
        // Include the installation directory: fixtures and alternative data roots
        // must never open a real user's WKWebsiteDataStore.
        let mut hash = Sha256::new();
        hash.update(b"zeron-wkwebsite-data-store-v1");
        hash.update(self.root.as_os_str().as_encoded_bytes());
        let mut id: [u8; 16] = hash.finalize()[..16].try_into().unwrap();
        id[6] = (id[6] & 0x0f) | 0x80; // UUID v8, application-defined hash.
        id[8] = (id[8] & 0x3f) | 0x80;
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_stable_and_scoped_to_account_organization_and_installation() {
        let root = tempfile::tempdir().unwrap();
        let make = |parts: &[&str]| BrowserProfile::new(root.path(), parts);
        let a = make(&["synced", "user-a", "org-a"]);
        assert_eq!(a, make(&["synced", "user-a", "org-a"]));
        for parts in [
            ["synced", "user-b", "org-a"],
            ["synced", "user-a", "org-b"],
            ["development", "user-a", "org-a"],
        ] {
            let b = make(&parts);
            assert_ne!(a.root, b.root);
            assert_ne!(a.data_store_identifier(), b.data_store_identifier());
        }
        let other = tempfile::tempdir().unwrap();
        let b = BrowserProfile::new(other.path(), &["synced", "user-a", "org-a"]);
        assert_ne!(a.data_store_identifier(), b.data_store_identifier());
        assert_ne!(make(&["ab", "c"]), make(&["a", "bc"]));
        assert!(
            make(&["../../outside"])
                .root
                .starts_with(root.path().join("browser"))
        );
    }

    #[test]
    fn unresolved_identity_does_not_select_a_shared_persistent_profile() {
        let root = Path::new("/unused");
        assert!(BrowserProfile::for_workspace(root, None, None, None).is_none());
        assert!(
            BrowserProfile::for_workspace(root, Some(WorkspaceScope::Local), None, None).is_none()
        );
        assert!(
            BrowserProfile::for_workspace(root, Some(WorkspaceScope::Synced), None, Some("device"))
                .is_none()
        );
        let local = |device| {
            BrowserProfile::for_workspace(root, Some(WorkspaceScope::Local), None, Some(device))
        };
        assert_eq!(local("one"), local("one"));
        assert_ne!(local("one"), local("two"));
    }
}
