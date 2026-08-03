use super::{fetch_json, format_day_length, validate_latitude, validate_longitude};
use crate::error::GeoError;
use crate::provider::GeoState;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<GeoState> {
    Arc::new(GeoState::new())
}

/// Response envelope of the sunrise-sunset.org JSON endpoint.
#[derive(Debug, Deserialize)]
struct SunResponse {
    #[serde(default, deserialize_with = "deserialize_results")]
    results: Option<SunResults>,
    status: String,
    #[serde(default)]
    tzid: Option<String>,
}

/// Tolerates the API's error envelope, where `results` is an empty string
/// (`{"results":"","status":"INVALID_DATE"}`) instead of an object; any
/// non-object value is treated as absent and the status field carries the
/// error.
fn deserialize_results<'de, D>(deserializer: D) -> Result<Option<SunResults>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

#[derive(Debug, Deserialize)]
struct SunResults {
    #[serde(default)]
    sunrise: Option<String>,
    #[serde(default)]
    sunset: Option<String>,
    #[serde(rename = "solar_noon", default)]
    solar_noon: Option<String>,
    #[serde(rename = "day_length", default)]
    day_length: Option<u64>,
    #[serde(rename = "civil_twilight_begin", default)]
    civil_twilight_begin: Option<String>,
    #[serde(rename = "civil_twilight_end", default)]
    civil_twilight_end: Option<String>,
    #[serde(rename = "nautical_twilight_begin", default)]
    nautical_twilight_begin: Option<String>,
    #[serde(rename = "nautical_twilight_end", default)]
    nautical_twilight_end: Option<String>,
    #[serde(rename = "astronomical_twilight_begin", default)]
    astronomical_twilight_begin: Option<String>,
    #[serde(rename = "astronomical_twilight_end", default)]
    astronomical_twilight_end: Option<String>,
}

/// Returns sunrise and sunset times for coordinates via sunrise-sunset.org.
///
/// Times are ISO 8601 timestamps in the requested timezone (`tzid`, IANA
/// name, default UTC); the date defaults to today (UTC) when omitted.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "geo",
    name = "sunrise_sunset",
    summary = "Get sunrise and sunset times for coordinates.",
    description = "Returns sunrise, sunset, solar noon, day length, and twilight times for the given latitude/longitude from sunrise-sunset.org. An optional date (YYYY-MM-DD; default today in UTC, computed by the tool) and IANA timezone name (e.g. \"Asia/Tokyo\", default UTC) can be given; times are returned as ISO 8601 timestamps in that timezone.",
    category = "Utility",
    keywords_primary = "sunrise, sunset, sun, daylight, twilight, dawn, dusk, solar noon",
    side_effects = "Network { external: true }"
)]
/// Action to get sunrise and sunset times.
pub struct SunAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<GeoState>,
    /// Latitude in degrees, -90 to 90.
    latitude: f64,
    /// Longitude in degrees, -180 to 180.
    longitude: f64,
    /// Date as YYYY-MM-DD; defaults to today (UTC).
    #[serde(default)]
    date: Option<String>,
    /// IANA timezone name (e.g. "Asia/Tokyo"); defaults to UTC.
    #[arg(default = "UTC")]
    #[serde(default)]
    tzid: Option<String>,
}

impl SunAction {
    /// Creates a new `SunAction` with the given shared state.
    #[must_use]
    pub fn new(state: Arc<GeoState>) -> Self {
        Self {
            state,
            latitude: 0.0,
            longitude: 0.0,
            date: None,
            tzid: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        // The service's own no-date default is its server-local day, which
        // can be the previous UTC day; the tool always sends an explicit
        // client-computed UTC date so "today" means the caller's today.
        let date = match self.date.as_deref() {
            Some(date) => date.to_string(),
            None => chrono::Utc::now().format("%Y-%m-%d").to_string(),
        };
        let url = build_sun_url(self.latitude, self.longitude, &date, self.tzid.as_deref())?;
        let body = fetch_json(self.state.client(), url, "sunrise-sunset.org").await?;
        let parsed: SunResponse = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Invalid sunrise-sunset.org response: {e}"))
        })?;
        format_sun(&parsed).map_err(ToolError::from)
    }
}

/// Builds the sunrise-sunset.org request URL.
///
/// `formatted=0` requests ISO 8601 timestamps instead of the default
/// locale-style strings, and `tzid` makes the service return them with the
/// given timezone's offset.
fn build_sun_url(
    latitude: f64,
    longitude: f64,
    date: &str,
    tzid: Option<&str>,
) -> Result<reqwest::Url, GeoError> {
    validate_latitude(latitude)?;
    validate_longitude(longitude)?;
    validate_date(date)?;
    if let Some(tzid) = tzid {
        validate_tzid(tzid)?;
    }

    let mut url = reqwest::Url::parse("https://api.sunrise-sunset.org/json")
        .map_err(|e| GeoError::Internal(format!("invalid sunrise-sunset.org URL: {e}")))?;
    url.query_pairs_mut()
        .append_pair("lat", &latitude.to_string())
        .append_pair("lng", &longitude.to_string())
        .append_pair("date", date)
        .append_pair("formatted", "0");
    if let Some(tzid) = tzid {
        url.query_pairs_mut().append_pair("tzid", tzid);
    }
    Ok(url)
}

