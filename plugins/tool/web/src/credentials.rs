//! Request-scoped vault credentials injected by the host.

use super::search::SearchBackend;
use serde_json::Value;

#[derive(Clone)]
pub(crate) struct WebCredentials {
    pub tavily: Option<String>,
    pub exa: Option<String>,
}

impl WebCredentials {
    #[cfg_attr(
        any(not(test), test),
        expect(dead_code, reason = "host-only credential source; exercised by tests")
    )]
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
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "test-scoped helper; exercised by tests")
)]
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
    CREDENTIALS.with(|cell| cell.borrow().clone())
}
