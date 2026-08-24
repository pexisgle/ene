//! Request-scoped vault credentials injected by the host.

use super::search::SearchBackend;
use serde_json::Value;

const ENV_CONFIG_KEY: &str = "ENE_PROVIDER_CONFIG";

thread_local! {
    static HOST_CREDENTIALS: std::cell::RefCell<Option<WebCredentials>> =
        const { std::cell::RefCell::new(None) };
}

/// Install the host-provided credential set for `run`. The fiber supervisor
/// calls this around every bundled `web.*` execution; the plugin process
/// instead seeds the same storage once from its spawn config.
pub(crate) fn install_host_credentials<T>(creds: Option<Value>, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            HOST_CREDENTIALS.with(std::cell::RefCell::take);
        }
    }
    let parsed = creds.map(|value| WebCredentials::from_config(&value));
    HOST_CREDENTIALS.with(|cell| cell.replace(parsed));
    let _reset = Reset;
    run()
}

fn host_credentials() -> Option<WebCredentials> {
    HOST_CREDENTIALS.with(|cell| cell.borrow().clone())
}

#[derive(Clone, Default)]
pub(crate) struct WebCredentials {
    pub(crate) tavily: Option<String>,
    pub(crate) exa: Option<String>,
}

impl WebCredentials {
    pub(crate) fn from_config(value: &Value) -> Self {
        let key = |names: &[&str]| {
            names.iter().find_map(|name| {
                value
                    .get(*name)
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .map(str::to_owned)
            })
        };
        Self {
            tavily: key(&["tavily_api_key"]),
            exa: key(&["exa_api_key"]),
        }
    }

    pub(crate) fn for_backend(&self, backend: SearchBackend) -> Option<&str> {
        match backend {
            SearchBackend::Tavily => self.tavily.as_deref(),
            SearchBackend::Exa => self.exa.as_deref(),
            _ => None,
        }
    }
}

thread_local! {
    static CREDENTIALS:
        std::cell::RefCell<Option<WebCredentials>> = const { std::cell::RefCell::new(None) };
}

/// Run `run` with `creds` as the only credential source. The host installs it
/// per invocation so a plugin process never holds secrets across calls; tests
/// use it to simulate broker injection.
#[cfg_attr(not(test), expect(dead_code, reason = "tests only"))]
pub(crate) fn with_credentials<T>(creds: WebCredentials, run: impl FnOnce() -> T) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            CREDENTIALS.with(std::cell::RefCell::take);
        }
    }
    CREDENTIALS.with(|cell| cell.replace(Some(creds)));
    let _reset = Reset;
    run()
}

#[must_use]
pub(crate) fn try_credentials() -> Option<WebCredentials> {
    if let Some(creds) = host_credentials() {
        return Some(creds);
    }
    if let Some(creds) = CREDENTIALS.with(|cell| cell.borrow().clone()) {
        return Some(creds);
    }
    spawn_env_credentials()
}

/// Read the provider config the host passed to this spawned plugin process.
/// The bundled in-process path instead receives per-call credentials through
/// the host context, so a spawned process never needs its own seeding step.
fn spawn_env_credentials() -> Option<WebCredentials> {
    let raw = std::env::var(ENV_CONFIG_KEY).ok()?;
    parse_config_credentials(&raw)
}

fn parse_config_credentials(raw: &str) -> Option<WebCredentials> {
    let value = serde_json::from_str::<Value>(raw)
        .map_err(|err| tracing::warn!(error = %err, "tool.web config is not valid JSON"))
        .ok()?;
    Some(WebCredentials::from_config(&value))
}

#[cfg(test)]
mod tests {
    use super::parse_config_credentials;

    #[test]
    fn spawned_process_parses_provider_config_payload() {
        assert_eq!(
            parse_config_credentials(r#"{"tavily_api_key":"tvly-x"}"#)
                .and_then(|creds| creds.tavily)
                .as_deref(),
            Some("tvly-x")
        );

        assert!(parse_config_credentials("not-json").is_none());
    }
}
