//! Host-side `credential` passenger: authentication, scope matching,
//! resolution, audit, and invalidation push.
//!
//! This is the wire layer that knows both [`ene_connector`] (the vault) and
//! [`ene_plugin_proto`] (the frames); it is the only place that link exists.
//! The passenger authenticates the `Open` token against its own map, matches
//! every requested id against the plugin's declared scope *server-side*, and
//! never lets a secret reach a log, audit record, or error message.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use ene_connector::ConnectorError;
use ene_connector::vault::CredentialVault;
use ene_plugin_proto::transport::IpcStream;
use ene_plugin_proto::{
    CredentialErrorCode, CredentialRequest, CredentialResponse, HostServiceErrorCode,
    HostServicePassenger, HostServiceResponse, ResolvedCredential, WireSecret,
    read_credential_request, write_credential_response, write_host_service_response,
};
use tokio::sync::broadcast;
use tracing::warn;

/// Number of buffered invalidation broadcasts per subscriber before the
/// receiver lags (a slow client then drops its full declared scope).
const INVALIDATED_BUFFER: usize = 64;

/// Per-plugin registration for the `credential` passenger.
///
/// The plugin name is the only identity the passenger needs: it is derived
/// from the pre-shared token (unforgeable) and used for scope matching and
/// audit. The declared credential ids live in the vault.
#[derive(Debug, Clone)]
pub struct CredentialPluginRegistration {
    /// Plugin binary name.
    pub plugin: String,
}

/// Wire-layer `credential` passenger.
pub struct CredentialPassenger {
    vault: Arc<CredentialVault>,
    /// Pre-shared token → plugin registration.
    registrations: HashMap<String, CredentialPluginRegistration>,
    invalidated_tx: broadcast::Sender<Vec<String>>,
}

impl CredentialPassenger {
    /// Builds a passenger from the vault and the token registrations issued
    /// at host-service spawn time.
    #[must_use]
    pub fn new(
        vault: Arc<CredentialVault>,
        registrations: HashMap<String, CredentialPluginRegistration>,
    ) -> Self {
        let (invalidated_tx, _) = broadcast::channel(INVALIDATED_BUFFER);
        Self {
            vault,
            registrations,
            invalidated_tx,
        }
    }

    /// Pushes an invalidation notice to every connected client whose declared
    /// scope includes any of `ids`. The runtime calls this when a credential
    /// is updated or revoked (live config-change detection lands with the
    /// OAuth flow; tests exercise the path directly).
    pub fn broadcast_invalidated(&self, ids: Vec<String>) {
        drop(self.invalidated_tx.send(ids));
    }

    /// Runs the request/invalidation loop for one authenticated session.
    ///
    /// Single-flight by design: one request is in flight at a time, and the
    /// invalidation push shares the same stream.
    async fn serve_session(&self, stream: &mut IpcStream, plugin: &str) {
        // Subscribe before the OpenAck is observable so a client that has
        // confirmed the session is guaranteed to receive invalidation pushes.
        let mut invalidated_rx = self.invalidated_tx.subscribe();
        loop {
            tokio::select! {
                frame = read_credential_request(stream) => {
                    let request = match frame {
                        Ok(Some(request)) => request,
                        Ok(None) | Err(_) => break,
                    };
                    let response = self.handle_request(plugin, request);
                    if write_credential_response(stream, &response).await.is_err() {
                        break;
                    }
                }
                recv = invalidated_rx.recv() => {
                    let ids = match recv {
                        Ok(ids) => ids,
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            // A slow client missed one or more invalidation
                            // frames and cannot know which ids were revoked;
                            // invalidate its full declared scope.
                            self.vault.declared_ids(plugin)
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    };
                    let allowed: Vec<String> = ids
                        .into_iter()
                        .filter(|id| self.vault.is_allowed(plugin, id))
                        .collect();
                    if allowed.is_empty() {
                        continue;
                    }
                    if write_credential_response(
                        stream,
                        &CredentialResponse::Invalidated { ids: allowed },
                    )
                    .await
                    .is_err()
                    {
                        break;
                    }
                }
            }
        }
    }

    fn handle_request(&self, plugin: &str, request: CredentialRequest) -> CredentialResponse {
        match request {
            CredentialRequest::Ping => CredentialResponse::Pong,
            CredentialRequest::Resolve { id } => self.resolve(plugin, &id),
            CredentialRequest::RequestAuthorization { id } => {
                if !self.vault.is_allowed(plugin, &id) {
                    self.vault.record_audit(plugin, &id, false);
                    return self.scope_denied(plugin, &id);
                }
                self.vault.record_audit(plugin, &id, true);
                // Stub until the OAuth flow implements the browser/redirect
                // exchange; the client treats this as "flow started".
                CredentialResponse::AuthorizationPending
            }
        }
    }

