//! Structural secret scrubbing for connector boundaries.
//!
//! The primary guarantee is contractual — connectors must never place raw
//! secrets in errors, status messages, or events. This module is the
//! defense-in-depth layer that catches accidental leakage at the registry
//! event, audit, and CLI boundaries, mirroring the host's schema-aware
//! redaction for key names.

use serde_json::Value;

/// Well-known secret-bearing key names, matched case-insensitively as
/// substrings — fail-safe by design: masking a non-secret key is harmless,
/// while a single leaked API key is not.
const SECRET_KEY_NAMES: &[&str] = &[
    "api_key",
    "api-key",
    "apikey",
    "token",
    "access_token",
    "secret",
    "password",
    "passwd",
    "authorization",
    "auth",
    "credential",
];

const REDACTED: &str = "***";

fn is_secret_key_name(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_KEY_NAMES
        .iter()
        .any(|candidate| lower.contains(candidate))
}

/// Redacts values under secret-bearing keys in a JSON value, recursively.
///
/// Nested objects under a secret key are replaced wholesale so embedded
/// values can never leak. Everything else is preserved.
#[must_use]
pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::Object(obj) => {
            let mut out = serde_json::Map::with_capacity(obj.len());
            for (key, value) in obj {
                if is_secret_key_name(key) {
                    out.insert(key.clone(), Value::String(REDACTED.to_string()));
                } else {
                    out.insert(key.clone(), redact_json(value));
                }
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        other => other.clone(),
    }
}

/// Scrubs common secret formats out of an arbitrary string (error text,
/// log lines, event payloads).
///
/// Recognized formats: `Bearer <token>`, `key=value` / `key = value`, and
/// JSON `"key": "value"` pairs, where the key matches a well-known secret
/// name. The value is replaced with `***`; everything else is preserved
/// byte-for-byte.
#[must_use]
pub fn scrub_secrets(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].to_ascii_lowercase().starts_with(b"bearer ") {
            let token_start = i + 7;
            let token_end = match bytes[token_start..].iter().position(|b| !is_token_char(*b)) {
                Some(p) if p > 0 => token_start + p,
                Some(_) => {
                    out.push_str(&input[i..token_start]);
                    i = token_start;
                    continue;
                }
                None => bytes.len(),
            };
            out.push_str(&input[i..token_start]);
            out.push_str(REDACTED);
            i = token_end;
            continue;
        }

        if let Some((key, value_start)) = key_value_at(bytes, i) {
            if is_secret_key_name(&key) {
                let (value_end, trailing) = value_span(input, value_start);
                // A quoted value keeps its opening quote in the preserved
                // prefix; the closing quote comes back via `trailing`.
                let prefix_end = if bytes.get(value_start) == Some(&b'"') {
                    value_start + 1
                } else {
                    value_start
                };
                out.push_str(&input[i..prefix_end]);
                out.push_str(REDACTED);
                out.push_str(trailing);
                i = value_end;
            } else {
                out.push_str(&input[i..value_start]);
                i = value_start;
            }
            continue;
        }
        let ch = input[i..].chars().next().unwrap_or('\u{FFFD}');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_token_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'+' | b'/' | b'=')
}

