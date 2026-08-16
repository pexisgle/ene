use crate::error::ConnectorError;
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, KeyInit, Mac};
use secrecy::{ExposeSecret, SecretString};
use sha2::Sha256;
use std::time::Duration as StdDuration;

type HmacSha256 = Hmac<Sha256>;

/// Validates webhook deliveries before they reach connector handlers.
///
/// The signature format follows the `sha256=<hex>` convention: the HMAC is
/// computed over `"{timestamp}.{body}"` with the shared secret, and the
/// timestamp must fall inside the replay window. The secret is held as a
/// [`SecretString`] and never appears in error output.
#[derive(Debug, Clone)]
pub struct WebhookValidator {
    secret: SecretString,
    max_age: StdDuration,
}

impl WebhookValidator {
    #[must_use]
    pub fn new(secret: SecretString, max_age: StdDuration) -> Self {
        Self { secret, max_age }
    }

    /// `signature` is the `sha256=<hex>` header value, `timestamp` an RFC 3339
    /// instant, and `body` the raw request body. Comparison is constant-time.
    ///
    /// # Errors
    /// Returns [`ConnectorError::WebhookRejected`] when the signature is
    /// malformed or invalid, or when the timestamp is outside the replay
    /// window.
    pub fn validate(
        &self,
        signature: &str,
        timestamp: &str,
        body: &[u8],
        now: DateTime<Utc>,
    ) -> Result<(), ConnectorError> {
        let received_at = DateTime::parse_from_rfc3339(timestamp)
            .map_err(|_| ConnectorError::webhook_rejected("malformed timestamp"))?
            .with_timezone(&Utc);
        let age = now.signed_duration_since(received_at);
        if age
            > Duration::from_std(self.max_age)
                .map_err(|_| ConnectorError::webhook_rejected("invalid replay window"))?
            || age
                < -Duration::from_std(self.max_age)
                    .map_err(|_| ConnectorError::webhook_rejected("invalid replay window"))?
        {
            return Err(ConnectorError::webhook_rejected(
                "timestamp outside the replay window",
            ));
        }

        let Some(provided) = signature.strip_prefix("sha256=") else {
            return Err(ConnectorError::webhook_rejected("malformed signature"));
        };
        let mut mac = HmacSha256::new_from_slice(self.secret.expose_secret().as_bytes())
            .map_err(|_| ConnectorError::webhook_rejected("invalid secret"))?;
        // The MAC input is the raw delivery bytes: `timestamp.body` with no
        // lossy transformation, so two distinct payloads can never collide
        // on the same signed message.
        mac.update(timestamp.as_bytes());
        mac.update(b".");
        mac.update(body);
        let provided_bytes = hex::decode(provided)
            .map_err(|_| ConnectorError::webhook_rejected("malformed signature"))?;
        mac.verify_slice(&provided_bytes)
            .map_err(|_| ConnectorError::webhook_rejected("invalid signature"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::SecretString;

    fn hmac_sha256(key: &[u8], message: &[u8]) -> Vec<u8> {
        let mut mac = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
        mac.update(message);
        mac.finalize().into_bytes().to_vec()
    }

    fn signature_for(secret: &str, timestamp: &str, body: &[u8]) -> String {
        let mut message = Vec::with_capacity(timestamp.len() + 1 + body.len());
        message.extend_from_slice(timestamp.as_bytes());
        message.push(b'.');
        message.extend_from_slice(body);
        format!(
            "sha256={}",
            hex::encode(hmac_sha256(secret.as_bytes(), &message))
        )
    }

    fn validator() -> WebhookValidator {
        WebhookValidator::new(
            SecretString::from("whsec-secret".to_string()),
            StdDuration::from_mins(5),
        )
    }

    #[test]
    fn valid_signature_passes() {
        let now = Utc::now();
        let timestamp = now.to_rfc3339();
        let body = r#"{"event":"push"}"#;
        let signature = signature_for("whsec-secret", &timestamp, body.as_bytes());
        validator()
            .validate(&signature, &timestamp, body.as_bytes(), now)
            .expect("matching signature passes");
    }

    #[test]
    fn wrong_secret_is_rejected() {
        let now = Utc::now();
        let timestamp = now.to_rfc3339();
        let body = "payload";
        let signature = signature_for("wrong-secret", &timestamp, body.as_bytes());
        assert!(
            validator()
                .validate(&signature, &timestamp, body.as_bytes(), now)
                .is_err()
        );
    }

    #[test]
    fn tampered_body_is_rejected() {
        let now = Utc::now();
        let timestamp = now.to_rfc3339();
        let signature = signature_for("whsec-secret", &timestamp, b"payload");
        assert!(
            validator()
                .validate(&signature, &timestamp, b"tampered", now)
                .is_err()
        );
    }

    #[test]
    fn invalid_utf8_bytes_are_signed_verbatim() {
        // A lossy transformation would collapse distinct invalid-UTF-8
        // sequences onto the same replacement character; signing the raw
        // bytes must keep each distinct payload distinguishable.
        let now = Utc::now();
        let timestamp = now.to_rfc3339();
        let body: &[u8] = b"payload\xff\xfe\x01";
        let mutated: &[u8] = b"payload\xfe\xff\x01";
        let signature = signature_for("whsec-secret", &timestamp, body);
        validator()
            .validate(&signature, &timestamp, body, now)
            .expect("verbatim body validates");
        assert!(
            validator()
                .validate(&signature, &timestamp, mutated, now)
                .is_err(),
            "mutated invalid-UTF-8 bytes must not share the signature"
        );
    }

    #[test]
    fn old_timestamp_is_rejected() {
        let now = Utc::now();
        let old = (now - Duration::minutes(10)).to_rfc3339();
        let signature = signature_for("whsec-secret", &old, b"payload");
        assert!(
            validator()
                .validate(&signature, &old, b"payload", now)
                .is_err()
        );
    }

    #[test]
    fn malformed_signature_and_timestamp_are_rejected() {
        let now = Utc::now();
        let timestamp = now.to_rfc3339();
        assert!(
            validator()
                .validate("not-a-signature", &timestamp, b"payload", now)
                .is_err()
        );
        assert!(
            validator()
                .validate(
                    &signature_for("whsec-secret", &timestamp, b"payload"),
                    "garbage",
                    b"payload",
                    now
                )
                .is_err()
        );
    }

    #[test]
    fn errors_never_contain_the_secret() {
        let now = Utc::now();
        let timestamp = now.to_rfc3339();
        let err = validator()
            .validate("sha256=deadbeef", &timestamp, b"payload", now)
            .expect_err("bad signature");
        assert!(!err.to_string().contains("whsec-secret"));
    }
}
