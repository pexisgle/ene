use super::{fetch_json, truncate, validate_latitude, validate_longitude};
use crate::approval::actions::GEO_WEATHER;
use crate::error::GeoError;
use crate::provider::GeoState;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<GeoState> {
    Arc::new(GeoState::new())
}

/// Response envelope of the wttr.in `format=j1` endpoint.
#[derive(Debug, Deserialize)]
struct WeatherResponse {
    #[serde(rename = "current_condition", default)]
    current_condition: Vec<CurrentCondition>,
    #[serde(rename = "nearest_area", default)]
    nearest_area: Vec<Area>,
}

#[derive(Debug, Deserialize)]
struct CurrentCondition {
    #[serde(rename = "temp_C", default)]
    temp_c: Option<String>,
    #[serde(rename = "FeelsLikeC", default)]
    feels_like_c: Option<String>,
    #[serde(default)]
    humidity: Option<String>,
    #[serde(default)]
    cloudcover: Option<String>,
    #[serde(rename = "winddir16Point", default)]
    wind_dir: Option<String>,
    #[serde(rename = "windspeedKmph", default)]
    wind_speed_kmph: Option<String>,
    #[serde(default)]
    pressure: Option<String>,
    #[serde(default)]
    visibility: Option<String>,
    #[serde(rename = "weatherDesc", default)]
    weather_desc: Vec<WeatherValue>,
    #[serde(rename = "observation_time", default)]
    observation_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Area {
    #[serde(rename = "areaName", default)]
    names: Vec<WeatherValue>,
    #[serde(default)]
    region: Vec<WeatherValue>,
    #[serde(default)]
    country: Vec<WeatherValue>,
    #[serde(default)]
    latitude: Option<String>,
    #[serde(default)]
    longitude: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WeatherValue {
    value: String,
}

/// Returns current weather conditions from wttr.in.
///
/// `location` is a city name (e.g. "Tokyo") or "lat,lon" coordinates. When
/// it is omitted, wttr.in derives the location from the caller's IP
/// address, which reveals the user's approximate location; that call
/// requires explicit user approval.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "geo",
    name = "weather",
    summary = "Get current weather conditions for a location.",
    description = "Returns current weather (temperature, humidity, cloud cover, wind, visibility, pressure) for a city name (e.g. \"Tokyo\") or \"lat,lon\" coordinates using wttr.in. When `location` is omitted, wttr.in derives the location from the caller's IP address, which reveals the user's approximate location; that call requires explicit user approval.",
    category = "Utility",
    keywords_primary = "weather, temperature, forecast, humidity, wind, rain, conditions",
    side_effects = "Network { external: true }"
)]
/// Action to get current weather conditions.
pub struct WeatherAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<GeoState>,
    /// City name (e.g. "Tokyo") or "lat,lon" coordinates. Omitted: derive from the caller's IP (requires approval).
    #[serde(default)]
    location: Option<String>,
}

