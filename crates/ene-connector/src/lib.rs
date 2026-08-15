//! # ene-connector
//!
//! Secure connector framework for external-service integrations.
//!
//! This crate owns the *connector* side of connecting to external services
//! (Calendar, GitHub, Discord, and the like): the [`Connector`] trait and
//! [`ConnectorRegistry`] lifecycle, common transport policies (timeout,
//! backoff retry, rate limiting, pagination), webhook validation, a
//! fail-closed per-action [`PermissionGate`], structural secret scrubbing,
//! and the secure `OAuth2` / API-key storage whose secrets are redacted from
//! `Debug`/`Serialize` and zeroed on drop.
//!
//! # Architecture
//!
//! - [`CredentialStore`] / [`CredentialData`] / [`AccountCredentials`] — secure
//!   credential storage (secrets never logged; raw material reachable only via
//!   the audited [`CredentialStore::expose_for_persistence`] path).
//! - [`Connector`] / [`ConnectorRegistry`] — the common lifecycle API:
//!   registration, connectivity checks, connect/disconnect, per-action
//!   permission grants, and status snapshots, with every operation wrapped
//!   in the connector's timeout policy and gated deny-by-default.
//! - [`ConnectorId`] — stable `namespace.name` identifier.
//! - [`CredentialId`] — stable identifier for a stored credential (no
//!   namespace required).
//! - [`PermissionScope`] — an OAuth scope requested by a connector.
//! - [`ConnectorIdentity`] — display metadata for configuration UIs.
//! - [`PermissionGate`] — per-connector fail-closed permission gate with
//!   turn-scoped approvals and conversation-scoped action patterns.
//! - [`policy`] — timeout / retry / rate-limit / pagination policies.
//! - [`webhook`] — HMAC signature and replay-window validation.
//! - [`redaction`] — structural secret scrubbing at event, audit, and error
//!   boundaries.
//! - [`CredentialDeclaration`] / [`resolve_scope`] — parsing of a plugin's
//!   `x-ene-credentials` declarations and scoped access resolution.
//! - [`ConnectorError`] — unified error type.
//!
//! Plugin process supervision and the MCP bridge's SSRF URL validation live
//! in `ene-plugin-host`; this crate stays the connector and credential
//! authority.
#![cfg_attr(
    test,
    expect(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        reason = "unit/integration tests use unwrap/expect/panic for concise assertions"
    )
)]

pub mod connector;
pub mod credential;
pub mod declaration;
pub mod error;
pub mod gate;
pub mod identity;
pub mod policy;
pub mod redaction;
pub mod registry;
pub mod webhook;

pub use connector::{
    AccountAuthKind, AuthenticatedAccount, ConnectionState, Connector, ConnectorAction,
    ConnectorStatus, ConnectorSummary, HealthStatus, PermissionGrant, actions,
};
pub use credential::{AccountCredentials, CredentialData, CredentialStore};
pub use declaration::{
    CredentialDeclaration, CredentialKind, CredentialParse, CredentialRejection, CredentialWarning,
    DegradedCredential, HeaderSpec, RejectedCredential, ScopeDecision, parse_credentials,
    resolve_scope,
};
pub use error::ConnectorError;
pub use gate::PermissionGate;
pub use identity::{ConnectorId, ConnectorIdentity, CredentialId, PermissionScope};
pub use policy::{
    ConnectorPolicy, Page, PaginationPolicy, RateLimitPolicy, RateLimiter, RetryPolicy,
    backoff_delay, collect_pages, retry_with_backoff,
};
pub use redaction::{redact_json, scrub_secrets};
pub use registry::{AccountRef, ConnectorEvent, ConnectorEventKind, ConnectorRegistry};
pub use webhook::WebhookValidator;

pub mod prelude {
    pub use crate::connector::{
        AccountAuthKind, AuthenticatedAccount, ConnectionState, Connector, ConnectorAction,
        ConnectorStatus, ConnectorSummary, HealthStatus, PermissionGrant, actions,
    };
    pub use crate::credential::{AccountCredentials, CredentialData, CredentialStore};
    pub use crate::declaration::{
        CredentialDeclaration, CredentialKind, CredentialParse, CredentialRejection,
        CredentialWarning, DegradedCredential, HeaderSpec, RejectedCredential, ScopeDecision,
        parse_credentials, resolve_scope,
    };
    pub use crate::error::ConnectorError;
    pub use crate::gate::PermissionGate;
    pub use crate::identity::{ConnectorId, ConnectorIdentity, CredentialId, PermissionScope};
    pub use crate::policy::{
        ConnectorPolicy, Page, PaginationPolicy, RateLimitPolicy, RateLimiter, RetryPolicy,
        backoff_delay, collect_pages, retry_with_backoff,
    };
    pub use crate::redaction::{redact_json, scrub_secrets};
    pub use crate::registry::{AccountRef, ConnectorEvent, ConnectorEventKind, ConnectorRegistry};
    pub use crate::webhook::WebhookValidator;
}
