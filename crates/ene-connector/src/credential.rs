use chrono::{DateTime, Utc};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone)]
pub enum CredentialStore {
    OAuth2 {
        access_token: SecretString,
        refresh_token: Option<SecretString>,
        expires_at: Option<DateTime<Utc>>,
    },
    ApiKey(SecretString),
    None,
}

impl CredentialStore {
    pub fn is_oauth2(&self) -> bool {
        matches!(self, Self::OAuth2 { .. })
    }
    pub fn is_api_key(&self) -> bool {
        matches!(self, Self::ApiKey(_))
    }
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
    pub fn access_token(&self) -> Option<&str> {
        match self {
            Self::OAuth2 { access_token, .. } => Some(access_token.expose_secret()),
            _ => None,
        }
    }
    pub fn refresh_token(&self) -> Option<&str> {
        match self {
            Self::OAuth2 { refresh_token, .. } => {
                refresh_token.as_ref().map(ExposeSecret::expose_secret)
            }
            _ => None,
        }
    }
    pub fn expires_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Self::OAuth2 { expires_at, .. } => *expires_at,
            _ => None,
        }
    }
    pub fn is_expired(&self) -> bool {
        match self {
            Self::OAuth2 { expires_at, .. } => expires_at.is_some_and(|t| t < Utc::now()),
            _ => false,
        }
    }
    pub fn api_key(&self) -> Option<&str> {
        match self {
            Self::ApiKey(key) => Some(key.expose_secret()),
            _ => None,
        }
    }
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
    pub fn from_api_key(key: impl Into<String>) -> Self {
        Self::ApiKey(SecretString::new(key.into().into_boxed_str()))
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

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
enum CredentialData {
    OAuth2 {
        access_token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        refresh_token: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        expires_at: Option<DateTime<Utc>>,
    },
    ApiKey {
        key: String,
    },
    None,
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

impl Serialize for CredentialStore {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let data = CredentialData::from(self);
        data.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for CredentialStore {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let data = CredentialData::deserialize(deserializer)?;
        Ok(CredentialStore::from(data))
    }
}

#[derive(Debug, Clone)]
pub struct AccountCredentials {
    pub account_label: String,
    pub credential: CredentialStore,
}

impl AccountCredentials {
    pub fn new(account_label: impl Into<String>, credential: CredentialStore) -> Self {
        Self {
            account_label: account_label.into(),
            credential,
        }
    }
}
