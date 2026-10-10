//! Device-local, non-secret profile data. Invalid fields remain visible for editing.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    #[default]
    Fit,
    ActualSize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Profile {
    pub id: Uuid,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub keyboard_layout: u32,
    pub domain: Option<String>,
    pub desktop_width: u16,
    pub desktop_height: u16,
    pub view_mode: ViewMode,
    pub resize_remote: bool,
    pub remember_password: bool,
    pub trusted_certificate_sha256: Option<String>,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            id: Uuid::new_v4(),
            name: String::new(),
            host: String::new(),
            port: 3389,
            username: String::new(),
            keyboard_layout: 0x0409,
            domain: None,
            desktop_width: 1280,
            desktop_height: 800,
            view_mode: ViewMode::Fit,
            resize_remote: false,
            remember_password: false,
            trusted_certificate_sha256: None,
        }
    }
}
impl Profile {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("Enter a connection name".into());
        }
        if self.host.is_empty()
            || self.host.trim() != self.host
            || self
                .host
                .chars()
                .any(|c| c.is_whitespace() || matches!(c, '/' | '@' | '?' | '#' | '[' | ']'))
        {
            return Err(
                "Enter a DNS name or IP address without a scheme, brackets, port or credentials"
                    .into(),
            );
        }
        if self.host.contains(':') && self.host.parse::<std::net::Ipv6Addr>().is_err() {
            return Err("Enter the port in its separate field".into());
        }
        if self.port == 0 {
            return Err("Port must be between 1 and 65535".into());
        }
        if self.username.trim().is_empty() {
            return Err("Enter a remote username".into());
        }
        zeron_rdp::validate_size(self.desktop_width, self.desktop_height).map_err(|e| e.message)?;
        if let Some(pin) = &self.trusted_certificate_sha256
            && (pin.len() != 64 || !pin.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(
                "Invalid saved certificate fingerprint; edit and save the connection to clear it"
                    .into(),
            );
        }
        Ok(())
    }
    pub fn same_identity(&self, other: &Self) -> bool {
        self.host.eq_ignore_ascii_case(&other.host)
            && self.port == other.port
            && self.username == other.username
            && self.domain == other.domain
    }
    /// Returns whether the previous credential must be deleted. A rename retains trust.
    pub fn invalidate_changed_identity(&mut self, previous: &Self) -> bool {
        if self.same_identity(previous) {
            return false;
        }
        self.trusted_certificate_sha256 = None;
        self.remember_password = false;
        true
    }
    pub fn endpoint(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Preserve duplicate rows but assign fresh identities, removing any association
/// with another row's password or trust. Host/port errors are never normalized away.
pub fn repair_duplicate_ids(profiles: &mut [Profile]) {
    let mut seen = std::collections::HashSet::new();
    for profile in profiles {
        if profile.id.is_nil() || !seen.insert(profile.id) {
            tracing::warn!("Reassigned duplicate or empty remote desktop profile ID");
            profile.id = Uuid::new_v4();
            seen.insert(profile.id);
            profile.remember_password = false;
            profile.trusted_certificate_sha256 = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile() -> Profile {
        Profile {
            name: "Development".into(),
            host: "::1".into(),
            username: "dev".into(),
            ..Profile::default()
        }
    }
    #[test]
    fn remote_desktop_validation_and_identity() {
        let mut original = profile();
        original.remember_password = true;
        original.trusted_certificate_sha256 = Some("ab".repeat(32));
        assert!(original.validate().is_ok());
        let mut edited = original.clone();
        edited.name = "Renamed".into();
        assert!(!edited.invalidate_changed_identity(&original));
        assert!(edited.remember_password);
        edited.host = "new.example".into();
        assert!(edited.invalidate_changed_identity(&original));
        assert!(!edited.remember_password);
        assert!(edited.trusted_certificate_sha256.is_none());
        for host in ["rdp://host", "user@host", "host:3389", "[::1]", " host", ""] {
            edited.host = host.into();
            assert!(edited.validate().is_err());
        }
    }
    #[test]
    fn remote_desktop_duplicate_ids_do_not_share_secrets() {
        let mut rows = vec![profile(); 2];
        rows[1].port = 0;
        rows[1].remember_password = true;
        repair_duplicate_ids(&mut rows);
        assert_ne!(rows[0].id, rows[1].id);
        assert!(!rows[1].remember_password);
        assert!(rows[1].validate().is_err());
        let json = serde_json::to_string(&rows).unwrap();
        assert!(!json.contains("password\":"));
    }
}

#[cfg(test)]
mod persistence_tests {
    use super::*;
    #[test]
    fn remote_desktop_old_settings_and_roundtrip() {
        let old: crate::settings::UiSettings =
            serde_json::from_str(r#"{"rightPaneWidth":600}"#).unwrap();
        assert!(old.remote_desktop_profiles.is_empty());
        let mut settings = old;
        settings.remote_desktop_profiles.push(Profile {
            name: "Local dev".into(),
            host: "localhost".into(),
            username: "dev".into(),
            ..Profile::default()
        });
        let json = serde_json::to_string(&settings).unwrap();
        let restored: crate::settings::UiSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored.remote_desktop_profiles,
            settings.remote_desktop_profiles
        );
        assert!(!json.contains("test-secret"));
    }
}
