use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use zeron_rdp::*;
fn config(port: u16) -> ConnectConfig {
    ConnectConfig {
        host: "127.0.0.1".into(),
        port,
        username: "test-user".into(),
        keyboard_layout: 0x0409,
        domain: None,
        password: Password::new("test-secret".into()),
        width: 800,
        height: 600,
        trusted_certificate_sha256: None,
        timeout: Duration::from_millis(200),
    }
}
async fn terminal(handle: &mut SessionHandle) -> SessionState {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = handle.snapshots.borrow_and_update().state.clone();
            if matches!(state, SessionState::Failed(_) | SessionState::Disconnected) {
                return state;
            }
            handle.snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn stalled_peer_times_out_and_socket_closes() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut handle = connect(config(listener.local_addr().unwrap().port()), 1).unwrap();
    let (mut peer, _) = listener.accept().await.unwrap();
    assert!(matches!(
        terminal(&mut handle).await,
        SessionState::Failed(SessionError {
            stage: ErrorStage::Protocol,
            ..
        })
    ));
    let mut bytes = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    assert!(!bytes.is_empty());
    assert!(!String::from_utf8_lossy(&bytes).contains("test-secret"));
}
#[tokio::test]
async fn cancel_during_negotiation_drops_socket_and_reconnect_has_new_generation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let mut handle = connect(config(port), 41).unwrap();
    let (mut peer, _) = listener.accept().await.unwrap();
    handle.disconnect();
    assert_eq!(terminal(&mut handle).await, SessionState::Disconnected);
    let mut bytes = Vec::new();
    peer.read_to_end(&mut bytes).await.unwrap();
    let mut next = connect(config(port), 42).unwrap();
    let (_peer, _) = listener.accept().await.unwrap();
    assert_eq!(next.snapshots.borrow().generation, 42);
    next.disconnect();
    assert_eq!(terminal(&mut next).await, SessionState::Disconnected);
    assert_eq!(handle.snapshots.borrow().generation, 41);
}

#[tokio::test]
async fn untrusted_tls_waits_for_decision_before_credentials_and_rejection_closes_it() {
    use std::sync::Arc;
    use tokio_rustls::{
        TlsAcceptor,
        rustls::{self, pki_types::PrivatePkcs8KeyDer},
    };
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let server = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        PrivatePkcs8KeyDer::from(signing_key.serialize_der()).into(),
    )
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut cfg = config(listener.local_addr().unwrap().port());
    cfg.timeout = Duration::from_secs(5);
    let mut handle = connect(cfg, 5).unwrap();
    let (mut peer, _) = listener.accept().await.unwrap();
    let mut header = [0; 4];
    peer.read_exact(&mut header).await.unwrap();
    let len = u16::from_be_bytes([header[2], header[3]]) as usize;
    let mut request = vec![0; len - 4];
    peer.read_exact(&mut request).await.unwrap();
    // X.224 Connection Confirm selecting HYBRID (TLS + CredSSP).
    peer.write_all(&[3, 0, 0, 19, 14, 0xd0, 0, 0, 0, 0, 0, 2, 0, 8, 0, 2, 0, 0, 0])
        .await
        .unwrap();
    let mut tls = TlsAcceptor::from(Arc::new(server))
        .accept(peer)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if handle.snapshots.borrow_and_update().state
                == SessionState::AwaitingCertificateDecision
            {
                break;
            }
            handle.snapshots.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    assert!(handle.snapshots.borrow().certificate.is_some());
    let mut byte = [0];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), tls.read(&mut byte))
            .await
            .is_err(),
        "Credentials were sent without certificate authorization"
    );
    handle
        .send(Command::Certificate(CertificateDecision::Reject))
        .unwrap();
    assert!(matches!(
        terminal(&mut handle).await,
        SessionState::Failed(SessionError {
            stage: ErrorStage::Certificate,
            ..
        })
    ));
    let _ = tokio::time::timeout(Duration::from_secs(1), tls.read(&mut byte))
        .await
        .unwrap();
}

#[tokio::test]
async fn cancellation_during_a_stalled_tls_handshake_closes_the_peer() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut cfg = config(listener.local_addr().unwrap().port());
    cfg.timeout = Duration::from_secs(20);
    let mut handle = connect(cfg, 99).unwrap();
    let (mut peer, _) = listener.accept().await.unwrap();
    let mut header = [0; 4];
    peer.read_exact(&mut header).await.unwrap();
    let len = u16::from_be_bytes([header[2], header[3]]) as usize;
    peer.read_exact(&mut vec![0; len - 4]).await.unwrap();
    peer.write_all(&[3, 0, 0, 19, 14, 0xd0, 0, 0, 0, 0, 0, 2, 0, 8, 0, 2, 0, 0, 0])
        .await
        .unwrap();
    let mut hello = [0; 5];
    peer.read_exact(&mut hello).await.unwrap();
    assert_eq!(hello[0], 22);
    handle.disconnect();
    assert_eq!(terminal(&mut handle).await, SessionState::Disconnected);
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(1), peer.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
}
