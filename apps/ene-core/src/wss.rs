use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ene_api::codec::MAX_FRAME_BYTES;
use ene_api::runtime::{HOST_RUNTIME_FILE_NAME, HostRuntimeInfo};
use ene_credential::{
    CredentialRef, CredentialStore, CredentialTechnicalError, VersionedCredentialStore as _,
};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::Digest as _;
use subtle::ConstantTimeEq as _;
use tokio::net::TcpListener;
use tokio::sync::OwnedSemaphorePermit;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use uuid::Uuid;

use crate::serve::{CoreError, CredStore};

pub(crate) const MAX_CONNECTIONS: usize = 64;
pub(crate) const UPGRADE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
pub(crate) const AUTH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);
pub(crate) const PING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(15);
pub(crate) const SUSPECT_AFTER: std::time::Duration = std::time::Duration::from_secs(30);
pub(crate) const LIVENESS_LIMIT: std::time::Duration = std::time::Duration::from_secs(90);
pub(crate) const MONITOR_TICK: std::time::Duration = std::time::Duration::from_secs(5);
// IPC §10.2 bounds the write wait; a peer that stops reading must not own a
// connection task indefinitely, and a write past the bound ends the
// connection because the sink may be left mid-frame.
pub(crate) const WRITE_WAIT: std::time::Duration = std::time::Duration::from_secs(30);
// IPC §10.2 bounds the device-authentication wait; the design fixes the
// limit, not the number, and IPC §9.3 keeps a pending pairing bound to its
// originating connection's lifetime.
pub(crate) const OWNER_CONFIRMATION_LIMIT: std::time::Duration =
    std::time::Duration::from_secs(600);
// IPC §10.2 bounds the number of pending pairings; the value is chosen here.
pub(crate) const MAX_PENDING_PAIRINGS: usize = 8;

const HOST_TLS_CRED_PROVIDER: &str = "host";
const HOST_TLS_CRED_LABEL: &str = "tls-key";
const HOST_TLS_KEY_VERSION: u64 = 1;
const STARTUP_GENERATION_HEADER: &str = "x-ene-startup-generation";

static STAGE_SEQ: AtomicU64 = AtomicU64::new(0);

pub(crate) type HostWebSocket =
    tokio_tungstenite::WebSocketStream<tokio_rustls::server::TlsStream<tokio::net::TcpStream>>;
pub(crate) type HostSink =
    futures_util::stream::SplitSink<HostWebSocket, tokio_tungstenite::tungstenite::Message>;
pub(crate) type HostStream = futures_util::stream::SplitStream<HostWebSocket>;

pub(crate) fn ws_config() -> WebSocketConfig {
    WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(16 * 1024)
        .max_write_buffer_size(MAX_FRAME_BYTES)
        .max_message_size(Some(MAX_FRAME_BYTES))
        .max_frame_size(Some(MAX_FRAME_BYTES))
}

