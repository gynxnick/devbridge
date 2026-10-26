//! rustls configuration. TLS 1.3 only, ring provider, mutual authentication
//! with self-signed certificates.
//!
//! - The server requires a client certificate and verifies the handshake
//!   signature (proof of key possession) but defers the *trust* decision to
//!   the application layer, which compares the fingerprint with the pinned
//!   one (or runs pairing for unknown peers).
//! - The client pins the server's fingerprint when it knows it; during
//!   pairing it accepts any certificate and the PAKE binds the fingerprints.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, WebPkiSupportedAlgorithms};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{ClientConfig, DigitallySignedStruct, DistinguishedName, Error, ServerConfig, SignatureScheme};

use crate::identity::{Identity, fingerprint};

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn algs() -> WebPkiSupportedAlgorithms {
    rustls::crypto::ring::default_provider().signature_verification_algorithms
}

fn key(id: &Identity) -> PrivateKeyDer<'static> {
    PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(id.key_der().to_vec()))
}

#[derive(Debug)]
struct RequireClientCert;

impl ClientCertVerifier for RequireClientCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 is not supported".into()))
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &algs())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        algs().supported_schemes()
    }
    fn client_auth_mandatory(&self) -> bool {
        true
    }
}

#[derive(Debug)]
struct PinnedServer {
    expected: Option<String>,
}

impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        match &self.expected {
            Some(fp) if fingerprint(end_entity) != *fp => {
                Err(Error::General("peer certificate does not match the paired device".into()))
            }
            _ => Ok(ServerCertVerified::assertion()),
        }
    }
    fn verify_tls12_signature(
        &self,
        _: &[u8],
        _: &CertificateDer<'_>,
        _: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        Err(Error::General("TLS 1.2 is not supported".into()))
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &algs())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        algs().supported_schemes()
    }
}

pub fn server_config(id: &Identity) -> Result<Arc<ServerConfig>, Error> {
    let cfg = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(Arc::new(RequireClientCert))
        .with_single_cert(vec![CertificateDer::from(id.cert_der.clone())], key(id))?;
    Ok(Arc::new(cfg))
}

/// `expected`: pinned server fingerprint, or `None` while pairing.
pub fn client_config(id: &Identity, expected: Option<&str>) -> Result<Arc<ClientConfig>, Error> {
    let cfg = ClientConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedServer { expected: expected.map(str::to_owned) }))
        .with_client_auth_cert(vec![CertificateDer::from(id.cert_der.clone())], key(id))?;
    Ok(Arc::new(cfg))
}