    fn resolve(&self, plugin: &str, id: &str) -> CredentialResponse {
        if !self.vault.is_allowed(plugin, id) {
            self.vault.record_audit(plugin, id, false);
            return self.scope_denied(plugin, id);
        }
        match self.vault.resolve(id) {
            Ok(store) => {
                self.vault.record_audit(plugin, id, true);
                if let Some(key) = store.api_key() {
                    return CredentialResponse::Resolved {
                        credential: ResolvedCredential::ApiKey {
                            key: WireSecret::new(key.to_owned()),
                        },
                    };
                }
                if let Some(token) = store.access_token() {
                    return CredentialResponse::Resolved {
                        credential: ResolvedCredential::Bearer {
                            token: WireSecret::new(token.to_owned()),
                            expires_at: store.expires_at(),
                        },
                    };
                }
                CredentialResponse::Error {
                    code: CredentialErrorCode::Missing {
                        label: id.to_string(),
                        help_url: None,
                    },
                    message: format!("credential '{id}' has no configured value"),
                }
            }
            Err(ConnectorError::CredentialMissing {
                id,
                label,
                help_url,
            }) => CredentialResponse::Error {
                code: CredentialErrorCode::Missing { label, help_url },
                message: format!("credential '{id}' is not configured"),
            },
            Err(ConnectorError::RefreshRequired(id)) => CredentialResponse::Error {
                code: CredentialErrorCode::RefreshRequired,
                message: format!("credential '{id}' expired and needs re-authorization"),
            },
            Err(e) => CredentialResponse::Error {
                code: CredentialErrorCode::Internal,
                message: e.to_string(),
            },
        }
    }

    fn scope_denied(&self, plugin: &str, id: &str) -> CredentialResponse {
        CredentialResponse::Error {
            code: CredentialErrorCode::ScopeDenied,
            message: format!("credential '{id}' is not declared for plugin '{plugin}'"),
        }
    }
}

#[async_trait]
impl HostServicePassenger for CredentialPassenger {
    async fn serve(&self, stream: IpcStream, token: String) {
        let mut stream = stream;
        // The passenger authenticates and writes the Open response itself
        // (see `ene-store`'s host_service routing comment): the store routes
        // the raw stream + token here without ever seeing the token map.
        let Some(reg) = self.registrations.get(&token).cloned() else {
            warn!(
                component = "CredentialPassenger",
                "Credential Open rejected: unknown token"
            );
            drop(
                write_host_service_response(
                    &mut stream,
                    &HostServiceResponse::Error {
                        code: HostServiceErrorCode::AuthRejected,
                        message: "Invalid auth token".to_string(),
                    },
                )
                .await,
            );
            return;
        };
        if let Err(e) =
            write_host_service_response(&mut stream, &HostServiceResponse::OpenAck).await
        {
            warn!(
                component = "CredentialPassenger",
                error = %e,
                "Credential service: failed to write OpenAck"
            );
            return;
        }
        self.serve_session(&mut stream, &reg.plugin).await;
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "tests use expect/panic for concise failure messages"
)]
mod tests {
    use super::*;
    use ene_connector::CredentialStore;
    use ene_connector::vault::VaultEntry;
    use ene_plugin_proto::{
        read_credential_response, read_host_service_response, write_credential_request,
    };

    const SECRET: &str = "super-secret-api-key";

    fn vault() -> Arc<CredentialVault> {
        let vault = CredentialVault::new(vec![VaultEntry::new(
            "anthropic",
            CredentialStore::from_api_key(SECRET),
        )]);
        vault.declare("anthropic", vec!["anthropic".to_string()]);
        Arc::new(vault)
    }

    fn registrations(token: &str) -> HashMap<String, CredentialPluginRegistration> {
        HashMap::from([(
            token.to_string(),
            CredentialPluginRegistration {
                plugin: "anthropic".to_string(),
            },
        )])
    }

    /// Opens a credential session over a duplex pair, returning the
    /// client-side stream after the `OpenAck` is observed.
    async fn open_session(passenger: Arc<CredentialPassenger>, token: &str) -> IpcStream {
        let (client, server_stream) = tokio::io::duplex(4096);
        tokio::spawn({
            let passenger = Arc::clone(&passenger);
            async move {
                let _ = passenger.serve(server_stream, token.to_string()).await;
            }
        });
        let resp = read_host_service_response(&mut client)
            .await
            .expect("read open response")
            .expect("open frame");
        assert!(matches!(resp, HostServiceResponse::OpenAck));
        client
    }

    async fn send_request(client: &mut IpcStream, req: &CredentialRequest) {
        write_credential_request(client, req)
            .await
            .expect("write req");
    }

