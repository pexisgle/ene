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

/// Fetches a URL through the host's `Network` broker and returns the body,
/// bounding the body size and requiring valid UTF-8.
pub(crate) async fn fetch_json(url: &str, service: &str) -> Result<String, ToolError> {
    let outcome = crate::broker::broker().fetch(url).await?;
    if !(200..300).contains(&outcome.status) {
        return Err(ToolError::execution_failed(format!(
            "{service} returned HTTP {}",
            outcome.status
        )));
    }
    if outcome.body.len() > MAX_BODY_BYTES {
        return Err(ToolError::execution_failed(format!(
            "API response too large (max {MAX_BODY_BYTES} bytes)"
        )));
    }
    String::from_utf8(outcome.body)
        .map_err(|_| ToolError::execution_failed("API response is not valid UTF-8"))
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
#[expect(
    clippy::await_holding_lock,
    reason = "tests serialize the shared broker with a std mutex across awaits"
)]
mod tests {
    use super::*;

    /// Serializes tests that reconfigure the process-wide shared broker.
    static TEST_SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
    async fn fetch_json_rejects_oversized_bodies() {
        let _serial = TEST_SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The broker mock answers with a body over the plugin's cap.
        let mock = crate::broker::tests::MockBroker::spawn();
        crate::broker::tests::configure_test_broker(&mock).await;
        mock.push(crate::broker::tests::MockResponse::ok(vec![
            0u8;
            MAX_BODY_BYTES
                .saturating_add(
                    1
                )
        ]));
        let err = fetch_json("https://wttr.in/?format=j1", "wttr.in")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("too large"), "{err}");
    }

    #[tokio::test]
    async fn fetch_json_accepts_small_bodies() {
        let _serial = TEST_SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mock = crate::broker::tests::MockBroker::spawn();
        crate::broker::tests::configure_test_broker(&mock).await;
        mock.push(crate::broker::tests::MockResponse::ok(
            br#"{"ok":true}"#.to_vec(),
        ));
        let body = fetch_json("https://wttr.in/?format=j1", "wttr.in")
            .await
            .unwrap();
        assert_eq!(body, r#"{"ok":true}"#);
    }
}
