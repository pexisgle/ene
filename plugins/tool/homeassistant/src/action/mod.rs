mod climate;
mod state;
mod turn;

pub use climate::SetTemperatureAction;
pub use state::StateAction;
pub use turn::{TurnOffAction, TurnOnAction};

use crate::error::HomeAssistantError;
use ene_plugin_proto::ToolError;
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};

/// Hard cap on API response bodies; Home Assistant responses are a few
/// kilobytes, so anything larger is a malfunction or an attack.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Hard cap on API-provided strings echoed into results.
const MAX_FIELD_CHARS: usize = 200;

/// Hard cap on the serialized `attributes` object in `state` output.
const MAX_ATTRIBUTES_CHARS: usize = 2000;

/// Validates a Home Assistant entity id: `domain.entity` where both parts
/// are lowercase alphanumeric or underscore.
///
/// The id is placed in the request path, so the charset check also rules
/// out path separators and encoded traversal sequences.
pub(crate) fn validate_entity_id(entity_id: &str) -> Result<(), HomeAssistantError> {
    let (domain, entity) = entity_id.split_once('.').ok_or_else(|| {
        HomeAssistantError::InvalidArguments(format!(
            "'{entity_id}' is not a valid entity id: expected 'domain.entity'"
        ))
    })?;
    if !is_entity_part(domain) || !is_entity_part(entity) {
        return Err(HomeAssistantError::InvalidArguments(format!(
            "'{entity_id}' is not a valid entity id: expected 'domain.entity' with lowercase \
             letters, digits, and underscores"
        )));
    }
    Ok(())
}

fn is_entity_part(part: &str) -> bool {
    !part.is_empty()
        && part
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// Builds the `Authorization: Bearer …` header for a Home Assistant token.
pub(crate) fn bearer_header(token: &str) -> Result<HeaderValue, HomeAssistantError> {
    HeaderValue::from_str(&format!("Bearer {token}")).map_err(|_| {
        HomeAssistantError::Internal(
            "the Home Assistant token contains characters that cannot be sent in an HTTP header"
                .to_string(),
        )
    })
}

/// Performs a GET request and returns the response body, bounding both the
/// wait time (client-level timeout) and the body size.
pub(crate) async fn get(
    client: &reqwest::Client,
    url: reqwest::Url,
    token: &str,
) -> Result<String, ToolError> {
    let response = client
        .get(url)
        .header(AUTHORIZATION, bearer_header(token)?)
        .send()
        .await
        .map_err(|e| {
            ToolError::execution_failed(format!(
                "HTTP request to Home Assistant failed: {}",
                sanitize_reqwest_error(&e)
            ))
        })?;
    let status = response.status();
    let body = read_bounded_body(response).await?;
    if !status.is_success() {
        return Err(map_http_error(status, &body));
    }
    Ok(body)
}

/// Performs a service POST with a JSON body; Home Assistant answers `[]`
/// on success, which is discarded.
pub(crate) async fn post_service(
    client: &reqwest::Client,
    url: reqwest::Url,
    token: &str,
    body: &serde_json::Value,
) -> Result<(), ToolError> {
    let response = client
        .post(url)
        .header(AUTHORIZATION, bearer_header(token)?)
        .header(CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| {
            ToolError::execution_failed(format!(
                "HTTP request to Home Assistant failed: {}",
                sanitize_reqwest_error(&e)
            ))
        })?;
    let status = response.status();
    let body = read_bounded_body(response).await?;
    if !status.is_success() {
        return Err(map_http_error(status, &body));
    }
    Ok(())
}

/// Turns a non-success HTTP response into a `ToolError`, preferring the
/// `message` field of Home Assistant's `{"code", "message"}` error body.
fn map_http_error(status: StatusCode, body: &str) -> ToolError {
    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "no details provided".to_string());
    ToolError::execution_failed(format!("Home Assistant returned HTTP {status}: {detail}"))
}

