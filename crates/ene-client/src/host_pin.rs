use std::path::Path;

use ene_api::runtime::HostRuntimeInfo;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{CertificateError, ClientConfig, DigitallySignedStruct, Error, SignatureScheme};
use serde::{Deserialize, Serialize};
use sha2::Digest as _;
use subtle::ConstantTimeEq as _;

use crate::error::ClientError;

pub(crate) const HOST_PIN_FILE_NAME: &str = "client-host-pin.json";

fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredHostPin {
    pin: String,
}

pub(crate) fn spki_pin_hex(certificate_der: &[u8]) -> Result<String, ClientError> {
    let (_, certificate) = x509_parser::parse_x509_certificate(certificate_der).map_err(|_| {
        ClientError::Transport(String::from(
            "the Host certificate could not be parsed; refuse the connection",
        ))
    })?;
    let digest = sha2::Sha256::digest(certificate.public_key().raw);
    Ok(hex_lower(digest.as_slice()))
}

fn pins_equal(left: &str, right: &str) -> bool {
    bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

fn verify_owner_only(path: &Path, data_dir: &Path) -> Result<(), ClientError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt as _;

        let metadata = std::fs::metadata(path).map_err(|error| {
            ClientError::Transport(format!("inspect the stored Host pin: {}", error.kind()))
        })?;
        if metadata.mode() & 0o077 != 0 {
            return Err(ClientError::Transport(String::from(
                "the stored Host pin is readable by more than its owner; refuse to trust it",
            )));
        }
        let directory = std::fs::metadata(data_dir).map_err(|error| {
            ClientError::Transport(format!("inspect the data directory: {}", error.kind()))
        })?;
        if metadata.uid() != directory.uid() {
            return Err(ClientError::Transport(String::from(
                "the stored Host pin is not owned with the data directory; refuse to trust it",
            )));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (path, data_dir);
    }
    Ok(())
}

fn read_stored_pin(path: &Path, data_dir: &Path) -> Result<Option<String>, ClientError> {
    match std::fs::read(path) {
        Ok(bytes) => {
            verify_owner_only(path, data_dir)?;
            let stored: StoredHostPin = serde_json::from_slice(&bytes).map_err(|_| {
                ClientError::Transport(String::from(
                    "the stored Host pin is malformed; remove it and re-trust the Host after verifying the offered pin",
                ))
            })?;
            if stored.pin.is_empty() {
                return Err(ClientError::Transport(String::from(
                    "the stored Host pin is empty; remove it and re-trust the Host after verifying the offered pin",
                )));
            }
            Ok(Some(stored.pin))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ClientError::Transport(format!(
            "read the stored Host pin: {}",
            error.kind()
        ))),
    }
}

fn write_pin(path: &Path, pin: &str) -> Result<(), ClientError> {
    let bytes = serde_json::to_vec(&StoredHostPin {
        pin: pin.to_owned(),
    })
    .map_err(|_| ClientError::Transport(String::from("encode the Host pin")))?;
    crate::device::atomic_replace(path, &bytes, Some(0o600), "host pin store failed")
}

/// First trust comes from the protected runtime file; a differing stored pin is
/// a key change that only an owner's explicit confirmation may replace.
pub(crate) fn establish_host_pin(
    data_dir: &Path,
    runtime: &HostRuntimeInfo,
) -> Result<String, ClientError> {
    let path = data_dir.join(HOST_PIN_FILE_NAME);
    match read_stored_pin(&path, data_dir)? {
        Some(stored) => {
            if pins_equal(&stored, &runtime.host_pin) {
                Ok(stored)
            } else {
                Err(ClientError::HostPinMismatch {
                    stored,
                    offered: runtime.host_pin.clone(),
                })
            }
        }
        None => {
            write_pin(&path, &runtime.host_pin)?;
            Ok(runtime.host_pin.clone())
        }
    }
}

pub fn trust_host_pin(data_dir: &Path, confirmed_pin: &str) -> Result<String, ClientError> {
    let runtime = crate::runtime_info::load_host_runtime(data_dir)?;
    if !pins_equal(confirmed_pin, &runtime.host_pin) {
        return Err(ClientError::Transport(String::from(
            "the confirmation does not match the pin the Host currently offers; nothing was replaced",
        )));
    }
    let path = data_dir.join(HOST_PIN_FILE_NAME);
    write_pin(&path, &runtime.host_pin)?;
    Ok(runtime.host_pin.clone())
}

pub(crate) struct PinnedHostVerifier {
    pub(crate) expected_pin: String,
    pub(crate) provider: rustls::crypto::CryptoProvider,
}

impl std::fmt::Debug for PinnedHostVerifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PinnedHostVerifier")
            .field("expected_pin", &self.expected_pin)
            .finish_non_exhaustive()
    }
}

impl ServerCertVerifier for PinnedHostVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, Error> {
        let offered = spki_pin_hex(end_entity.as_ref())
            .map_err(|_| Error::InvalidCertificate(CertificateError::BadEncoding))?;
        if pins_equal(&offered, &self.expected_pin) {
            return Ok(rustls::client::danger::ServerCertVerified::assertion());
        }
        Err(Error::InvalidCertificate(
            CertificateError::ApplicationVerificationFailure,
        ))
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
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
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub(crate) fn pinned_tls_config(pin: &str) -> Result<ClientConfig, Error> {
    let verifier = std::sync::Arc::new(PinnedHostVerifier {
        expected_pin: pin.to_owned(),
        provider: rustls::crypto::aws_lc_rs::default_provider(),
    });
    Ok(ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth())
}
