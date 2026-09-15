//! Complete TLS with signature verification, then quarantine the transport until
//! chain/name verification or an explicit certificate decision authorizes NLA.
use crate::{CertificateChallenge, ErrorStage, SessionError};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use tokio::net::TcpStream;
use tokio_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::{
        self,
        client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
        pki_types::{CertificateDer, ServerName, UnixTime},
    },
};
use x509_cert::der::Decode;

#[derive(Debug)]
struct Verifier {
    standard: Option<Arc<rustls::client::WebPkiServerVerifier>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
    pin: Option<String>,
    issue: Arc<Mutex<Option<(String, String)>>>,
}
impl ServerCertVerifier for Verifier {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        name: &ServerName<'_>,
        ocsp: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fingerprint = format!("{:x}", Sha256::digest(end.as_ref()));
        let validation = self
            .standard
            .as_ref()
            .map(|v| v.verify_server_cert(end, intermediates, name, ocsp, now));
        let issue = match &self.pin {
            Some(pin) if pin.eq_ignore_ascii_case(&fingerprint) => None,
            Some(_) => Some("The server certificate has changed since you trusted it".to_string()),
            None => match validation {
                Some(Ok(_)) => None,
                Some(Err(e)) => Some(e.to_string()),
                None => Some("No operating system certificate roots are available".into()),
            },
        };
        *self.issue.lock().expect("certificate mutex") = issue.map(|reason| (fingerprint, reason));
        // This only permits the TLS handshake. upgrade returns the challenge and
        // the caller MUST wait for its answer before invoking connect_finalize.
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub(crate) async fn upgrade(
    stream: TcpStream,
    host: &str,
    port: u16,
    pin: Option<String>,
) -> Result<(TlsStream<TcpStream>, Vec<u8>, Option<CertificateChallenge>), SessionError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    for certificate in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(certificate);
    }
    let standard = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        provider.clone(),
    )
    .build()
    .ok();
    let issue = Arc::new(Mutex::new(None));
    let verifier = Verifier {
        standard,
        provider: provider.clone(),
        pin: pin.clone(),
        issue: issue.clone(),
    };
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(tls_error)?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth();
    config.resumption = rustls::client::Resumption::disabled();
    let name = ServerName::try_from(host.to_string()).map_err(tls_error)?;
    let stream = TlsConnector::from(Arc::new(config))
        .connect(name, stream)
        .await
        .map_err(tls_error)?;
    let der = stream
        .get_ref()
        .1
        .peer_certificates()
        .and_then(|certs| certs.first())
        .ok_or_else(|| tls_error("Server certificate missing"))?;
    let certificate = x509_cert::Certificate::from_der(der).map_err(tls_error)?;
    let key = certificate
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .as_bytes()
        .ok_or_else(|| tls_error("Invalid server public key"))?
        .to_vec();
    let challenge = issue
        .lock()
        .expect("certificate mutex")
        .take()
        .map(|(sha256, reason)| CertificateChallenge {
            endpoint: if host.contains(':') {
                format!("[{host}]:{port}")
            } else {
                format!("{host}:{port}")
            },
            sha256,
            reason,
            previous_sha256: pin,
        });
    Ok((stream, key, challenge))
}
fn tls_error(error: impl std::fmt::Display) -> SessionError {
    SessionError::new(ErrorStage::Certificate, error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn certificate_pins_still_require_handshake_signatures_and_changed_pins_prompt() {
        use rustls::internal::msgs::codec::{Codec, Reader};
        let rcgen::CertifiedKey { cert, .. } =
            rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
        let fingerprint = format!("{:x}", Sha256::digest(cert.der()));
        let issue = Arc::new(Mutex::new(None));
        let mut verifier = Verifier {
            standard: None,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
            pin: Some(fingerprint),
            issue: issue.clone(),
        };
        let name = ServerName::try_from("localhost").unwrap();
        verifier
            .verify_server_cert(cert.der(), &[], &name, &[], UnixTime::now())
            .unwrap();
        assert!(issue.lock().unwrap().is_none());
        // ECDSA_NISTP256_SHA256 with an invalid one-byte signature.
        let invalid =
            rustls::DigitallySignedStruct::read(&mut Reader::init(&[4, 3, 0, 1, 0])).unwrap();
        assert!(
            verifier
                .verify_tls12_signature(b"handshake", cert.der(), &invalid)
                .is_err()
        );
        assert!(
            verifier
                .verify_tls13_signature(b"handshake", cert.der(), &invalid)
                .is_err()
        );
        verifier.pin = Some("00".repeat(32));
        verifier
            .verify_server_cert(cert.der(), &[], &name, &[], UnixTime::now())
            .unwrap();
        assert!(
            issue
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .1
                .contains("changed")
        );
        verifier.pin = None;
        verifier
            .verify_server_cert(cert.der(), &[], &name, &[], UnixTime::now())
            .unwrap();
        assert!(issue.lock().unwrap().is_some());
    }
}
