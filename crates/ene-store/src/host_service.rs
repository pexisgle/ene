//! Multiplexed host-service acceptor.
//!
//! Binds a single shared socket and routes authenticated
//! [`HostServiceRequest::Open`](ene_plugin_proto::HostServiceRequest::Open)
//! sessions to passenger handlers. The `db` service is authenticated here;
//! other services are delegated to a registered
//! [`HostServicePassenger`](ene_plugin_proto::HostServicePassenger) that
//! authenticates and writes its own `Open` response. Unregistered services
//! keep the historical [`HostServiceErrorCode::UnknownService`] rejection.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ene_plugin_db::{DbErrorCode, DbRequest, DbResponse};
use ene_plugin_proto::{
    HOST_SERVICE_MAX_MESSAGE_SIZE, HostServiceErrorCode, HostServiceId, HostServicePassenger,
    HostServiceRequest, HostServiceResponse, write_host_service_response,
};
use sea_orm::DatabaseConnection;
use tokio::io::AsyncReadExt;
use tracing::{debug, error, info, warn};

use crate::db_server::{DbIpcServer, DbServerError};

/// Per-plugin registration for the `db` host service.
#[derive(Debug, Clone)]
pub struct DbPluginRegistration {
    /// Plugin binary name (also used as the table-name prefix stem).
    pub tool_name: String,
    /// Required table/index name prefix (`{name}_`).
    pub prefix: String,
    /// Optional storage quota in bytes (`None` = unbounded).
    pub quota_bytes: Option<u64>,
}

/// Tracks rejected `Open`/handshake attempts so brute-force probing of the
/// shared socket cannot flood the log while the cumulative count stays
/// measurable.
#[derive(Default)]
struct OpenFailureTracker {
    count: u64,
    last_logged: Option<std::time::Instant>,
}

impl OpenFailureTracker {
    /// Records one rejection; returns `true` when the caller should log it
    /// (at most once per second).
    fn record(&mut self) -> bool {
        self.count = self.count.saturating_add(1);
        let now = std::time::Instant::now();
        let should_log = self
            .last_logged
            .is_none_or(|t| now.duration_since(t) >= std::time::Duration::from_secs(1));
        if should_log {
            self.last_logged = Some(now);
        }
        should_log
    }
    fn count(&self) -> u64 {
        self.count
    }
}

/// Shared host-service listener that multiplexes passenger services.
pub struct HostServiceServer {
    socket_path: PathBuf,
    db: DatabaseConnection,
    /// Auth token → `db` registration. Identity and prefix isolation are
    /// derived from this map so a shared socket cannot forge another
    /// plugin's namespace.
    db_plugins: Arc<HashMap<String, DbPluginRegistration>>,
    /// Service id → passenger. Only services present here are openable;
    /// everything else keeps the `UnknownService` rejection.
    passengers: Arc<HashMap<HostServiceId, Arc<dyn HostServicePassenger>>>,
    failed_opens: Arc<Mutex<OpenFailureTracker>>,
}

impl HostServiceServer {
    /// Creates a host-service server bound to `socket_path`.
    pub fn new(
        db: DatabaseConnection,
        socket_path: PathBuf,
        db_plugins: HashMap<String, DbPluginRegistration>,
        passengers: HashMap<HostServiceId, Arc<dyn HostServicePassenger>>,
    ) -> Self {
        Self {
            socket_path,
            db,
            db_plugins: Arc::new(db_plugins),
            passengers: Arc::new(passengers),
            failed_opens: Arc::new(Mutex::new(OpenFailureTracker::default())),
        }
    }

    /// Returns the socket path this server listens on.
    pub fn socket_path(&self) -> &std::path::Path {
        &self.socket_path
    }

    /// Runs the accept loop until the task is cancelled.
    pub async fn run(self) -> Result<(), DbServerError> {
        #[cfg(unix)]
        if self.socket_path.exists() {
            tokio::fs::remove_file(&self.socket_path).await?;
        }

        let mut listener = ene_plugin_proto::transport::IpcListener::bind(&self.socket_path)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            if let Err(e) = std::fs::set_permissions(&self.socket_path, perms) {
                return Err(DbServerError::Internal(format!(
                    "failed to chmod host-service socket to 0o600: {e}"
                )));
            }
        }

        info!(
            socket = %self.socket_path.display(),
            db_plugins = self.db_plugins.len(),
            "Host service listening"
        );