/// Reads a response body, rejecting it when the declared or actual size
/// exceeds [`MAX_BODY_BYTES`] or when it is not valid UTF-8. The body is
/// consumed in chunks and the running total is checked before each chunk is
/// buffered, so a body without a known size (e.g. chunked transfer) cannot
/// force the whole body into memory.
async fn read_bounded_body(response: reqwest::Response) -> Result<String, ToolError> {
    if let Some(len) = response.content_length()
        && let Ok(len) = usize::try_from(len)
        && len > MAX_BODY_BYTES
    {
        return Err(ToolError::execution_failed(format!(
            "Home Assistant response too large ({len} bytes, max {MAX_BODY_BYTES})"
        )));
    }
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        ToolError::execution_failed(format!(
            "Failed to read Home Assistant response: {}",
            sanitize_reqwest_error(&e)
        ))
    })? {
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            return Err(ToolError::execution_failed(format!(
                "Home Assistant response too large (max {MAX_BODY_BYTES} bytes)"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    std::str::from_utf8(&body)
        .map(str::to_string)
        .map_err(|_| ToolError::execution_failed("Home Assistant response is not valid UTF-8"))
}

/// Renders a reqwest error without its request URL, whose userinfo could
/// carry credentials that do not belong in logs or tool results. reqwest's
/// `Display` appends `for url (...)`, so a raw `{e}` would echo it.
fn sanitize_reqwest_error(e: &reqwest::Error) -> String {
    strip_reqwest_url(&e.to_string())
}

/// Drops the `for url (...)` suffix reqwest's `Display` appends.
fn strip_reqwest_url(text: &str) -> String {
    match text.find(" for url (") {
        Some(idx) => text[..idx].to_string(),
        None => text.to_string(),
    }
}

/// Caps a free-form API string at [`MAX_FIELD_CHARS`] characters.
pub(crate) fn truncate(value: &str) -> String {
    if value.chars().count() <= MAX_FIELD_CHARS {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(MAX_FIELD_CHARS - 3).collect();
    truncated.push_str("...");
    truncated
}

/// Renders an object as compact JSON, capping its length so an
/// attribute-heavy entity cannot blow up the tool result.
fn truncate_attributes(value: &serde_json::Value) -> String {
    let json = value.to_string();
    if json.chars().count() <= MAX_ATTRIBUTES_CHARS {
        return json;
    }
    let mut truncated: String = json.chars().take(MAX_ATTRIBUTES_CHARS - 3).collect();
    truncated.push_str("...");
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_entity_ids_pass() {
        for id in [
            "light.living_room",
            "sensor.outdoor_temperature",
            "climate.aircon",
            "switch.plug_1",
        ] {
            validate_entity_id(id).expect("valid entity id");
        }
    }

    #[test]
    fn invalid_entity_ids_are_rejected() {
        for id in [
            "",
            "light",
            ".living_room",
            "light.",
            "Light.LivingRoom",
            "a.b.c",
            "../etc/passwd",
            "light/living",
        ] {
            assert!(
                matches!(
                    validate_entity_id(id),
                    Err(HomeAssistantError::InvalidArguments(_))
                ),
                "expected rejection for {id:?}"
            );
        }
    }

    #[test]
    fn bearer_header_prefixes_token() {
        let header = bearer_header("tok-123").unwrap();
        assert_eq!(header, HeaderValue::from_static("Bearer tok-123"));
    }

    #[test]
    fn bearer_header_rejects_control_characters() {
        let err = bearer_header("bad\ntoken").unwrap_err();
        assert!(!err.to_string().contains("bad"));
    }

    #[test]
    fn http_error_prefers_ha_message() {
        let err = map_http_error(
            StatusCode::NOT_FOUND,
            r#"{"code":"not_found","message":"Entity light.x not found"}"#,
        );
        assert!(err.to_string().contains("404"));
        assert!(err.to_string().contains("Entity light.x not found"));
    }

    #[test]
    fn http_error_falls_back_to_status_only() {
        let err = map_http_error(StatusCode::UNAUTHORIZED, "not json");
        assert!(err.to_string().contains("401"));
        assert!(err.to_string().contains("no details provided"));
    }

    #[test]
    fn truncate_caps_long_strings() {
        let long = "x".repeat(500);
        let out = truncate(&long);
        assert_eq!(out.chars().count(), MAX_FIELD_CHARS);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn truncate_attributes_caps_object_length() {
        let value = serde_json::json!({ "key": "v".repeat(MAX_ATTRIBUTES_CHARS + 100) });
        let out = truncate_attributes(&value);
        assert_eq!(out.chars().count(), MAX_ATTRIBUTES_CHARS);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn short_attributes_are_kept_verbatim() {
        let value = serde_json::json!({ "brightness": 180 });
        assert_eq!(truncate_attributes(&value), r#"{"brightness":180}"#);
    }

    #[test]
    fn strip_reqwest_url_removes_url_suffix() {
        let text = "error sending request for url (http://user:secret@ha.local/api/states/x)";
        let out = strip_reqwest_url(text);
        assert_eq!(out, "error sending request");
        assert!(!out.contains("secret"));
    }

    #[test]
    fn strip_reqwest_url_keeps_plain_messages() {
        assert_eq!(strip_reqwest_url("timeout"), "timeout");
    }
}
