use super::{post_service, validate_entity_id};
use crate::approval::actions::HOMEASSISTANT_SET_TEMPERATURE;
use crate::provider::HomeAssistantState;
use ene_plugin::prelude::*;
use std::sync::Arc;

fn default_state() -> Arc<HomeAssistantState> {
    Arc::new(HomeAssistantState::new())
}

/// HVAC operation modes accepted by Home Assistant's `climate` domain.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum HvacMode {
    /// Heating only.
    Heat,
    /// Cooling only.
    Cool,
    /// Both heating and cooling.
    HeatCool,
    /// Home Assistant decides the mode automatically.
    Auto,
    /// Dehumidifying.
    Dry,
    /// Fan only.
    FanOnly,
    /// Climate off.
    Off,
}

impl HvacMode {
    /// The wire value sent to Home Assistant.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Heat => "heat",
            Self::Cool => "cool",
            Self::HeatCool => "heat_cool",
            Self::Auto => "auto",
            Self::Dry => "dry",
            Self::FanOnly => "fan_only",
            Self::Off => "off",
        }
    }
}

/// Sets the target temperature of a Home Assistant climate entity.
///
/// Changes physical state, so it runs only after explicit user approval.
#[derive(Clone, Deserialize, JsonSchema, ToolAction)]
#[tool(
    namespace = "homeassistant",
    name = "set_temperature",
    summary = "Set the target temperature of a Home Assistant climate entity.",
    description = "Sets the target temperature of an air conditioner, heater, or other climate entity via the Home Assistant REST API. An optional HVAC mode can be set together with the temperature. This changes the physical state of the device and requires explicit user approval.",
    category = "Utility",
    keywords_primary = "home assistant, smart home, climate, temperature, air conditioner, thermostat, hvac",
    side_effects = "Network { external: true }"
)]
/// Action to set a climate entity's target temperature.
pub struct SetTemperatureAction {
    #[tool(skip)]
    #[serde(skip, default = "default_state")]
    state: Arc<HomeAssistantState>,
    /// Entity id of the climate device, e.g. `climate.living_room`.
    entity_id: String,
    /// Target temperature in the unit configured for the entity.
    temperature: f64,
    /// Optional HVAC mode to apply together with the temperature.
    #[serde(default)]
    hvac_mode: Option<HvacMode>,
}

impl SetTemperatureAction {
    /// Creates a new `SetTemperatureAction` with the given shared state.
    #[must_use]
    pub fn new(state: Arc<HomeAssistantState>) -> Self {
        Self {
            state,
            entity_id: String::new(),
            temperature: 0.0,
            hvac_mode: None,
        }
    }

    async fn run(&self) -> Result<String, ToolError> {
        validate_entity_id(&self.entity_id)?;
        // JSON cannot carry non-finite numbers, so this is defense against a
        // caller constructing the action struct directly.
        if !self.temperature.is_finite() {
            return Err(ToolError::InvalidArguments {
                message: "temperature must be a finite number".to_string(),
            });
        }

        let entity_id = &self.entity_id;
        let temperature = self.temperature;
        let mode_suffix = match self.hvac_mode {
            Some(mode) => format!(" with HVAC mode {}", mode.as_str()),
            None => String::new(),
        };
        let description =
            format!("Set {entity_id} temperature to {temperature} in Home Assistant{mode_suffix}");
        self.state.gate().check(
            HOMEASSISTANT_SET_TEMPERATURE,
            &format!("homeassistant:entity:{entity_id}#"),
            &description,
        )?;

        let config = self.state.config();
        let base_url = config.base_url()?;
        let token = config.token()?;
        let url = crate::config::api_url(&base_url, "api/services/climate/set_temperature")?;
        let body = service_body(entity_id, temperature, self.hvac_mode);
        post_service(self.state.client(), url, token, &body).await?;
        Ok(serde_json::json!({
            "entity_id": entity_id,
            "temperature": temperature,
            "set": true
        })
        .to_string())
    }
}

/// The JSON body for the `climate.set_temperature` service call.
fn service_body(
    entity_id: &str,
    temperature: f64,
    hvac_mode: Option<HvacMode>,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "entity_id": entity_id,
        "temperature": temperature,
    });
    if let Some(mode) = hvac_mode {
        body["hvac_mode"] = serde_json::json!(mode.as_str());
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_body_carries_entity_and_temperature() {
        assert_eq!(
            service_body("climate.living_room", 22.5, None),
            serde_json::json!({ "entity_id": "climate.living_room", "temperature": 22.5 })
        );
    }

    #[test]
    fn service_body_includes_optional_hvac_mode() {
        assert_eq!(
            service_body("climate.bedroom", 18.0, Some(HvacMode::Heat)),
            serde_json::json!({
                "entity_id": "climate.bedroom",
                "temperature": 18.0,
                "hvac_mode": "heat"
            })
        );
        assert_eq!(
            service_body("climate.bedroom", 18.0, Some(HvacMode::HeatCool)),
            serde_json::json!({
                "entity_id": "climate.bedroom",
                "temperature": 18.0,
                "hvac_mode": "heat_cool"
            })
        );
    }

    #[test]
    fn hvac_mode_wire_values_are_stable() {
        assert_eq!(HvacMode::Heat.as_str(), "heat");
        assert_eq!(HvacMode::Cool.as_str(), "cool");
        assert_eq!(HvacMode::HeatCool.as_str(), "heat_cool");
        assert_eq!(HvacMode::Auto.as_str(), "auto");
        assert_eq!(HvacMode::Dry.as_str(), "dry");
        assert_eq!(HvacMode::FanOnly.as_str(), "fan_only");
        assert_eq!(HvacMode::Off.as_str(), "off");
    }

    #[test]
    fn hvac_mode_deserializes_from_snake_case() {
        let mode: HvacMode = serde_json::from_str(r#""heat_cool""#).unwrap();
        assert!(matches!(mode, HvacMode::HeatCool));
        assert!(serde_json::from_str::<HvacMode>(r#""turbo""#).is_err());
    }

    #[tokio::test]
    async fn unapproved_set_temperature_requires_permission() {
        let state = Arc::new(HomeAssistantState::new());
        let action = SetTemperatureAction {
            state,
            entity_id: "climate.living_room".to_string(),
            temperature: 22.0,
            hvac_mode: None,
        };
        let err = action.run().await.unwrap_err();
        assert!(matches!(err, ToolError::PermissionRequired { .. }));
    }

    #[tokio::test]
    async fn invalid_entity_id_fails_before_gate() {
        let state = Arc::new(HomeAssistantState::new());
        let action = SetTemperatureAction {
            state,
            entity_id: "../evil".to_string(),
            temperature: 22.0,
            hvac_mode: None,
        };
        let err = action.run().await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidArguments { .. }));
    }

    #[test]
    fn spec_has_expected_name() {
        assert_eq!(
            SetTemperatureAction::spec().name.as_str(),
            "homeassistant.set_temperature"
        );
    }
}
