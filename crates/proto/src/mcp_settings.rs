//! Local settings for the built-in MCP server. Extra servers are independent.
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

const FILE: &str = "agent-tools.json";
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentTools {
    pub enabled: bool,
    pub tools: bool,
    pub computer: bool,
}
impl Default for AgentTools {
    fn default() -> Self {
        Self {
            enabled: true,
            tools: true,
            computer: false,
        }
    }
}
impl AgentTools {
    pub fn load(dir: &Path) -> io::Result<Self> {
        match fs::read(dir.join(FILE)) {
            Ok(data) => Ok(serde_json::from_slice(&data)?),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let mut file = tempfile::NamedTempFile::new_in(dir)?;
        serde_json::to_writer(file.as_file_mut(), self)?;
        file.persist(dir.join(FILE))?;
        Ok(())
    }
    pub fn for_run(dir: Option<&Path>) -> Self {
        let mut settings = dir.map_or_else(Self::default, |dir| {
            Self::load(dir).unwrap_or(Self {
                enabled: false,
                tools: false,
                computer: false,
            })
        });
        settings.tools &= settings.enabled;
        settings.computer &= settings.enabled
            && cfg!(target_os = "macos")
            && !std::env::var("ZERON_COMPUTER_USE")
                .is_ok_and(|v| matches!(v.trim(), "0" | "off" | "false" | "no"));
        settings
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switches_round_trip_and_invalid_preferences_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();
        assert_eq!(
            AgentTools::load(path).unwrap(),
            AgentTools {
                enabled: true,
                tools: true,
                computer: false
            }
        );
        for tools in [false, true] {
            for computer in [false, true] {
                let saved = AgentTools {
                    enabled: true,
                    tools,
                    computer,
                };
                saved.save(path).unwrap();
                assert_eq!(AgentTools::load(path).unwrap(), saved);
            }
        }
        AgentTools {
            enabled: false,
            tools: true,
            computer: true,
        }
        .save(path)
        .unwrap();
        let off = AgentTools::for_run(Some(path));
        assert!(!off.tools && !off.computer);
        fs::write(path.join(FILE), b"{").unwrap();
        assert!(AgentTools::load(path).is_err());
        assert_eq!(
            AgentTools::for_run(Some(path)),
            AgentTools {
                enabled: false,
                tools: false,
                computer: false
            }
        );
    }
}
