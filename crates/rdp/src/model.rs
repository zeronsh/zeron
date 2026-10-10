use std::{fmt, sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

pub const MAX_DIMENSION: u16 = 3840;
pub const MAX_PIXELS: usize = 3840 * 2160;
pub const MAX_CLIPBOARD_BYTES: usize = 1024 * 1024;
pub const COMMAND_CAPACITY: usize = 256;
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

pub fn validate_size(width: u16, height: u16) -> Result<usize, SessionError> {
    let pixels = usize::from(width).checked_mul(usize::from(height));
    match pixels {
        Some(n)
            if width >= 200
                && height >= 200
                && width <= MAX_DIMENSION
                && height <= MAX_DIMENSION
                && n <= MAX_PIXELS =>
        {
            Ok(n * 4)
        }
        _ => Err(SessionError::new(
            ErrorStage::Protocol,
            "Desktop size must be at least 200×200 and at most 3840 pixels per side / 8,294,400 pixels",
        )),
    }
}

/// Deliberately neither serializable nor printable.
pub struct Password(Zeroizing<String>);
impl Password {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}
impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password([redacted])")
    }
}

pub struct ConnectConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub keyboard_layout: u32,
    pub domain: Option<String>,
    pub password: Password,
    pub width: u16,
    pub height: u16,
    pub trusted_certificate_sha256: Option<String>,
    pub timeout: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorStage {
    Dns,
    Network,
    Certificate,
    Authentication,
    Protocol,
    Session,
    Input,
    Clipboard,
}
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{stage:?}: {message}")]
pub struct SessionError {
    pub stage: ErrorStage,
    pub message: String,
}
impl SessionError {
    pub fn new(stage: ErrorStage, message: impl Into<String>) -> Self {
        Self {
            stage,
            message: message.into(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Connecting,
    AwaitingCertificateDecision,
    Authenticating,
    Connected,
    Closing,
    Disconnected,
    Failed(SessionError),
}
impl SessionState {
    pub fn is_running(&self) -> bool {
        matches!(
            self,
            Self::Connecting
                | Self::AwaitingCertificateDecision
                | Self::Authenticating
                | Self::Connected
                | Self::Closing
        )
    }
}
#[derive(Clone, Debug)]
pub struct CertificateChallenge {
    pub endpoint: String,
    pub sha256: String,
    pub reason: String,
    pub previous_sha256: Option<String>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CertificateDecision {
    Reject,
    TrustOnce,
    SavePin,
}
#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    pub resize: bool,
    pub clipboard_text: bool,
}

/// Top-down, tightly packed BGRA8 with opaque alpha; never encoded for transport to UI.
#[derive(Clone)]
pub struct Frame {
    pub generation: u64,
    pub sequence: u64,
    pub width: u16,
    pub height: u16,
    pub bgra: Arc<[u8]>,
}
#[derive(Clone, Default)]
pub enum RemoteCursor {
    #[default]
    Default,
    Hidden,
    Bitmap {
        width: u16,
        height: u16,
        hotspot_x: u16,
        hotspot_y: u16,
        rgba: Arc<[u8]>,
    },
}
#[derive(Clone)]
pub struct Snapshot {
    pub generation: u64,
    pub state: SessionState,
    pub certificate: Option<CertificateChallenge>,
    pub accepted_pin: Option<String>,
    pub capabilities: Capabilities,
    pub frame: Option<Arc<Frame>>,
    pub cursor: RemoteCursor,
    pub reactivating: bool,
    pub clipboard: Option<(u64, Result<Arc<str>, SessionError>)>,
}
impl Snapshot {
    pub fn new(generation: u64) -> Self {
        Self {
            generation,
            state: SessionState::Idle,
            certificate: None,
            accepted_pin: None,
            capabilities: Capabilities::default(),
            frame: None,
            cursor: RemoteCursor::Default,
            reactivating: false,
            clipboard: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
    Back,
    Forward,
}
#[derive(Clone, Debug)]
pub enum InputEvent {
    ScanCode {
        code: u16,
        down: bool,
    },
    Text(String),
    Button {
        button: PointerButton,
        down: bool,
        x: u16,
        y: u16,
    },
    Wheel {
        horizontal: bool,
        amount: i16,
    },
}
pub enum Command {
    Certificate(CertificateDecision),
    Input(InputEvent),
    ReleaseAll,
    CtrlAltDelete,
    SendClipboard(String),
    RequestClipboard(u64),
}
#[derive(Clone, Debug)]
pub struct Presentation {
    pub visible: bool,
    pub pointer: Option<(u16, u16)>,
    pub resize: Option<(u16, u16)>,
}
impl Default for Presentation {
    fn default() -> Self {
        Self {
            visible: true,
            pointer: None,
            resize: None,
        }
    }
}

/// The sole session owner. Dropping it cancels even a stalled handshake. Pointer
/// motion and geometry replace previous values, whereas key/button edges retain order.
pub struct SessionHandle {
    pub generation: u64,
    commands: mpsc::Sender<Command>,
    presentation: watch::Sender<Presentation>,
    cancellation: CancellationToken,
    pub snapshots: watch::Receiver<Snapshot>,
}
pub struct SessionChannels {
    pub commands: mpsc::Receiver<Command>,
    pub presentation: watch::Receiver<Presentation>,
    pub cancellation: CancellationToken,
    pub snapshots: watch::Sender<Snapshot>,
}
impl SessionHandle {
    pub fn channel(generation: u64) -> (Self, SessionChannels) {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let (presentation, control) = watch::channel(Presentation::default());
        let (snapshots, updates) = watch::channel(Snapshot::new(generation));
        let cancellation = CancellationToken::new();
        (
            Self {
                generation,
                commands,
                presentation,
                cancellation: cancellation.clone(),
                snapshots: updates,
            },
            SessionChannels {
                commands: receiver,
                presentation: control,
                cancellation,
                snapshots,
            },
        )
    }
    pub fn send(&self, command: Command) -> Result<(), SessionError> {
        match &command {
            Command::Input(InputEvent::Text(text)) if text.len() > 16 * 1024 => {
                return Err(SessionError::new(
                    ErrorStage::Input,
                    "Text input exceeds 16 KiB; use Send local clipboard for longer text",
                ));
            }
            Command::SendClipboard(text) if text.len() > MAX_CLIPBOARD_BYTES => {
                return Err(SessionError::new(
                    ErrorStage::Clipboard,
                    "Clipboard text exceeds 1 MiB",
                ));
            }
            _ => {}
        }
        self.commands.try_send(command).map_err(|_| {
            // A lost release could leave the server holding a key. Terminate the
            // transport on congestion; the task performs releases during shutdown.
            self.cancellation.cancel();
            SessionError::new(
                ErrorStage::Input,
                "Input queue unavailable; connection stopped to release held keys",
            )
        })
    }
    pub fn set_visible(&self, visible: bool) {
        self.presentation.send_if_modified(|p| {
            if p.visible == visible {
                return false;
            }
            p.visible = visible;
            true
        });
    }
    pub fn move_pointer(&self, x: u16, y: u16) {
        self.presentation.send_if_modified(|p| {
            let changed = p.pointer != Some((x, y));
            p.pointer = Some((x, y));
            changed
        });
    }
    pub fn resize(&self, width: u16, height: u16) -> Result<(), SessionError> {
        validate_size(width, height)?;
        self.presentation.send_if_modified(|p| {
            let changed = p.resize != Some((width, height));
            p.resize = Some((width, height));
            changed
        });
        Ok(())
    }
    pub fn clear_resize(&self) {
        self.presentation
            .send_if_modified(|p| p.resize.take().is_some());
    }
    pub fn disconnect(&self) {
        self.cancellation.cancel();
    }
}
impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn dropping_owner_cancels_even_with_observers() {
        let (handle, channels) = SessionHandle::channel(2);
        let _observer = handle.snapshots.clone();
        drop(handle);
        tokio::time::timeout(Duration::from_millis(50), channels.cancellation.cancelled())
            .await
            .unwrap();
    }
    #[test]
    fn saturated_input_stops_instead_of_silently_losing_release() {
        let (handle, channels) = SessionHandle::channel(1);
        for _ in 0..COMMAND_CAPACITY {
            handle
                .send(Command::Input(InputEvent::ScanCode {
                    code: 29,
                    down: true,
                }))
                .unwrap();
        }
        assert!(handle.send(Command::ReleaseAll).is_err());
        assert!(channels.cancellation.is_cancelled());
    }
    #[test]
    fn presentation_is_coalesced_and_size_checked() {
        let (handle, channels) = SessionHandle::channel(1);
        for n in 0..1000 {
            handle.move_pointer(n, n);
        }
        assert_eq!(channels.presentation.borrow().pointer, Some((999, 999)));
        assert!(handle.resize(3840, 3840).is_err());
        assert!(handle.resize(2160, 3840).is_ok());
        assert!(handle.resize(0, 0).is_err());
    }
}