pub(crate) fn hex_lower(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn spki_pin(certificate_der: &[u8]) -> Result<String, CoreError> {
    let (_, certificate) =
        x509_parser::parse_x509_certificate(certificate_der).map_err(|error| {
            CoreError::RuntimeInfo(format!(
                "parse the freshly issued Host certificate: {error}"
            ))
        })?;
    let digest = sha2::Sha256::digest(certificate.public_key().raw);
    Ok(hex_lower(digest.as_slice()))
}

fn generate_key_pem() -> Result<String, CoreError> {
    rcgen::KeyPair::generate()
        .map(|key| key.serialize_pem())
        .map_err(|error| CoreError::ProtectedStore(format!("generate a Host key: {error}")))
}

fn read_or_put_host_key(
    read: impl Fn() -> Result<String, CredentialTechnicalError>,
    put: impl Fn(&str) -> Result<(), CredentialTechnicalError>,
) -> Result<String, CoreError> {
    if let Ok(existing) = read() {
        return Ok(existing);
    }
    let generated = generate_key_pem()?;
    match put(&generated) {
        Ok(()) => Ok(generated),
        Err(_) => {
            read().map_err(|error| CoreError::ProtectedStore(format!("Host TLS key: {error}")))
        }
    }
}

fn host_key_reference() -> Result<CredentialRef, CoreError> {
    CredentialRef::new(HOST_TLS_CRED_PROVIDER, HOST_TLS_CRED_LABEL)
        .map_err(|error| CoreError::ProtectedStore(format!("Host key reference: {error}")))
}

fn host_key_pem(store: &CredStore) -> Result<String, CoreError> {
    match store {
        CredStore::Os(inner) => {
            let cred = host_key_reference()?;
            read_or_put_host_key(
                || inner.with_version(&cred, HOST_TLS_KEY_VERSION, str::to_owned),
                |pem| inner.put_version(&cred, HOST_TLS_KEY_VERSION, pem),
            )
        }
        CredStore::MemoryVersioned(inner) => {
            let cred = host_key_reference()?;
            read_or_put_host_key(
                || inner.with_version(&cred, HOST_TLS_KEY_VERSION, str::to_owned),
                |pem| inner.put_version(&cred, HOST_TLS_KEY_VERSION, pem),
            )
        }
        CredStore::Memory(inner) => {
            let cred = host_key_reference()?;
            if let Ok(existing) = inner.with_bearer(&cred, str::to_owned) {
                return Ok(existing);
            }
            let generated = generate_key_pem()?;
            inner.insert(cred, &generated);
            Ok(generated)
        }
        CredStore::Env(_) => Err(CoreError::ProtectedStore(String::from(
            "the environment-backed store cannot hold the Host TLS key; unset ENE_API_KEY so the protected OS store is used",
        ))),
    }
}

fn issue_certificate(key: &rcgen::KeyPair) -> Result<rcgen::Certificate, CoreError> {
    let params = rcgen::CertificateParams::new(Vec::new()).map_err(|error| {
        CoreError::ProtectedStore(format!("Host certificate parameters: {error}"))
    })?;
    params
        .self_signed(key)
        .map_err(|error| CoreError::ProtectedStore(format!("issue the Host certificate: {error}")))
}

fn server_tls_config(
    certificate: &rcgen::Certificate,
    key: &rcgen::KeyPair,
) -> Result<tokio_rustls::TlsAcceptor, CoreError> {
    let config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![certificate.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .map_err(|error| CoreError::ProtectedStore(format!("Host TLS configuration: {error}")))?;
    Ok(tokio_rustls::TlsAcceptor::from(Arc::new(config)))
}

fn forbid() -> ErrorResponse {
    tokio_tungstenite::tungstenite::http::Response::builder()
        .status(403)
        .body(None)
        .unwrap_or_else(|_| tokio_tungstenite::tungstenite::http::Response::new(None))
}

fn header_matches(request: &Request, name: &str, expected: &str) -> bool {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| bool::from(value.as_bytes().ct_eq(expected.as_bytes())))
}

#[cfg(unix)]
fn stage_runtime_file(staged: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt as _;

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    options.open(staged)
}

#[cfg(windows)]
fn stage_runtime_file(staged: &Path) -> std::io::Result<std::fs::File> {
    crate::win_acl::create_owner_only(staged)
}

#[cfg(not(any(unix, windows)))]
fn stage_runtime_file(staged: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    options.open(staged)
}

fn publish_runtime(data_dir: &Path, runtime: &HostRuntimeInfo) -> Result<(), CoreError> {
    let json = serde_json::to_vec(runtime)
        .map_err(|error| CoreError::RuntimeInfo(format!("encode runtime information: {error}")))?;
    let target = data_dir.join(HOST_RUNTIME_FILE_NAME);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let seq = STAGE_SEQ.fetch_add(1, Ordering::Relaxed);
    let staged = data_dir.join(format!(
        ".{HOST_RUNTIME_FILE_NAME}.{}.{nanos}.{seq}.tmp",
        std::process::id()
    ));
    let result = (|| {
        let mut file = stage_runtime_file(&staged).map_err(|error| {
            CoreError::RuntimeInfo(format!("stage runtime information: {error}"))
        })?;
        std::io::Write::write_all(&mut file, &json).map_err(|error| {
            CoreError::RuntimeInfo(format!("write runtime information: {error}"))
        })?;
        std::io::Write::flush(&mut file).map_err(|error| {
            CoreError::RuntimeInfo(format!("flush runtime information: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            CoreError::RuntimeInfo(format!("sync runtime information: {error}"))
        })?;
        drop(file);
        std::fs::rename(&staged, &target).map_err(|error| {
            CoreError::RuntimeInfo(format!("publish runtime information: {error}"))
        })
    })();
    if result.is_err() && std::fs::remove_file(&staged).is_err() {
        // Best effort only; the real publication error is returned.
    }
    result
}

pub(crate) fn remove_runtime(data_dir: &Path) -> Result<(), CoreError> {
    match std::fs::remove_file(data_dir.join(HOST_RUNTIME_FILE_NAME)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CoreError::Serving(format!(
            "remove runtime information: {error}"
        ))),
    }
}

/// An admitted TCP connection holding its `MAX_CONNECTIONS` permit while its
/// own TLS handshake and WebSocket upgrade run, independent of the accept
/// loop and of every other connection.
pub(crate) struct PendingUpgrade {
    stream: tokio::net::TcpStream,
    permit: OwnedSemaphorePermit,
    acceptor: tokio_rustls::TlsAcceptor,
    token: String,
    startup_generation: String,
}

impl PendingUpgrade {
    /// Completes this connection's handshake under the production upgrade
    /// timeout. `None` releases the admission permit without ever reaching
    /// the connection table; success transfers the permit onward.
    pub(crate) async fn finish(self) -> Option<(HostWebSocket, OwnedSemaphorePermit)> {
        let Self {
            stream,
            permit,
            acceptor,
            token,
            startup_generation,
        } = self;
        let check = UpgradeCheck {
            token,
            startup_generation,
        };
        let upgraded = tokio::time::timeout(UPGRADE_TIMEOUT, async move {
            let tls = acceptor.accept(stream).await.ok()?;
            let socket =
                tokio_tungstenite::accept_hdr_async_with_config(tls, check, Some(ws_config()))
                    .await
                    .ok()?;
            Some(socket)
        })
        .await;
        match upgraded {
            Ok(Some(socket)) => Some((socket, permit)),
            Ok(None) | Err(_) => None,
        }
    }
}

pub(crate) struct WssListener {
    listener: TcpListener,
    acceptor: tokio_rustls::TlsAcceptor,
    token: String,
    startup_generation: String,
    connections: Arc<tokio::sync::Semaphore>,
}

impl WssListener {
    pub(crate) async fn prepare(data_dir: &Path, store: &CredStore) -> Result<Self, CoreError> {
        crate::serve::lifecycle::ensure_data_dir(data_dir)?;
        let key_pem = host_key_pem(store)?;
        let key = rcgen::KeyPair::from_pem(&key_pem)
            .map_err(|error| CoreError::ProtectedStore(format!("read the Host key: {error}")))?;
        let certificate = issue_certificate(&key)?;
        let host_pin = spki_pin(certificate.der().as_ref())?;
        let acceptor = server_tls_config(&certificate, &key)?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|error| CoreError::Bind(format!("bind the local WSS listener: {error}")))?;
        let port = listener
            .local_addr()
            .map_err(|error| CoreError::Bind(format!("read the WSS listener address: {error}")))?
            .port();
        let runtime = HostRuntimeInfo {
            url: format!("wss://127.0.0.1:{port}"),
            host_pin,
            startup_generation: Uuid::new_v4().simple().to_string(),
            local_token: format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()),
        };
        publish_runtime(data_dir, &runtime)?;
        Ok(Self {
            listener,
            acceptor,
            token: runtime.local_token,
            startup_generation: runtime.startup_generation,
            connections: Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS)),
        })
    }

    /// Takes the next TCP connection and admits it under a bounded permit.
    /// Cancel-safe: while pending only the listener is held, and an accepted
    /// connection is returned in the same poll it arrives; `Ok(None)` means
    /// the connection limit refused it without queueing.
    pub(crate) async fn accept(&self) -> Result<Option<PendingUpgrade>, CoreError> {
        let (stream, _) = self
            .listener
            .accept()
            .await
            .map_err(|error| CoreError::Bind(format!("accept on the WSS listener: {error}")))?;
        let Ok(permit) = Arc::clone(&self.connections).try_acquire_owned() else {
            return Ok(None);
        };
        Ok(Some(PendingUpgrade {
            stream,
            permit,
            acceptor: self.acceptor.clone(),
            token: self.token.clone(),
            startup_generation: self.startup_generation.clone(),
        }))
    }
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

