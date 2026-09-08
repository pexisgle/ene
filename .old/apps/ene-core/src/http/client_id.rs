use axum::http::HeaderMap;

use super::error::{ApiReject, forbidden};

/// Caller identity from `X-Client-Id` (HTTP) — never trust JSON body `client_id`.
#[must_use]
pub fn client_id_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("X-Client-Id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("anonymous")
        .to_owned()
}

/// Web UI is read-only for mutating routes (P-107).
pub fn web_mutate_forbidden(client_id: &str) -> Result<(), ApiReject> {
    if client_id == "web" {
        Err(forbidden("web client cannot mutate"))
    } else {
        Ok(())
    }
}