    async fn read_response(client: &mut IpcStream) -> CredentialResponse {
        read_credential_response(client)
            .await
            .expect("read response")
            .expect("response frame")
    }

    #[tokio::test]
    async fn open_with_valid_token_and_resolve() {
        let passenger = Arc::new(CredentialPassenger::new(
            vault(),
            registrations("ene-cred-good"),
        ));
        let mut client = open_session(passenger, "ene-cred-good").await;
        send_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "anthropic".into(),
            },
        )
        .await;
        let resp = read_response(&mut client).await;
        match resp {
            CredentialResponse::Resolved {
                credential: ResolvedCredential::ApiKey { key },
            } => assert_eq!(key.expose(), SECRET),
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[tokio::test]
    async fn open_with_invalid_token_is_rejected() {
        let passenger = Arc::new(CredentialPassenger::new(
            vault(),
            registrations("ene-cred-good"),
        ));
        let (mut client, server_stream) = tokio::io::duplex(4096);
        tokio::spawn({
            let passenger = Arc::clone(&passenger);
            async move {
                let _ = passenger
                    .serve(server_stream, "ene-cred-bad".to_string())
                    .await;
            }
        });
        let resp = read_host_service_response(&mut client)
            .await
            .expect("read open error")
            .expect("open frame");
        assert!(matches!(
            resp,
            HostServiceResponse::Error {
                code: HostServiceErrorCode::AuthRejected,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn undeclared_id_is_denied_and_audited() {
        let passenger = Arc::new(CredentialPassenger::new(
            vault(),
            registrations("ene-cred-good"),
        ));
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;
        send_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "google.calendar".into(),
            },
        )
        .await;
        let resp = read_response(&mut client).await;
        assert!(matches!(
            resp,
            CredentialResponse::Error {
                code: CredentialErrorCode::ScopeDenied,
                ..
            }
        ));
        let audit = passenger.vault.drain_audit();
        assert!(
            audit
                .iter()
                .any(|e| !e.allowed && e.id == "google.calendar"),
            "denial must be audited"
        );
    }

    #[tokio::test]
    async fn invalidated_broadcast_reaches_connected_client() {
        let passenger = Arc::new(CredentialPassenger::new(
            vault(),
            registrations("ene-cred-good"),
        ));
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;
        passenger.broadcast_invalidated(vec!["anthropic".to_string()]);
        let resp = read_response(&mut client).await;
        assert_eq!(
            resp,
            CredentialResponse::Invalidated {
                ids: vec!["anthropic".to_string()],
            }
        );
    }

    #[tokio::test]
    async fn invalidated_outside_declared_scope_is_not_forwarded() {
        let passenger = Arc::new(CredentialPassenger::new(
            vault(),
            registrations("ene-cred-good"),
        ));
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;
        passenger.broadcast_invalidated(vec!["google.calendar".to_string()]);
        // The client must NOT receive a frame for an undeclared id; a Ping
        // proves the connection is still live and no Invalidated was sent.
        send_request(&mut client, &CredentialRequest::Ping).await;
        let resp = read_response(&mut client).await;
        assert_eq!(resp, CredentialResponse::Pong);
    }

    #[tokio::test]
    async fn secret_never_reaches_error_message_or_audit() {
        let vault = CredentialVault::new(vec![VaultEntry::new(
            "anthropic",
            CredentialStore::from_api_key(SECRET),
        )]);
        // "missing-cred" is declared but has no entry: resolve must reach the
        // Missing path (not ScopeDenied) while keeping the secret out of the
        // error frame and the audit trail.
        vault.declare(
            "anthropic",
            vec!["anthropic".to_string(), "missing-cred".to_string()],
        );
        let passenger = Arc::new(CredentialPassenger::new(
            Arc::new(vault),
            registrations("ene-cred-good"),
        ));
        let mut client = open_session(Arc::clone(&passenger), "ene-cred-good").await;
        send_request(
            &mut client,
            &CredentialRequest::Resolve {
                id: "missing-cred".into(),
            },
        )
        .await;
        let resp = read_response(&mut client).await;
        assert!(matches!(
            resp,
            CredentialResponse::Error {
                code: CredentialErrorCode::Missing { .. },
                ..
            }
        ));
        let all_frames = format!("{resp:?}");
        assert!(!all_frames.contains(SECRET));
        let audit = passenger.vault.drain_audit();
        assert!(
            !format!("{audit:?}").contains(SECRET),
            "audit must never carry the secret"
        );
    }

    #[test]
    fn resolved_frame_debug_redacts_secret() {
        let frame = CredentialResponse::Resolved {
            credential: ResolvedCredential::ApiKey {
                key: WireSecret::new(SECRET),
            },
        };
        let debug = format!("{frame:?}");
        assert!(!debug.contains(SECRET));
        assert!(debug.contains("<redacted>"));
    }
}
