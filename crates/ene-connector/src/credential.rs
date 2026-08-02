//! Secure credential storage for external-service connectors.

use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// An in-memory credential, held securely and redacted on display/serialize.
///
/// Secrets are wrapped in [`SecretString`], which zeroes its buffer on drop and
/// is redacted by both the [`Debug`](std::fmt::Debug) and [`Serialize`] impls.
/// Raw material is reachable only through the explicit, audited
/// [`expose_for_persistence`](Self::expose_for_persistence) path.
#[derive(Clone)]
pub enum CredentialStore {
    /// An `OAuth2` token set.
    OAuth2 {
        /// `OAuth2` access token (secret).
        access_token: SecretString,
        /// `OAuth2` refresh token, if issued (secret).
        refresh_token: Option<SecretString>,
        /// Access-token expiry, if known (not secret).
        expires_at: Option<DateTime<Utc>>,
    },
    /// A bare API key (secret).
    ApiKey(SecretString),
    /// No credential.
    None,
}

impl CredentialStore {
    /// Returns `true` for the [`OAuth2`](Self::OAuth2) variant.
    #[must_use]
    pub fn is_oauth2(&self) -> bool {
        matches!(self, Self::OAuth2 { .. })
    }

    /// Returns `true` for the [`ApiKey`](Self::ApiKey) variant.
    #[must_use]
    pub fn is_api_key(&self) -> bool {
        matches!(self, Self::ApiKey(_))
    }

    /// Returns `true` for the [`None`](Self::None) variant.
    #[must_use]
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Returns the `OAuth2` access token, if this is an [`OAuth2`](Self::OAuth2)
    /// credential.
    #[must_use]
    pub fn access_token(&self) -> Option<&str> {
        match self {
            Self::OAuth2 { access_token, .. } => Some(access_token.expose_secret()),
            _ => None,
        }
    }

    /// Returns the `OAuth2` refresh token, if present.
    #[must_use]
    pub fn refresh_token(&self) -> Option<&str> {
        match self {
            Self::OAuth2 { refresh_token, .. } => {
                refresh_token.as_ref().map(ExposeSecret::expose_secret)
            }
            _ => None,
        }
    }

    /// Returns the `OAuth2` access-token expiry, if known.
    #[must_use]
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::OAuth2 { expires_at, .. } => *expires_at,
            _ => None,
        }
    }

    /// Returns `true` when an `OAuth2` credential is past its expiry.
    ///
    /// Credentials without an expiry (and non-`OAuth2` variants) are never
    /// considered expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        match self {
            Self::OAuth2 { expires_at, .. } => expires_at.is_some_and(|t| t < Utc::now()),
            _ => false,
        }
    }

    /// Returns `true` when an `OAuth2` credential expires within `within`.
    ///
    /// The lead time lets a refresher re-issue the token *before* it lapses
    /// instead of discovering the expiry mid-request. Credentials without an
    /// expiry (and non-`OAuth2` variants) are never considered imminent.
    #[must_use]
    pub fn expires_within(&self, within: Duration) -> bool {
        match self {
            Self::OAuth2 { expires_at, .. } => expires_at.is_some_and(|t| {
                t <= Utc::now() + chrono::Duration::from_std(within).unwrap_or_default()
            }),
            _ => false,
        }
    }

    /// Returns the API key, if this is an [`ApiKey`](Self::ApiKey) credential.
    #[must_use]
    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(key) => Some(key.expose_secret()),
            _ => None,
        }
    }

    /// Builds an [`OAuth2`](Self::OAuth2) credential from raw tokens.
    #[must_use]
    pub fn oauth2(
        access_token: impl Into<String>,
        refresh_token: Option<impl Into<String>>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self::OAuth2 {
            access_token: SecretString::new(access_token.into().into_boxed_str()),
            refresh_token: refresh_token.map(|t| SecretString::new(t.into().into_boxed_str())),
            expires_at,
        }
    }

    /// Builds an [`ApiKey`](Self::ApiKey) credential from a raw key.
    #[must_use]
    pub fn from_api_key(key: impl Into<String>) -> Self {
        Self::ApiKey(SecretString::new(key.into().into_boxed_str()))
    }

    /// Exposes the raw credential for the audited persistence path only.
    ///
    /// This is the **only** sanctioned way to obtain secret material for
    /// secure storage (e.g. an OS keychain). The returned [`CredentialData`]
    /// serializes raw tokens; callers must route it to a secure store and must
    /// never log it or place it in ordinary state. Ordinary [`Serialize`]
    /// always redacts instead.
    pub fn expose_for_persistence(&self) -> CredentialData {
        CredentialData::from(self)
    }

    /// Restores a credential from a previously exported [`CredentialData`].
    ///
    /// Inverse of [`expose_for_persistence`](Self::expose_for_persistence);
    /// used when loading secrets back out of secure storage.
    pub fn from_exported(data: CredentialData) -> Self {
        CredentialStore::from(data)
    }
}

