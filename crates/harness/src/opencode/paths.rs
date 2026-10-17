//! OpenCode uses Node's home directory, which differs from Git's HOME on Windows.
//! Keep discovery, credential management and cache invalidation on the same roots.
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub data: PathBuf,
    pub config: PathBuf,
}

impl Paths {
    pub fn detect() -> Self {
        Self::with_env(cfg!(windows), &|name| std::env::var_os(name))
    }

    fn with_env(windows: bool, env: &impl Fn(&str) -> Option<OsString>) -> Self {
        let value = |name| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
        let home = if windows {
            value("USERPROFILE")
        } else {
            value("HOME")
        }
        .unwrap_or_else(|| {
            if windows {
                // Match Node's OS profile fallback when USERPROFILE is absent;
                // Git's HOME must not take precedence over the Windows token.
                #[allow(deprecated)]
                let profile = std::env::home_dir();
                profile.unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
            } else {
                crate::executable::home_or_current_dir()
            }
        });
        // xdg-basedir ignores relative overrides.
        let root = |name, fallback| value(name).filter(|p| p.is_absolute()).unwrap_or(fallback);
        let data = root("XDG_DATA_HOME", home.join(".local/share")).join("opencode");
        let config = value("OPENCODE_CONFIG_DIR")
            .unwrap_or_else(|| root("XDG_CONFIG_HOME", home.join(".config")).join("opencode"));
        Self { home, data, config }
    }

    pub fn auth_file(&self) -> PathBuf {
        self.data.join("auth.json")
    }

    pub fn database(&self) -> PathBuf {
        self.data.join("opencode.db")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_uses_the_profile_even_when_git_sets_home() {
        let profile = std::env::temp_dir().join("native-profile");
        let paths = Paths::with_env(true, &|key| match key {
            "USERPROFILE" => Some(profile.clone().into()),
            "HOME" => Some(std::env::temp_dir().join("git-home").into()),
            "XDG_DATA_HOME" => Some("relative-data".into()),
            _ => None,
        });
        assert_eq!(paths.home, profile);
        assert_eq!(
            paths.auth_file(),
            profile.join(".local/share/opencode/auth.json")
        );
    }

    #[test]
    fn absolute_xdg_and_config_overrides_are_shared() {
        let root = std::env::temp_dir().join("opencode-paths");
        let paths = Paths::with_env(cfg!(windows), &|key| match key {
            "XDG_DATA_HOME" => Some(root.join("data").into()),
            "OPENCODE_CONFIG_DIR" => Some(root.join("config").into()),
            _ => None,
        });
        assert_eq!(paths.database(), root.join("data/opencode/opencode.db"));
        assert_eq!(paths.config, root.join("config"));
    }
}
