use super::{get, truncate, truncate_attributes, validate_entity_id};
use crate::provider::HomeAssistantState;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<HomeAssistantState> {
    Arc::new(HomeAssistantState::new())
}

/// Response envelope of the Home Assistant `GET /api/states/{entity_id}` endpoint.
#[derive(Debug, Deserialize)]
struct StateResponse {
    #[serde(rename = "entity_id", default)]
    entity_id: String,
    #[serde(default)]
    state: String,
    #[serde(default)]
    attributes: serde_json::Map<String, serde_json::Value>,
    #[serde(rename = "last_updated", default)]
    last_updated: Option<String>,
}

/// Returns the current state of a Home Assistant entity.
///
/// The call is read-only; it does not require approval, but it does reach
/// the configured Home Assistant instance over the network.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "homeassistant",
    name = "state",
    summary = "Get the current state of a Home Assistant entity.",
    description = "Returns the current state, attributes, and last-updated time of a Home Assistant entity (sensor, switch, light, climate, plug, etc.) from the configured instance via its REST API.",
    category = "Utility",
    keywords_primary = "home assistant, smart home, entity, state, sensor, value, read",
    side_effects = "Network { external: true }"
)]
/// Action to read an entity's state.
pub struct StateAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<HomeAssistantState>,
    /// Entity id to read, e.g. `light.living_room` or `sensor.outdoor_temperature`.
    entity_id: String,
}

impl StateAction {
    /// Creates a new `StateAction` with the given shared state.
    #[must_use]
    pub fn new(state: Arc<HomeAssistantState>) -> Self {
        Self {
            state,
            entity_id: String::new(),
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        validate_entity_id(&self.entity_id)?;
        let config = self.state.config();
        let base_url = config.base_url()?;
        let token = config.token()?;
        let url = crate::config::api_url(&base_url, &format!("api/states/{}", self.entity_id))?;
        let body = get(self.state.client(), url, token).await?;
        let parsed: StateResponse = serde_json::from_str(&body).map_err(|e| {
            ToolError::execution_failed(format!("Invalid Home Assistant response: {e}"))
        })?;
        Ok(format_state(&parsed))
    }
}

/// Formats a state response into readable lines, bounding every echoed field.
fn format_state(response: &StateResponse) -> String {
    let mut lines = vec![
        format!("Entity: {}", truncate(&response.entity_id)),
        format!("State: {}", truncate(&response.state)),
    ];
    if !response.attributes.is_empty() {
        lines.push(format!(
            "Attributes: {}",
            truncate_attributes(&serde_json::Value::Object(response.attributes.clone()))
        ));
    }
    if let Some(updated) = response.last_updated.as_deref() {
        lines.push(format!("Last updated: {}", truncate(updated)));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
        "entity_id": "light.living_room",
        "state": "on",
        "attributes": { "brightness": 180, "friendly_name": "Living Room Light" },
        "last_updated": "2026-08-04T12:00:00+00:00"
    }"#;

    fn parse(fixture: &str) -> StateResponse {
        serde_json::from_str(fixture).unwrap()
    }

    #[test]
    fn formats_success_response() {
        let out = format_state(&parse(FIXTURE));
        assert!(out.contains("Entity: light.living_room"), "{out}");
        assert!(out.contains("State: on"), "{out}");
        assert!(out.contains("brightness"), "{out}");
        assert!(
            out.contains("Last updated: 2026-08-04T12:00:00+00:00"),
            "{out}"
        );
    }

    #[test]
    fn missing_optional_fields_are_skipped() {
        let out = format_state(&parse(r#"{"entity_id":"sensor.x","state":"21.5"}"#));
        assert_eq!(out, "Entity: sensor.x\nState: 21.5");
    }

    #[test]
    fn long_attributes_are_capped() {
        let fixture = format!(
            r#"{{"entity_id":"media_player.tv","state":"playing","attributes":{{"detail":"{}"}}}}"#,
            "x".repeat(10_000)
        );
        let out = format_state(&parse(&fixture));
        assert!(out.contains("..."), "{out}");
    }

    #[test]
    fn spec_has_expected_name() {
        assert_eq!(StateAction::spec().name.as_str(), "homeassistant.state");
    }
}
