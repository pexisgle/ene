use crate::credential::CredentialStore;
use crate::error::ConnectorError;
use crate::policy::{RateLimiter, RetryPolicy, TimeoutPolicy};
use async_trait::async_trait;
use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct ConnectorId(String);

impl ConnectorId {
    const fn valid_char(c: u8) -> bool {
        c.is_ascii_alphanumeric() || c == b'.' || c == b'_'
    }
    pub fn is_valid(id: &str) -> bool {
        if id.is_empty() {
            return false;
        }
        let bytes = id.as_bytes();
        if !bytes.iter().all(|&b| Self::valid_char(b)) {
            return false;
        }
        bytes.contains(&b'.')
    }
    pub fn try_new(id: impl Into<String>) -> Result<Self, ConnectorError> {
        let s = id.into();
        if Self::is_valid(&s) {
            Ok(Self(s))
        } else {
            Err(ConnectorError::internal(format!(
                "invalid connector ID: '{s}'"
            )))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn namespace(&self) -> &str {
        self.0.split_once('.').map_or(&self.0, |(ns, _)| ns)
    }
    pub fn name(&self) -> &str {
        self.0.split_once('.').map_or("", |(_, name)| name)
    }
}

impl fmt::Display for ConnectorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl FromStr for ConnectorId {
    type Err = ConnectorError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_new(s)
    }
}
impl From<ConnectorId> for String {
    fn from(id: ConnectorId) -> Self {
        id.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectorStatus {
    Disconnected,
    Connected,
    Degraded,
    Error(String),
}

impl ConnectorStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected | Self::Degraded)
    }
    pub fn is_error(&self) -> bool {
        matches!(self, Self::Error(_))
    }
}

impl fmt::Display for ConnectorStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Disconnected => write!(f, "disconnected"),
            Self::Connected => write!(f, "connected"),
            Self::Degraded => write!(f, "degraded"),
            Self::Error(e) => write!(f, "error: {e}"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConnectorIdentity {
    pub id: ConnectorId,
    pub display_name: String,
    pub version: Option<String>,
    pub vendor: Option<String>,
    pub base_url: Option<String>,
    pub description: Option<String>,
}

impl ConnectorIdentity {
    pub fn new(id: ConnectorId, display_name: impl Into<String>) -> Self {
        Self {
            id,
            display_name: display_name.into(),
            version: None,
            vendor: None,
            base_url: None,
            description: None,
        }
    }
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = Some(version.into());
        self
    }
    #[must_use]
    pub fn with_vendor(mut self, vendor: impl Into<String>) -> Self {
        self.vendor = Some(vendor.into());
        self
    }
    #[must_use]
    pub fn with_base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PermissionScope(String);

impl PermissionScope {
    pub fn new(scope: impl Into<String>) -> Self {
        Self(scope.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PermissionScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl From<String> for PermissionScope {
    fn from(s: String) -> Self {
        Self(s)
    }
}
impl<'a> From<&'a str> for PermissionScope {
    fn from(s: &'a str) -> Self {
        Self(s.to_owned())
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConnectorConfig {
    pub timeout: TimeoutPolicy,
    pub retry: RetryPolicy,
    pub rate_limiter: Option<(f64, f64)>,
    pub extras: std::collections::HashMap<String, String>,
}

#[async_trait]
pub trait Connector: Send + Sync {
    fn id(&self) -> &ConnectorId;
    fn name(&self) -> &str;
    fn identity(&self) -> &ConnectorIdentity;
    fn config(&self) -> &ConnectorConfig;
    fn status(&self) -> &ConnectorStatus;
    fn credentials(&self) -> &CredentialStore;
    fn scopes(&self) -> &[PermissionScope];
    async fn connect(&mut self, credentials: &CredentialStore) -> Result<(), ConnectorError>;
    async fn disconnect(&mut self) -> Result<(), ConnectorError>;
    async fn health_check(&self) -> Result<ConnectorStatus, ConnectorError> {
        Ok(self.status().clone())
    }
    fn rate_limiter(&self) -> Option<&RateLimiter>;
}
