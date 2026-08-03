mod location;
mod sun;
mod timezone;
mod weather;

pub use location::LocationAction;
pub use sun::SunAction;
pub use timezone::TimezoneAction;
pub use weather::WeatherAction;

use crate::error::GeoError;
use ene_plugin::prelude::*;

/// Hard cap on API response bodies; the geo endpoints return a few
/// kilobytes, so anything larger is a malfunction or an attack.
const MAX_BODY_BYTES: usize = 1024 * 1024;

/// Hard cap on API-provided free-form strings echoed into results.
const MAX_FIELD_CHARS: usize = 200;

/// Performs a GET request and returns the response body, bounding both the
/// wait time (client-level timeout) and the body size.
pub(crate) async fn fetch_json(
    client: &reqwest::Client,
    url: reqwest::Url,
    service: &str,
) -> Result<String, ToolError> {
    let response = client.get(url).send().await.map_err(|e| {
        ToolError::execution_failed(format!(
            "HTTP request to {service} failed: {}",
            sanitize_reqwest_error(e)
        ))
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(ToolError::execution_failed(format!(
            "{service} returned HTTP {status}"
        )));
    }
    read_bounded_body(response).await
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
            "API response too large ({len} bytes, max {MAX_BODY_BYTES})"
        )));
    }
    let mut body = Vec::new();
    let mut response = response;
    while let Some(chunk) = response.chunk().await.map_err(|e| {
        ToolError::execution_failed(format!(
            "Failed to read API response: {}",
            sanitize_reqwest_error(e)
        ))
    })? {
        if body.len().saturating_add(chunk.len()) > MAX_BODY_BYTES {
            return Err(ToolError::execution_failed(format!(
                "API response too large (max {MAX_BODY_BYTES} bytes)"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    std::str::from_utf8(&body)
        .map(str::to_string)
        .map_err(|_| ToolError::execution_failed("API response is not valid UTF-8"))
}

/// Renders a reqwest error without its request URL, whose query string may
/// carry location parameters that do not belong in logs or tool results.
/// reqwest's `Display` appends `for url (...)`, so a raw `{e}` would echo it.
fn sanitize_reqwest_error(e: reqwest::Error) -> String {
    e.without_url().to_string()
}

/// Truncates an API-provided string to at most [`MAX_FIELD_CHARS`]
/// characters, including the trailing `...`.
pub(crate) fn truncate(value: &str) -> String {
    if value.chars().count() <= MAX_FIELD_CHARS {
        return value.to_string();
    }
    let mut truncated: String = value.chars().take(MAX_FIELD_CHARS - 3).collect();
    truncated.push_str("...");
    truncated
}

/// Formats a coordinate for display, trimming trailing zeros
/// (`35.680` -> `35.68`).
pub(crate) fn format_coord(value: f64) -> String {
    let value = if value == 0.0 { 0.0 } else { value };
    format!("{value:.3}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

/// Formats a duration in seconds as `Xh Ym Zs` (or `Ym Zs` under an hour).
pub(crate) fn format_day_length(total_seconds: u64) -> String {
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours == 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{hours}h {minutes}m {seconds}s")
    }
}

/// Validates a latitude in degrees.
pub(crate) fn validate_latitude(value: f64) -> Result<(), GeoError> {
    if value.is_finite() && (-90.0..=90.0).contains(&value) {
        Ok(())
    } else {
        Err(GeoError::InvalidArguments(format!(
            "latitude must be between -90 and 90, got {value}"
        )))
    }
}

/// Validates a longitude in degrees.
pub(crate) fn validate_longitude(value: f64) -> Result<(), GeoError> {
    if value.is_finite() && (-180.0..=180.0).contains(&value) {
        Ok(())
    } else {
        Err(GeoError::InvalidArguments(format!(
            "longitude must be between -180 and 180, got {value}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[test]
    fn truncate_keeps_short_strings() {
        assert_eq!(truncate("Tokyo"), "Tokyo");
    }

    #[test]
    fn truncate_caps_long_strings() {
        let long = "x".repeat(300);
        let out = truncate(&long);
        assert_eq!(out.len(), MAX_FIELD_CHARS);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn truncate_counts_chars_not_bytes() {
        let long = "あ".repeat(300);
        let out = truncate(&long);
        assert!(out.ends_with("..."));
        assert_eq!(out.chars().count(), MAX_FIELD_CHARS);
    }

    #[test]
    fn coord_formatting_trims_trailing_zeros() {
        assert_eq!(format_coord(35.680), "35.68");
        assert_eq!(format_coord(139.0), "139");
        assert_eq!(format_coord(-0.5), "-0.5");
        assert_eq!(format_coord(-0.0), "0");
    }

    #[test]
    fn day_length_formats_hours() {
        assert_eq!(format_day_length(50083), "13h 54m 43s");
    }

    #[test]
    fn day_length_formats_subhour() {
        assert_eq!(format_day_length(3542), "59m 2s");
    }

    #[test]
    fn coordinate_ranges_are_validated() {
        assert!(validate_latitude(90.0).is_ok());
        assert!(validate_latitude(-90.0).is_ok());
        assert!(validate_latitude(90.1).is_err());
        assert!(validate_longitude(180.0).is_ok());
        assert!(validate_longitude(-180.0).is_ok());
        assert!(validate_longitude(180.1).is_err());
        assert!(validate_latitude(f64::NAN).is_err());
        assert!(validate_longitude(f64::INFINITY).is_err());
    }

    #[tokio::test]
    async fn bounded_body_rejects_known_oversize() {
        let http_response = http::Response::builder()
            .body(reqwest::Body::from(vec![
                0u8;
                MAX_BODY_BYTES.saturating_add(1)
            ]))
            .unwrap();
        let err = read_bounded_body(reqwest::Response::from(http_response))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[tokio::test]
    async fn bounded_body_rejects_unknown_size_stream() {
        // A body with no size hint (as with chunked transfer) must still
        // bail on the running total instead of buffering past the cap.
        let reader = tokio::io::repeat(0)
            .take(u64::try_from(MAX_BODY_BYTES.saturating_add(64 * 1024)).unwrap());
        let stream = tokio_util::io::ReaderStream::new(reader);
        let http_response = http::Response::builder()
            .body(reqwest::Body::wrap_stream(stream))
            .unwrap();
        let err = read_bounded_body(reqwest::Response::from(http_response))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[tokio::test]
    async fn bounded_body_accepts_small_bodies() {
        let http_response = http::Response::builder()
            .body(reqwest::Body::from(br#"{"ok":true}"#.to_vec()))
            .unwrap();
        let body = read_bounded_body(reqwest::Response::from(http_response))
            .await
            .unwrap();
        assert_eq!(body, r#"{"ok":true}"#);
    }
}
