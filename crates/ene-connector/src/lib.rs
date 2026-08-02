//! # ene-connector
//!
//! Credential and identity authority for external-service integrations.
//!
//! This crate owns the *credential* side of connecting to external services
//! (Calendar, GitHub, MCP, and the like): secure `OAuth2` / API-key storage whose
//! secrets are redacted from `Debug`/`Serialize` and zeroed on drop, stable
//! connector identifiers, OAuth permission scopes, and display metadata. It
//! deliberately does **not** own connection lifecycle — process supervision,
//! restarts, and health probing live in `ene-plugin-host`, which already
//! consumes the identity and declaration types from this crate.
//!
//! # Architecture
//!
//! - [`CredentialStore`] / [`CredentialData`] / [`AccountCredentials`] — secure
//!   credential storage (secrets never logged; raw material reachable only via
//!   the audited [`CredentialStore::expose_for_persistence`] path).
//! - [`ConnectorId`] — stable `namespace.name` identifier.
//! - [`CredentialId`] — stable identifier for a stored credential (no
//!   namespace required).
//! - [`PermissionScope`] — an OAuth scope requested by a connector.
//! - [`ConnectorIdentity`] — display metadata for configuration UIs.
//! - [`CredentialDeclaration`] / [`resolve_scope`] — parsing of a plugin's
//!   `x-ene-credentials` declarations and scoped access resolution.
//! - [`ConnectorError`] — unified error type.
//!
//! # Connection lifecycle lives elsewhere
//!
//! Process supervision, restarts, health probing, and the MCP bridge's SSRF
//! URL validation all live in `ene-plugin-host`; this crate stays the
//! credential and identity authority.
#![warn(missing_docs)]
#![cfg_attr(
    test,
    expect(clippy::unwrap_used, reason = "unit tests use unwrap for assertions")
)]

pub mod credential;
pub mod declaration;
pub mod error;
pub mod identity;

pub use credential::{AccountCredentials, CredentialData, CredentialStore};
pub use declaration::{
    CredentialDeclaration, CredentialKind, CredentialParse, CredentialRejection, CredentialWarning,
    DegradedCredential, HeaderSpec, RejectedCredential, ScopeDecision, parse_credentials,
    resolve_scope,
};
pub use error::ConnectorError;
pub use identity::{ConnectorId, ConnectorIdentity, CredentialId, PermissionScope};

/// Convenience re-exports of the crate's public API.
pub mod prelude {
    pub use crate::credential::{AccountCredentials, CredentialData, CredentialStore};
    pub use crate::declaration::{
        CredentialDeclaration, CredentialKind, CredentialParse, CredentialRejection,
        CredentialWarning, DegradedCredential, HeaderSpec, RejectedCredential, ScopeDecision,
        parse_credentials, resolve_scope,
    };
    pub use crate::error::ConnectorError;
    pub use crate::identity::{ConnectorId, ConnectorIdentity, CredentialId, PermissionScope};
}