impl fmt::Debug for CredentialStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OAuth2 { expires_at, .. } => f
                .debug_struct("OAuth2")
                .field("access_token", &"<redacted>")
                .field("refresh_token", &"<redacted>")
                .field("expires_at", expires_at)
                .finish(),
            Self::ApiKey(_) => f.debug_tuple("ApiKey").field(&"<redacted>").finish(),
            Self::None => f.debug_tuple("None").finish(),
        }
    }
}

/// Secret-bearing export form for the audited persistence path only.
///
/// Unlike the blanket [`Serialize`] impl on [`CredentialStore`] — which always
/// redacts — this type intentionally carries raw tokens. Obtain it exclusively
/// through [`CredentialStore::expose_for_persistence`] and route the serialized
/// output through a secure, audited store (e.g. an OS keychain). Never log it
/// or embed it in ordinary state dumps.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum CredentialData {
    /// `OAuth2` token set (raw).
    OAuth2 {
        /// Raw access token.
        access_token: String,
        /// Raw refresh token, if any.
        #[serde(skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        /// Expiry timestamp (not secret).
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<DateTime<Utc>>,
    },
    /// Raw API key.
    ApiKey {
        /// Raw API key.
        key: String,
    },
    /// No credential.
    None,
}

/// Redacted serialization form used by the blanket [`Serialize`]/[`Deserialize`]
/// impls on [`CredentialStore`].
///
/// This deliberately omits every secret so that accidental serialization
/// (diagnostics, state snapshots, derived impls on containing structs) can never
/// leak a token. Only the credential variant — and, for `OAuth2`, the non-secret
/// expiry — survives a redacted round-trip.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
enum RedactedCredential {
    OAuth2 {
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<DateTime<Utc>>,
    },
    ApiKey,
    None,
}

impl From<&CredentialStore> for RedactedCredential {
    fn from(store: &CredentialStore) -> Self {
        match store {
            CredentialStore::OAuth2 { expires_at, .. } => RedactedCredential::OAuth2 {
                expires_at: *expires_at,
            },
            CredentialStore::ApiKey(_) => RedactedCredential::ApiKey,
            CredentialStore::None => RedactedCredential::None,
        }
    }
}

impl From<RedactedCredential> for CredentialStore {
    fn from(data: RedactedCredential) -> Self {
        // The redacted form carries no secret; rebuild a placeholder of the
        // same variant so type information survives without exposing tokens.
        match data {
            RedactedCredential::OAuth2 { expires_at } => CredentialStore::OAuth2 {
                access_token: SecretString::new(String::new().into_boxed_str()),
                refresh_token: None,
                expires_at,
            },
            RedactedCredential::ApiKey => {
                CredentialStore::ApiKey(SecretString::new(String::new().into_boxed_str()))
            }
            RedactedCredential::None => CredentialStore::None,
        }
    }
}

impl From<&CredentialStore> for CredentialData {
    fn from(store: &CredentialStore) -> Self {
        match store {
            CredentialStore::OAuth2 {
                access_token,
                refresh_token,
                expires_at,
            } => CredentialData::OAuth2 {
                access_token: access_token.expose_secret().to_owned(),
                refresh_token: refresh_token
                    .as_ref()
                    .map(ExposeSecret::expose_secret)
                    .map(ToOwned::to_owned),
                expires_at: *expires_at,
            },
            CredentialStore::ApiKey(key) => CredentialData::ApiKey {
                key: key.expose_secret().to_owned(),
            },
            CredentialStore::None => CredentialData::None,
        }
    }
}

impl From<CredentialData> for CredentialStore {
    fn from(data: CredentialData) -> Self {
        match data {
            CredentialData::OAuth2 {
                access_token,
                refresh_token,
                expires_at,
            } => CredentialStore::OAuth2 {
                access_token: SecretString::new(access_token.into_boxed_str()),
                refresh_token: refresh_token.map(|t| SecretString::new(t.into_boxed_str())),
                expires_at,
            },
            CredentialData::ApiKey { key } => {
                CredentialStore::ApiKey(SecretString::new(key.into_boxed_str()))
            }
            CredentialData::None => CredentialStore::None,
        }
    }
}

/// Serializes **without** secrets.
///
/// The blanket impl deliberately redacts: any code path that serializes a
/// [`CredentialStore`] (diagnostics, derived impls on containing structs, state
/// snapshots) receives only the credential variant and non-secret metadata.
/// Raw tokens are reachable only through the explicit, audited
/// [`expose_for_persistence`](CredentialStore::expose_for_persistence) path.
impl Serialize for CredentialStore {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        RedactedCredential::from(self).serialize(serializer)
    }
}

