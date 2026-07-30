//! # ene-connector
//!
//! Credential and identity authority for external-service integrations.
//!
//! This crate owns the *credential* side of connecting to external services
//! (Calendar, GitHub, MCP, and the like): secure `OAuth2` / API-key storage whose
//! secrets are redacted from `Debug`/`Serialize` and zeroed on drop, stable
//! connector identifiers, OAuth permission scopes, and display metadata. It
//! deliberately does **not** own connection lifecycle — process supervision,
//! restarts, and health probing live in `ene-plugin-host`. These types have no
//! consumer yet: the MCP credential bridge that will consume them is
//! reintroduced in `ene-plugin-host` under #412, backed by the credential vault
//! landing here under #415.
//!
//! # Architecture
//!
//! - [`CredentialStore`] / [`CredentialData`] / [`AccountCredentials`] — secure
//!   credential storage (secrets never logged; raw material reachable only via
//!   the audited [`CredentialStore::expose_for_persistence`] path).
//! - [`ConnectorId`] — stable `namespace.name` identifier, also used as a
//!   credential id.
//! - [`PermissionScope`] — an OAuth scope requested by a connector.
//! - [`ConnectorIdentity`] — display metadata for configuration UIs.
//! - [`ConnectorError`] — unified error type.
//!
//! # History and roadmap
//!
//! An earlier revision shipped a `Connector` lifecycle trait and a
//! `ConnectorRegistry`. That layer was never wired into the live MCP path and
//! duplicated supervision that `ene-plugin-host` already provides, so it was
//! removed (#416); the MCP bridge's SSRF URL validation now lives in
//! `ene-plugin-host`'s `mcp_registry`. A credential vault and OAuth flow land
//! here in #415, and the client-side credential policy (retry / rate limiting)
//! is reintroduced in `ene-plugin` under #413.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(clippy::unwrap_used, reason = "unit tests use unwrap for assertions")
)]

pub mod credential;
pub mod error;
pub mod identity;

pub use credential::{AccountCredentials, CredentialData, CredentialStore};
pub use error::ConnectorError;
pub use identity::{ConnectorId, ConnectorIdentity, PermissionScope};

/// Convenience re-exports of the crate's public API.
pub mod prelude {
    pub use crate::credential::{AccountCredentials, CredentialData, CredentialStore};
    pub use crate::error::ConnectorError;
    pub use crate::identity::{ConnectorId, ConnectorIdentity, PermissionScope};
}