        #[expect(
            clippy::infinite_loop,
            reason = "host-service accept loop runs until the server task is cancelled"
        )]
        loop {
            let stream = match listener.accept().await {
                Ok(stream) => stream,
                Err(e) => {
                    error!(
                        error = %e,
                        "Host service accept failed; backing off and continuing"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    continue;
                }
            };
            debug!("Accepted host-service connection");

            let db = self.db.clone();
            let db_plugins = Arc::clone(&self.db_plugins);
            let passengers = Arc::clone(&self.passengers);
            let failed_opens = Arc::clone(&self.failed_opens);

            tokio::spawn(async move {
                if let Err(e) =
                    Self::handle_connection(stream, db, db_plugins, passengers, failed_opens).await
                {
                    error!(error = %e, "Host service connection error");
                }
            });
        }
    }

    async fn handle_connection(
        mut stream: ene_plugin_proto::transport::IpcStream,
        db: DatabaseConnection,
        db_plugins: Arc<HashMap<String, DbPluginRegistration>>,
        passengers: Arc<HashMap<HostServiceId, Arc<dyn HostServicePassenger>>>,
        failed_opens: Arc<Mutex<OpenFailureTracker>>,
    ) -> Result<(), DbServerError> {
        // Read the first frame manually so a legacy `DbRequest::Handshake`
        // can be recognized after `HostServiceRequest::Open` deserialization
        // fails (the frame is consumed either way).
        let Some(frame) = read_first_frame(&mut stream).await? else {
            debug!("Host service connection closed before Open");
            return Ok(());
        };

        // New-protocol Open.
        if let Ok(HostServiceRequest::Open { service, token }) = serde_json::from_slice(&frame) {
            match service {
                HostServiceId::Db => {}
                // The `Open` response is written by the service that owns the
                // session: `db` authenticates here and writes `OpenAck`
                // below, while a registered passenger authenticates and
                // writes its own response. This asymmetry keeps per-service
                // secrets and token maps out of the store.
                HostServiceId::Assets | HostServiceId::Capability | HostServiceId::Credential => {
                    let Some(passenger) = passengers.get(&service) else {
                        warn!(?service, "Host service Open for unimplemented service");
                        write_host_service_response(
                            &mut stream,
                            &HostServiceResponse::Error {
                                code: HostServiceErrorCode::UnknownService,
                                message: format!("service {service:?} is not implemented"),
                            },
                        )
                        .await?;
                        return Ok(());
                    };
                    passenger.serve(stream, token).await;
                    return Ok(());
                }
            }

            let Some(reg) = db_plugins.get(&token).cloned() else {
                Self::log_rejected_open(&failed_opens, "Host service Open rejected: unknown token");
                write_host_service_response(
                    &mut stream,
                    &HostServiceResponse::Error {
                        code: HostServiceErrorCode::AuthRejected,
                        message: "Invalid auth token".to_string(),
                    },
                )
                .await?;
                return Ok(());
            };

            write_host_service_response(&mut stream, &HostServiceResponse::OpenAck).await?;
            return DbIpcServer::serve_authenticated_connection(
                stream,
                db,
                reg.tool_name,
                reg.prefix,
                reg.quota_bytes,
            )
            .await;
        }

        // Legacy `DbRequest::Handshake`: the sandbox `db_socket` alias points
        // old plugin binaries at this shared socket, so route their handshake
        // through the same token → registration map.
        if let Ok(DbRequest::Handshake { token }) = serde_json::from_slice(&frame) {
            let Some(reg) = db_plugins.get(&token).cloned() else {
                Self::log_rejected_open(
                    &failed_opens,
                    "Host service legacy handshake rejected: unknown token",
                );
                DbIpcServer::send_response(
                    &mut stream,
                    &DbResponse::Error {
                        code: DbErrorCode::PermissionDenied,
                        message: "Invalid auth token".to_string(),
                    },
                )
                .await?;
                return Ok(());
            };
            DbIpcServer::send_response(&mut stream, &DbResponse::HandshakeAck).await?;
            return DbIpcServer::serve_authenticated_connection(
                stream,
                db,
                reg.tool_name,
                reg.prefix,
                reg.quota_bytes,
            )
            .await;
        }

        warn!("Host service connection sent an unrecognized first frame");
        Ok(())
    }

    /// Logs a rejected `Open`/handshake at `warn` at most once per second,
    /// falling back to `debug` for the rejections in between so brute-force
    /// probing of the shared socket cannot flood the log while the cumulative
    /// attempt count stays measurable.
    fn log_rejected_open(failed_opens: &Arc<Mutex<OpenFailureTracker>>, message: &str) {
        let (should_log, attempts) = {
            let mut tracker = failed_opens
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let should_log = tracker.record();
            (should_log, tracker.count())
        };
        if should_log {
            warn!(attempts, "{message}");
        } else {
            debug!(attempts, "{message}");
        }
    }
}

