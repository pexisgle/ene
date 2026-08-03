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

/// Response envelope of the ip-api.com JSON endpoint.
#[derive(Debug, Deserialize)]
struct LocationResponse {
    status: String,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(rename = "regionName", default)]
    region_name: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(rename = "lat", default)]
    latitude: Option<f64>,
    #[serde(rename = "lon", default)]
    longitude: Option<f64>,
    #[serde(default)]
    timezone: Option<String>,
    #[serde(default)]
    query: Option<String>,
}

/// Looks up the geographic location of an IP address via ip-api.com.
///
/// When `ip` is omitted, the caller's own public IP is located, which
/// reveals the user's approximate location; that call requires explicit
/// user approval. ip-api.com's free tier only supports plain HTTP, so the
/// request itself is not encrypted.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "geo",
    name = "location",
    summary = "Get the geographic location of an IP address.",
    description = "Looks up the approximate geographic location (country, region, city, coordinates, timezone) of an IP address using ip-api.com. When `ip` is omitted, the caller's own public IP is located, which reveals the user's approximate location; that call requires explicit user approval. The free ip-api.com tier is plain HTTP only, so the request is not encrypted.",
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
        if self.ip.is_none() {
            self.state.gate().check(
                GEO_LOCATION,
                "geo:ip-location",
                "Look up your approximate location from your IP address and send it to ip-api.com",
            )?;
        }

        let url = build_location_url(self.ip.as_deref())?;
        let body = fetch_json(self.state.client(), url, "ip-api.com").await?;
        let parsed: LocationResponse = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Invalid ip-api.com response: {e}"))
        })?;
        format_location(&parsed).map_err(ToolError::from)
    }
}

/// Builds the ip-api.com request URL.
///
/// The free tier exposes only the HTTP endpoint; HTTPS answers with
/// "SSL unavailable for this endpoint". The `fields` parameter keeps the
/// response to the fields the tool actually reports.
fn build_location_url(ip: Option<&str>) -> Result<reqwest::Url, GeoError> {
    let base = if let Some(ip) = ip {
        let parsed: IpAddr = ip
            .parse()
            .map_err(|_| GeoError::InvalidArguments(format!("'{ip}' is not a valid IP address")))?;
        format!("http://ip-api.com/json/{parsed}")
    } else {
        "http://ip-api.com/json/".to_string()
    };
    let mut url = reqwest::Url::parse(&base)
        .map_err(|e| GeoError::Internal(format!("invalid ip-api.com URL: {e}")))?;
    url.query_pairs_mut().append_pair(
        "fields",
        "status,message,country,regionName,city,lat,lon,timezone,query",
    );
    Ok(url)
}

fn format_location(response: &LocationResponse) -> Result<String, GeoError> {
    if response.status != "success" {
        let detail = response.message.as_deref().unwrap_or("no details provided");
        return Err(GeoError::ApiFailure(format!(
            "ip-api.com rejected the request: {detail}"
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
            "ip-api.com response contains no location data".to_string(),
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUCCESS_FIXTURE: &str = r#"{
        "status": "success",
        "country": "Japan",
        "regionName": "Tokyo",
        "city": "Tokyo",
        "lat": 35.6837,
        "lon": 139.6805,
        "timezone": "Asia/Tokyo",
        "query": "153.246.222.121"
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
        let fixture = r#"{"status":"success","country":"Japan"}"#;
        let out = format_location(&parse(fixture)).unwrap();
        assert_eq!(out, "Country: Japan");
    }

    #[test]
    fn api_failure_carries_the_message() {
        let fixture = r#"{"status":"fail","message":"invalid query"}"#;
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
        assert_eq!(
            url.as_str(),
            "http://ip-api.com/json/?fields=status%2Cmessage%2Ccountry%2CregionName%2Ccity%2Clat%2Clon%2Ctimezone%2Cquery"
        );
    }

    #[test]
    fn explicit_ip_is_placed_in_the_path() {
        let url = build_location_url(Some("8.8.8.8")).unwrap();
        assert!(url.as_str().starts_with("http://ip-api.com/json/8.8.8.8?"));
    }

    #[test]
    fn invalid_ip_is_rejected() {
        assert!(build_location_url(Some("not-an-ip")).is_err());
    }
}