/// Validates a `YYYY-MM-DD` date against the Gregorian calendar, including
/// month lengths and leap years.
fn validate_date(date: &str) -> Result<(), GeoError> {
    let mut parts = date.split('-');
    let (Some(year), Some(month), Some(day), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(GeoError::InvalidArguments(format!(
            "date must be YYYY-MM-DD, got '{date}'"
        )));
    };
    let well_formed = year.len() == 4
        && month.len() == 2
        && day.len() == 2
        && year.bytes().all(|b| b.is_ascii_digit())
        && month.bytes().all(|b| b.is_ascii_digit())
        && day.bytes().all(|b| b.is_ascii_digit());
    if !well_formed {
        return Err(GeoError::InvalidArguments(format!(
            "date must be YYYY-MM-DD, got '{date}'"
        )));
    }
    let parse = |part: &str| {
        part.parse::<u32>().map_err(|_| {
            GeoError::InvalidArguments(format!("date must be YYYY-MM-DD, got '{date}'"))
        })
    };
    let year = parse(year)?;
    let month = parse(month)?;
    let day = parse(day)?;
    let valid = (1..=12).contains(&month) && (1..=days_in_month(year, month)).contains(&day);
    if valid {
        Ok(())
    } else {
        Err(GeoError::InvalidArguments(format!(
            "date must be YYYY-MM-DD, got '{date}'"
        )))
    }
}

/// Days in a Gregorian month; invalid months return 0.
const fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Gregorian leap-year rule: every 4 years, except every 100, except every 400.
const fn is_leap_year(year: u32) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

/// Validates an IANA timezone name against the characters the tz database
/// actually uses; the service rejects unknown names.
fn validate_tzid(tzid: &str) -> Result<(), GeoError> {
    let valid = !tzid.is_empty()
        && tzid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'+' | b'-' | b'/' | b'.'));
    if valid {
        Ok(())
    } else {
        Err(GeoError::InvalidArguments(format!(
            "'{tzid}' is not a valid IANA timezone name"
        )))
    }
}

fn format_sun(response: &SunResponse) -> Result<String, GeoError> {
    if response.status != "OK" {
        return Err(GeoError::ApiFailure(format!(
            "sunrise-sunset.org rejected the request (status {})",
            response.status
        )));
    }

    let results = response.results.as_ref().ok_or_else(|| {
        GeoError::InvalidResponse("sunrise-sunset.org response contains no sun data".to_string())
    })?;
    let mut lines = Vec::new();
    if let Some(sunrise) = results.sunrise.as_deref() {
        lines.push(format!("Sunrise: {sunrise}"));
    }
    if let Some(sunset) = results.sunset.as_deref() {
        lines.push(format!("Sunset: {sunset}"));
    }
    if let Some(solar_noon) = results.solar_noon.as_deref() {
        lines.push(format!("Solar noon: {solar_noon}"));
    }
    if let Some(day_length) = results.day_length {
        lines.push(format!("Day length: {}", format_day_length(day_length)));
    }
    if let Some(line) = twilight_line(
        "Civil twilight",
        results.civil_twilight_begin.as_deref(),
        results.civil_twilight_end.as_deref(),
    ) {
        lines.push(line);
    }
    if let Some(line) = twilight_line(
        "Nautical twilight",
        results.nautical_twilight_begin.as_deref(),
        results.nautical_twilight_end.as_deref(),
    ) {
        lines.push(line);
    }
    if let Some(line) = twilight_line(
        "Astronomical twilight",
        results.astronomical_twilight_begin.as_deref(),
        results.astronomical_twilight_end.as_deref(),
    ) {
        lines.push(line);
    }
    if let Some(tzid) = response.tzid.as_deref() {
        lines.push(format!("Timezone: {tzid}"));
    }

    if lines.is_empty() {
        return Err(GeoError::InvalidResponse(
            "sunrise-sunset.org response contains no sun data".to_string(),
        ));
    }
    Ok(lines.join("\n"))
}

