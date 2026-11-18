//! The server the app signs in to (zeron_client::server): Zeron's own, or a
//! self-hosted one found by its address (docs/self-hosting.md).
//!
//! The platform stores the server it signed in to beside the credentials
//! (`url`, `workos_client_id`, `name`) and rebuilds the rest with
//! [`server_info`]; copy and checks live here so both phones say the same.

use zeron_client as zc;
use zc::server::Server;

/// A server an app can sign in to, with what the UI shows of it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ServerInfo {
    /// Origin, `https://host[:port]`: the `edge_url` of everything else.
    pub url: String,
    /// The WorkOS client its sign-in goes through (`workos_authorize_url`).
    pub workos_client_id: String,
    /// What its operator calls it, when they named it.
    pub name: Option<String>,
    /// `host[:port]`.
    pub host: String,
    /// The name, else the host: what to call it in a sentence.
    pub label: String,
    /// Zeron's own server.
    pub is_official: bool,
}

impl From<Server> for ServerInfo {
    fn from(server: Server) -> Self {
        Self {
            host: server.host(),
            label: server.label(),
            is_official: server.is_official(),
            url: server.url,
            workos_client_id: server.workos_client_id,
            name: server.name,
        }
    }
}

/// Why the server at an address can't be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, uniffi::Error)]
pub enum ServerLookupError {
    /// `message` is the sentence to show; `code` names the case
    /// (zeron_proto::server::ServerError::code) for UIs that branch on it.
    #[error("{message}")]
    Failed { code: String, message: String },
}

/// Zeron's own server.
#[uniffi::export]
pub fn server_official() -> ServerInfo {
    Server::official().into()
}

/// A stored server, described again (its host, label, whether it's Zeron's).
#[uniffi::export]
pub fn server_info(url: String, workos_client_id: String, name: Option<String>) -> ServerInfo {
    Server {
        url,
        workos_client_id,
        name,
    }
    .into()
}

/// The server at `address` as someone typed it (`zeron.example.com`,
/// `http://192.168.1.20:8787`), read from its discovery document.
#[uniffi::export]
pub async fn server_discover(address: String) -> Result<ServerInfo, ServerLookupError> {
    zc::server::discover(&address)
        .await
        .map(Into::into)
        .map_err(|failure| ServerLookupError::Failed {
            code: failure.error.code().to_owned(),
            message: failure.message(),
        })
}

/// How to run a server of one's own (the "learn more" link).
#[uniffi::export]
pub fn server_self_hosting_guide() -> String {
    zc::server::SELF_HOSTING_GUIDE.to_owned()
}
