//! Multiplexed host-service acceptor.
//!
//! Binds a single shared socket and routes authenticated
//! [`HostServiceRequest::Open`](ene_plugin_proto::HostServiceRequest::Open)
//! sessions to passenger handlers. The `db` passenger is served here; the
//! `capability` passenger is handed to a host-provided handler (the
//! mediation layer). Unimplemented service ids are rejected with
//! [`HostServiceErrorCode::UnknownService`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use ene_plugin_proto::{
    CapabilityServiceHandler, HostServiceErrorCode, HostServiceId, HostServiceRequest,
    HostServiceResponse, SharedBrokerHandler, read_framed_json, read_host_service_request,
    write_framed_json, write_host_service_response,
};
use sea_orm::DatabaseConnection;
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
    /// Optional handler for the `capability` passenger, supplied by the host
    /// runtime's mediation layer. `None` keeps the service unimplemented.
    capability: Option<Arc<dyn CapabilityServiceHandler>>,
    /// Optional broker handler for the v8 broker passengers (`file`,
    /// `network`, `process`, `credential`, `artifact`, `platform`).
    broker: Option<SharedBrokerHandler>,
    failed_opens: Arc<Mutex<OpenFailureTracker>>,
}

impl HostServiceServer {
    /// Creates a host-service server bound to `socket_path`.
    pub fn new(
        db: DatabaseConnection,
        socket_path: PathBuf,
        db_plugins: HashMap<String, DbPluginRegistration>,
    ) -> Self {
        Self {
            socket_path,
            db,
            db_plugins: Arc::new(db_plugins),
            capability: None,
            broker: None,
            failed_opens: Arc::new(Mutex::new(OpenFailureTracker::default())),
        }
    }

    /// Registers the handler that serves `capability` passenger sessions.
    #[must_use]
    pub fn with_capability_handler(mut self, handler: Arc<dyn CapabilityServiceHandler>) -> Self {
        self.capability = Some(handler);
        self
    }

