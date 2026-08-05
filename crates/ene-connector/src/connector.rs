//! Connector lifecycle model: trait, declared actions, accounts, and health.
//!
//! A connector is a stateful integration with one external service. The
//! trait deliberately stays small — identity, declared permission surface,
//! policy, and four lifecycle operations — so concrete integrations
//! (GitHub, Discord, Slack, …) implement only what their service needs while
//! the [`ConnectorRegistry`](crate::registry::ConnectorRegistry) provides
//! registration, permission gating, timeout wrapping, and event emission for
//! free.

use crate::credential::AccountCredentials;
use crate::error::ConnectorError;
use crate::identity::ConnectorIdentity;
use crate::policy::{ConnectorPolicy, RateLimitPolicy, RetryPolicy};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::time::Duration;

/// Canonical framework-level action names.
///
/// Connector-specific actions are declared per connector via
/// [`Connector::actions`] and are also gated per action; these two names
/// cover the lifecycle operations every connector shares.
pub mod actions {
    /// Establishing an authenticated session.
    pub const CONNECT: &str = "connector.connect";
    /// Tearing down an authenticated session.
    pub const DISCONNECT: &str = "connector.disconnect";
}

/// A permission surface entry declared by a connector.
///
/// Declared actions are displayed in permission prompts and are grantable /
/// revocable individually through the registry. `requires_approval` marks
/// side-effecting actions; read-only actions still honor an explicit grant
/// but never prompt on their own.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorAction {
    /// Stable action name (e.g. `send_message`).
    pub name: &'static str,
    /// Human-readable description for the permission prompt.
    pub description: &'static str,
    /// Whether the action prompts for approval by default.
    pub requires_approval: bool,
}

impl ConnectorAction {
    /// Declares a side-effecting action that prompts for approval.
    #[must_use]
    pub const fn side_effecting(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            requires_approval: true,
        }
    }

    /// Declares a read-only action that never prompts on its own.
    #[must_use]
    pub const fn read_only(name: &'static str, description: &'static str) -> Self {
        Self {
            name,
            description,
            requires_approval: false,
        }
    }
}

/// How an authenticated account authenticates with the service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountAuthKind {
    /// `OAuth2` token flow.
    OAuth2,
    /// Static API key.
    ApiKey,
    /// Local credential helper (keychain, agent, …).
    LocalHelper,
}

/// An authenticated account exposed by a connected connector.
///
/// Carries no secret material — only stable identifiers and display
/// metadata — so it is safe to cache, export, and display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedAccount {
    /// Stable account id within the connector (e.g. an email address).
    pub id: String,
    /// Human-readable account label.
    pub label: String,
    /// Authentication kind used for this account.
    pub auth: AccountAuthKind,
    /// Permission scopes granted during authentication.
    pub scopes: Vec<String>,
    /// When the account was authenticated.
    pub connected_at: DateTime<Utc>,
}

impl AuthenticatedAccount {
    /// Creates an account snapshot.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        label: impl Into<String>,
        auth: AccountAuthKind,
        scopes: Vec<String>,
        connected_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            auth,
            scopes,
            connected_at,
        }
    }
}

/// Result of a connectivity check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthStatus {
    /// Whether the service is reachable and the credential state is usable.
    pub healthy: bool,
    /// Optional detail; scrubbed at the registry boundary.
    pub message: Option<String>,
    /// When the check ran.
    pub checked_at: DateTime<Utc>,
}

/// Connection state of a registered connector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// No authenticated session.
    Disconnected,
    /// At least one authenticated account.
    Connected {
        /// When the session was established.
        at: DateTime<Utc>,
    },
    /// The last lifecycle operation failed.
    Error {
        /// Scrubbed failure detail.
        message: String,
    },
}

/// Cached snapshot of a registered connector.
///
/// Built and refreshed by the registry from lifecycle operation results;
/// read-only consumers never trigger connector I/O through this type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorStatus {
    /// Display metadata.
    pub identity: ConnectorIdentity,
    /// Current connection state.
    pub connection: ConnectionState,
    /// Last connectivity check result, when one ran.
    pub health: Option<HealthStatus>,
    /// Authenticated accounts.
    pub accounts: Vec<AuthenticatedAccount>,
}

/// Lightweight entry for [`ConnectorRegistry::list`], I/O-free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectorSummary {
    /// Display metadata.
    pub identity: ConnectorIdentity,
    /// Current connection state.
    pub connection: ConnectionState,
    /// Number of authenticated accounts.
    pub account_count: usize,
    /// Number of declared actions.
    pub action_count: usize,
}

/// A standing per-action permission grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionGrant {
    /// Granted action name.
    pub action: String,
    /// Target prefix the grant covers.
    pub target_pattern: String,
    /// When the grant was recorded.
    pub granted_at: DateTime<Utc>,
}

/// An external-service connector.
///
/// Implementations are responsible for their own transport; the registry
/// wraps every operation in the [`ConnectorPolicy::timeout`] and routes
/// permission decisions through the shared gate. Secrets are handled
/// exclusively via [`AccountCredentials`] and must never be placed in
/// status messages, events, or error strings.
#[async_trait]
pub trait Connector: Send + Sync {
    /// Stable identity and display metadata.
    fn identity(&self) -> &ConnectorIdentity;

    /// Declared permission surface.
    fn actions(&self) -> &'static [ConnectorAction];

    /// Transport policy applied to lifecycle operations by the registry.
    fn policy(&self) -> ConnectorPolicy {
        ConnectorPolicy::default()
    }

    /// Probes service reachability without authenticating.
    ///
    /// # Errors
    /// Returns a transport or timeout error when the service is unreachable.
    async fn check_connectivity(&self) -> Result<HealthStatus, ConnectorError>;

    /// Authenticates with the service and returns the accessible accounts.
    ///
    /// # Errors
    /// Returns [`ConnectorError::Auth`] when the credential is rejected and
    /// [`ConnectorError::PermissionRequired`] when the user has not approved
    /// the connect action.
    async fn connect(
        &self,
        credential: &AccountCredentials,
    ) -> Result<Vec<AuthenticatedAccount>, ConnectorError>;

    /// Ends the authenticated session for one account.
    ///
    /// # Errors
    /// Returns [`ConnectorError::PermissionRequired`] when the user has not
    /// approved the disconnect action.
    async fn disconnect(&self, account: &AuthenticatedAccount) -> Result<(), ConnectorError>;
}

/// Default policy helpers shared by connectors.
impl ConnectorPolicy {
    /// A 15-second per-operation timeout with no retries or rate limiting.
    #[must_use]
    pub const fn default() -> Self {
        Self {
            timeout: Duration::from_secs(15),
            retry: None,
            rate_limit: None,
        }
    }

    /// Sets the per-operation timeout.
    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Sets the backoff retry policy.
    #[must_use]
    pub const fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = Some(retry);
        self
    }

    /// Sets the rate-limit policy.
    #[must_use]
    pub const fn with_rate_limit(mut self, rate_limit: RateLimitPolicy) -> Self {
        self.rate_limit = Some(rate_limit);
        self
    }
}