/// Attempts to parse a key starting at `start`; returns the key and the
/// index just past the separator (`=`, `:`, or `": "`).
fn key_value_at(bytes: &[u8], start: usize) -> Option<(String, usize)> {
    let rest = &bytes[start..];
    let quoted = rest.first() == Some(&b'"');
    if quoted {
        let close = rest[1..].iter().position(|b| *b == b'"')? + 1;
        let key = String::from_utf8_lossy(&rest[1..close]).into_owned();
        let mut i = close + 1;
        while i < rest.len() && (rest[i] == b' ' || rest[i] == b'\t') {
            i += 1;
        }
        if rest.get(i) != Some(&b':') {
            return None;
        }
        i += 1;
        while i < rest.len() && (rest[i] == b' ' || rest[i] == b'\t') {
            i += 1;
        }
        Some((key, start + i))
    } else {
        let key_end = rest
            .iter()
            .position(|b| !(b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')))
            .unwrap_or(rest.len());
        if key_end == 0 {
            return None;
        }
        let key = String::from_utf8_lossy(&rest[..key_end]).into_owned();
        let mut i = key_end;
        while i < rest.len() && (rest[i] == b' ' || rest[i] == b'\t') {
            i += 1;
        }
        if rest.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while i < rest.len() && (rest[i] == b' ' || rest[i] == b'\t') {
            i += 1;
        }
        Some((key, start + i))
    }
}

/// Returns the span of a value starting at `value_start` plus the literal
/// text to keep after the redaction (quotes and delimiters).
fn value_span(input: &str, value_start: usize) -> (usize, &str) {
    let bytes = input.as_bytes();
    match bytes.get(value_start) {
        Some(b'"') => {
            // Quoted value: redact the inner content and keep the closing
            // quote; a backslash escapes the next byte, so `\"` cannot
            // terminate the value early.
            let mut i = value_start + 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                } else if bytes[i] == b'"' {
                    return (i + 1, "\"");
                } else {
                    i += 1;
                }
            }
            (bytes.len(), "")
        }
        Some(b'{' | b'[') => {
            // Nested object/array: redact the whole container so embedded
            // values cannot survive under a secret-bearing key.
            let mut depth = 1_u32;
            let mut in_string = false;
            let mut i = value_start + 1;
            while i < bytes.len() {
                if in_string {
                    if bytes[i] == b'\\' {
                        i += 2;
                    } else if bytes[i] == b'"' {
                        in_string = false;
                        i += 1;
                    } else {
                        i += 1;
                    }
                } else {
                    match bytes[i] {
                        b'"' => {
                            in_string = true;
                            i += 1;
                        }
                        b'{' | b'[' => {
                            depth += 1;
                            i += 1;
                        }
                        b'}' | b']' => {
                            depth -= 1;
                            if depth == 0 {
                                return (i + 1, "");
                            }
                            i += 1;
                        }
                        _ => i += 1,
                    }
                }
            }
            (bytes.len(), "")
        }
        _ => {
            let end = bytes[value_start..]
                .iter()
                .position(|b| {
                    b.is_ascii_whitespace() || matches!(b, b',' | b'}' | b']' | b')' | b'"')
                })
                .map_or(bytes.len(), |p| value_start + p);
            (end, "")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::{prop_assert, prop_assert_eq};

    fn any_json_value() -> impl proptest::strategy::Strategy<Value = Value> {
        use proptest::prelude::*;
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(Value::from),
            "[a-zA-Z0-9_-]{0,32}".prop_map(Value::String),
        ];
        leaf.prop_recursive(3, 24, 6, |inner| {
            prop_oneof![
                proptest::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
                proptest::collection::hash_map("[a-zA-Z0-9_-]{0,16}", inner, 0..6)
                    .prop_map(|map| Value::Object(map.into_iter().collect())),
            ]
        })
    }

    fn secrets_masked(value: &Value) -> bool {
        match value {
            Value::Object(obj) => obj.iter().all(|(key, value)| {
                if is_secret_key_name(key) {
                    value == &Value::String(REDACTED.to_string())
                } else {
                    secrets_masked(value)
                }
            }),
            Value::Array(items) => items.iter().all(secrets_masked),
            _ => true,
        }
    }

    proptest::proptest! {
        #[test]
        fn scrub_secrets_is_idempotent(input in "\\PC{0,128}") {
            let once = scrub_secrets(&input);
            prop_assert_eq!(scrub_secrets(&once), once);
        }

        #[test]
        fn scrub_bearer_never_leaks_token(token in "[A-Za-z0-9._~+/=-]{4,64}") {
            let out = scrub_secrets(&format!("Authorization: Bearer {token}"));
            prop_assert!(!out.contains(&token));
        }

        #[test]
        fn scrub_key_value_never_leaks_value(
            key in "api_key|access_token|password|auth|token",
            value in "[A-Za-z0-9._~+/=-]{4,64}",
        ) {
            let out = scrub_secrets(&format!("{key}={value}"));
            prop_assert!(!out.contains(&value));
        }

        #[test]
        fn scrub_json_pair_never_leaks_value(
            key in "api_key|access_token|password|auth|token",
            value in "[A-Za-z0-9._~+/=-]{4,64}",
        ) {
            let out = scrub_secrets(&format!(r#"{{"{key}": "{value}"}}"#));
            prop_assert!(!out.contains(&value));
        }

        #[test]
        fn redact_json_masks_all_secret_keys(value in any_json_value()) {
            let once = redact_json(&value);
            prop_assert_eq!(&redact_json(&once), &once);
            prop_assert!(secrets_masked(&once));
        }
    }

    #[test]
    fn redact_json_masks_secret_keys_recursively() {
        let value = serde_json::json!({
            "api_key": {"source": "inline", "inline": "sk-deep-secret"},
            "base_url": "https://api.example.com",
            "nested": {"password": "hunter2", "keep": 42},
            "list": [{"token": "t1"}, {"keep": "k"}]
        });
        let redacted = redact_json(&value);
        let text = serde_json::to_string(&redacted).unwrap();
        assert!(!text.contains("sk-deep-secret"));
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("t1"));
        assert!(text.contains("https://api.example.com"));
        assert!(text.contains("keep"));
    }

    #[test]
    fn scrub_bearer_tokens() {
        assert_eq!(
            scrub_secrets("Authorization: Bearer abc.def-ghi"),
            "Authorization: Bearer ***"
        );
        assert_eq!(scrub_secrets("auth: bearer AbCdEf"), "auth: bearer ***");
        // A plain word that merely starts with "bearer" is not a token.
        assert_eq!(scrub_secrets("bearer-bond"), "bearer-bond");
    }

    #[test]
    fn scrub_key_value_pairs() {
        assert_eq!(
            scrub_secrets("connect failed: api_key=sk-12345, base_url=https://x"),
            "connect failed: api_key=***, base_url=https://x"
        );
        assert_eq!(
            scrub_secrets("token = hunter2 and password: p"),
            "token = *** and password: p"
        );
    }

    #[test]
    fn scrub_json_pairs() {
        assert_eq!(
            scrub_secrets(r#"{"api_key":"sk-123","keep":"v"}"#),
            r#"{"api_key":"***","keep":"v"}"#
        );
        assert_eq!(
            scrub_secrets(r#"{"nested": {"access_token": "abc"}}"#),
            r#"{"nested": {"access_token": "***"}}"#
        );
    }

    #[test]
    fn scrub_nested_values_wholesale() {
        assert_eq!(
            scrub_secrets(r#"{"api_key": {"value": "sk-top-secret-123"}}"#),
            r#"{"api_key": ***}"#
        );
        assert_eq!(
            scrub_secrets(r#"{"api_key": ["a", {"secret": "b"}]}"#),
            r#"{"api_key": ***}"#
        );
    }

    #[test]
    fn scrub_escaped_quotes_inside_values() {
        // Escaped quotes belong to the value; redaction must not stop at
        // them and leak the remainder.
        assert_eq!(
            scrub_secrets(r#"{"api_key": "sk-\"secret\"", "keep": "v"}"#),
            r#"{"api_key": "***", "keep": "v"}"#
        );
        assert_eq!(
            scrub_secrets(r#"{"token": "a\\b\"c"}"#),
            r#"{"token": "***"}"#
        );
    }

    #[test]
    fn non_secret_text_is_preserved() {
        let text = "connector github connected, 2 accounts, base https://api.example.com";
        assert_eq!(scrub_secrets(text), text);
    }

    #[test]
    fn redaction_of_embedded_secret_never_contains_the_value() {
        let secret = "sk-super-secret-9f8e";
        let scrubbed = scrub_secrets(&format!(
            r#"error: {{"api_key": "{secret}", "message": "auth failed"}}"#
        ));
        assert!(!scrubbed.contains(secret));
        assert!(scrubbed.contains("***"));
    }
}
