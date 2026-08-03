//! Credential host-service wire types.
//!
//! These types travel between a plugin and the host's `credential` passenger
//! once [`crate::HostServiceId::Credential`] is opened. Secrets are
//! transported in [`WireSecret`], whose `Serialize`/`Deserialize` carry the
//! raw value but whose [`Debug`] and [`Display`] render `<redacted>` — a
//! secret that reaches a log, audit record, or error message is redacted by
//! construction.

use crate::frame::{read_framed_json, write_framed_json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use tokio::io::{AsyncRead, AsyncWrite};

/// A secret transported over the credential service wire.
///
/// `Serialize`/`Deserialize` carry the raw value for transport only;
/// [`Debug`] and [`Display`] render `<redacted>`. Never place an instance in
/// a log, audit record, or error message — the redaction contract lives in
/// the type so a mistake cannot leak the value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct WireSecret(String);

impl WireSecret {
    /// Wraps a raw secret value.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the raw value (transport / SDK handoff only).
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for WireSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl fmt::Display for WireSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl Serialize for WireSecret {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for WireSecret {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self)
    }
}

/// Requests sent by a plugin to the `credential` passenger after `Open`.
///
/// Single-flight by design: one request is in flight at a time per
/// connection, and the response order disambiguates replies (parallel
/// requests are re-evaluated after the IPC serialization rework).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialRequest {
    /// Liveness probe.
    Ping,
    /// Resolves a credential by id (a plugin name such as `anthropic` or a
    /// `namespace.name` connector id such as `google.calendar`).
    Resolve {
        /// Credential id.
        id: String,
    },
    /// Asks the host to start the authorization flow for `id` (the
    /// browser/redirect/token-exchange flow is host-owned; the host responds
    /// [`CredentialResponse::AuthorizationPending`] until then).
    RequestAuthorization {
        /// Credential id.
        id: String,
    },
}

/// Responses from the `credential` passenger.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CredentialResponse {
    /// Reply to [`CredentialRequest::Ping`].
    Pong,
    /// A credential resolved successfully.
    Resolved {
        /// The resolved credential.
        credential: ResolvedCredential,
    },
    /// The host accepted the authorization request; the flow completes
    /// out-of-band and the credential arrives via a later invalidation.
    AuthorizationPending,
    /// Server-initiated invalidation notice: the client should drop its
    /// cached copies of `ids` (one or more credentials were updated or
    /// revoked).
    Invalidated {
        /// Credential ids whose cached values are stale.
        ids: Vec<String>,
    },
    /// The request failed.
    Error {
        /// Structured error code.
        code: CredentialErrorCode,
        /// Human-readable detail (never contains a secret).
        message: String,
    },
}

/// Header injection specification carried alongside a resolved API key.
///
/// Mirrors the declaration's `header` block ([`HeaderSpec`] in
/// `ene-connector`) so the plugin-side client can apply the declared header
/// name/format without re-parsing schemas. `format` is a `{value}` template
/// the client substitutes the secret into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireHeaderSpec {
    /// Header name, e.g. `x-api-key` or `Authorization`.
    pub name: String,
    /// Template containing the `{value}` placeholder, e.g. `Bearer {value}`.
    pub format: String,
}

/// A resolved credential payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResolvedCredential {
    /// A bare API key (`x-api-key`-style authentication).
    ApiKey {
        /// The API key (secret).
        key: WireSecret,
        /// Declared header override for the client to inject; `None` means
        /// the client falls back to `x-api-key` + the raw value.
        #[serde(default)]
        header: Option<WireHeaderSpec>,
    },
    /// A bearer/access token (`Authorization: Bearer`-style authentication).
    Bearer {
        /// The access token (secret).
        token: WireSecret,
        /// Access-token expiry, when known (not secret).
        #[serde(default)]
        expires_at: Option<DateTime<Utc>>,
    },
}

/// Structured errors for credential requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialErrorCode {
    /// The credential does not exist; `label`/`help_url` are non-secret
    /// display metadata for guiding the user to a setup UI.
    Missing {
        /// Non-secret display label.
        label: String,
        /// Optional URL pointing at setup/help UI.
        #[serde(default)]
        help_url: Option<String>,
    },
    /// The requested id is outside the plugin's declared scope.
    ScopeDenied,
    /// The credential expired and needs re-authorization.
    RefreshRequired,
    /// The requested operation is not supported by this host.
    Unsupported,
    /// An internal host error occurred.
    Internal,
    /// The host returned an unknown code (forward compatibility for new
    /// hosts talking to older clients). Unknown tags deserialize to this
    /// variant instead of failing.
    #[serde(other)]
    Unknown,
}

/// Writes a length-prefixed JSON [`CredentialRequest`].
pub async fn write_credential_request<W: AsyncWrite + Unpin>(
    writer: &mut W,
    request: &CredentialRequest,
) -> std::io::Result<()> {
    write_framed_json(writer, request).await
}

