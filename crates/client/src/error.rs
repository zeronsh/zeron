//! The client's error surface — coarse, user-presentable categories. Transport
//! detail rides the message string.

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ClientError {
    /// Unknown chat / project / device / queue row.
    #[error("not found: {0}")]
    NotFound(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
    /// The host device could not be reached over the relay (offline, relay
    /// down, or the call timed out).
    #[error("host unavailable: {0}")]
    HostUnavailable(String),
    /// The host is reachable but does not advertise the needed capability.
    #[error("not supported by the host: {0}")]
    Unsupported(String),
    #[error("network: {0}")]
    Network(String),
    /// Credentials rejected / refresh failed. Irrecoverable failures also
    /// raise [`crate::ClientEvent::AuthExpired`].
    #[error("auth: {0}")]
    Auth(String),
    #[error("storage: {0}")]
    Storage(String),
    /// A live-network path that is not wired yet (phase-2 surface).
    #[error("not implemented yet: {0}")]
    NotImplemented(String),
    /// The client was shut down (signed out) while the call was in flight.
    #[error("client is shut down")]
    Closed,
    #[error("{0}")]
    Internal(String),
}

impl From<zeron_doc::DocError> for ClientError {
    fn from(err: zeron_doc::DocError) -> Self {
        ClientError::Internal(err.to_string())
    }
}

pub type Result<T, E = ClientError> = std::result::Result<T, E>;
