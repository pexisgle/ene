//! Multiplexed host-service acceptor.
//!
//! Binds a single shared socket and routes authenticated
//! [`HostServiceRequest::Open`](ene_plugin_proto::HostServiceRequest::Open)
//! sessions to passenger handlers. Only the `db` service is implemented
//! today; reserved service ids are rejected with
//! [`HostServiceErrorCode::UnknownService`].

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use ene_plugin_proto::{
    HostServiceErrorCode, HostServiceId, HostServiceRequest, HostServiceResponse,
    read_host_service_request, write_host_service_response,
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

/// Shared host-service listener that multiplexes passenger services.
pub struct HostServiceServer {
    socket_path: PathBuf,
    db: DatabaseConnection,
    /// Auth token → `db` registration. Identity and prefix isolation are
    /// derived from this map so a shared socket cannot forge another
    /// plugin's namespace.
    db_plugins: Arc<HashMap<String, DbPluginRegistration>>,
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

            tokio::spawn(async move {
                if let Err(e) = Self::handle_connection(stream, db, db_plugins).await {
                    error!(error = %e, "Host service connection error");
                }
            });
        }
    }

    async fn handle_connection(
        mut stream: ene_plugin_proto::transport::IpcStream,
        db: DatabaseConnection,
        db_plugins: Arc<HashMap<String, DbPluginRegistration>>,
    ) -> Result<(), DbServerError> {
        let Some(request) = read_host_service_request(&mut stream).await? else {
            debug!("Host service connection closed before Open");
            return Ok(());
        };

        let HostServiceRequest::Open { service, token } = request;

        match service {
            HostServiceId::Db => {}
            HostServiceId::Assets | HostServiceId::Capability | HostServiceId::Credential => {
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
            }
        }

        let Some(reg) = db_plugins.get(&token).cloned() else {
            warn!("Host service Open rejected: unknown token");
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

    #[test]
    fn host_service_socket_path_is_stable() {
        let path = host_service_socket_path();
        let rendered = path.to_string_lossy();
        assert!(
            rendered.contains("ene-host-service"),
            "unexpected path: {rendered}"
        );
    }

    #[test]
    fn db_registration_holds_prefix() {
        let reg = DbPluginRegistration {
            tool_name: "fs".into(),
            prefix: "fs_".into(),
            quota_bytes: Some(1024),
        };
        assert_eq!(reg.prefix, "fs_");
        assert_eq!(reg.quota_bytes, Some(1024));
    }
}
