use super::{format_coord, validate_latitude, validate_longitude};
use crate::error::GeoError;
use ene_plugin::prelude::*;

/// Calculates the solar UTC offset for a longitude.
///
/// The Earth rotates 360° in 24 hours, so every 15° of longitude corresponds
/// to one hour of solar time. The result is the *theoretical* offset: civil
/// timezone boundaries and daylight saving time are not reflected.
#[derive(Clone, Default, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "geo",
    name = "timezone",
    summary = "Calculate the solar UTC offset for a longitude.",
    description = "Computes the theoretical UTC offset from a longitude (15 degrees of longitude per hour, e.g. 135°E ≈ UTC+9). This is the solar offset, not the civil timezone: political boundaries and daylight saving time are not reflected. Returns the offset in hours and as a UTC+HH:MM string.",
    category = "Utility",
    keywords_primary = "timezone, utc, offset, longitude, time, solar",
    side_effects = "ReadOnly"
)]
/// Action to calculate the solar UTC offset for a longitude.
pub struct TimezoneAction {
    /// Longitude in degrees, -180 to 180 (positive = east).
    longitude: f64,
    /// Latitude in degrees, -90 to 90; included in the output for context.
    #[serde(default)]
    latitude: Option<f64>,
}

impl TimezoneAction {
    async fn run(&self) -> Result<String, ToolError> {
        timezone_info(self.longitude, self.latitude)
    }
}

fn timezone_info(longitude: f64, latitude: Option<f64>) -> Result<String, ToolError> {
    validate_longitude(longitude)?;
    if let Some(latitude) = latitude {
        validate_latitude(latitude)?;
    }

    let hours = longitude / 15.0;
    let mut lines = vec![
        format!("Longitude: {}", format_coord(longitude)),
        format!(
            "Solar UTC offset: {} ({hours:.3} hours)",
            format_offset(hours)
        ),
        "Note: solar offset from longitude; civil timezone boundaries and \
         daylight saving time are not reflected."
            .to_string(),
    ];
    if let Some(latitude) = latitude {
        lines.insert(0, format!("Latitude: {}", format_coord(latitude)));
    }
    Ok(lines.join("\n"))
}

/// Formats a signed hour offset as `UTC+HH:MM`, rounded to the nearest minute.
fn format_offset(hours: f64) -> String {
    let sign = if hours < 0.0 { '-' } else { '+' };
    let total_minutes = (hours.abs() * 60.0).round() as i64;
    format!(
        "UTC{sign}{:02}:{:02}",
        total_minutes / 60,
        total_minutes % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(longitude: f64) -> Result<String, ToolError> {
        timezone_info(longitude, None)
    }

    #[test]
    fn prime_meridian_is_utc_zero() {
        let out = info(0.0).unwrap();
        assert!(out.contains("UTC+00:00"), "{out}");
        assert!(out.contains("0.000 hours"), "{out}");
    }

    #[test]
    fn tokyo_longitude_is_about_utc_plus_nine() {
        let out = info(139.68).unwrap();
        assert!(out.contains("UTC+09:19"), "{out}");
        assert!(out.contains("9.312 hours"), "{out}");
    }

    #[test]
    fn western_longitude_is_negative() {
        let out = info(-75.0).unwrap();
        assert!(out.contains("UTC-05:00"), "{out}");
    }

    #[test]
    fn antimeridian_bounds_are_twelve_hours() {
        assert!(info(180.0).unwrap().contains("UTC+12:00"));
        assert!(info(-180.0).unwrap().contains("UTC-12:00"));
    }

    #[test]
    fn out_of_range_longitude_is_rejected() {
        assert!(matches!(
            info(180.1),
            Err(ToolError::InvalidArguments { .. })
        ));
        assert!(matches!(
            info(-180.1),
            Err(ToolError::InvalidArguments { .. })
        ));
        assert!(info(f64::NAN).is_err());
    }

    #[test]
    fn latitude_is_validated_when_present() {
        assert!(timezone_info(139.68, Some(35.68)).is_ok());
        assert!(timezone_info(139.68, Some(90.1)).is_err());
    }

    #[test]
    fn latitude_is_included_in_output() {
        let out = timezone_info(139.68, Some(35.68)).unwrap();
        assert!(out.starts_with("Latitude: 35.68"), "{out}");
    }
}
