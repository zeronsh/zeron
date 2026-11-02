//! Local preference for Zeron's chat/visual MCP tools, independent of computer use.
//! Marker files are read at agent startup by both the engine and its MCP bridge.

use std::path::{Path, PathBuf};

pub const DISABLED_MARKER: &str = "zeron-mcp-disabled";
pub const COMPUTER_DISABLED_MARKER: &str = "computer-use-disabled";

pub fn settings_dir(data_dir: Option<&Path>) -> Option<PathBuf> {
    data_dir.map(Path::to_path_buf).or_else(|| {
        std::env::var_os("ZERON_DATA_DIR").map(PathBuf::from).or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".zeron"))
        })
    })
}

/// Missing marker preserves the existing default: chat and visual tools enabled.
pub fn user_enabled(data_dir: Option<&Path>) -> bool {
    settings_dir(data_dir).is_none_or(|dir| !dir.join(DISABLED_MARKER).exists())
}

pub fn set_user_enabled(data_dir: &Path, enabled: bool) -> std::io::Result<()> {
    let marker = data_dir.join(DISABLED_MARKER);
    if enabled {
        match std::fs::remove_file(marker) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    } else {
        std::fs::create_dir_all(data_dir)?;
        std::fs::write(marker, b"")
    }
}

/// Keep the bridge when computer use alone is enabled; omit it when both are off.
pub fn server_enabled(data_dir: Option<&Path>) -> bool {
    let computer_env_off = std::env::var("ZERON_COMPUTER_USE")
        .is_ok_and(|value| matches!(value.trim(), "0" | "false" | "off" | "no"));
    server_enabled_with_computer_env(data_dir, computer_env_off)
}

fn server_enabled_with_computer_env(data_dir: Option<&Path>, computer_env_off: bool) -> bool {
    user_enabled(data_dir)
        || (!computer_env_off
            && settings_dir(data_dir)
                .is_none_or(|dir| !dir.join(COMPUTER_DISABLED_MARKER).exists()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_setting_is_persistent_and_independent_of_computer_use() {
        let dir = std::env::temp_dir().join(format!("zeron-mcp-setting-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(user_enabled(Some(&dir)));
        set_user_enabled(&dir, false).unwrap();
        assert!(!user_enabled(Some(&dir)));
        assert!(server_enabled_with_computer_env(Some(&dir), false));
        assert!(!server_enabled_with_computer_env(Some(&dir), true));
        assert!(!dir.join(COMPUTER_DISABLED_MARKER).exists());
        std::fs::write(dir.join(COMPUTER_DISABLED_MARKER), b"").unwrap();
        assert!(!server_enabled_with_computer_env(Some(&dir), false));
        set_user_enabled(&dir, true).unwrap();
        assert!(user_enabled(Some(&dir)));
        assert!(server_enabled_with_computer_env(Some(&dir), false));
        assert!(dir.join(COMPUTER_DISABLED_MARKER).exists());
        set_user_enabled(&dir, true).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
    }
}
