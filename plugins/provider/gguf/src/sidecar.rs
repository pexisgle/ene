//! Loopback `llama-server` URL injected by the host (`sidecar_base_url`).

use std::sync::OnceLock;

static BASE: OnceLock<String> = OnceLock::new();

/// Holds nothing when the host did not inject a sidecar URL.
pub struct SidecarGuard;

impl Drop for SidecarGuard {
    fn drop(&mut self) {}
}

#[must_use]
pub fn managed_base() -> Option<&'static str> {
    BASE.get().map(String::as_str)
}

/// Read `sidecar_base_url` from `ENE_PROVIDER_CONFIG` (host-spawned sidecar).
#[must_use]
pub fn maybe_start() -> Option<SidecarGuard> {
    maybe_start_with("/v1")
}

pub(crate) fn maybe_start_with(url_suffix: &str) -> Option<SidecarGuard> {
    let raw = std::env::var("ENE_PROVIDER_CONFIG")
        .ok()
        .and_then(|encoded| serde_json::from_str::<serde_json::Value>(&encoded).ok())
        .and_then(|value| {
            value
                .get("sidecar_base_url")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .filter(|url| !url.trim().is_empty());
    let mut base = raw?;
    let suffix = url_suffix.trim_end_matches('/');
    if !base.ends_with(suffix) {
        base = format!("{}{suffix}", base.trim_end_matches('/'));
    }
    drop(BASE.set(base));
    Some(SidecarGuard)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_injected_sidecar_base_url() {
        let original = std::env::var_os("ENE_PROVIDER_CONFIG");
        // SAFETY: test restores env before returning.
        unsafe {
            std::env::set_var(
                "ENE_PROVIDER_CONFIG",
                r#"{"sidecar_base_url":"http://127.0.0.1:9"}"#,
            );
        }
        let _guard = maybe_start().expect("start");
        assert_eq!(managed_base(), Some("http://127.0.0.1:9/v1"));
        match original {
            Some(value) => unsafe {
                std::env::set_var("ENE_PROVIDER_CONFIG", value);
            },
            None => unsafe {
                std::env::remove_var("ENE_PROVIDER_CONFIG");
            },
        }
    }
}