/// Reads one length-prefixed frame from a new host-service connection,
/// returning `None` when the peer closes before sending anything.
async fn read_first_frame(
    stream: &mut ene_plugin_proto::transport::IpcStream,
) -> Result<Option<Vec<u8>>, DbServerError> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(DbServerError::Io(e)),
    }
    let Ok(msg_len) = usize::try_from(u32::from_le_bytes(len_buf)) else {
        return Err(DbServerError::Internal(
            "message length overflow on this platform".to_string(),
        ));
    };
    if msg_len > HOST_SERVICE_MAX_MESSAGE_SIZE {
        return Err(DbServerError::Internal(format!(
            "message too large: {msg_len}"
        )));
    }
    let mut msg_buf = vec![0u8; msg_len];
    stream.read_exact(&mut msg_buf).await?;
    Ok(Some(msg_buf))
}

/// Socket path for the shared host-service endpoint.
pub fn host_service_socket_path() -> PathBuf {
    #[cfg(unix)]
    {
        ene_config::paths::tool_socket_dir().join("ene-host-service.sock")
    }
    #[cfg(windows)]
    {
        PathBuf::from(r"\\.\pipe\ene-host-service")
    }
    #[cfg(not(any(unix, windows)))]
    {
        ene_config::paths::tool_socket_dir().join("ene-host-service.sock")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use ene_plugin_db::{DbRequest, DbResponse};
    use ene_plugin_proto::transport::{IpcStream, cleanup_path};
    use ene_plugin_proto::{
        CredentialRequest, CredentialResponse, HostServicePassenger, read_credential_request,
        read_credential_response, write_credential_request,
    };
    use sea_orm::Database;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn test_registration(token: &str) -> HashMap<String, DbPluginRegistration> {
        HashMap::from([(
            token.to_string(),
            DbPluginRegistration {
                tool_name: "fs".into(),
                prefix: "fs_".into(),
                quota_bytes: Some(1024),
            },
        )])
    }

    /// A minimal credential passenger for delegation tests: accepts any token
    /// and answers one `Resolve` with a fixed key before closing.
    struct EchoCredentialPassenger;

    #[async_trait]
    impl HostServicePassenger for EchoCredentialPassenger {
        async fn serve(&self, stream: IpcStream, _token: String) {
            let mut stream = stream;
            drop(write_host_service_response(&mut stream, &HostServiceResponse::OpenAck).await);
            if let Some(request) = read_credential_request(&mut stream)
                .await
                .expect("read request")
                && let CredentialRequest::Resolve { id: _ } = request
            {
                let response = CredentialResponse::Resolved {
                    credential: ene_plugin_proto::ResolvedCredential::ApiKey {
                        key: ene_plugin_proto::WireSecret::new("echo-key"),
                    },
                };
                drop(ene_plugin_proto::write_credential_response(&mut stream, &response).await);
            }
        }
    }

    fn test_passengers() -> HashMap<HostServiceId, Arc<dyn HostServicePassenger>> {
        HashMap::from([(
            HostServiceId::Credential,
            Arc::new(EchoCredentialPassenger) as Arc<dyn HostServicePassenger>,
        )])
    }

    async fn read_framed(stream: &mut IpcStream) -> DbResponse {
        let mut len_buf = [0u8; 4];
        stream.read_exact(&mut len_buf).await.expect("read len");
        let mut body = vec![0u8; u32::from_le_bytes(len_buf) as usize];
        stream.read_exact(&mut body).await.expect("read body");
        serde_json::from_slice(&body).expect("decode response")
    }

    async fn write_framed(stream: &mut IpcStream, req: &DbRequest) {
        let json = serde_json::to_vec(req).expect("encode request");
        stream
            .write_all(&(json.len() as u32).to_le_bytes())
            .await
            .expect("write len");
        stream.write_all(&json).await.expect("write body");
        stream.flush().await.expect("flush");
    }

    async fn wait_for_socket(path: &std::path::Path) {
        for _ in 0..50 {
            if path.exists() {
                return;
            }
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test]
    async fn legacy_handshake_routes_to_registered_plugin() {
        let socket = std::env::temp_dir().join(format!("ene-hs-test-{}", std::process::id()));
        let db = Database::connect("sqlite::memory:").await.expect("open db");
        let server = HostServiceServer::new(
            db,
            socket.clone(),
            test_registration("ene-db-good"),
            test_passengers(),
        );
        let server_task = tokio::spawn(server.run());
        wait_for_socket(&socket).await;

        let mut client = IpcStream::connect(&socket).await.expect("connect");
        write_framed(
            &mut client,
            &DbRequest::Handshake {
                token: "ene-db-good".into(),
            },
        )
        .await;
        let resp = read_framed(&mut client).await;
        write_framed(&mut client, &DbRequest::Ping).await;
        let pong = read_framed(&mut client).await;

        server_task.abort();
        cleanup_path(&socket);
        assert!(matches!(resp, DbResponse::HandshakeAck));
        assert!(matches!(pong, DbResponse::Pong));
    }

    #[tokio::test]
    async fn legacy_handshake_rejects_unknown_token() {
        let socket = std::env::temp_dir().join(format!("ene-hs-test2-{}", std::process::id()));
        let db = Database::connect("sqlite::memory:").await.expect("open db");
        let server = HostServiceServer::new(
            db,
            socket.clone(),
            test_registration("ene-db-good"),
            test_passengers(),
        );
        let server_task = tokio::spawn(server.run());
        wait_for_socket(&socket).await;

        let mut client = IpcStream::connect(&socket).await.expect("connect");
        write_framed(
            &mut client,
            &DbRequest::Handshake {
                token: "ene-db-bad".into(),
            },
        )
        .await;
        let resp = read_framed(&mut client).await;

        server_task.abort();
        cleanup_path(&socket);
        assert!(matches!(
            resp,
            DbResponse::Error {
                code: DbErrorCode::PermissionDenied,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn credential_open_delegates_to_registered_passenger() {
        let socket = std::env::temp_dir().join(format!("ene-hs-test3-{}", std::process::id()));
        let db = Database::connect("sqlite::memory:").await.expect("open db");
        let server = HostServiceServer::new(
            db,
            socket.clone(),
            test_registration("ene-db-good"),
            test_passengers(),
        );
        let server_task = tokio::spawn(server.run());
        wait_for_socket(&socket).await;

        let mut client = IpcStream::connect(&socket).await.expect("connect");
        let open = ene_plugin_proto::HostServiceRequest::Open {
            service: HostServiceId::Credential,
            token: "any-token".into(),
        };
        ene_plugin_proto::write_host_service_request(&mut client, &open)
            .await
            .expect("write open");
        let ack = ene_plugin_proto::read_host_service_response(&mut client)
            .await
            .expect("read ack")
            .expect("ack frame");
        assert!(matches!(ack, HostServiceResponse::OpenAck));

        write_credential_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "anthropic".into(),
            },
        )
        .await
        .expect("write resolve");
        let resp = read_credential_response(&mut client)
            .await
            .expect("read resolve")
            .expect("resolve frame");

        server_task.abort();
        cleanup_path(&socket);
        assert!(matches!(resp, CredentialResponse::Resolved { .. }));
    }

    #[tokio::test]
    async fn unregistered_service_keeps_unknown_service_rejection() {
        let socket = std::env::temp_dir().join(format!("ene-hs-test4-{}", std::process::id()));
        let db = Database::connect("sqlite::memory:").await.expect("open db");
        // No passengers registered: `Credential` must fall back to the
        // historical `UnknownService` rejection, and so must the other
        // reserved-but-unimplemented ids.
        let server = HostServiceServer::new(
            db,
            socket.clone(),
            test_registration("ene-db-good"),
            HashMap::new(),
        );
        let server_task = tokio::spawn(server.run());
        wait_for_socket(&socket).await;

        let mut client = IpcStream::connect(&socket).await.expect("connect");
        let open = ene_plugin_proto::HostServiceRequest::Open {
            service: HostServiceId::Credential,
            token: "ene-db-good".into(),
        };
        ene_plugin_proto::write_host_service_request(&mut client, &open)
            .await
            .expect("write open");
        let resp = ene_plugin_proto::read_host_service_response(&mut client)
            .await
            .expect("read response")
            .expect("response frame");

        server_task.abort();
        cleanup_path(&socket);
        assert!(matches!(
            resp,
            HostServiceResponse::Error {
                code: HostServiceErrorCode::UnknownService,
                ..
            }
        ));
    }
}