/// Reads a length-prefixed JSON [`CredentialRequest`].
pub async fn read_credential_request<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<CredentialRequest>> {
    read_framed_json(reader).await
}

/// Writes a length-prefixed JSON [`CredentialResponse`].
pub async fn write_credential_response<W: AsyncWrite + Unpin>(
    writer: &mut W,
    response: &CredentialResponse,
) -> std::io::Result<()> {
    write_framed_json(writer, response).await
}

/// Reads a length-prefixed JSON [`CredentialResponse`].
pub async fn read_credential_response<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<CredentialResponse>> {
    read_framed_json(reader).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_secret_debug_and_display_redact() {
        let secret = WireSecret::new("super-secret-value");
        let debug = format!("{secret:?}");
        let display = format!("{secret}");
        assert!(!debug.contains("super-secret-value"));
        assert!(debug.contains("<redacted>"));
        assert!(!display.contains("super-secret-value"));
        assert!(display.contains("<redacted>"));
    }

    #[test]
    fn wire_secret_serializes_raw_for_transport() {
        let secret = WireSecret::new("super-secret-value");
        let json = serde_json::to_string(&secret).unwrap();
        assert_eq!(json, "\"super-secret-value\"");
        let back: WireSecret = serde_json::from_str(&json).unwrap();
        assert_eq!(back, secret);
    }

    #[test]
    fn resolved_api_key_roundtrip() {
        let resolved = ResolvedCredential::ApiKey {
            key: WireSecret::new("sk-test"),
            header: None,
        };
        let json = serde_json::to_string(&resolved).unwrap();
        assert!(json.contains("api_key"));
        assert!(json.contains("sk-test"));
        let back: ResolvedCredential = serde_json::from_str(&json).unwrap();
        assert_eq!(back, resolved);
    }

    #[test]
    fn resolved_api_key_without_header_deserializes() {
        // Wire compatibility: a host built before the header field existed
        // omits it entirely; the `#[serde(default)]` must keep old frames
        // decoding.
        let json = r#"{"kind":"api_key","key":"sk-old-host"}"#;
        let resolved: ResolvedCredential = serde_json::from_str(json).unwrap();
        assert_eq!(
            resolved,
            ResolvedCredential::ApiKey {
                key: WireSecret::new("sk-old-host"),
                header: None,
            }
        );
    }

    #[test]
    fn resolved_api_key_header_roundtrips() {
        let resolved = ResolvedCredential::ApiKey {
            key: WireSecret::new("sk-test"),
            header: Some(WireHeaderSpec {
                name: "X-Custom-Auth".into(),
                format: "Bearer {value}".into(),
            }),
        };
        let json = serde_json::to_string(&resolved).unwrap();
        assert!(json.contains("X-Custom-Auth"));
        let back: ResolvedCredential = serde_json::from_str(&json).unwrap();
        assert_eq!(back, resolved);
    }

    #[test]
    fn resolved_bearer_without_expiry_deserializes() {
        let json = r#"{"kind":"bearer","token":"tok-1"}"#;
        let resolved: ResolvedCredential = serde_json::from_str(json).unwrap();
        assert_eq!(
            resolved,
            ResolvedCredential::Bearer {
                token: WireSecret::new("tok-1"),
                expires_at: None,
            }
        );
    }

    #[test]
    fn error_codes_serialize_snake_case() {
        let json = serde_json::to_string(&CredentialErrorCode::ScopeDenied).unwrap();
        assert_eq!(json, "\"scope_denied\"");
        let json = serde_json::to_string(&CredentialErrorCode::RefreshRequired).unwrap();
        assert_eq!(json, "\"refresh_required\"");
        let json = serde_json::to_string(&CredentialErrorCode::Missing {
            label: "Anthropic API Key".into(),
            help_url: None,
        })
        .unwrap();
        assert!(json.contains("missing"));
        assert!(json.contains("Anthropic API Key"));
    }

    #[test]
    fn unknown_error_code_deserializes_as_unknown() {
        // Forward compatibility: a new host can emit a code an old client has
        // never seen; it must deserialize to Unknown instead of failing.
        let json = r#"{"Error":{"code":"brand_new_code","message":"boom"}}"#;
        let resp: CredentialResponse = serde_json::from_str(json).unwrap();
        assert!(matches!(
            resp,
            CredentialResponse::Error {
                code: CredentialErrorCode::Unknown,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn credential_request_roundtrip() {
        let (mut a, mut b) = tokio::io::duplex(4096);
        let req = CredentialRequest::Resolve {
            id: "anthropic".into(),
        };
        write_credential_request(&mut a, &req).await.unwrap();
        drop(a);
        let got = read_credential_request(&mut b).await.unwrap().unwrap();
        assert_eq!(got, req);
    }
}
