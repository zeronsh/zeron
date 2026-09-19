use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeRole {
    Client,
    Server,
}

impl std::fmt::Display for NodeRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Client => "client",
            Self::Server => "server",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Node {
    pub device_id: String,
    pub name: String,
    pub role: NodeRole,
    pub paired_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invitation {
    pub code: String,
    pub expires_at: i64,
    pub hub_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceInfo {
    pub protocol_version: u32,
    pub workspace_id: String,
    pub name: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrivateConfig {
    pub workspace_id: String,
    pub user_id: String,
    pub device_id: String,
    pub name: String,
    pub hub_url: String,
    pub role: NodeRole,
    pub host_hub: bool,
    pub enabled: bool,
    pub token: String,
    pub listen_port: u16,
    pub serve_port: u16,
}

impl std::fmt::Debug for PrivateConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrivateConfig")
            .field("workspace_id", &self.workspace_id)
            .field("device_id", &self.device_id)
            .field("hub_url", &self.hub_url)
            .field("role", &self.role)
            .field("host_hub", &self.host_hub)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl PrivateConfig {
    pub fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("private.json")
    }

    pub fn load(data_dir: &Path) -> Result<Option<Self>> {
        let bytes = match std::fs::read(Self::path(data_dir)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let config: Self =
            serde_json::from_slice(&bytes).context("Invalid private workspace configuration")?;
        config.validate()?;
        Ok(Some(config))
    }

    pub fn save(&self, data_dir: &Path) -> Result<()> {
        self.validate()?;
        std::fs::create_dir_all(data_dir)?;
        let path = Self::path(data_dir);
        let temporary = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4().simple()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        Ok(())
    }

    pub fn remove(data_dir: &Path) -> Result<()> {
        match std::fs::remove_file(Self::path(data_dir)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    pub fn create(
        data_dir: &Path,
        name: &str,
        device_id: &str,
        node_name: &str,
        role: NodeRole,
        hub_url: &str,
    ) -> Result<Self> {
        ensure!(
            Self::load(data_dir)?.is_none(),
            "A private workspace is already configured"
        );
        valid_name(name)?;
        valid_name(node_name)?;
        let workspace_id = uuid::Uuid::new_v4().to_string();
        let config = Self {
            user_id: format!("private-{workspace_id}"),
            workspace_id,
            device_id: device_id.into(),
            name: name.into(),
            hub_url: normalize_url(hub_url)?,
            role,
            host_hub: true,
            enabled: true,
            token: crate::new_token(),
            listen_port: 27655,
            serve_port: 8443,
        };
        config.validate()?;
        let mut store = crate::store::Store::open(data_dir, &config)?;
        store.enroll(
            &Node {
                device_id: device_id.into(),
                name: node_name.into(),
                role,
                paired_at: crate::now_ms(),
            },
            &config.token,
        )?;
        config.save(data_dir)?;
        Ok(config)
    }

    pub async fn join(
        data_dir: &Path,
        hub_url: &str,
        code: &str,
        name: &str,
        device_id: &str,
        role: NodeRole,
    ) -> Result<Self> {
        ensure!(
            Self::load(data_dir)?.is_none(),
            "A private workspace is already configured"
        );
        valid_name(name)?;
        ensure!(valid_id(device_id), "Invalid device identity");
        let hub_url = normalize_url(hub_url)?;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let info: WorkspaceInfo = client
            .get(format!("{hub_url}/private/info"))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        ensure!(
            info.protocol_version == 1,
            "Unsupported private hub protocol"
        );
        let response = client
            .post(format!("{hub_url}/private/pair"))
            .json(&PairRequest {
                code: code.into(),
                name: name.into(),
                device_id: device_id.into(),
                role,
            })
            .send()
            .await?;
        ensure!(
            response.status().is_success(),
            "Pairing rejected: check the invitation code, expiry, and assigned role"
        );
        let paired: PairResponse = response.json().await?;
        ensure!(
            paired.workspace_id == info.workspace_id && paired.device_id == device_id,
            "Hub returned a different workspace or device identity"
        );
        let config = Self {
            workspace_id: paired.workspace_id,
            user_id: paired.user_id,
            device_id: paired.device_id,
            name: info.name,
            hub_url,
            role,
            host_hub: false,
            enabled: true,
            token: paired.token,
            listen_port: 27655,
            serve_port: 8443,
        };
        config.save(data_dir)?;
        Ok(config)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            valid_id(&self.workspace_id) && valid_id(&self.device_id),
            "Invalid private workspace or device identity"
        );
        valid_name(&self.name)?;
        normalize_url(&self.hub_url)?;
        ensure!(
            self.token.len() >= 32 && self.token.len() <= 256,
            "Invalid private credential"
        );
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairRequest {
    pub code: String,
    pub name: String,
    pub device_id: String,
    pub role: NodeRole,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PairResponse {
    pub workspace_id: String,
    pub user_id: String,
    pub device_id: String,
    pub token: String,
}

pub(crate) fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

pub(crate) fn valid_name(name: &str) -> Result<()> {
    ensure!(
        !name.trim().is_empty() && name.len() <= 200 && !name.chars().any(char::is_control),
        "Name must contain 1–200 characters without control characters"
    );
    Ok(())
}

pub(crate) fn normalize_url(value: &str) -> Result<String> {
    let url = url::Url::parse(value).context("Invalid hub URL")?;
    let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && loopback) {
        bail!("The hub URL must use HTTPS (HTTP is allowed only for loopback testing)");
    }
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && url.path() == "/",
        "Hub URL must contain only a scheme, hostname, and optional port"
    );
    Ok(url.as_str().trim_end_matches('/').to_string())
}
