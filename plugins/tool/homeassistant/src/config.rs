use crate::error::HomeAssistantError;
use serde::{Deserialize, Serialize};

/// Default base URL of a local Home Assistant instance.
const DEFAULT_BASE_URL: &str = "http://homeassistant.local:8123";

/// Plugin configuration delivered by the host from
/// `plugins.list.homeassistant.config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct HomeAssistantConfig {
    /// Base URL of the Home Assistant instance, e.g.
    /// `http://homeassistant.local:8123`. A reverse-proxy path prefix is
    /// supported and must end with `/`.
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// Long-lived access token created in Home Assistant under
    /// Profile → Security → Long-Lived Access Tokens. Marked
    /// `x-ene-secret` so the host masks and redacts it.
    pub token: String,
}

fn default_base_url() -> String {
    DEFAULT_BASE_URL.to_string()
}

impl Default for HomeAssistantConfig {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            token: String::new(),
        }
    }
}

impl HomeAssistantConfig {
    pub fn token(&self) -> Result<&str, HomeAssistantError> {
        if self.token.trim().is_empty() {
            return Err(HomeAssistantError::NotConfigured(
                "Home Assistant is not configured: set \
                 plugins.list.homeassistant.config.token to a long-lived access token \
                 (Profile → Security → Long-Lived Access Tokens in Home Assistant), \
                 then reconfigure the plugin"
                    .to_string(),
            ));
        }
        Ok(self.token.trim())
    }

