//! Host-injected HTTP for bundled web tools.
//!
//! Production `web.*` calls run in the host with this hook set to the net
//! broker. A plugin process without the hook fails closed and never dials.

use serde_json::Value;
use std::cell::RefCell;

type HostFetch = Box<dyn Fn(&str) -> Result<Value, String>>;
type HostPostJson = Box<dyn Fn(&str, &Value, &str) -> Result<Value, String>>;

thread_local! {
    static HOST_FETCH: RefCell<Option<HostFetch>> = const { RefCell::new(None) };
    static HOST_POST: RefCell<Option<HostPostJson>> = const { RefCell::new(None) };
}

/// Run `run` with `fetch` as the only HTTP path for bundled web tools.
pub fn with_http_fetch<T>(
    fetch: impl Fn(&str) -> Result<Value, String> + 'static,
    run: impl FnOnce() -> T,
) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            HOST_FETCH.with(|cell| {
                cell.replace(None);
            });
        }
    }
    HOST_FETCH.with(|cell| cell.replace(Some(Box::new(fetch))));
    let _reset = Reset;
    run()
}

/// Host broker fetch, if the current thread installed one.
#[must_use]
pub fn try_host_fetch(url: &str) -> Option<Result<Value, String>> {
    HOST_FETCH.with(|cell| cell.borrow().as_ref().map(|fetch| fetch(url)))
}

/// Run `run` with `post` as the only JSON POST path for bundled web tools.
pub fn with_post_json<T>(
    post: impl Fn(&str, &Value, &str) -> Result<Value, String> + 'static,
    run: impl FnOnce() -> T,
) -> T {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            HOST_POST.with(|cell| cell.replace(None));
        }
    }
    HOST_POST.with(|cell| cell.replace(Some(Box::new(post))));
    let _reset = Reset;
    run()
}

/// Host broker JSON POST with bearer credential, if a hook is installed.
#[must_use]
pub fn try_host_post_json(url: &str, body: &Value, bearer: &str) -> Option<Result<Value, String>> {
    HOST_POST.with(|cell| cell.borrow().as_ref().map(|post| post(url, body, bearer)))
}

#[cfg(test)]
mod tests {
    use super::{try_host_fetch, try_host_post_json, with_http_fetch, with_post_json};
    use serde_json::json;

    #[test]
    fn with_http_fetch_is_scoped_to_closure() {
        let value = with_http_fetch(
            |url| {
                assert_eq!(url, "https://example.invalid/");
                Ok(json!({"status": 200, "content_type": "text/plain", "text": "ok"}))
            },
            || try_host_fetch("https://example.invalid/").unwrap(),
        )
        .unwrap();
        assert_eq!(value["text"], "ok");
        assert!(try_host_fetch("https://example.invalid/").is_none());
    }

    #[test]
    fn with_post_json_is_scoped_to_closure() {
        let value = with_post_json(
            |url, body, bearer| {
                assert_eq!(url, "https://example.invalid/api");
                assert_eq!(body["q"], "test");
                assert_eq!(bearer, "Bearer tok");
                Ok(json!({"status": 200, "text": "{}"}))
            },
            || {
                try_host_post_json(
                    "https://example.invalid/api",
                    &json!({"q":"test"}),
                    "Bearer tok",
                )
                .unwrap()
            },
        )
        .unwrap();
        assert_eq!(value["status"], 200);
        assert!(try_host_post_json("https://example.invalid/api", &json!({}), "").is_none());
    }
}
