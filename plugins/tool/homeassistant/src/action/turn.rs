use super::{post_service, validate_entity_id};
use crate::approval::actions::{HOMEASSISTANT_TURN_OFF, HOMEASSISTANT_TURN_ON};
use crate::provider::HomeAssistantState;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<HomeAssistantState> {
    Arc::new(HomeAssistantState::new())
}

/// Turns a Home Assistant entity (switch, light, plug) on.
///
/// Changes physical state, so it runs only after explicit user approval.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "homeassistant",
    name = "turn_on",
    summary = "Turn a Home Assistant entity on.",
    description = "Turns a switch, light, smart plug, or other on/off entity on via the Home Assistant REST API. This changes the physical state of the device and requires explicit user approval.",
    category = "Utility",
    keywords_primary = "home assistant, smart home, turn on, switch, light, plug, power",
    side_effects = "Network { external: true }"
)]
/// Action to turn an entity on.
pub struct TurnOnAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<HomeAssistantState>,
    /// Entity id to turn on, e.g. `light.living_room` or `switch.kitchen_plug`.
    entity_id: String,
}

impl TurnOnAction {
    /// Creates a new `TurnOnAction` with the given shared state.
    #[must_use]
    pub fn new(state: Arc<HomeAssistantState>) -> Self {
        Self {
            state,
            entity_id: String::new(),
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        run_turn(&self.state, &self.entity_id, true).await
    }
}

/// Turns a Home Assistant entity (switch, light, plug) off.
///
/// Changes physical state, so it runs only after explicit user approval.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "homeassistant",
    name = "turn_off",
    summary = "Turn a Home Assistant entity off.",
    description = "Turns a switch, light, smart plug, or other on/off entity off via the Home Assistant REST API. This changes the physical state of the device and requires explicit user approval.",
    category = "Utility",
    keywords_primary = "home assistant, smart home, turn off, switch, light, plug, power",
    side_effects = "Network { external: true }"
)]
/// Action to turn an entity off.
pub struct TurnOffAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<HomeAssistantState>,
    /// Entity id to turn off, e.g. `light.living_room` or `switch.kitchen_plug`.
    entity_id: String,
}

impl TurnOffAction {
    /// Creates a new `TurnOffAction` with the given shared state.
    #[must_use]
    pub fn new(state: Arc<HomeAssistantState>) -> Self {
        Self {
            state,
            entity_id: String::new(),
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        run_turn(&self.state, &self.entity_id, false).await
    }
}

/// Shared turn-on/turn-off flow: validate, gate, then call the generic
/// `homeassistant.turn_on` / `homeassistant.turn_off` service.
async fn run_turn(
    state: &HomeAssistantState,
    entity_id: &str,
    turn_on: bool,
) -> Result<String, ToolError> {
    validate_entity_id(entity_id)?;
    let action = if turn_on {
        HOMEASSISTANT_TURN_ON
    } else {
        HOMEASSISTANT_TURN_OFF
    };
    let target = format!("homeassistant:entity:{entity_id}");
    let description = if turn_on {
        format!("Turn on {entity_id} in Home Assistant")
    } else {
        format!("Turn off {entity_id} in Home Assistant")
    };
    state.gate().check(action, &target, &description)?;

    let config = state.config();
    let base_url = config.base_url()?;
    let token = config.token()?;
    let service = if turn_on { "turn_on" } else { "turn_off" };
    let url = crate::config::api_url(&base_url, &format!("api/services/homeassistant/{service}"))?;
    let body = service_body(entity_id);
    post_service(state.client(), url, token, &body).await?;
    let payload = if turn_on {
        serde_json::json!({ "entity_id": entity_id, "turned_on": true })
    } else {
        serde_json::json!({ "entity_id": entity_id, "turned_off": true })
    };
    Ok(payload.to_string())
}

/// The JSON body for the generic turn service call.
fn service_body(entity_id: &str) -> serde_json::Value {
    serde_json::json!({ "entity_id": entity_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_body_carries_entity_id() {
        assert_eq!(
            service_body("light.living_room"),
            serde_json::json!({ "entity_id": "light.living_room" })
        );
    }

    #[tokio::test]
    async fn unapproved_turn_on_requires_permission() {
        let state = Arc::new(HomeAssistantState::new());
        let action = TurnOnAction {
            state,
            entity_id: "light.living_room".to_string(),
        };
        let err = action.run().await.unwrap_err();
        assert!(matches!(err, ToolError::PermissionRequired { .. }));
    }

    #[tokio::test]
    async fn unapproved_turn_off_requires_permission() {
        let state = Arc::new(HomeAssistantState::new());
        let action = TurnOffAction {
            state,
            entity_id: "switch.kitchen_plug".to_string(),
        };
        let err = action.run().await.unwrap_err();
        assert!(matches!(err, ToolError::PermissionRequired { .. }));
    }

    #[tokio::test]
    async fn approval_leads_to_configuration_error_before_network() {
        // After approval the gate passes and the next failure is the missing
        // token — proving the gate runs before any HTTP request.
        let state = Arc::new(HomeAssistantState::new());
        let action = TurnOnAction {
            state: state.clone(),
            entity_id: "light.living_room".to_string(),
        };
        let err = action.run().await.unwrap_err();
        let ToolError::PermissionRequired { request_id, .. } = err else {
            panic!("expected PermissionRequired");
        };
        state.gate().approve_request(&request_id);
        let err = action.run().await.unwrap_err();
        assert!(
            err.to_string().contains("not configured"),
            "expected configuration error, got {err}"
        );
    }

    #[test]
    fn spec_names_are_expected() {
        assert_eq!(TurnOnAction::spec().name.as_str(), "homeassistant.turn_on");
        assert_eq!(
            TurnOffAction::spec().name.as_str(),
            "homeassistant.turn_off"
        );
    }
}
