//! Device identity and mutual TLS.
//!
//! Every device owns one self-signed certificate. Its hash is the device id.
//! TLS accepts any certificate; trust comes from comparing that id against the
//! paired list after the handshake, so no IP address or hostname is ever trusted.

use std::{path::Path, sync::Arc};

use anyhow::{anyhow, Context, Result};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    DigitallySignedStruct, DistinguishedName, SignatureScheme,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

use crate::Stream;

pub struct Tls {
    pub id: String,
    pub connector: TlsConnector,
    pub acceptor: TlsAcceptor,
}

#[derive(Debug)]
struct AnyCert(Arc<CryptoProvider>);

impl ServerCertVerifier for AnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

impl ClientCertVerifier for AnyCert {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _now: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub fn cert_id(der: &[u8]) -> String {
    hex::encode(blake3::hash(der).as_bytes())
}

impl Tls {
    pub fn load_or_create(dir: &Path) -> Result<Tls> {
        let cert_path = dir.join("device.cert");
        let key_path = dir.join("device.key");
        let (cert, key) = match (std::fs::read(&cert_path), std::fs::read(&key_path)) {
            (Ok(c), Ok(k)) => (c, k),
            _ => {
                let key = rcgen::KeyPair::generate()?;
                let cert = rcgen::CertificateParams::new(vec!["dastbedast".to_string()])?.self_signed(&key)?;
                let (c, k) = (cert.der().to_vec(), key.serialize_der());
                std::fs::write(&cert_path, &c)?;
                std::fs::write(&key_path, &k)?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
                }
                (c, k)
            }
        };
        let id = cert_id(&cert);
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let verifier = Arc::new(AnyCert(provider.clone()));
        let chain = vec![CertificateDer::from(cert)];
        let pkey = || PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.clone()));

        let client = rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()?
            .dangerous()
            .with_custom_certificate_verifier(verifier.clone())
            .with_client_auth_cert(chain.clone(), pkey())
            .context("client tls")?;
        let server = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()?
            .with_client_cert_verifier(verifier)
            .with_single_cert(chain, pkey())
            .context("server tls")?;
        Ok(Tls {
            id,
            connector: TlsConnector::from(Arc::new(client)),
            acceptor: TlsAcceptor::from(Arc::new(server)),
        })
    }
}

/// Id of the device on the other end of an established connection.
pub fn peer_id(stream: &Stream) -> Result<String> {
    let certs = stream.get_ref().1.peer_certificates().ok_or_else(|| anyhow!("no peer certificate"))?;
    let cert = certs.first().ok_or_else(|| anyhow!("no peer certificate"))?;
    Ok(cert_id(cert.as_ref()))
}

pub fn server_name() -> ServerName<'static> {
    ServerName::try_from("dastbedast").expect("static name")
}