impl WeatherAction {
    /// Creates a new `WeatherAction` with the given shared state.
    #[must_use]
    pub fn new(state: Arc<GeoState>) -> Self {
        Self {
            state,
            location: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        let description = match self.location.as_deref() {
            Some(location) => {
                format!("Look up weather for {location} and send it to wttr.in")
            }
            None => "Look up the weather for your approximate location, derived from your IP address, via wttr.in"
                .to_string(),
        };
        self.state
            .gate()
            .check(GEO_WEATHER, "geo:ip-weather", &description)?;

        let url = build_weather_url(self.location.as_deref())?;
        let body = fetch_json(self.state.client(), url, "wttr.in").await?;
        let parsed: WeatherResponse = serde_json::from_str(&body)
            .map_err(|e| ToolError::execution_failed(format!("Invalid wttr.in response: {e}")))?;
        format_weather(&parsed).map_err(ToolError::from)
    }
}

/// Builds the wttr.in request URL, percent-encoding the location path
/// segment and requesting the `j1` JSON format.
fn build_weather_url(location: Option<&str>) -> Result<reqwest::Url, GeoError> {
    if let Some(location) = location
        && location.trim().is_empty()
    {
        return Err(GeoError::InvalidArguments(
            "location must not be empty".to_string(),
        ));
    }
    if let Some(location) = location {
        validate_weather_location(location)?;
    }

    let mut url = reqwest::Url::parse("https://wttr.in/")
        .map_err(|e| GeoError::Internal(format!("invalid wttr.in URL: {e}")))?;
    if let Some(location) = location {
        url.path_segments_mut()
            .map_err(|()| GeoError::Internal("wttr.in URL has no path".to_string()))?
            .push(location.trim());
    }
    url.query_pairs_mut().append_pair("format", "j1");
    Ok(url)
}

/// Validates a "lat,lon" location; city names, including names with commas,
/// are passed through.
fn validate_weather_location(location: &str) -> Result<(), GeoError> {
    let Some((latitude_part, longitude_part)) = location.split_once(',') else {
        return Ok(());
    };
    let latitude = latitude_part.trim().parse::<f64>();
    let longitude = longitude_part.trim().parse::<f64>();
    match (latitude, longitude) {
        (Ok(latitude), Ok(longitude)) => {
            validate_latitude(latitude)?;
            validate_longitude(longitude)?;
        }
        (Ok(_), Err(_)) | (Err(_), Ok(_)) => {
            return Err(GeoError::InvalidArguments(format!(
                "'{location}' is not a valid location"
            )));
        }
        (Err(_), Err(_)) => {}
    }
    Ok(())
}

fn format_weather(response: &WeatherResponse) -> Result<String, GeoError> {
    let condition = response.current_condition.first().ok_or_else(|| {
        GeoError::InvalidResponse("wttr.in response is missing current_condition".to_string())
    })?;

    let mut lines = Vec::new();
    if let Some(area) = response.nearest_area.first() {
        let mut parts: Vec<&str> = Vec::new();
        for part in [
            area.names.first().map(|v| v.value.as_str()),
            area.region.first().map(|v| v.value.as_str()),
            area.country.first().map(|v| v.value.as_str()),
        ]
        .into_iter()
        .flatten()
        {
            if !part.is_empty() && !parts.contains(&part) {
                parts.push(part);
            }
        }
        let names: Vec<String> = parts.into_iter().map(truncate).collect();
        let coords = match (area.latitude.as_deref(), area.longitude.as_deref()) {
            (Some(lat), Some(lon)) => format!(" ({lat}, {lon})"),
            _ => String::new(),
        };
        if !names.is_empty() {
            lines.push(format!("Location: {}{coords}", names.join(", ")));
        }
    }

    if let Some(desc) = condition.weather_desc.first() {
        lines.push(format!("Condition: {}", truncate(&desc.value)));
    }
    if let Some(temp) = condition.temp_c.as_deref() {
        lines.push(format!("Temperature: {temp} °C"));
    }
    if let Some(feels_like) = condition.feels_like_c.as_deref() {
        lines.push(format!("Feels like: {feels_like} °C"));
    }
    if let Some(humidity) = condition.humidity.as_deref() {
        lines.push(format!("Humidity: {humidity}%"));
    }
    if let Some(cloudcover) = condition.cloudcover.as_deref() {
        lines.push(format!("Cloud cover: {cloudcover}%"));
    }
    if let Some(wind_speed) = condition.wind_speed_kmph.as_deref() {
        let direction = condition
            .wind_dir
            .as_deref()
            .map_or_else(String::new, |dir| format!(" {dir}"));
        lines.push(format!("Wind: {wind_speed} km/h{direction}"));
    }
    if let Some(pressure) = condition.pressure.as_deref() {
        lines.push(format!("Pressure: {pressure} hPa"));
    }
    if let Some(visibility) = condition.visibility.as_deref() {
        lines.push(format!("Visibility: {visibility} km"));
    }
    if let Some(observed_at) = condition.observation_time.as_deref() {
        lines.push(format!("Observed at: {observed_at} UTC"));
    }

    if lines.is_empty() {
        return Err(GeoError::InvalidResponse(
            "wttr.in response contains no weather data".to_string(),
        ));
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const WEATHER_FIXTURE: &str = r#"{
        "current_condition": [
            {
                "temp_C": "24",
                "FeelsLikeC": "25",
                "humidity": "83",
                "cloudcover": "75",
                "winddir16Point": "NE",
                "windspeedKmph": "48",
                "pressure": "1014",
                "visibility": "10",
                "weatherDesc": [{"value": "Patchy rain nearby"}],
                "observation_time": "05:21 PM"
            }
        ],
        "nearest_area": [
            {
                "areaName": [{"value": "Tokyo"}],
                "region": [{"value": "Tokyo"}],
                "country": [{"value": "Japan"}],
                "latitude": "35.68",
                "longitude": "139.69"
            }
        ]
    }"#;

    fn parse(fixture: &str) -> WeatherResponse {
        serde_json::from_str(fixture).unwrap()
    }

    #[test]
    fn formats_weather_response() {
        let out = format_weather(&parse(WEATHER_FIXTURE)).unwrap();
        assert!(
            out.contains("Location: Tokyo, Japan (35.68, 139.69)"),
            "{out}"
        );
        assert!(out.contains("Condition: Patchy rain nearby"), "{out}");
        assert!(out.contains("Temperature: 24 °C"), "{out}");
        assert!(out.contains("Feels like: 25 °C"), "{out}");
        assert!(out.contains("Humidity: 83%"), "{out}");
        assert!(out.contains("Cloud cover: 75%"), "{out}");
        assert!(out.contains("Wind: 48 km/h NE"), "{out}");
        assert!(out.contains("Pressure: 1014 hPa"), "{out}");
        assert!(out.contains("Visibility: 10 km"), "{out}");
        assert!(out.contains("Observed at: 05:21 PM UTC"), "{out}");
    }

    #[test]
    fn duplicate_area_names_are_deduplicated() {
        let out = format_weather(&parse(WEATHER_FIXTURE)).unwrap();
        let location_line = out.lines().find(|l| l.starts_with("Location:")).unwrap();
        assert_eq!(location_line.matches("Tokyo").count(), 1, "{location_line}");
    }

    #[test]
    fn missing_condition_is_rejected() {
        let err = format_weather(&parse(r#"{"nearest_area":[]}"#)).unwrap_err();
        assert!(matches!(err, GeoError::InvalidResponse(_)));
    }

    #[test]
    fn coordinate_location_is_validated() {
        assert!(build_weather_url(Some("35.68,139.69")).is_ok());
        assert!(build_weather_url(Some("91,139")).is_err());
        assert!(build_weather_url(Some("35,181")).is_err());
        assert!(build_weather_url(Some("35,not-a-number")).is_err());
        assert!(build_weather_url(Some("   ")).is_err());
    }

    #[test]
    fn city_name_with_comma_is_accepted() {
        let url = build_weather_url(Some("Paris, France")).unwrap();
        assert_eq!(url.as_str(), "https://wttr.in/Paris,%20France?format=j1");
    }

    #[test]
    fn city_name_is_encoded_into_the_path() {
        let url = build_weather_url(Some("New York")).unwrap();
        assert_eq!(url.as_str(), "https://wttr.in/New%20York?format=j1");
    }

    #[test]
    fn hostile_location_stays_in_a_single_path_segment() {
        let url = build_weather_url(Some("../../etc")).unwrap();
        assert_eq!(url.host_str(), Some("wttr.in"));
        assert_eq!(url.path(), "/..%2F..%2Fetc");

        let url = build_weather_url(Some("a?b#c")).unwrap();
        assert_eq!(url.host_str(), Some("wttr.in"));
        assert_eq!(url.path(), "/a%3Fb%23c");
        assert_eq!(url.query(), Some("format=j1"));
    }

    #[test]
    fn omitted_location_keeps_the_root_path() {
        let url = build_weather_url(None).unwrap();
        assert_eq!(url.as_str(), "https://wttr.in/?format=j1");
    }
}
