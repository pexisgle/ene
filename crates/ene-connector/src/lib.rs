//! # ene-connector
//!
//! Shared connector framework for external service integrations.
//!
//! This crate provides a reusable framework for building connectors to
//! external services such as Calendar, MCP, GitHub, and more. It manages:
//!
//! - Connector identity, authenticated accounts, and health status
//! - OAuth token management and API key storage (secrets are never logged)
//! - Common policies: timeouts, retry with backoff, rate limiting, pagination
//! - Integration points for the permission center and audit events
//!
//! # Architecture
//!
//! - [`Connector`] — the trait that all external service connectors implement
//! - [`ConnectorRegistry`] — thread-safe registry for managing connector lifecycle
//! - [`CredentialStore`] — secure OAuth2/API key storage (zeroed on drop)
//! - [`RetryPolicy`], [`RateLimiter`], [`TimeoutPolicy`] — composable policy types
//! - [`ConnectorError`] — unified error type
#![expect(missing_docs, reason = "crate under active development")]
#![cfg_attr(
    test,
    expect(clippy::unwrap_used, reason = "unit tests use unwrap for assertions")
)]

pub mod connector;
pub mod credential;
pub mod error;
pub mod policy;
pub mod registry;

pub use connector::{
    Connector, ConnectorConfig, ConnectorId, ConnectorIdentity, ConnectorStatus, PermissionScope,
};
pub use credential::{AccountCredentials, CredentialData, CredentialStore};
pub use error::ConnectorError;
pub use policy::{
    PaginationCursor, RateLimitCounter, RateLimiter, RequestTimeout, RetryPolicy, TimeoutPolicy,
};
pub use registry::ConnectorRegistry;

pub mod prelude {
    pub use crate::connector::{
        Connector, ConnectorConfig, ConnectorId, ConnectorIdentity, ConnectorStatus,
        PermissionScope,
    };
    pub use crate::credential::{AccountCredentials, CredentialData, CredentialStore};
    pub use crate::error::ConnectorError;
    pub use crate::policy::{
        PaginationCursor, RateLimiter, RequestTimeout, RetryPolicy, TimeoutPolicy,
    };
    pub use crate::registry::ConnectorRegistry;
}

pub use async_trait::async_trait;