struct UpgradeCheck {
    token: String,
    startup_generation: String,
}

impl tokio_tungstenite::tungstenite::handshake::server::Callback for UpgradeCheck {
    fn on_request(self, request: &Request, response: Response) -> Result<Response, ErrorResponse> {
        let origin_forbidden = request.headers().contains_key("origin");
        let token_ok = header_matches(request, "authorization", &bearer(&self.token));
        let generation_ok =
            header_matches(request, STARTUP_GENERATION_HEADER, &self.startup_generation);
        if origin_forbidden || !generation_ok || !token_ok {
            return Err(forbid());
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::{hex_lower, spki_pin, ws_config};
    use ene_api::codec::MAX_FRAME_BYTES;

    #[test]
    fn hex_encoding_is_lowercase_and_paired() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex_lower(&[]), "");
    }

    #[test]
    fn ws_limits_match_the_wire_frame_cap() {
        let config = ws_config();
        assert_eq!(config.max_message_size, Some(MAX_FRAME_BYTES));
        assert_eq!(config.max_frame_size, Some(MAX_FRAME_BYTES));
        assert!(config.max_write_buffer_size <= MAX_FRAME_BYTES);
        assert!(!config.accept_unmasked_frames, "client frames stay masked");
    }

    #[test]
    fn the_issued_certificate_pins_to_its_subject_public_key_info() {
        let key = rcgen::KeyPair::generate().expect("key");
        let params = rcgen::CertificateParams::new(Vec::new()).expect("params");
        let first = params.self_signed(&key).expect("first certificate");
        let second = params.self_signed(&key).expect("second certificate");
        let pin_first = spki_pin(first.der().as_ref()).expect("pin first");
        let pin_second = spki_pin(second.der().as_ref()).expect("pin second");
        assert_eq!(
            pin_first, pin_second,
            "re-issuing a certificate from the same key must keep the pin"
        );
        assert_eq!(pin_first.len(), 64, "the pin is a sha256 hex digest");
        let other_key = rcgen::KeyPair::generate().expect("other key");
        let other = params.self_signed(&other_key).expect("other certificate");
        let pin_other = spki_pin(other.der().as_ref()).expect("pin other");
        assert_ne!(
            pin_first, pin_other,
            "a different key must present a different pin"
        );
    }
}