/// Deserializes the redacted form (no secrets are recovered).
///
/// To restore a real credential from secure storage, deserialize a
/// [`CredentialData`] and convert it via
/// [`from_exported`](CredentialStore::from_exported).
impl<'de> serde::Deserialize<'de> for CredentialStore {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let data = RedactedCredential::deserialize(deserializer)?;
        Ok(CredentialStore::from(data))
    }
}

/// A credential paired with a human-readable account label.
///
/// Used when a connector manages multiple authenticated accounts (e.g. several
/// calendars) and needs to distinguish them in a UI. The [`Debug`] impl
/// redacts the underlying secret via [`CredentialStore`].
#[derive(Debug, Clone)]
pub struct AccountCredentials {
    /// Human-readable label for the account (e.g. an email address).
    pub account_label: String,
    /// The account's credential.
    pub credential: CredentialStore,
}

impl AccountCredentials {
    /// Creates an account/credential pair.
    #[must_use]
    pub fn new(account_label: impl Into<String>, credential: CredentialStore) -> Self {
        Self {
            account_label: account_label.into(),
            credential,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_secrets() {
        let cred = CredentialStore::from_api_key("super-secret-key");
        let debug_output = format!("{cred:?}");
        assert!(!debug_output.contains("super-secret-key"));
        assert!(debug_output.contains("<redacted>"));

        let oauth = CredentialStore::oauth2("access-token", Some("refresh-token"), None);
        let debug_output = format!("{oauth:?}");
        assert!(!debug_output.contains("access-token"));
        assert!(!debug_output.contains("refresh-token"));
        assert!(debug_output.contains("<redacted>"));
    }

    #[test]
    fn serialize_does_not_leak_secrets() {
        let cred = CredentialStore::from_api_key("super-secret-key");
        let json = serde_json::to_string(&cred).unwrap();
        assert!(!json.contains("super-secret-key"));
        assert!(json.contains("api_key"));

        let oauth = CredentialStore::oauth2("access-token", Some("refresh-token"), None);
        let json = serde_json::to_string(&oauth).unwrap();
        assert!(!json.contains("access-token"));
        assert!(!json.contains("refresh-token"));
        assert!(json.contains("o_auth2"));
    }

    #[test]
    fn expose_for_persistence_returns_raw_secrets() {
        let cred = CredentialStore::from_api_key("super-secret-key");
        let data = cred.expose_for_persistence();
        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("super-secret-key"));
    }

    #[test]
    fn roundtrip_via_exported_preserves_secrets() {
        let original = CredentialStore::from_api_key("super-secret-key");
        let exported = original.expose_for_persistence();
        let restored = CredentialStore::from_exported(exported);
        assert_eq!(restored.api_key(), Some("super-secret-key"));

        let oauth_original = CredentialStore::oauth2("access-token", Some("refresh-token"), None);
        let oauth_exported = oauth_original.expose_for_persistence();
        let oauth_restored = CredentialStore::from_exported(oauth_exported);
        assert_eq!(oauth_restored.access_token(), Some("access-token"));
        assert_eq!(oauth_restored.refresh_token(), Some("refresh-token"));
    }

    #[test]
    fn is_expired_works() {
        use chrono::Duration;

        let past = Utc::now() - Duration::hours(1);
        let expired = CredentialStore::oauth2("token", None::<&str>, Some(past));
        assert!(expired.is_expired());

        let future = Utc::now() + Duration::hours(1);
        let valid = CredentialStore::oauth2("token", None::<&str>, Some(future));
        assert!(!valid.is_expired());

        let no_expiry = CredentialStore::oauth2("token", None::<&str>, None);
        assert!(!no_expiry.is_expired());

        let api_key = CredentialStore::from_api_key("key");
        assert!(!api_key.is_expired());
    }

    #[test]
    fn expires_within_detects_imminent_expiry() {
        use std::time::Duration;

        let soon = Utc::now() + chrono::Duration::seconds(30);
        let imminent = CredentialStore::oauth2("token", None::<&str>, Some(soon));
        assert!(imminent.expires_within(Duration::from_mins(1)));

        let later = Utc::now() + chrono::Duration::minutes(10);
        let distant = CredentialStore::oauth2("token", None::<&str>, Some(later));
        assert!(!distant.expires_within(Duration::from_mins(1)));

        let expired = CredentialStore::oauth2("token", None::<&str>, Some(Utc::now()));
        assert!(expired.expires_within(Duration::from_mins(1)));

        let no_expiry = CredentialStore::oauth2("token", None::<&str>, None);
        assert!(!no_expiry.expires_within(Duration::from_mins(1)));

        let api_key = CredentialStore::from_api_key("key");
        assert!(!api_key.expires_within(Duration::from_mins(1)));
    }
}