    /// Returns the base URL normalized to end with `/`, so relative path
    /// joins keep any reverse-proxy prefix. A query string or fragment is
    /// rejected because it would corrupt the joined path.
    pub fn base_url(&self) -> Result<String, HomeAssistantError> {
        let raw = self.base_url.trim();
        if raw.is_empty() {
            return Err(HomeAssistantError::NotConfigured(
                "Home Assistant is not configured: set \
                 plugins.list.homeassistant.config.base_url to your instance URL \
                 (e.g. http://homeassistant.local:8123)"
                    .to_string(),
            ));
        }
        let parsed = url::Url::parse(raw).map_err(|e| {
            HomeAssistantError::InvalidArguments(format!("base_url is not a valid URL: {e}"))
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(HomeAssistantError::InvalidArguments(
                "base_url must use http or https".to_string(),
            ));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(HomeAssistantError::InvalidArguments(
                "base_url must not contain userinfo".to_string(),
            ));
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(HomeAssistantError::InvalidArguments(
                "base_url must not contain a query string or fragment".to_string(),
            ));
        }
        let mut normalized = parsed.as_str().to_string();
        if !parsed.path().ends_with('/') {
            normalized.push('/');
        }
        Ok(normalized)
    }
}

/// Builds the absolute URL for a Home Assistant API path under `base_url`.
///
/// `base_url` must already be normalized (see [`HomeAssistantConfig::base_url`]);
/// the path is joined as a relative reference so a path prefix is preserved.
pub fn api_url(base_url: &str, path: &str) -> Result<url::Url, HomeAssistantError> {
    url::Url::parse(base_url)
        .and_then(|base| base.join(path))
        .map_err(|e| {
            HomeAssistantError::Internal(format!("failed to build Home Assistant request URL: {e}"))
        })
}

/// JSON Schema for the plugin configuration.
///
/// `token` carries the `x-ene-secret` marker so the host masks it in logs
/// and the UI. The top-level `x-ene-credentials` block registers the
/// canonical credential id with the host's credential registry, ready for
/// the host-side credential client API once it reaches the mainline.
pub fn config_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "base_url": {
                "type": "string",
                "description": "Base URL of the Home Assistant instance (http or https only), e.g. http://homeassistant.local:8123 (a reverse-proxy path prefix must end with /)"
            },
            "token": {
                "type": "string",
                "x-ene-secret": true,
                "description": "Home Assistant long-lived access token (Profile -> Security -> Long-Lived Access Tokens)"
            }
        },
        "x-ene-credentials": [
            {
                "id": "homeassistant",
                "kind": "api_key",
                "shared": false,
                "header": { "name": "Authorization", "format": "Bearer {value}" },
                "env_fallback": "HOME_ASSISTANT_TOKEN",
                "label": "Home Assistant Long-Lived Access Token",
                "help_url": "https://www.home-assistant.io/docs/authentication/"
            }
        ]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_preset_base_url() {
        // The host always delivers a config blob (possibly `{}`), so the
        // serde default — not the in-memory `Default` — is what users see.
        let config: HomeAssistantConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert!(config.token().is_err());
    }

    #[test]
    fn missing_token_is_a_configuration_error() {
        let config = HomeAssistantConfig::default();
        let err = config.token().unwrap_err();
        assert!(matches!(err, HomeAssistantError::NotConfigured(_)));
        assert!(!err.to_string().contains("secret"));
    }

    #[test]
    fn token_is_trimmed() {
        let config = HomeAssistantConfig {
            token: "  tok-123  ".to_string(),
            ..HomeAssistantConfig::default()
        };
        assert_eq!(config.token().unwrap(), "tok-123");
    }

    #[test]
    fn base_url_gains_trailing_slash() {
        let config = HomeAssistantConfig {
            base_url: "http://ha.local:8123".to_string(),
            ..HomeAssistantConfig::default()
        };
        assert_eq!(config.base_url().unwrap(), "http://ha.local:8123/");
    }

    #[test]
    fn base_url_path_prefix_keeps_trailing_slash() {
        let config = HomeAssistantConfig {
            base_url: "http://ha.local/hass".to_string(),
            ..HomeAssistantConfig::default()
        };
        assert_eq!(config.base_url().unwrap(), "http://ha.local/hass/");
    }

    #[test]
    fn base_url_rejects_query_and_fragment() {
        for raw in ["http://ha.local/?a=1", "http://ha.local/#frag"] {
            let config = HomeAssistantConfig {
                base_url: raw.to_string(),
                ..HomeAssistantConfig::default()
            };
            assert!(matches!(
                config.base_url(),
                Err(HomeAssistantError::InvalidArguments(_))
            ));
        }
    }

    #[test]
    fn base_url_rejects_unparseable_input() {
        let config = HomeAssistantConfig {
            base_url: "not a url".to_string(),
            ..HomeAssistantConfig::default()
        };
        assert!(matches!(
            config.base_url(),
            Err(HomeAssistantError::InvalidArguments(_))
        ));
    }

    #[test]
    fn base_url_rejects_non_http_schemes() {
        for raw in ["ftp://ha.local:8123", "file:///tmp/hass"] {
            let config = HomeAssistantConfig {
                base_url: raw.to_string(),
                ..HomeAssistantConfig::default()
            };
            assert!(
                matches!(
                    config.base_url(),
                    Err(HomeAssistantError::InvalidArguments(_))
                ),
                "scheme {raw} must be rejected"
            );
        }
    }

    #[test]
    fn base_url_rejects_userinfo() {
        let config = HomeAssistantConfig {
            base_url: "http://user:pass@ha.local:8123".to_string(),
            ..HomeAssistantConfig::default()
        };
        assert!(matches!(
            config.base_url(),
            Err(HomeAssistantError::InvalidArguments(_))
        ));
    }

    #[test]
    fn api_url_joins_under_path_prefix() {
        let url = api_url("http://ha.local/hass/", "api/states/light.living_room").unwrap();
        assert_eq!(
            url.as_str(),
            "http://ha.local/hass/api/states/light.living_room"
        );
    }

    #[test]
    fn api_url_joins_at_root() {
        let url = api_url("http://ha.local/", "api/services/homeassistant/turn_on").unwrap();
        assert_eq!(
            url.as_str(),
            "http://ha.local/api/services/homeassistant/turn_on"
        );
    }

    #[test]
    fn schema_marks_token_secret_and_declares_credential() {
        let schema = config_schema();
        assert_eq!(
            schema.pointer("/properties/token/x-ene-secret"),
            Some(&serde_json::json!(true))
        );
        assert_eq!(
            schema.pointer("/x-ene-credentials/0/id"),
            Some(&serde_json::json!("homeassistant"))
        );
        assert_eq!(
            schema.pointer("/x-ene-credentials/0/header/format"),
            Some(&serde_json::json!("Bearer {value}"))
        );
    }
}