    /// Registers the handler that serves v8 broker passenger sessions.
    #[must_use]
    pub fn with_broker_handler(mut self, handler: SharedBrokerHandler) -> Self {
        self.broker = Some(handler);
        self
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
            let capability = self.capability.clone();
            let broker = self.broker.clone();
            let failed_opens = Arc::clone(&self.failed_opens);

            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(
                    stream,
                    db,
                    db_plugins,
                    capability,
                    broker,
                    failed_opens,
                )
                .await
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
        capability: Option<Arc<dyn CapabilityServiceHandler>>,
        broker: Option<SharedBrokerHandler>,
        failed_opens: Arc<Mutex<OpenFailureTracker>>,
    ) -> Result<(), DbServerError> {
        let request = match read_host_service_request(&mut stream).await {
            Ok(Some(request)) => request,
            Ok(None) => {
                debug!("Host service connection closed before Open");
                return Ok(());
            }
            Err(_) => {
                warn!("Host service connection sent an unrecognized first frame");
                return Ok(());
            }
        };
        // The request type has a single `Open` variant; anything else failed
        // to deserialize above and was already rejected.
        let HostServiceRequest::Open { service, token } = request;

        match service {
            HostServiceId::Db => {}
            HostServiceId::Capability => {
                let Some(handler) = capability else {
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
                let Some(reg) = db_plugins.get(&token).cloned() else {
                    Self::log_rejected_open(
                        &failed_opens,
                        "Host service Open rejected: unknown token",
                    );
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
                if let Err(e) = handler.serve(stream, reg.tool_name).await {
                    error!(error = %e, "Capability service session error");
                }
                return Ok(());
            }
            HostServiceId::Artifact
            | HostServiceId::Credential
            | HostServiceId::File
            | HostServiceId::Network
            | HostServiceId::Process
            | HostServiceId::Platform => {
                Self::open_broker_session(stream, db_plugins, broker, failed_opens, token).await?;
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
        DbIpcServer::serve_authenticated_connection(
            stream,
            db,
            reg.tool_name,
            reg.prefix,
            reg.quota_bytes,
        )
        .await
    }

    /// Opens a v8 broker session: authenticates the token, acknowledges, and
    /// serves `BrokerRequest`/`BrokerResponse` frames until the peer closes.
    async fn open_broker_session(
        mut stream: ene_plugin_proto::transport::IpcStream,
        db_plugins: Arc<HashMap<String, DbPluginRegistration>>,
        broker: Option<SharedBrokerHandler>,
        failed_opens: Arc<Mutex<OpenFailureTracker>>,
        token: String,
    ) -> Result<(), DbServerError> {
        let Some(handler) = broker else {
            warn!("Host service Open for broker service without a broker handler");
            write_host_service_response(
                &mut stream,
                &HostServiceResponse::Error {
                    code: HostServiceErrorCode::UnknownService,
                    message: "broker services are not implemented by this host".to_string(),
                },
            )
            .await?;
            return Ok(());
        };
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
        loop {
            let Some(request) = read_framed_json(&mut stream).await? else {
                return Ok(());
            };
            if matches!(
                request,
                ene_plugin_proto::BrokerRequest::NetworkFetchStream { .. }
            ) {
                let mut sink = StreamSink {
                    stream: &mut stream,
                };
                handler
                    .handle_stream(&reg.tool_name, request, &mut sink)
                    .await?;
            } else {
                let response = handler.handle(&reg.tool_name, request).await;
                write_framed_json(&mut stream, &response).await?;
            }
        }
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

/// Frame sink that writes streaming broker responses to the session socket.
struct StreamSink<'a> {
    stream: &'a mut ene_plugin_proto::transport::IpcStream,
}

#[async_trait::async_trait]
impl ene_plugin_proto::BrokerSink for StreamSink<'_> {
    async fn write(&mut self, response: &ene_plugin_proto::BrokerResponse) -> std::io::Result<()> {
        write_framed_json(self.stream, response).await
    }
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
    use ene_plugin_db::{DbRequest, DbResponse};
    use ene_plugin_proto::{
        read_host_service_response,
        transport::{IpcStream, cleanup_path},
        write_host_service_request,
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
    async fn open_db_service_serves_authenticated_requests() {
        let socket = std::env::temp_dir().join(format!("ene-hs-test-{}", std::process::id()));
        let db = Database::connect("sqlite::memory:").await.expect("open db");
        let server = HostServiceServer::new(db, socket.clone(), test_registration("ene-db-good"));
        let server_task = tokio::spawn(server.run());
        wait_for_socket(&socket).await;

        let mut client = IpcStream::connect(&socket).await.expect("connect");
        write_host_service_request(
            &mut client,
            &HostServiceRequest::Open {
                service: HostServiceId::Db,
                token: "ene-db-good".into(),
            },
        )
        .await
        .expect("write open");
        let resp = read_host_service_response(&mut client)
            .await
            .expect("read open ack")
            .expect("open response");
        assert!(matches!(resp, HostServiceResponse::OpenAck));

        write_framed(&mut client, &DbRequest::Ping).await;
        let pong = read_framed(&mut client).await;

        server_task.abort();
        cleanup_path(&socket);
        assert!(matches!(pong, DbResponse::Pong));
    }

    #[tokio::test]
    async fn open_db_service_rejects_unknown_token() {
        let socket = std::env::temp_dir().join(format!("ene-hs-test2-{}", std::process::id()));
        let db = Database::connect("sqlite::memory:").await.expect("open db");
        let server = HostServiceServer::new(db, socket.clone(), test_registration("ene-db-good"));
        let server_task = tokio::spawn(server.run());
        wait_for_socket(&socket).await;

        let mut client = IpcStream::connect(&socket).await.expect("connect");
        write_host_service_request(
            &mut client,
            &HostServiceRequest::Open {
                service: HostServiceId::Db,
                token: "ene-db-bad".into(),
            },
        )
        .await
        .expect("write open");
        let resp = read_host_service_response(&mut client)
            .await
            .expect("read rejection")
            .expect("rejection response");

        server_task.abort();
        cleanup_path(&socket);
        assert!(matches!(
            resp,
            HostServiceResponse::Error {
                code: HostServiceErrorCode::AuthRejected,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn legacy_handshake_frame_is_rejected_and_closed() {
        // A legacy `DbRequest::Handshake` frame (raw JSON, the variant is
        // gone) must not authenticate: the server closes the connection
        // without a response.
        let socket = std::env::temp_dir().join(format!("ene-hs-test3-{}", std::process::id()));
        let db = Database::connect("sqlite::memory:").await.expect("open db");
        let server = HostServiceServer::new(db, socket.clone(), test_registration("ene-db-good"));
        let server_task = tokio::spawn(server.run());
        wait_for_socket(&socket).await;

        let mut client = IpcStream::connect(&socket).await.expect("connect");
        let json = br#"{"Handshake":{"token":"ene-db-good"}}"#;
        client
            .write_all(&(json.len() as u32).to_le_bytes())
            .await
            .expect("write len");
        client.write_all(json).await.expect("write body");
        client.flush().await.expect("flush");

        let mut len_buf = [0u8; 4];
        let closed = client.read_exact(&mut len_buf).await.is_err();

        server_task.abort();
        cleanup_path(&socket);
        assert!(closed, "legacy handshake must not receive a response");
    }
}
