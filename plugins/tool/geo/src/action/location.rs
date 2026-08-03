use super::{fetch_json, format_coord, truncate};
use crate::approval::actions::GEO_LOCATION;
use crate::error::GeoError;
use crate::provider::GeoState;
use ene_plugin::prelude::*;
use std::net::IpAddr;
use std::sync::Arc;

fn default_state() -> Arc<GeoState> {
    Arc::new(GeoState::new())
}

/// Response envelope of the ipapi.co JSON endpoint.
#[derive(Debug, Deserialize)]
struct LocationResponse {
    #[serde(default)]
    error: bool,
    #[serde(default)]
    reason: Option<String>,
    #[serde(rename = "country_name", default)]
    country: Option<String>,
    #[serde(rename = "region", default)]
    region_name: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(rename = "lat", default)]
    latitude: Option<f64>,
    #[serde(rename = "lon", default)]
    longitude: Option<f64>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(rename = "ip", default)]
    query: Option<String>,
}

/// Looks up the geographic location of an IP address via ipapi.co.
///
/// When `ip` is omitted, the caller's own public IP is located, which
/// reveals the user's approximate location; every lookup requires explicit
/// user approval because the address is sent to the external service.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "geo",
    name = "location",
    summary = "Get the geographic location of an IP address.",
    description = "Looks up the approximate geographic location (country, region, city, coordinates, timezone) of an IP address using ipapi.co. Every lookup requires explicit user approval because the address is sent to the external service.",
    category = "Utility",
    keywords_primary = "location, geolocation, ip, address, country, city, coordinates, where",
    side_effects = "Network { external: true }"
)]
/// Action to look up the location of an IP address.
pub struct LocationAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<GeoState>,
    /// IP address to locate (IPv4 or IPv6). Omitted: locate the caller's own IP (requires approval).
    #[serde(default)]
    ip: Option<String>,
}

impl LocationAction {
    /// Creates a new `LocationAction` with the given shared state.
    #[must_use]
    pub fn new(state: Arc<GeoState>) -> Self {
        Self { state, ip: None }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let description = match self.ip.as_deref() {
            Some(ip) => format!(
                "Look up the approximate location of IP address {ip} and send it to ipapi.co"
            ),
            None => {
                "Look up your approximate location from your IP address and send it to ipapi.co"
                    .to_string()
            }
        };
        self.state
            .gate()
            .check(GEO_LOCATION, "geo:ip-location", &description)?;

        let url = build_location_url(self.ip.as_deref())?;
        let body = fetch_json(self.state.client(), url, "ipapi.co").await?;
        let parsed: LocationResponse = serde_json::from_str(&body)
            .map_err(|e| ToolError::execution_failed(format!("Invalid ipapi.co response: {e}")))?;
        format_location(&parsed).map_err(ToolError::from)
    }
}

/// Builds the ipapi.co request URL.
fn build_location_url(ip: Option<&str>) -> Result<reqwest::Url, GeoError> {
    let base = if let Some(ip) = ip {
        let parsed: IpAddr = ip
            .parse()
            .map_err(|_| GeoError::InvalidArguments(format!("'{ip}' is not a valid IP address")))?;
        format!("https://ipapi.co/{parsed}/json/")
    } else {
        "https://ipapi.co/json/".to_string()
    };
    let url = reqwest::Url::parse(&base)
        .map_err(|e| GeoError::Internal(format!("invalid ipapi.co URL: {e}")))?;
    Ok(url)
}

fn format_location(response: &LocationResponse) -> Result<String, GeoError> {
    if response.error {
        let detail = response.reason.as_deref().unwrap_or("no details provided");
        return Err(GeoError::ApiFailure(format!(
            "ipapi.co rejected the request: {detail}"
        )));
    }

    let mut lines = Vec::new();
    if let Some(country) = response.country.as_deref() {
        lines.push(format!("Country: {}", truncate(country)));
    }
    if let Some(region) = response.region_name.as_deref() {
        lines.push(format!("Region: {}", truncate(region)));
    }
    if let Some(city) = response.city.as_deref() {
        lines.push(format!("City: {}", truncate(city)));
    }
    if let (Some(latitude), Some(longitude)) = (response.latitude, response.longitude) {
        lines.push(format!(
            "Coordinates: {}, {}",
            format_coord(latitude),
            format_coord(longitude)
        ));
    }
    if let Some(timezone) = response.timezone.as_deref() {
        lines.push(format!("Timezone: {}", truncate(timezone)));
    }
    if let Some(query) = response.query.as_deref() {
        lines.push(format!("IP: {query}"));
    }
    if lines.is_empty() {
        return Err(GeoError::InvalidResponse(
            "ipapi.co response contains no location data".to_string(),
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS_FIXTURE: &str = r#"{
        "country_name": "Japan",
        "region": "Tokyo",
        "city": "Tokyo",
        "lat": 35.6837,
        "lon": 139.6805,
        "timezone": "Asia/Tokyo",
        "ip": "153.246.222.121"
    }"#;

    fn parse(fixture: &str) -> LocationResponse {
        serde_json::from_str(fixture).unwrap()
    }

    #[test]
    fn formats_success_response() {
        let out = format_location(&parse(SUCCESS_FIXTURE)).unwrap();
        assert!(out.contains("Country: Japan"), "{out}");
        assert!(out.contains("Region: Tokyo"), "{out}");
        assert!(out.contains("City: Tokyo"), "{out}");
        assert!(
            out.contains(&format!(
                "Coordinates: {}, {}",
                format_coord(35.6837),
                format_coord(139.6805)
            )),
            "{out}"
        );
        assert!(out.contains("Timezone: Asia/Tokyo"), "{out}");
        assert!(out.contains("IP: 153.246.222.121"), "{out}");
    }

    #[test]
    fn missing_optional_fields_are_skipped() {
        let fixture = r#"{"country_name":"Japan"}"#;
        let out = format_location(&parse(fixture)).unwrap();
        assert_eq!(out, "Country: Japan");
    }

    #[test]
    fn api_failure_carries_the_message() {
        let fixture = r#"{"error":true,"reason":"invalid query"}"#;
        let err = format_location(&parse(fixture)).unwrap_err();
        assert!(matches!(err, GeoError::ApiFailure(_)));
        assert!(err.to_string().contains("invalid query"));
    }

    #[test]
    fn empty_success_response_is_rejected() {
        let err = format_location(&parse(r#"{"status":"success"}"#)).unwrap_err();
        assert!(matches!(err, GeoError::InvalidResponse(_)));
    }

    #[test]
    fn own_ip_url_has_no_address_segment() {
        let url = build_location_url(None).unwrap();
        assert_eq!(url.as_str(), "https://ipapi.co/json/");
    }

    #[test]
    fn explicit_ip_is_placed_in_the_path() {
        let url = build_location_url(Some("8.8.8.8")).unwrap();
        assert_eq!(url.as_str(), "https://ipapi.co/8.8.8.8/json/");
    }

    #[test]
    fn invalid_ip_is_rejected() {
        assert!(build_location_url(Some("not-an-ip")).is_err());
    }
}