fn twilight_line(label: &str, begin: Option<&str>, end: Option<&str>) -> Option<String> {
    match (begin, end) {
        (Some(begin), Some(end)) => Some(format!("{label}: {begin} – {end}")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUN_FIXTURE: &str = r#"{
        "results": {
            "sunrise": "2026-08-04T04:50:02+09:00",
            "sunset": "2026-08-04T18:44:45+09:00",
            "solar_noon": "2026-08-04T11:47:24+09:00",
            "day_length": 50083,
            "civil_twilight_begin": "2026-08-04T04:23:32+09:00",
            "civil_twilight_end": "2026-08-04T19:11:15+09:00",
            "nautical_twilight_begin": "2026-08-04T03:49:45+09:00",
            "nautical_twilight_end": "2026-08-04T19:45:02+09:00",
            "astronomical_twilight_begin": "2026-08-04T03:13:43+09:00",
            "astronomical_twilight_end": "2026-08-04T20:21:04+09:00"
        },
        "status": "OK",
        "tzid": "Asia/Tokyo"
    }"#;

    fn parse(fixture: &str) -> SunResponse {
        serde_json::from_str(fixture).unwrap()
    }

    #[test]
    fn formats_sun_response() {
        let out = format_sun(&parse(SUN_FIXTURE)).unwrap();
        assert!(out.contains("Sunrise: 2026-08-04T04:50:02+09:00"), "{out}");
        assert!(out.contains("Sunset: 2026-08-04T18:44:45+09:00"), "{out}");
        assert!(
            out.contains("Solar noon: 2026-08-04T11:47:24+09:00"),
            "{out}"
        );
        assert!(out.contains("Day length: 13h 54m 43s"), "{out}");
        assert!(
            out.contains("Civil twilight: 2026-08-04T04:23:32+09:00 – 2026-08-04T19:11:15+09:00"),
            "{out}"
        );
        assert!(out.contains("Timezone: Asia/Tokyo"), "{out}");
    }

    #[test]
    fn api_error_envelope_is_reported() {
        // The live API returns the results field as an empty string on
        // errors (verified with an invalid date); the status check must
        // win over the unparseable results shape.
        let fixture = r#"{"results":"","status":"INVALID_DATE"}"#;
        let err = format_sun(&parse(fixture)).unwrap_err();
        assert!(matches!(err, GeoError::ApiFailure(_)));
        assert!(err.to_string().contains("INVALID_DATE"), "{err}");
    }

    #[test]
    fn missing_results_are_rejected() {
        let fixture = r#"{"results":{},"status":"OK"}"#;
        let err = format_sun(&parse(fixture)).unwrap_err();
        assert!(matches!(err, GeoError::InvalidResponse(_)));
    }

    #[test]
    fn url_includes_coordinates_and_format() {
        let url = build_sun_url(35.68, 139.69, "2026-08-04", None).unwrap();
        assert_eq!(
            url.as_str(),
            "https://api.sunrise-sunset.org/json?lat=35.68&lng=139.69&date=2026-08-04&formatted=0"
        );
    }

    #[test]
    fn url_includes_date_and_tzid_when_given() {
        let url = build_sun_url(35.68, 139.69, "2026-08-04", Some("Asia/Tokyo")).unwrap();
        let params: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        assert!(params.contains(&("date".to_string(), "2026-08-04".to_string())));
        assert!(params.contains(&("tzid".to_string(), "Asia/Tokyo".to_string())));
    }

    #[test]
    fn invalid_coordinates_are_rejected() {
        assert!(build_sun_url(90.1, 0.0, "2026-08-04", None).is_err());
        assert!(build_sun_url(0.0, 180.1, "2026-08-04", None).is_err());
    }

    #[test]
    fn date_validation() {
        assert!(validate_date("2026-08-04").is_ok());
        assert!(validate_date("2024-02-29").is_ok());
        assert!(validate_date("2026-02-28").is_ok());
        assert!(validate_date("2026-02-29").is_err());
        assert!(validate_date("2026-04-30").is_ok());
        assert!(validate_date("2026-04-31").is_err());
        assert!(validate_date("2026-8-4").is_err());
        assert!(validate_date("2026-13-01").is_err());
        assert!(validate_date("2026-08-32").is_err());
        assert!(validate_date("26-08-04").is_err());
        assert!(validate_date("2026/08/04").is_err());
        assert!(validate_date("").is_err());
    }

    #[test]
    fn century_leap_year_rules() {
        assert!(is_leap_year(2000));
        assert!(!is_leap_year(1900));
        assert!(is_leap_year(2024));
        assert!(!is_leap_year(2026));
    }

    #[test]
    fn tzid_validation() {
        assert!(validate_tzid("Asia/Tokyo").is_ok());
        assert!(validate_tzid("Etc/GMT+9").is_ok());
        assert!(validate_tzid("UTC").is_ok());
        assert!(validate_tzid("Asia/Tokyo; rm -rf /").is_err());
        assert!(validate_tzid("").is_err());
    }
}
