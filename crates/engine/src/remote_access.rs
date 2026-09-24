//! Remote exposure is explicit. Conflicting or invalid configuration stays local.
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::remote_auth::RemoteAuthorizer;

const FILE: &str = "remote-access.json";
pub const DEFAULT_REMOTE_PORT: u16 = 27655;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub struct RemoteAccessSettings {
    pub enabled: bool,
    pub bind_address: SocketAddr,
}

impl Default for RemoteAccessSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), DEFAULT_REMOTE_PORT),
        }
    }
}

/// Startup overrides for the saved settings. The desktop edits the settings
/// file; a headless engine adds `--network`/`ZERON_NETWORK` and an optional
/// bind address on top.
#[derive(Debug, Clone, Default)]
pub struct NetworkOptions {
    pub flag: Option<bool>,
    pub environment: Option<bool>,
    pub invalid_environment: bool,
    pub bind_address: Option<SocketAddr>,
}

impl NetworkOptions {
    /// `flag`/`bind_address` come from CLI parsing; the environment variable
    /// is read here. Conflicting values disable remote access rather than
    /// silently guessing which one wins.
    pub fn from_environment(flag: Option<bool>, bind_address: Option<SocketAddr>) -> Self {
        let raw = std::env::var("ZERON_NETWORK").ok();
        let environment =
            raw.as_deref()
                .and_then(|value| match value.trim().to_ascii_lowercase().as_str() {
                    "1" | "true" => Some(true),
                    "0" | "false" => Some(false),
                    _ => None,
                });
        Self {
            flag,
            environment,
            invalid_environment: raw.is_some() && environment.is_none(),
            bind_address,
        }
    }
}

pub struct LoadedSettings {
    pub settings: RemoteAccessSettings,
    pub stored: bool,
    pub error: Option<String>,
}

pub fn load(directory: &Path) -> LoadedSettings {
    match std::fs::read(directory.join(FILE)) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(settings) => LoadedSettings {
                settings,
                stored: true,
                error: None,
            },
            Err(error) => LoadedSettings {
                settings: Default::default(),
                stored: true,
                error: Some(format!("Remote access settings are invalid: {error}")),
            },
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LoadedSettings {
            settings: Default::default(),
            stored: false,
            error: None,
        },
        Err(error) => LoadedSettings {
            settings: Default::default(),
            stored: true,
            error: Some(format!("Remote access settings could not be read: {error}")),
        },
    }
}

pub fn save(directory: &Path, settings: &RemoteAccessSettings) -> anyhow::Result<()> {
    std::fs::create_dir_all(directory)?;
    let temporary = directory.join(format!("remote-access-{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(settings)?)?;
    if let Err(error) = std::fs::rename(&temporary, directory.join(FILE)) {
        let _ = std::fs::remove_file(temporary);
        return Err(error.into());
    }
    Ok(())
}

/// The resolved state of remote access after merging the settings file with
/// the startup overrides.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAccessStatus {
    pub enabled: bool,
    pub configured_enabled: bool,
    pub address: Option<SocketAddr>,
    pub source: String,
    pub error: Option<String>,
}

pub fn resolve(loaded: &LoadedSettings, options: &NetworkOptions) -> RemoteAccessStatus {
    let values: Vec<_> = [
        loaded.stored.then_some(loaded.settings.enabled),
        options.flag,
        options.environment,
    ]
    .into_iter()
    .flatten()
    .collect();
    let conflict = values.windows(2).any(|pair| pair[0] != pair[1]);
    let error = loaded
        .error
        .clone()
        .or_else(|| {
            options
                .invalid_environment
                .then(|| "ZERON_NETWORK must be true, false, 1, or 0".to_owned())
        })
        .or_else(|| {
            conflict.then(|| {
                "Remote access settings and startup override disagree; remote access is off"
                    .to_owned()
            })
        });
    let source = if options.flag.is_some() {
        "--network"
    } else if options.environment.is_some() || options.invalid_environment {
        "ZERON_NETWORK"
    } else if loaded.stored {
        FILE
    } else {
        "default"
    };
    RemoteAccessStatus {
        enabled: error.is_none() && values.last().copied().unwrap_or(false),
        configured_enabled: loaded.settings.enabled,
        address: None,
        source: source.into(),
        error,
    }
}

/// Applies the remote-access settings file and startup overrides to the
/// engine's remote listener: binds or unbinds as configuration changes, owns
/// the listener task, and stops it on shutdown.
pub struct RemoteAccessController {
    directory: PathBuf,
    service: Mutex<Option<Arc<dyn zeron_rpc::RpcService>>>,
    authorizer: Mutex<Option<Arc<RemoteAuthorizer>>>,
    state: tokio::sync::Mutex<ControlState>,
}

struct ControlState {
    options: NetworkOptions,
    status: RemoteAccessStatus,
    listener: Option<crate::EngineListener>,
}

impl RemoteAccessController {
    pub fn new(directory: &Path) -> Arc<Self> {
        let status = resolve(&load(directory), &NetworkOptions::default());
        Arc::new(Self {
            directory: directory.into(),
            service: Mutex::new(None),
            authorizer: Mutex::new(None),
            state: tokio::sync::Mutex::new(ControlState {
                options: Default::default(),
                status,
                listener: None,
            }),
        })
    }

    pub async fn initialize(
        &self,
        service: Arc<dyn zeron_rpc::RpcService>,
        options: NetworkOptions,
        authorizer: Arc<RemoteAuthorizer>,
    ) {
        *self.service.lock().unwrap() = Some(service);
        *self.authorizer.lock().unwrap() = Some(authorizer);
        let mut state = self.state.lock().await;
        state.options = options;
        self.apply(&mut state).await;
    }

    async fn apply(&self, state: &mut ControlState) {
        let loaded = load(&self.directory);
        state.status = resolve(&loaded, &state.options);
        if let Some(mut listener) = state.listener.take() {
            listener.stop().await;
        }
        if state.status.enabled {
            let service = self.service.lock().unwrap().clone();
            let authorizer = self.authorizer.lock().unwrap().clone();
            if let (Some(service), Some(authorizer)) = (service, authorizer) {
                let address = state
                    .options
                    .bind_address
                    .unwrap_or(loaded.settings.bind_address);
                match crate::serve_engine_remote(address, service, authorizer).await {
                    Ok(listener) => {
                        state.status.address = Some(listener.address);
                        state.listener = Some(listener);
                    }
                    Err(error) => {
                        state.status.enabled = false;
                        state.status.error =
                            Some(format!("Remote listener could not start: {error}"));
                    }
                }
            } else {
                state.status.enabled = false;
                state.status.error = Some("Engine is still starting".into());
            }
        }
        tracing::info!(source = %state.status.source, enabled = state.status.enabled, address = ?state.status.address, "remote access configuration applied");
        if let Some(error) = &state.status.error {
            tracing::warn!(message = %error, "remote access remains local-only");
        }
    }

    /// Persist the enabled flag and re-apply: the listener binds or unbinds
    /// without an engine restart.
    pub async fn set_enabled(&self, enabled: bool) -> anyhow::Result<()> {
        let mut state = self.state.lock().await;
        let mut settings = load(&self.directory).settings;
        settings.enabled = enabled;
        save(&self.directory, &settings)?;
        self.apply(&mut state).await;
        Ok(())
    }

    pub async fn shutdown(&self) {
        let mut state = self.state.lock().await;
        if let Some(mut listener) = state.listener.take() {
            listener.stop().await;
        }
        state.status.enabled = false;
        state.status.address = None;
        self.service.lock().unwrap().take();
        self.authorizer.lock().unwrap().take();
    }
}
